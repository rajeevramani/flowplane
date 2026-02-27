#!/usr/bin/env python3
"""
Option C Test Matrix: Reliability validation across multiple scenarios.

Runs TestScenarios sequentially against a single CP instance. Scenarios
build on each other — T1 creates base resources, T2-T4 add filters, T5
adds a route, T6 creates a new service on a different port.

Usage:
    cd agents

    # Run full matrix (no env reset, existing CP):
    python spike/test_matrix.py --no-reset

    # Run single scenario:
    python spike/test_matrix.py --scenario T1 --no-reset

    # Run a subset:
    python spike/test_matrix.py --scenario T1 --scenario T5 --no-reset

    # Full reset + matrix:
    python spike/test_matrix.py
"""

import sys
from pathlib import Path

_AGENTS_DIR = Path(__file__).resolve().parent.parent
if str(_AGENTS_DIR) not in sys.path:
    sys.path.insert(0, str(_AGENTS_DIR))

import argparse
import json
import time
import traceback

import httpx

from mcp_client import FlowplaneMCPClient
from testing.cp_helpers import CPBootstrapper

from spike.shared import (
    DEFAULT_ENV_FILE,
    SCENARIOS,
    AgentConfig,
    HttpCheck,
    SpikeTrace,
    TestScenario,
    env_reset,
)
from spike import option_c


# ---------------------------------------------------------------------------
# HTTP verification
# ---------------------------------------------------------------------------

def run_http_checks(checks: list[HttpCheck]) -> list[dict]:
    """Run HTTP checks through Envoy and return results."""
    results = []
    for check in checks:
        url = f"http://localhost:{check.port}{check.path}"
        try:
            resp = httpx.get(url, timeout=10.0)
            try:
                body = resp.json()
            except Exception:
                body = {"raw": resp.text[:500]}

            passed = resp.status_code == check.expect_status
            if passed and check.expect_body_contains:
                actual_url = body.get("url", "")
                body_str = json.dumps(body)
                passed = check.expect_body_contains in actual_url or check.expect_body_contains in body_str

            results.append({
                "path": check.path,
                "port": check.port,
                "passed": passed,
                "status_code": resp.status_code,
                "backend_url": body.get("url", ""),
                "detail": f"status={resp.status_code}" + (
                    f", body contains '{check.expect_body_contains}'" if check.expect_body_contains else ""
                ),
            })
        except Exception as e:
            results.append({
                "path": check.path,
                "port": check.port,
                "passed": False,
                "detail": str(e),
            })
    return results


# ---------------------------------------------------------------------------
# Run one scenario
# ---------------------------------------------------------------------------

def run_scenario(
    scenario: TestScenario,
    config: AgentConfig,
    mcp: FlowplaneMCPClient,
) -> dict:
    """Run a single scenario through option_c and verify results."""
    print(f"\n{'─' * 60}", file=sys.stderr)
    print(f"  {scenario.name}", file=sys.stderr)
    print(f"{'─' * 60}", file=sys.stderr)
    print(f"  Prompt: {scenario.prompt[:100]}...", file=sys.stderr)

    # Run agent
    trace = SpikeTrace()
    error_msg = ""
    try:
        trace = option_c.run(config, mcp, scenario)
    except Exception as e:
        error_msg = f"{type(e).__name__}: {e}"
        traceback.print_exc(file=sys.stderr)

    # MCP verification
    print(f"  Verifying MCP state...", file=sys.stderr)
    mcp_checks: dict[str, dict] = {}
    try:
        mcp_checks = scenario.verify(mcp)
    except Exception as e:
        mcp_checks["verify_error"] = {"passed": False, "detail": str(e)}

    # HTTP checks
    http_results: list[dict] = []
    if scenario.http_checks:
        print(f"  Running HTTP checks...", file=sys.stderr)
        http_results = run_http_checks(scenario.http_checks)

    # Print check results inline
    for name, check in mcp_checks.items():
        status = "PASS" if check["passed"] else "FAIL"
        print(f"    {name:<40} {status}  ({check.get('detail', '')})", file=sys.stderr)
    for hr in http_results:
        status = "PASS" if hr["passed"] else "FAIL"
        print(f"    HTTP {hr['path']:<36} {status}  ({hr.get('detail', '')})", file=sys.stderr)

    return {
        "scenario": scenario.name,
        "trace": trace,
        "mcp_checks": mcp_checks,
        "http_results": http_results,
        "error": error_msg,
    }


# ---------------------------------------------------------------------------
# Summary printing
# ---------------------------------------------------------------------------

def print_summary(results: list[dict], model: str) -> None:
    """Print a summary table of all scenario results."""
    print(f"\n{'=' * 80}")
    print(f"  TEST MATRIX SUMMARY")
    print(f"{'=' * 80}")
    print(f"  Model: {model}")
    print()

    # Header
    col_name = 30
    col_turns = 6
    col_tools = 6
    col_tokens = 12
    col_time = 8
    col_mcp = 8
    col_http = 8

    header = (
        f"  {'Scenario':<{col_name}} "
        f"{'Turns':>{col_turns}} "
        f"{'Tools':>{col_tools}} "
        f"{'Tokens':>{col_tokens}} "
        f"{'Time':>{col_time}} "
        f"{'MCP':>{col_mcp}} "
        f"{'HTTP':>{col_http}}"
    )
    print(header)
    print(f"  {'─' * (len(header) - 2)}")

    total_pass = 0
    total_fail = 0

    for r in results:
        trace: SpikeTrace = r["trace"]
        total_tokens = trace.total_prompt_tokens + trace.total_completion_tokens

        # Count MCP check passes/fails
        mcp_checks = r.get("mcp_checks", {})
        mcp_passed = sum(1 for c in mcp_checks.values() if c.get("passed"))
        mcp_total = len(mcp_checks)
        mcp_str = f"{mcp_passed}/{mcp_total}" if mcp_total else "N/A"

        # Count HTTP check passes/fails
        http_results = r.get("http_results", [])
        http_passed = sum(1 for h in http_results if h.get("passed"))
        http_total = len(http_results)
        http_str = f"{http_passed}/{http_total}" if http_total else "N/A"

        all_passed = (
            mcp_passed == mcp_total
            and http_passed == http_total
            and not r.get("error")
        )
        if all_passed:
            total_pass += 1
        else:
            total_fail += 1

        status_icon = "PASS" if all_passed else "FAIL"
        name_display = r["scenario"][:col_name]

        print(
            f"  {name_display:<{col_name}} "
            f"{trace.turn_count:>{col_turns}} "
            f"{trace.total_tool_calls:>{col_tools}} "
            f"{total_tokens:>{col_tokens},} "
            f"{trace.elapsed_s:>{col_time}.1f}s "
            f"{mcp_str:>{col_mcp}} "
            f"{http_str:>{col_http}}  "
            f"{status_icon}"
        )

        if r.get("error"):
            print(f"    ERROR: {r['error'][:70]}")

    print(f"  {'─' * (len(header) - 2)}")
    print(f"  Total: {total_pass} passed, {total_fail} failed out of {len(results)} scenarios")
    print()


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description="Option C Test Matrix")
    parser.add_argument(
        "--scenario", action="append", dest="scenarios",
        help="Run specific scenario(s) by key (T1, T2, ...). Can be repeated.",
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

    # Select scenarios
    if args.scenarios:
        scenario_keys = args.scenarios
        for key in scenario_keys:
            if key not in SCENARIOS:
                print(f"Error: Unknown scenario '{key}'. Available: {list(SCENARIOS.keys())}", file=sys.stderr)
                sys.exit(1)
    else:
        scenario_keys = list(SCENARIOS.keys())

    print(f"Config: model={config.llm_model}, base_url={config.llm_base_url}", file=sys.stderr)
    print(f"Flowplane: {config.flowplane_url}, team={config.flowplane_team}", file=sys.stderr)
    print(f"Scenarios: {scenario_keys}", file=sys.stderr)
    print(f"Reset: {not args.no_reset}\n", file=sys.stderr)

    # Optionally reset environment
    if not args.no_reset:
        print("[1/3] Resetting environment...", file=sys.stderr)
        env_reset()

        print("[2/3] Bootstrapping CP...", file=sys.stderr)
        bootstrapper = CPBootstrapper(config.flowplane_url)
        bootstrapper.wait_for_cp(timeout_s=120.0)
        boot = bootstrapper.bootstrap()
        config = AgentConfig(
            flowplane_url=config.flowplane_url,
            flowplane_team=boot.team,
            flowplane_token=boot.token,
            llm_base_url=config.llm_base_url,
            llm_api_key=config.llm_api_key,
            llm_model=config.llm_model,
        )
    else:
        print("[1/3] Skipping env reset (using existing CP)...", file=sys.stderr)
        print("[2/3] Using .env credentials...", file=sys.stderr)

    # Connect MCP once
    print("[3/3] Initializing MCP client...", file=sys.stderr)
    mcp = FlowplaneMCPClient(config.flowplane_url, config.flowplane_team, config.flowplane_token)
    mcp.initialize()
    tools = mcp.list_tools()
    print(f"       MCP session ready, {len(tools)} tools available", file=sys.stderr)

    # Run scenarios sequentially
    all_results: list[dict] = []
    matrix_start = time.monotonic()

    for key in scenario_keys:
        scenario = SCENARIOS[key]
        try:
            result = run_scenario(scenario, config, mcp)
            all_results.append(result)
        except Exception as e:
            print(f"\nFATAL: {scenario.name} failed: {e}", file=sys.stderr)
            traceback.print_exc(file=sys.stderr)
            all_results.append({
                "scenario": scenario.name,
                "trace": SpikeTrace(),
                "mcp_checks": {},
                "http_results": [],
                "error": str(e),
            })

    matrix_elapsed = time.monotonic() - matrix_start
    mcp.close()

    # Print summary
    print_summary(all_results, config.llm_model)
    print(f"  Total wall clock: {matrix_elapsed:.1f}s", file=sys.stderr)

    # Dump traces for post-analysis
    trace_file = Path(__file__).resolve().parent / "last_matrix_run.json"
    dump = []
    for r in all_results:
        t: SpikeTrace = r["trace"]
        dump.append({
            "scenario": r["scenario"],
            "turn_count": t.turn_count,
            "total_tool_calls": t.total_tool_calls,
            "prompt_tokens": t.prompt_tokens,
            "completion_tokens": t.completion_tokens,
            "elapsed_s": t.elapsed_s,
            "tool_calls": t.tool_calls,
            "mcp_checks": {
                k: {"passed": v["passed"], "detail": str(v.get("detail", ""))}
                for k, v in r.get("mcp_checks", {}).items()
            },
            "http_results": r.get("http_results", []),
            "error": r.get("error", ""),
        })
    trace_file.write_text(json.dumps(dump, indent=2))
    print(f"  Traces saved to: {trace_file}", file=sys.stderr)


if __name__ == "__main__":
    main()
