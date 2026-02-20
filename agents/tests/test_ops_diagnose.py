"""Ops agent integration tests — diagnostic scenarios.

Tests that the ops agent uses the correct diagnostic tools when asked
about missing routes, topology, configuration validation, bad upstreams,
and unattached filters.

Unlike dev tests, ops tests primarily assert on which tools were called
(from the ConversationTrace) rather than on CP state changes.
"""

from __future__ import annotations

import pytest

from testing.harness import run_agent_scenario
from testing.fixtures import make_scenario, scenario_to_prompt, unique_port


def assert_answer_mentions(trace, terms: list[str], context: str = ""):
    """Assert the agent answer mentions at least one of the given terms."""
    answer_lower = trace.answer.lower()
    matched = [t for t in terms if t.lower() in answer_lower]
    assert matched, (
        f"Expected answer to mention one of {terms}{f' ({context})' if context else ''}, "
        f"but answer was: {trace.answer[:500]}"
    )


@pytest.mark.timeout(360)
def test_diagnose_missing_route(dev_agent, ops_agent, state, test_prefix):
    """Deploy an API with /get, then ask why /api/users returns 404."""
    # Setup: deploy API with only /get
    port = unique_port()
    scenario = make_scenario(test_prefix, port=port, path="/get")
    prompt = scenario_to_prompt(scenario)
    setup_trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    setup_trace.assert_no_error()
    state.assert_listener_exists(scenario.listener_name, port=port)

    # Diagnose: ask about a path that doesn't exist
    diag_prompt = (
        f"Requests to localhost:{port}/api/users are returning 404. "
        f"Trace the request and explain why."
    )
    trace = run_agent_scenario(ops_agent, diag_prompt, timeout_s=120.0)
    trace.assert_no_error()
    trace.assert_max_turns(8)

    trace.assert_tool_called("ops_trace_request")
    assert_answer_mentions(
        trace,
        ["route", "no route", "not found", "path", "missing", "404"],
        context="diagnosis should mention route/path issue",
    )


@pytest.mark.timeout(360)
def test_diagnose_bad_upstream(dev_agent, ops_agent, state, test_prefix):
    """Deploy an API pointing to a nonexistent backend, then diagnose 503."""
    port = unique_port()
    scenario = make_scenario(
        test_prefix, port=port,
        backend_host="localhost", backend_port=59999,
    )
    prompt = scenario_to_prompt(scenario)
    setup_trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    setup_trace.assert_no_error()
    state.assert_listener_exists(scenario.listener_name, port=port)

    diag_prompt = (
        f"Requests to port {port} are failing with 503. What's wrong?"
    )
    trace = run_agent_scenario(ops_agent, diag_prompt, timeout_s=120.0)
    trace.assert_no_error()
    trace.assert_max_turns(8)

    # Agent should use diagnostic tools
    diag_tools = trace.called_tools() & {
        "ops_trace_request", "ops_topology", "ops_config_validate",
        "cp_list_clusters", "cp_query_cluster",
    }
    assert len(diag_tools) >= 1, (
        f"Expected diagnostic tools, but only called: {trace.called_tools()}"
    )
    assert_answer_mentions(
        trace,
        ["upstream", "backend", "cluster", "unreachable", "connection", "503", "endpoint"],
        context="diagnosis should mention upstream/backend issue",
    )


@pytest.mark.timeout(420)
def test_diagnose_filter_not_attached(dev_agent, ops_agent, state, mcp_client, test_prefix):
    """Deploy an API, create an unattached filter, then diagnose."""
    port = unique_port()
    scenario = make_scenario(test_prefix, port=port)
    prompt = scenario_to_prompt(scenario)
    setup_trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    setup_trace.assert_no_error()
    state.assert_listener_exists(scenario.listener_name, port=port)

    # Create filter directly via MCP (not attached to anything)
    filter_name = f"{test_prefix}-rl-orphan"
    mcp_client.call_tool("cp_create_filter", {
        "name": filter_name,
        "filterType": "local_rate_limit",
        "configuration": {
            "stat_prefix": "test_rate_limit",
            "token_bucket": {"max_tokens": 50, "tokens_per_fill": 50, "fill_interval_ms": 60000},
            "status_code": 429,
        },
    })

    diag_prompt = (
        f"I created a rate limit filter named '{filter_name}' but rate limiting "
        f"isn't working on port {port}. Why?"
    )
    trace = run_agent_scenario(ops_agent, diag_prompt, timeout_s=120.0)
    trace.assert_no_error()
    trace.assert_max_turns(8)

    assert_answer_mentions(
        trace,
        ["attach", "not attached", "unattached", "apply", "bind", "associate", "listener"],
        context="diagnosis should mention filter not attached",
    )


@pytest.mark.timeout(180)
def test_diagnose_topology(ops_agent):
    """Ask for the gateway topology — agent should call ops_topology."""
    prompt = "Show me the complete gateway topology. What's deployed and how are resources connected?"

    trace = run_agent_scenario(ops_agent, prompt, timeout_s=120.0)
    trace.assert_no_error()

    # Ops agent should call topology for a big-picture view
    trace.assert_tool_called("ops_topology")

    # Should also cross-check with list tools (per system prompt)
    list_tools_called = trace.called_tools() & {
        "cp_list_clusters", "cp_list_listeners",
        "cp_list_virtual_hosts", "cp_list_routes",
    }
    assert len(list_tools_called) >= 1, (
        f"Ops agent should cross-check topology with list tools, "
        f"but only called: {trace.called_tools()}"
    )


@pytest.mark.timeout(180)
def test_diagnose_config_validation(ops_agent):
    """Ask for a health check — agent should call ops_config_validate."""
    prompt = "Is the gateway configuration healthy? Run a full configuration validation and report any issues."

    trace = run_agent_scenario(ops_agent, prompt, timeout_s=120.0)
    trace.assert_no_error()

    # Ops agent must use config_validate for health checks
    trace.assert_tool_called("ops_config_validate")

    assert len(trace.answer) > 0, "Ops agent should provide a health report"
