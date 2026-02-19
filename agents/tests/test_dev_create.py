"""Dev agent integration tests — resource creation scenarios.

Tests that the dev agent can create gateway resources (cluster, route_config,
listener, virtual_host, route) via MCP tools, and that the resources exist
in the CP after the conversation completes.
"""

from __future__ import annotations

import pytest

from testing.harness import run_agent_scenario
from testing.fixtures import make_scenario, scenario_to_prompt, multi_path_prompt, unique_port


@pytest.mark.timeout(300)
def test_create_simple_api(dev_agent, state, test_prefix):
    """Dev agent creates a single-path API: cluster, RC, listener, VH, route."""
    scenario = make_scenario(test_prefix, port=unique_port())
    prompt = scenario_to_prompt(scenario)

    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()
    trace.assert_max_turns(12)

    # Assert all resources were created
    state.assert_cluster_exists(scenario.cluster_name)
    state.assert_route_config_exists(scenario.route_config_name)
    state.assert_listener_exists(scenario.listener_name, port=scenario.port)
    state.assert_virtual_host_exists(scenario.virtual_host_name)
    state.assert_route_exists(scenario.virtual_host_name, scenario.path)

    # Agent should have called cp_create_* tools
    created_tools = {n for n in trace.called_tools() if n.startswith("cp_create_")}
    assert len(created_tools) >= 3, f"Expected >= 3 create tools, got: {created_tools}"


@pytest.mark.timeout(300)
def test_create_multi_path_api(dev_agent, state, test_prefix):
    """Dev agent creates an API with /get and /post under the same VH."""
    scenario = make_scenario(test_prefix, port=unique_port())
    paths = ["/get", "/post"]
    prompt = multi_path_prompt(scenario, paths)

    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()

    state.assert_cluster_exists(scenario.cluster_name)
    state.assert_listener_exists(scenario.listener_name, port=scenario.port)
    state.assert_virtual_host_exists(scenario.virtual_host_name)

    # Both routes should exist
    state.assert_route_exists(scenario.virtual_host_name, "/get")
    state.assert_route_exists(scenario.virtual_host_name, "/post")


@pytest.mark.timeout(300)
def test_create_specific_port(dev_agent, state, test_prefix):
    """Dev agent creates a listener on a specific requested port."""
    port = unique_port()
    scenario = make_scenario(test_prefix, port=port)
    prompt = scenario_to_prompt(scenario)

    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()

    listener = state.assert_listener_exists(scenario.listener_name, port=port)
    assert listener is not None

    # Also verify via port query
    state.assert_listener_port(port)
