"""
Shared infrastructure for the agent architecture comparison spike.

Components:
- SkillLoader: loads .claude/skills/{name}/SKILL.md + reference files
- AgentConfig: loads LLM and Flowplane config from .env + env vars
- SpikeTrace: captures metrics per agent run (turns, tokens, tool calls)
- TestScenario: defines a test case for the test matrix (prompt, pre-queries, verification)
- run_traced: generic traced agent loop used by both options
- verify_deployment: real HTTP call through Envoy to confirm the agent worked
- env_reset: make commands to get a clean environment
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

# Ensure agents/ is on the path for importing agent.py, mcp_client.py
_AGENTS_DIR = Path(__file__).resolve().parent.parent
if str(_AGENTS_DIR) not in sys.path:
    sys.path.insert(0, str(_AGENTS_DIR))

import httpx
from openai import OpenAI

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

TEST_PROMPT = (
    "Expose an API at /api/v1/users on port 10019. The backend is httpbin "
    "running at httpbin:80. Map /api/v1/users to /anything/users on the backend."
)

PREAMBLE = """\
You are a Flowplane API deployment agent. Deploy the user's requested API \
configuration through the gateway using the available tools.

Follow the skill documentation below for naming conventions, port selection, \
resource creation order, and verification steps. After deployment, verify \
with ops_trace_request and ops_config_validate.

When a tool call fails, do NOT retry with the same parameters. Diagnose first \
using the error handling guidance in the skill documentation."""

PROJECT_DIR = Path(__file__).resolve().parent.parent.parent


# ---------------------------------------------------------------------------
# .env loader
# ---------------------------------------------------------------------------

SPIKE_DIR = Path(__file__).resolve().parent
DEFAULT_ENV_FILE = SPIKE_DIR / ".env"


def _load_dotenv(path: Path) -> None:
    """Minimal .env parser. Does not override existing env vars."""
    if not path.exists():
        return
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" in line:
            key, _, value = line.partition("=")
            key = key.strip()
            # Strip trailing backslash (shell continuation), then quotes
            value = value.strip().rstrip("\\").strip().strip('"').strip("'")
            if key and key not in os.environ:
                os.environ[key] = value


# ---------------------------------------------------------------------------
# SkillLoader
# ---------------------------------------------------------------------------

class SkillLoader:
    """Load skill content from .claude/skills/{name}/SKILL.md + references."""

    SKILLS_DIR = PROJECT_DIR / ".claude" / "skills"

    @classmethod
    def load(cls, skill_name: str, refs: list[str] | None = None) -> str:
        """Return the full skill text with optional reference files appended."""
        skill_file = cls.SKILLS_DIR / skill_name / "SKILL.md"
        if not skill_file.exists():
            raise FileNotFoundError(f"Skill not found: {skill_file}")

        content = skill_file.read_text()

        if refs:
            for ref in refs:
                ref_path = cls.SKILLS_DIR / skill_name / "references" / ref
                if ref_path.exists():
                    content += f"\n\n---\n# Reference: {ref}\n\n"
                    content += ref_path.read_text()

        return content


# ---------------------------------------------------------------------------
# AgentConfig
# ---------------------------------------------------------------------------

@dataclass
class AgentConfig:
    """LLM and Flowplane connection config."""
    flowplane_url: str
    flowplane_team: str
    flowplane_token: str
    llm_base_url: str
    llm_api_key: str
    llm_model: str

    @classmethod
    def from_env(cls, env_file: Path | None = None) -> AgentConfig:
        if env_file:
            _load_dotenv(env_file)
        return cls(
            flowplane_url=os.environ.get("FLOWPLANE_URL", "http://localhost:8080"),
            flowplane_team=os.environ.get("FLOWPLANE_TEAM", ""),
            flowplane_token=os.environ.get("FLOWPLANE_TOKEN", ""),
            llm_base_url=os.environ.get("LLM_BASE_URL", "https://api.openai.com/v1"),
            llm_api_key=os.environ.get("LLM_API_KEY", ""),
            llm_model=os.environ.get("LLM_MODEL", "gpt-4o"),
        )


# ---------------------------------------------------------------------------
# SpikeTrace
# ---------------------------------------------------------------------------

@dataclass
class SpikeTrace:
    """Metrics captured from a single agent run."""
    turn_count: int = 0
    tool_calls: list[dict] = field(default_factory=list)
    prompt_tokens: list[int] = field(default_factory=list)
    completion_tokens: list[int] = field(default_factory=list)
    elapsed_s: float = 0.0
    final_answer: str = ""

    @property
    def total_prompt_tokens(self) -> int:
        return sum(self.prompt_tokens)

    @property
    def total_completion_tokens(self) -> int:
        return sum(self.completion_tokens)

    @property
    def total_tool_calls(self) -> int:
        return len(self.tool_calls)


# ---------------------------------------------------------------------------
# run_traced — generic traced agent loop
# ---------------------------------------------------------------------------

def run_traced(
    llm: OpenAI,
    model: str,
    system_prompt: str,
    user_message: str,
    tools: list[dict],
    execute_fn: Callable[[str, dict], str],
    max_turns: int = 15,
) -> SpikeTrace:
    """Run an agentic loop, capturing all metrics into a SpikeTrace.

    Args:
        llm: OpenAI-compatible client.
        model: Model name.
        system_prompt: System prompt text.
        user_message: The user's task.
        tools: OpenAI function-calling tool schemas.
        execute_fn: Callback (tool_name, args) -> result_json_string.
        max_turns: Safety limit on LLM round trips.

    Returns:
        SpikeTrace with all metrics.
    """
    trace = SpikeTrace()
    start = time.monotonic()

    messages: list[dict] = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_message},
    ]

    for _turn in range(max_turns):
        response = llm.chat.completions.create(
            model=model,
            messages=messages,
            tools=tools if tools else None,
        )

        # Track tokens
        if response.usage:
            trace.prompt_tokens.append(response.usage.prompt_tokens or 0)
            trace.completion_tokens.append(response.usage.completion_tokens or 0)
        trace.turn_count += 1

        choice = response.choices[0]
        message = choice.message

        # No tool calls — final answer
        if not message.tool_calls:
            trace.final_answer = message.content or ""
            trace.elapsed_s = time.monotonic() - start
            return trace

        # Process tool calls
        messages.append(message)

        for tool_call in message.tool_calls:
            fn_name = tool_call.function.name
            try:
                fn_args = json.loads(tool_call.function.arguments)
            except json.JSONDecodeError:
                fn_args = {}

            print(f"  -> {fn_name}({json.dumps(fn_args, separators=(',', ':'))})", file=sys.stderr)
            trace.tool_calls.append({"name": fn_name, "args": fn_args})

            result_str = execute_fn(fn_name, fn_args)

            messages.append({
                "role": "tool",
                "tool_call_id": tool_call.id,
                "content": result_str,
            })

    trace.final_answer = "Max turns reached without final answer."
    trace.elapsed_s = time.monotonic() - start
    return trace


# ---------------------------------------------------------------------------
# verify_deployment — real HTTP through Envoy
# ---------------------------------------------------------------------------

def verify_deployment(
    port: int,
    path: str = "/api/v1/users",
    expected_backend_path: str = "/anything/users",
    timeout_s: float = 10.0,
) -> dict:
    """Make a real HTTP request through Envoy and check the response.

    httpbin's /anything endpoint echoes the request URL in its response body.
    If Envoy correctly rewrites the path, the response URL will contain
    the expected_backend_path.
    """
    url = f"http://localhost:{port}{path}"
    try:
        resp = httpx.get(url, timeout=timeout_s)
        try:
            body = resp.json()
        except Exception:
            body = {"raw": resp.text[:500]}

        # httpbin /anything echoes "url" containing the backend path
        actual_url = body.get("url", "")
        success = (
            resp.status_code == 200
            and expected_backend_path in actual_url
        )
        return {
            "success": success,
            "status_code": resp.status_code,
            "request_url": url,
            "backend_url": actual_url,
            "body_preview": json.dumps(body)[:500],
        }
    except Exception as e:
        return {"success": False, "error": str(e), "request_url": url}


# ---------------------------------------------------------------------------
# env_reset — clean environment via make
# ---------------------------------------------------------------------------

def env_reset(project_dir: Path | None = None) -> None:
    """Reset the environment: stop, clean volumes, rebuild, start fresh.

    Runs: make clean && make build && make up HTTPBIN=1 ENVOY=1 MOCKBACKEND=1
    """
    cwd = str(project_dir or PROJECT_DIR)

    steps = [
        (["make", "clean"], "Cleaning environment"),
        (["make", "build"], "Building Docker image"),
        (["make", "up", "HTTPBIN=1", "ENVOY=1", "MOCKBACKEND=1"], "Starting services"),
    ]

    for cmd, label in steps:
        print(f"  [{label}] {' '.join(cmd)}", file=sys.stderr)
        result = subprocess.run(
            cmd, cwd=cwd, capture_output=True, text=True, timeout=300,
        )
        if result.returncode != 0:
            print(f"  FAILED: {result.stderr[:500]}", file=sys.stderr)
            raise RuntimeError(f"{label} failed (exit {result.returncode}): {result.stderr[:500]}")


# ---------------------------------------------------------------------------
# TestScenario — test matrix scenario definition
# ---------------------------------------------------------------------------

@dataclass
class HttpCheck:
    """A single HTTP verification check through Envoy."""
    port: int
    path: str
    expect_status: int = 200
    expect_body_contains: str | None = None


@dataclass
class TestScenario:
    """A test scenario for the Option C test matrix.

    Each scenario defines a prompt, pre-queries for context gathering,
    a verification function, and optional HTTP checks through Envoy.
    """
    name: str
    prompt: str
    pre_queries: dict[str, tuple[str, dict]]
    verify: Callable  # (FlowplaneMCPClient) -> dict[str, dict]
    http_checks: list[HttpCheck] | None = None


# ---------------------------------------------------------------------------
# Scenario verification functions
# ---------------------------------------------------------------------------

def _verify_t1(mcp) -> dict[str, dict]:
    """Verify T1: Basic exposure with rewrite."""
    checks: dict[str, dict] = {}

    # Trace resolves for /api/v1/users on port 10019
    try:
        result = mcp.call_tool("ops_trace_request", {"path": "/api/v1/users", "port": 10019})
        matches = result.get("matches", [])
        checks["trace_users"] = {
            "passed": len(matches) > 0,
            "detail": matches[0].get("cluster_name", "") if matches else "no match",
        }
    except Exception as e:
        checks["trace_users"] = {"passed": False, "detail": str(e)}

    return checks


def _verify_t2(mcp) -> dict[str, dict]:
    """Verify T2: Rate limit filter on listener."""
    checks: dict[str, dict] = {}
    rate_filters: list[dict] = []

    # Filter exists with type local_rate_limit
    try:
        result = mcp.call_tool("cp_list_filters", {})
        filters = result.get("filters") or result.get("items") or []
        rate_filters = [f for f in filters if f.get("filter_type") == "local_rate_limit"]
        checks["rate_limit_filter_exists"] = {
            "passed": len(rate_filters) > 0,
            "detail": rate_filters[0].get("name", "") if rate_filters else "not found",
        }
    except Exception as e:
        checks["rate_limit_filter_exists"] = {"passed": False, "detail": str(e)}

    # Filter is attached to a listener
    if rate_filters:
        try:
            result = mcp.call_tool("cp_list_filter_attachments", {"filter": rate_filters[0]["name"]})
            listener_attachments = result.get("listener_attachments") or []
            checks["rate_limit_attached_to_listener"] = {
                "passed": len(listener_attachments) > 0,
                "detail": [a.get("listener_name", a.get("name", "")) for a in listener_attachments],
            }
        except Exception as e:
            checks["rate_limit_attached_to_listener"] = {"passed": False, "detail": str(e)}

    return checks


def _verify_t3(mcp) -> dict[str, dict]:
    """Verify T3: CORS filter on route config."""
    checks: dict[str, dict] = {}
    cors_filters: list[dict] = []

    # Filter exists with type cors
    try:
        result = mcp.call_tool("cp_list_filters", {})
        filters = result.get("filters") or result.get("items") or []
        cors_filters = [f for f in filters if f.get("filter_type") == "cors"]
        checks["cors_filter_exists"] = {
            "passed": len(cors_filters) > 0,
            "detail": cors_filters[0].get("name", "") if cors_filters else "not found",
        }
    except Exception as e:
        checks["cors_filter_exists"] = {"passed": False, "detail": str(e)}

    # Filter is attached to a route config
    if cors_filters:
        try:
            result = mcp.call_tool("cp_list_filter_attachments", {"filter": cors_filters[0]["name"]})
            rc_attachments = result.get("route_config_attachments") or []
            checks["cors_attached_to_route_config"] = {
                "passed": len(rc_attachments) > 0,
                "detail": [a.get("route_config_name", a.get("name", "")) for a in rc_attachments],
            }
        except Exception as e:
            checks["cors_attached_to_route_config"] = {"passed": False, "detail": str(e)}

    return checks


def _verify_t4(mcp) -> dict[str, dict]:
    """Verify T4: Header mutation filter on listener."""
    checks: dict[str, dict] = {}
    hm_filters: list[dict] = []

    # Filter exists with type header_mutation
    try:
        result = mcp.call_tool("cp_list_filters", {})
        filters = result.get("filters") or result.get("items") or []
        hm_filters = [f for f in filters if f.get("filter_type") == "header_mutation"]
        checks["header_mutation_filter_exists"] = {
            "passed": len(hm_filters) > 0,
            "detail": hm_filters[0].get("name", "") if hm_filters else "not found",
        }
    except Exception as e:
        checks["header_mutation_filter_exists"] = {"passed": False, "detail": str(e)}

    # Filter is attached to a listener
    if hm_filters:
        try:
            result = mcp.call_tool("cp_list_filter_attachments", {"filter": hm_filters[0]["name"]})
            listener_attachments = result.get("listener_attachments") or []
            checks["header_mutation_attached_to_listener"] = {
                "passed": len(listener_attachments) > 0,
                "detail": [a.get("listener_name", a.get("name", "")) for a in listener_attachments],
            }
        except Exception as e:
            checks["header_mutation_attached_to_listener"] = {"passed": False, "detail": str(e)}

    return checks


def _verify_t5(mcp) -> dict[str, dict]:
    """Verify T5: New route added, existing route preserved."""
    checks: dict[str, dict] = {}

    # Trace resolves for /api/v1/orders on port 10019
    try:
        result = mcp.call_tool("ops_trace_request", {"path": "/api/v1/orders", "port": 10019})
        matches = result.get("matches", [])
        checks["trace_orders"] = {
            "passed": len(matches) > 0,
            "detail": matches[0].get("cluster_name", "") if matches else "no match",
        }
    except Exception as e:
        checks["trace_orders"] = {"passed": False, "detail": str(e)}

    # Trace still resolves for /api/v1/users on port 10019 (not broken)
    try:
        result = mcp.call_tool("ops_trace_request", {"path": "/api/v1/users", "port": 10019})
        matches = result.get("matches", [])
        checks["trace_users_preserved"] = {
            "passed": len(matches) > 0,
            "detail": matches[0].get("cluster_name", "") if matches else "no match",
        }
    except Exception as e:
        checks["trace_users_preserved"] = {"passed": False, "detail": str(e)}

    return checks


def _verify_t6(mcp) -> dict[str, dict]:
    """Verify T6: Second service on different port."""
    checks: dict[str, dict] = {}

    # Trace resolves for /health on port 10020
    try:
        result = mcp.call_tool("ops_trace_request", {"path": "/health", "port": 10020})
        matches = result.get("matches", [])
        checks["trace_health"] = {
            "passed": len(matches) > 0,
            "detail": matches[0].get("cluster_name", "") if matches else "no match",
        }
    except Exception as e:
        checks["trace_health"] = {"passed": False, "detail": str(e)}

    return checks


# ---------------------------------------------------------------------------
# Scenario definitions
# ---------------------------------------------------------------------------

SCENARIOS: dict[str, TestScenario] = {}


def _build_scenarios() -> dict[str, TestScenario]:
    """Build all test scenarios. Called once at module load."""
    scenarios: dict[str, TestScenario] = {}

    # T1: Basic Exposure with Rewrite (baseline)
    scenarios["T1"] = TestScenario(
        name="T1: Basic Exposure with Rewrite",
        prompt=(
            "Expose an API at /api/v1/users on port 10019. The backend is httpbin "
            "running at httpbin:80. Map /api/v1/users to /anything/users on the backend."
        ),
        pre_queries={
            "dataplanes": ("cp_list_dataplanes", {}),
            "listeners": ("cp_list_listeners", {}),
            "clusters": ("cp_list_clusters", {}),
            "port_10019": ("cp_query_port", {"port": 10019}),
        },
        verify=_verify_t1,
        http_checks=[
            HttpCheck(port=10019, path="/api/v1/users", expect_status=200, expect_body_contains="/anything/users"),
        ],
    )

    # T2: Add Rate Limit Filter to Listener
    scenarios["T2"] = TestScenario(
        name="T2: Rate Limit Filter on Listener",
        prompt=(
            "Add a rate limit to the listener on port 10019. Allow 5 requests per "
            "minute. Return 429 when exceeded."
        ),
        pre_queries={
            "dataplanes": ("cp_list_dataplanes", {}),
            "listeners": ("cp_list_listeners", {}),
            "clusters": ("cp_list_clusters", {}),
            "filters": ("cp_list_filters", {}),
        },
        verify=_verify_t2,
    )

    # T3: Add CORS Filter to Route Config
    scenarios["T3"] = TestScenario(
        name="T3: CORS Filter on Route Config",
        prompt=(
            "Add a CORS policy to the route config serving /api/v1/users. Allow "
            "origin https://app.example.com, methods GET and POST, and headers "
            "Authorization and Content-Type. Max age 86400 seconds."
        ),
        pre_queries={
            "dataplanes": ("cp_list_dataplanes", {}),
            "listeners": ("cp_list_listeners", {}),
            "clusters": ("cp_list_clusters", {}),
            "route_configs": ("cp_list_route_configs", {}),
            "filters": ("cp_list_filters", {}),
        },
        verify=_verify_t3,
    )

    # T4: Add Header Mutation Filter
    scenarios["T4"] = TestScenario(
        name="T4: Header Mutation Filter on Listener",
        prompt=(
            "Add a header mutation filter to the listener on port 10019 that adds "
            "X-Gateway-Version: v1 to all requests and removes the Server header "
            "from all responses."
        ),
        pre_queries={
            "dataplanes": ("cp_list_dataplanes", {}),
            "listeners": ("cp_list_listeners", {}),
            "clusters": ("cp_list_clusters", {}),
            "filters": ("cp_list_filters", {}),
        },
        verify=_verify_t4,
    )

    # T5: Add New Route to Existing Config
    # pre_queries include route_configs AND the full route config detail
    scenarios["T5"] = TestScenario(
        name="T5: Add Route to Existing Config",
        prompt=(
            "Add a new route to the existing route config on port 10019. Map "
            "/api/v1/orders (prefix match) to /anything/orders on the same httpbin "
            "backend cluster. Keep the existing /api/v1/users route intact."
        ),
        pre_queries={
            "dataplanes": ("cp_list_dataplanes", {}),
            "listeners": ("cp_list_listeners", {}),
            "clusters": ("cp_list_clusters", {}),
            "route_configs": ("cp_list_route_configs", {}),
        },
        verify=_verify_t5,
        http_checks=[
            HttpCheck(port=10019, path="/api/v1/orders", expect_status=200, expect_body_contains="/anything/orders"),
            HttpCheck(port=10019, path="/api/v1/users", expect_status=200, expect_body_contains="/anything/users"),
        ],
    )

    # T6: Expose Second Service on Different Port
    scenarios["T6"] = TestScenario(
        name="T6: Second Service on New Port",
        prompt=(
            "Expose a health check API on port 10020. Route /health to httpbin:80's "
            "/get endpoint. Use exact match on /health with a path rewrite to /get."
        ),
        pre_queries={
            "dataplanes": ("cp_list_dataplanes", {}),
            "listeners": ("cp_list_listeners", {}),
            "clusters": ("cp_list_clusters", {}),
            "port_10020": ("cp_query_port", {"port": 10020}),
        },
        verify=_verify_t6,
        http_checks=[
            HttpCheck(port=10020, path="/health", expect_status=200),
        ],
    )

    return scenarios


SCENARIOS = _build_scenarios()
