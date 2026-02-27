#!/usr/bin/env python3
"""
Spike: Agent Architecture Comparison Runner.

Usage:
    cd agents

    # Run just Option A against already-running CP:
    python spike/compare.py --option a --no-reset

    # Run just Option B against already-running CP:
    python spike/compare.py --option b --no-reset

    # Run both with full env reset between each (automated):
    python spike/compare.py

Requires agents/spike/.env with:
    LLM_API_KEY=...
    LLM_BASE_URL=...
    LLM_MODEL=...
    FLOWPLANE_URL=...
    FLOWPLANE_TEAM=...
    FLOWPLANE_TOKEN=...
"""

import sys
from pathlib import Path

_AGENTS_DIR = Path(__file__).resolve().parent.parent
if str(_AGENTS_DIR) not in sys.path:
    sys.path.insert(0, str(_AGENTS_DIR))

import argparse
import json
import traceback

from mcp_client import FlowplaneMCPClient
from testing.cp_helpers import CPBootstrapper, CPStateHelper

from spike.shared import (
    DEFAULT_ENV_FILE,
    PROJECT_DIR,
    TEST_PROMPT,
    AgentConfig,
    SpikeTrace,
    env_reset,
    verify_deployment,
)
from spike import option_a, option_b, option_c


# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------

def verify_mcp_state(mcp: FlowplaneMCPClient) -> dict[str, dict]:
    """Run MCP state checks and return pass/fail for each."""
    state = CPStateHelper(mcp)
    checks: dict[str, dict] = {}
    clusters: list[dict] = []

    # Cluster exists (agent chooses the name, so find any new cluster)
    try:
        snap = state.snapshot()
        clusters = snap.clusters
        checks["cluster_created"] = {
            "passed": len(clusters) > 0,
            "detail": [c.get("name") for c in clusters],
        }
    except Exception as e:
        checks["cluster_created"] = {"passed": False, "detail": str(e)}

    # Listener on port 10019
    try:
        listener = state.assert_listener_port(10019)
        checks["listener_10019"] = {
            "passed": True,
            "detail": listener.get("name", ""),
        }
    except AssertionError as e:
        checks["listener_10019"] = {"passed": False, "detail": str(e)}

    # Trace resolves — find the agent-created cluster (not the seed httpbin one)
    try:
        # The agent should have created a cluster for httpbin:80.
        # The seed also creates clusters, so look for any cluster that
        # traces to /api/v1/users on port 10019.
        result = mcp.call_tool("ops_trace_request", {"path": "/api/v1/users", "port": 10019})
        matches = result.get("matches", [])
        if matches:
            matched_cluster = matches[0].get("cluster_name", "")
            checks["trace_resolves"] = {"passed": True, "detail": matched_cluster}
        else:
            checks["trace_resolves"] = {
                "passed": False,
                "detail": f"No match. match_count={result.get('match_count', 0)}",
            }
    except Exception as e:
        checks["trace_resolves"] = {"passed": False, "detail": str(e)}

    # Config validates
    try:
        result = mcp.call_tool("ops_config_validate", {})
        issues = result.get("issues", [])
        checks["config_valid"] = {
            "passed": len(issues) == 0,
            "detail": f"{len(issues)} issues" if issues else "clean",
        }
    except Exception as e:
        checks["config_valid"] = {"passed": False, "detail": str(e)}

    return checks


def verify_http() -> dict:
    """Run real HTTP check through Envoy."""
    return verify_deployment(10019)


# ---------------------------------------------------------------------------
# Run one option
# ---------------------------------------------------------------------------

def run_option(
    name: str,
    run_fn,
    config: AgentConfig,
    do_reset: bool = True,
) -> dict:
    """Optionally reset env, connect to CP, run agent, verify. Returns all results."""
    print(f"\n{'=' * 60}", file=sys.stderr)
    print(f"  {name}", file=sys.stderr)
    print(f"{'=' * 60}\n", file=sys.stderr)

    if do_reset:
        # 1. Reset environment
        print("[1/5] Resetting environment...", file=sys.stderr)
        env_reset()

        # 2. Bootstrap (seed data, get token)
        print("[2/5] Bootstrapping CP...", file=sys.stderr)
        bootstrapper = CPBootstrapper(config.flowplane_url)
        bootstrapper.wait_for_cp(timeout_s=120.0)
        boot = bootstrapper.bootstrap()
        run_config = AgentConfig(
            flowplane_url=config.flowplane_url,
            flowplane_team=boot.team,
            flowplane_token=boot.token,
            llm_base_url=config.llm_base_url,
            llm_api_key=config.llm_api_key,
            llm_model=config.llm_model,
        )
    else:
        # Use credentials from .env directly
        print("[1/5] Skipping env reset (using existing CP)...", file=sys.stderr)
        print("[2/5] Using .env credentials...", file=sys.stderr)
        run_config = config

    # 3. Initialize MCP client
    print("[3/5] Initializing MCP client...", file=sys.stderr)
    mcp = FlowplaneMCPClient(run_config.flowplane_url, run_config.flowplane_team, run_config.flowplane_token)
    mcp.initialize()
    tools = mcp.list_tools()
    print(f"       MCP session ready, {len(tools)} tools available", file=sys.stderr)

    # 4. Run agent
    print(f"[4/5] Running agent ({run_config.llm_model})...", file=sys.stderr)
    trace = SpikeTrace()
    error_msg = ""
    try:
        trace = run_fn(run_config, mcp)
    except Exception as e:
        error_msg = f"{type(e).__name__}: {e}"
        traceback.print_exc(file=sys.stderr)

    # 5. Verify
    print("[5/5] Verifying deployment...", file=sys.stderr)
    mcp_checks = verify_mcp_state(mcp)
    http_result = verify_http()
    mcp.close()

    return {
        "trace": trace,
        "mcp_checks": mcp_checks,
        "http_result": http_result,
        "error": error_msg,
    }


# ---------------------------------------------------------------------------
# Result printing
# ---------------------------------------------------------------------------

def print_single_result(name: str, result: dict, model: str) -> None:
    """Print results for a single option run."""
    trace: SpikeTrace = result["trace"]

    print(f"\n{'=' * 60}")
    print(f"  {name} — Results")
    print(f"{'=' * 60}")
    print(f"\nPrompt: {TEST_PROMPT}")
    print(f"Model:  {model}\n")

    # Metrics
    print(f"  LLM round trips:    {trace.turn_count}")
    print(f"  Total tool calls:   {trace.total_tool_calls}")
    print(f"  Input tokens:       {trace.total_prompt_tokens:,}")
    print(f"  Output tokens:      {trace.total_completion_tokens:,}")
    print(f"  Wall clock (s):     {trace.elapsed_s:.1f}")

    # Checks
    print(f"\n  Verification:")
    for key, check in result.get("mcp_checks", {}).items():
        status = "PASS" if check["passed"] else "FAIL"
        print(f"    {key:<20} {status}  ({check.get('detail', '')})")

    hr = result.get("http_result", {})
    http_status = "PASS" if hr.get("success") else "FAIL"
    print(f"    {'http_works':<20} {http_status}  ({hr.get('backend_url', hr.get('error', 'N/A'))})")

    if result.get("error"):
        print(f"\n  Error: {result['error']}")

    # Tool call log
    print(f"\n  Tool calls ({trace.total_tool_calls}):")
    for i, tc in enumerate(trace.tool_calls):
        args_preview = json.dumps(tc["args"], separators=(",", ":"))
        if len(args_preview) > 80:
            args_preview = args_preview[:80] + "..."
        print(f"    {i + 1:2d}. {tc['name']}({args_preview})")

    # Final answer
    if trace.final_answer:
        preview = trace.final_answer[:500] + ("..." if len(trace.final_answer) > 500 else "")
        print(f"\n  Final answer:\n    {preview}")

    print()


def print_comparison(results: dict[str, dict], model: str) -> None:
    """Print a side-by-side comparison table."""
    names = list(results.keys())
    if len(names) != 2:
        for name, result in results.items():
            print_single_result(name, result, model)
        return

    a_name, b_name = names
    a, b = results[a_name], results[b_name]
    a_trace: SpikeTrace = a["trace"]
    b_trace: SpikeTrace = b["trace"]

    def check_str(result: dict, key: str) -> str:
        checks = result.get("mcp_checks", {})
        if key in checks:
            return "PASS" if checks[key]["passed"] else "FAIL"
        return "N/A"

    def http_str(result: dict) -> str:
        hr = result.get("http_result", {})
        return "PASS" if hr.get("success") else "FAIL"

    rows = [
        ("LLM round trips", str(a_trace.turn_count), str(b_trace.turn_count)),
        ("Total tool calls", str(a_trace.total_tool_calls), str(b_trace.total_tool_calls)),
        ("Input tokens", f"{a_trace.total_prompt_tokens:,}", f"{b_trace.total_prompt_tokens:,}"),
        ("Output tokens", f"{a_trace.total_completion_tokens:,}", f"{b_trace.total_completion_tokens:,}"),
        ("Wall clock (s)", f"{a_trace.elapsed_s:.1f}", f"{b_trace.elapsed_s:.1f}"),
        ("Cluster created", check_str(a, "cluster_created"), check_str(b, "cluster_created")),
        ("Listener on 10019", check_str(a, "listener_10019"), check_str(b, "listener_10019")),
        ("Trace resolves", check_str(a, "trace_resolves"), check_str(b, "trace_resolves")),
        ("Config valid", check_str(a, "config_valid"), check_str(b, "config_valid")),
        ("HTTP works", http_str(a), http_str(b)),
    ]

    if a.get("error"):
        rows.append(("Error (A)", a["error"][:60], ""))
    if b.get("error"):
        rows.append(("Error (B)", "", b["error"][:60]))

    col0_w = max(len(r[0]) for r in rows) + 2
    col1_w = max(len(a_name), max(len(r[1]) for r in rows)) + 2
    col2_w = max(len(b_name), max(len(r[2]) for r in rows)) + 2

    header = f"{'Metric':<{col0_w}} {a_name:<{col1_w}} {b_name:<{col2_w}}"
    sep = "-" * len(header)

    print(f"\n{sep}")
    print("  COMPARISON RESULTS")
    print(sep)
    print(f"\nPrompt: {TEST_PROMPT}\nModel:  {model}\n")
    print(header)
    print(sep)
    for label, val_a, val_b in rows:
        print(f"{label:<{col0_w}} {val_a:<{col1_w}} {val_b:<{col2_w}}")
    print(sep)

    for name, result in results.items():
        hr = result.get("http_result", {})
        print(f"\n{name} HTTP detail:")
        print(f"  URL:     {hr.get('request_url', 'N/A')}")
        print(f"  Status:  {hr.get('status_code', 'N/A')}")
        print(f"  Backend: {hr.get('backend_url', 'N/A')}")
        if hr.get("error"):
            print(f"  Error:   {hr['error']}")

    print()


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description="Spike: Agent Architecture Comparison")
    parser.add_argument(
        "--option", choices=["a", "b", "c", "both"], default="both",
        help="Which option to run (default: both = a + b)",
    )
    parser.add_argument(
        "--no-reset", action="store_true",
        help="Skip env reset, use already-running CP with .env credentials",
    )
    parser.add_argument(
        "--env", type=Path, default=DEFAULT_ENV_FILE,
        help=f"Path to .env file (default: {DEFAULT_ENV_FILE})",
    )
    args = parser.parse_args()

    config = AgentConfig.from_env(args.env)

    if not config.llm_api_key:
        print("Error: LLM_API_KEY is required (set in .env or environment)", file=sys.stderr)
        sys.exit(1)

    if args.no_reset and not config.flowplane_token:
        print("Error: FLOWPLANE_TOKEN is required in .env when using --no-reset", file=sys.stderr)
        sys.exit(1)

    print(f"Config: model={config.llm_model}, base_url={config.llm_base_url}", file=sys.stderr)
    print(f"Flowplane: {config.flowplane_url}, team={config.flowplane_team}", file=sys.stderr)
    print(f"Prompt: {TEST_PROMPT}", file=sys.stderr)
    print(f"Options: {args.option}, reset={not args.no_reset}\n", file=sys.stderr)

    all_options = {
        "a": ("Option A (Tool-Calling)", option_a.run),
        "b": ("Option B (Plan-Based)", option_b.run),
        "c": ("Option C (Single-Shot)", option_c.run),
    }

    if args.option == "both":
        selected = list(all_options.values())
    else:
        selected = [all_options[args.option]]

    results: dict[str, dict] = {}
    for name, run_fn in selected:
        try:
            results[name] = run_option(name, run_fn, config, do_reset=not args.no_reset)
        except Exception as e:
            print(f"\nFATAL: {name} failed: {e}", file=sys.stderr)
            traceback.print_exc(file=sys.stderr)
            results[name] = {
                "trace": SpikeTrace(),
                "mcp_checks": {},
                "http_result": {"success": False, "error": str(e)},
                "error": str(e),
            }

    # Print results
    if len(results) == 1:
        name, result = next(iter(results.items()))
        print_single_result(name, result, config.llm_model)
    else:
        print_comparison(results, config.llm_model)

    # Dump traces for post-analysis
    trace_file = Path(__file__).resolve().parent / "last_run.json"
    dump = {}
    for name, result in results.items():
        t = result["trace"]
        dump[name] = {
            "turn_count": t.turn_count,
            "total_tool_calls": t.total_tool_calls,
            "prompt_tokens": t.prompt_tokens,
            "completion_tokens": t.completion_tokens,
            "elapsed_s": t.elapsed_s,
            "tool_calls": t.tool_calls,
            "mcp_checks": {
                k: {"passed": v["passed"], "detail": str(v.get("detail", ""))}
                for k, v in result.get("mcp_checks", {}).items()
            },
            "http_result": result.get("http_result", {}),
            "error": result.get("error", ""),
        }
    trace_file.write_text(json.dumps(dump, indent=2))
    print(f"Traces saved to: {trace_file}", file=sys.stderr)


if __name__ == "__main__":
    main()
