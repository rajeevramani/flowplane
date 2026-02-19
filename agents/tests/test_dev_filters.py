"""Dev agent integration tests — filter creation and management.

Tests that the dev agent can create and attach various filter types
(rate limit, CORS, JWT) and update existing filters.
"""

from __future__ import annotations

import pytest

from testing.harness import run_agent_scenario
from testing.fixtures import make_scenario, scenario_to_prompt, multi_path_prompt, unique_port
from testing.conftest import needs_envoy


def _deploy_base(dev_agent, state, test_prefix, port):
    """Helper: deploy a base API for filter tests."""
    scenario = make_scenario(test_prefix, port=port)
    prompt = scenario_to_prompt(scenario)
    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()
    state.assert_listener_exists(scenario.listener_name, port=port)
    return scenario


@pytest.mark.timeout(360)
def test_add_rate_limit(dev_agent, state, test_prefix):
    """Create an API, then add a rate limit filter."""
    scenario = _deploy_base(dev_agent, state, test_prefix, port=unique_port())

    filter_name = f"{test_prefix}-ratelimit"
    prompt = (
        f"Create a rate limit filter named '{filter_name}' that allows "
        f"100 requests per minute, and attach it to the listener "
        f"'{scenario.listener_name}'."
    )
    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()

    state.assert_filter_exists(filter_name)
    # Should have called cp_create_filter and cp_attach_filter
    assert "cp_create_filter" in trace.called_tools()


@pytest.mark.timeout(360)
def test_add_cors(dev_agent, state, test_prefix):
    """Create an API, then add a CORS filter."""
    scenario = _deploy_base(dev_agent, state, test_prefix, port=unique_port())

    filter_name = f"{test_prefix}-cors"
    prompt = (
        f"Create a CORS filter named '{filter_name}' that allows origins "
        f"'http://localhost:3000' with methods GET, POST, PUT, DELETE. "
        f"Attach it to the listener '{scenario.listener_name}'."
    )
    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()

    state.assert_filter_exists(filter_name)


@pytest.mark.timeout(360)
def test_add_jwt(dev_agent, state, test_prefix):
    """Create an API, then add a JWT authentication filter."""
    scenario = _deploy_base(dev_agent, state, test_prefix, port=unique_port())

    filter_name = f"{test_prefix}-jwt"
    prompt = (
        f"Create a JWT authentication filter named '{filter_name}' with "
        f"issuer 'https://auth.example.com' and JWKS URI "
        f"'https://auth.example.com/.well-known/jwks.json'. "
        f"Attach it to the listener '{scenario.listener_name}'."
    )
    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()

    state.assert_filter_exists(filter_name)


@pytest.mark.timeout(480)
def test_rate_limit_route_override(dev_agent, state, test_prefix):
    """Add a global rate limit, then override it on a specific route."""
    scenario = make_scenario(test_prefix, port=unique_port())
    prompt = multi_path_prompt(scenario, ["/get", "/post"])
    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()
    state.assert_listener_exists(scenario.listener_name, port=scenario.port)

    # Step 2: Add global rate limit on the virtual host
    global_prompt = (
        f"Add a rate limit filter to listener '{scenario.listener_name}' that limits "
        f"the entire virtual host '{scenario.virtual_host_name}' to 100 requests per minute."
    )
    trace2 = run_agent_scenario(dev_agent, global_prompt, timeout_s=240.0)
    trace2.assert_no_error()
    trace2.assert_max_turns(12)

    # Step 3: Override rate limit on /post route only
    override_prompt = (
        f"Now override the rate limit on the '/post' route under virtual host "
        f"'{scenario.virtual_host_name}' to only 10 requests per minute. "
        f"The global 100 req/min limit should still apply to '/get'."
    )
    trace3 = run_agent_scenario(dev_agent, override_prompt, timeout_s=240.0)
    trace3.assert_no_error()
    trace3.assert_max_turns(12)

    # Agent should acknowledge the per-route override
    answer = trace3.answer.lower()
    assert "post" in answer or "/post" in answer, (
        "Agent answer should reference the /post route override"
    )


@pytest.mark.timeout(480)
def test_jwt_route_exemption(dev_agent, state, test_prefix):
    """Add JWT validation globally but exempt the /health route."""
    scenario = make_scenario(test_prefix, port=unique_port())
    prompt = multi_path_prompt(scenario, ["/api/data", "/health"])
    trace = run_agent_scenario(dev_agent, prompt, timeout_s=240.0)
    trace.assert_no_error()
    state.assert_listener_exists(scenario.listener_name, port=scenario.port)

    # Step 2: Add JWT with /health exemption
    jwt_prompt = (
        f"Add JWT authentication to listener '{scenario.listener_name}' with "
        f"issuer 'https://auth.example.com' and JWKS URI "
        f"'https://auth.example.com/.well-known/jwks.json'. "
        f"Apply JWT to all routes EXCEPT '/health' — the health endpoint must be "
        f"accessible without authentication."
    )
    trace2 = run_agent_scenario(dev_agent, jwt_prompt, timeout_s=240.0)
    trace2.assert_no_error()
    trace2.assert_max_turns(12)

    # Agent should mention the /health exemption
    answer = trace2.answer.lower()
    assert "health" in answer, (
        "Agent answer should mention /health exemption"
    )
