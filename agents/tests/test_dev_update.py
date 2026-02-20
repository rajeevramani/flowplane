"""Dev agent integration tests — update/modification scenarios.

Tests that the dev agent can add paths to existing virtual hosts and
add routes to existing listeners.
"""

from __future__ import annotations

import pytest

from testing.harness import run_agent_scenario
from testing.fixtures import make_scenario, scenario_to_prompt, unique_port


@pytest.mark.timeout(360)
def test_add_path_to_virtual_host(dev_agent, state, test_prefix):
    """Deploy with /get, then add /post to the same virtual host."""
    scenario = make_scenario(test_prefix, port=unique_port(), path="/get")
    prompt = scenario_to_prompt(scenario)

    # Step 1: Create initial deployment
    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()
    state.assert_route_exists(scenario.virtual_host_name, "/get")

    # Step 2: Add a second path
    add_prompt = (
        f"Add a new route with path prefix /post to the existing virtual host "
        f"'{scenario.virtual_host_name}' under route config '{scenario.route_config_name}'. "
        f"Use the existing cluster '{scenario.cluster_name}'. "
        f"Do NOT create a new listener or virtual host."
    )
    trace2 = run_agent_scenario(dev_agent, add_prompt, timeout_s=240.0)
    trace2.assert_no_error()

    # Both routes should exist
    state.assert_route_exists(scenario.virtual_host_name, "/get")
    state.assert_route_exists(scenario.virtual_host_name, "/post")


@pytest.mark.timeout(360)
def test_add_route_to_listener(dev_agent, state, test_prefix):
    """Deploy, then add a second route to the same listener."""
    scenario = make_scenario(test_prefix, port=unique_port(), path="/api/v1")

    # Step 1: Create initial deployment
    prompt = scenario_to_prompt(scenario)
    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()
    state.assert_listener_exists(scenario.listener_name, port=scenario.port)

    # Step 2: Add a second route
    add_prompt = (
        f"Add a new route with path prefix /api/v2 to the existing virtual host "
        f"'{scenario.virtual_host_name}'. Use the same cluster '{scenario.cluster_name}'. "
        f"The listener '{scenario.listener_name}' on port {scenario.port} already exists — "
        f"do NOT create a new one."
    )
    trace2 = run_agent_scenario(dev_agent, add_prompt, timeout_s=240.0)
    trace2.assert_no_error()

    state.assert_route_exists(scenario.virtual_host_name, "/api/v1")
    state.assert_route_exists(scenario.virtual_host_name, "/api/v2")
