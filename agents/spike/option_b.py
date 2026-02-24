"""
Option B: Plan-based agent with PlanExecutor and 3 meta-tools.

Instead of exposing individual MCP tools, this agent gets 3 meta-tools:
  - execute_plan: executes an ordered list of steps sequentially, fail-fast
  - query_state: reads current gateway state via any read-only MCP tool
  - ask_user: logs a clarifying question (no real user in test)

The LLM is expected to reason about the full deployment upfront, produce a
structured plan, and submit it in one execute_plan call. Guardrails are applied
to each step during execution.
"""

import sys
from pathlib import Path

_AGENTS_DIR = Path(__file__).resolve().parent.parent
if str(_AGENTS_DIR) not in sys.path:
    sys.path.insert(0, str(_AGENTS_DIR))

import json

from openai import OpenAI

from agent import GuardrailReject, Guardrails
from mcp_client import FlowplaneMCPClient
from spike.shared import (
    PREAMBLE,
    TEST_PROMPT,
    AgentConfig,
    SkillLoader,
    SpikeTrace,
    run_traced,
)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

READ_PREFIXES = ("cp_list_", "cp_get_", "cp_query_", "ops_", "dev_")

PLAN_INSTRUCTIONS = """
## Plan Execution Model

You have 3 meta-tools instead of individual MCP tools:

1. **query_state** — Query current gateway state. Pass any read-only MCP tool name and its params.
2. **execute_plan** — Execute a deployment plan. Pass an ordered list of steps [{tool, params}, ...]. Steps run sequentially with guardrails applied. Execution stops on the first error.
3. **ask_user** — Ask a clarifying question (logged only, no real user in test).

### Workflow:
1. Use query_state to understand current state (check ports, existing resources)
2. Formulate a complete deployment plan as ordered steps
3. Use execute_plan to execute all steps at once
4. Review results and use query_state to verify

### Step format for execute_plan:
Each step is an object: {"tool": "<mcp_tool_name>", "params": {<tool_parameters>}}

You know all MCP tool names from the skill documentation above. Use them directly in your plan steps.
"""

# ---------------------------------------------------------------------------
# Meta-tool schemas
# ---------------------------------------------------------------------------

META_TOOLS: list[dict] = [
    {
        "type": "function",
        "function": {
            "name": "execute_plan",
            "description": (
                "Execute a deployment plan. Steps are executed sequentially with "
                "guardrails. Stops on first error. Returns results for all executed steps."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "description": "Ordered list of MCP tool calls to execute.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": {
                                    "type": "string",
                                    "description": "MCP tool name to call.",
                                },
                                "params": {
                                    "type": "object",
                                    "description": "Parameters to pass to the tool.",
                                },
                            },
                            "required": ["tool", "params"],
                        },
                    },
                },
                "required": ["steps"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "query_state",
            "description": (
                "Query the current gateway state. Executes any read-only MCP tool "
                "(cp_list_*, cp_get_*, cp_query_*, ops_*, dev_*)."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "tool": {
                        "type": "string",
                        "description": "Read-only MCP tool name to call.",
                    },
                    "params": {
                        "type": "object",
                        "description": "Parameters to pass to the tool.",
                    },
                },
                "required": ["tool"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "ask_user",
            "description": "Ask the user a clarifying question.",
            "parameters": {
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question to ask the user.",
                    },
                },
                "required": ["question"],
            },
        },
    },
]


# ---------------------------------------------------------------------------
# plan_executor
# ---------------------------------------------------------------------------

def _lowercase_allcaps_strings(obj: object) -> object:
    """Recursively lowercase any ALL_CAPS string values in a JSON-like structure.

    LLMs sometimes emit enum values as UPPERCASE (e.g. "HUNDRED" instead of
    "hundred"). Serde enums in Rust are snake_case and reject uppercase.
    Only lowercases strings that are entirely uppercase/underscores to avoid
    mangling mixed-case values like header names ("X-Gateway-Version").
    """
    if isinstance(obj, dict):
        return {k: _lowercase_allcaps_strings(v) for k, v in obj.items()}
    if isinstance(obj, list):
        return [_lowercase_allcaps_strings(v) for v in obj]
    if isinstance(obj, str) and obj == obj.upper() and obj != obj.lower():
        return obj.lower()
    return obj


def plan_executor(
    steps: list[dict],
    mcp: FlowplaneMCPClient,
    guardrails: Guardrails | None,
) -> list[dict]:
    """Execute a sequence of MCP tool steps sequentially with guardrails.

    Applies guardrails.before_call / after_call around each MCP call.
    Stops on the first error (fail-fast). Returns a list of result dicts,
    one per step attempted.
    """
    results: list[dict] = []

    for i, step in enumerate(steps):
        tool = step.get("tool", "")
        params = step.get("params", {})

        # Normalize ALL_CAPS enum values to lowercase (e.g. "HUNDRED" -> "hundred")
        if "configuration" in params:
            params = {**params, "configuration": _lowercase_allcaps_strings(params["configuration"])}

        try:
            # Pre-call guardrails
            if guardrails is not None:
                params = guardrails.before_call(tool, params)

            result = mcp.call_tool(tool, params)

            # Post-call guardrails
            if guardrails is not None:
                guardrails.after_call(tool, params, result)

            results.append({"step": i, "tool": tool, "result": result})

        except (GuardrailReject, Exception) as e:
            results.append({"step": i, "tool": tool, "error": str(e)})
            break  # fail-fast

    return results


# ---------------------------------------------------------------------------
# run
# ---------------------------------------------------------------------------

def run(config: AgentConfig, mcp: FlowplaneMCPClient) -> SpikeTrace:
    """Run the plan-based agent and return a SpikeTrace.

    Builds system prompt from PREAMBLE + flowplane-api skill + PLAN_INSTRUCTIONS.
    Exposes only 3 meta-tools (execute_plan, query_state, ask_user).
    All guardrail features are enabled.
    """
    guardrails = (
        Guardrails(mcp)
        .enable_auto_preflight()
        .enable_dataplane_injection()
        .enable_name_dedup()
        .enable_port_validation()
    )

    skill_text = SkillLoader.load("flowplane-api", refs=["routing-cookbook.md"])
    system_prompt = PREAMBLE + "\n\n" + skill_text + "\n\n" + PLAN_INSTRUCTIONS

    llm = OpenAI(base_url=config.llm_base_url, api_key=config.llm_api_key)

    guardrails.reset_turn()

    def execute_fn(tool_name: str, args: dict) -> str:
        if tool_name == "execute_plan":
            steps = args.get("steps", [])
            results = plan_executor(steps, mcp, guardrails)
            return json.dumps({"results": results})

        if tool_name == "query_state":
            query_tool = args.get("tool", "")
            if not any(query_tool.startswith(prefix) for prefix in READ_PREFIXES):
                return json.dumps({
                    "error": (
                        f"Tool '{query_tool}' is not a read-only tool. "
                        f"query_state only allows tools starting with: "
                        f"{', '.join(READ_PREFIXES)}"
                    )
                })
            params = args.get("params") or {}
            try:
                result = mcp.call_tool(query_tool, params)
                return json.dumps(result)
            except Exception as e:
                return json.dumps({"error": str(e)})

        if tool_name == "ask_user":
            question = args.get("question", "")
            print(f"  [ask_user] {question}", file=sys.stderr)
            return json.dumps({"logged": True, "question": question})

        return json.dumps({"error": f"Unknown meta-tool: {tool_name}"})

    return run_traced(
        llm=llm,
        model=config.llm_model,
        system_prompt=system_prompt,
        user_message=TEST_PROMPT,
        tools=META_TOOLS,
        execute_fn=execute_fn,
    )
