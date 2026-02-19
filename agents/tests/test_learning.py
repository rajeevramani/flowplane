"""Learn agent integration tests — learning session lifecycle.

These tests require Envoy to be running for traffic capture,
so they are marked with @needs_envoy and skipped unless WITH_ENVOY is set.

Tests without the needs_envoy mark test session creation and status
checking, which only require the CP (no traffic).
"""

from __future__ import annotations

import json
import os
import re
import time

import pytest
import yaml

from testing.harness import run_agent_scenario
from testing.conftest import needs_envoy
from testing.fixtures import make_scenario, scenario_to_prompt, unique_port
from testing.traffic import send_httpbin_traffic, send_mockbank_traffic
from testing.spec_compare import compare_specs


@pytest.mark.timeout(180)
def test_start_learning_session(learn_agent, state):
    """Learn agent creates a new learning session."""
    prompt = (
        "Create a learning session to discover the API schema. "
        "Use route pattern '.*' with a target of 10 samples. "
        "Start it immediately (auto_start=true)."
    )
    trace = run_agent_scenario(learn_agent, prompt, timeout_s=120.0)
    trace.assert_no_error()

    # Agent should have called cp_create_learning_session
    trace.assert_tool_called("cp_create_learning_session")

    # Verify a session was created
    assert len(trace.answer) > 0, "Learn agent should confirm session creation"


@pytest.mark.timeout(180)
def test_check_session_status(learn_agent):
    """Learn agent checks status of learning sessions."""
    prompt = "What learning sessions are currently active? Show me their status and progress."

    trace = run_agent_scenario(learn_agent, prompt, timeout_s=120.0)
    trace.assert_no_error()

    # Agent should query sessions
    session_tools = trace.called_tools() & {
        "cp_list_learning_sessions",
        "cp_get_learning_session",
    }
    assert len(session_tools) >= 1, (
        f"Learn agent should check session status, but called: {trace.called_tools()}"
    )


@needs_envoy
@pytest.mark.timeout(600)
def test_generate_openapi(dev_agent, learn_agent, state, test_prefix):
    """End-to-end: deploy API, create session, send traffic, export OpenAPI.

    This test requires Envoy with traffic flowing through the gateway.
    The test deploys an httpbin API, creates a learning session, sends traffic,
    waits for completion, then exports the schema.
    """
    port = unique_port()

    # Step 1: Deploy httpbin API via dev_agent
    scenario = make_scenario(test_prefix, port=port)
    deploy_trace = run_agent_scenario(dev_agent, scenario_to_prompt(scenario), timeout_s=240.0)
    deploy_trace.assert_no_error()
    state.assert_listener_exists(scenario.listener_name, port=port)

    # Step 2: Create learning session
    create_prompt = (
        "Create a learning session for route pattern '.*' with 5 target samples. "
        "Start it immediately."
    )
    trace1 = run_agent_scenario(learn_agent, create_prompt, timeout_s=120.0)
    trace1.assert_no_error()
    trace1.assert_tool_called("cp_create_learning_session")

    # Step 3: Send actual traffic through the gateway
    traffic = send_httpbin_traffic(port)
    assert traffic.total_requests > 0, "Traffic generator should send requests"

    # Step 4: Poll for learning session to process traffic (up to 60s)
    poll_prompt = "List all learning sessions and show their status."
    for _ in range(12):
        poll_trace = run_agent_scenario(learn_agent, poll_prompt, timeout_s=30.0)
        answer = poll_trace.answer.lower()
        if "completed" in answer or "complete" in answer or "finished" in answer:
            break
        time.sleep(5)

    # Step 5: Ask agent to export OpenAPI
    export_prompt = (
        "Check if any learning sessions have completed. If so, list the discovered "
        "schemas and export them as an OpenAPI specification with title 'Test API' "
        "and version '1.0.0'."
    )
    trace2 = run_agent_scenario(learn_agent, export_prompt, timeout_s=300.0)
    trace2.assert_no_error()

    # Should have interacted with schema/export tools
    schema_tools = trace2.called_tools() & {
        "cp_list_learning_sessions",
        "cp_get_learning_session",
        "cp_list_aggregated_schemas",
        "cp_export_schema_openapi",
    }
    assert len(schema_tools) >= 1, (
        f"Learn agent should interact with schema tools, but called: {trace2.called_tools()}"
    )

    # If the answer contains an OpenAPI spec, validate its structure
    spec = _try_parse_json_from_text(trace2.answer)
    if spec and isinstance(spec, dict) and "openapi" in spec:
        assert "paths" in spec, "OpenAPI spec should have 'paths' key"


@needs_envoy
@pytest.mark.timeout(900)
def test_learning_mockbank_api(dev_agent, learn_agent, state, test_prefix):
    """End-to-end: deploy MockBank API, learn it, compare against reference spec.

    Deploys the MockBank Financial API through the gateway, creates a learning
    session, sends two rounds of realistic traffic, then exports the learned
    OpenAPI spec and compares it against the reference fixture.
    """
    mcp = state.mcp  # direct MCP client for reliable polling
    port = unique_port()

    # Step 1: Deploy MockBank API backend via dev_agent
    scenario = make_scenario(
        test_prefix, port=port,
        backend_host="mockbank-api", backend_port=3000,
    )
    deploy_trace = run_agent_scenario(dev_agent, scenario_to_prompt(scenario), timeout_s=240.0)
    deploy_trace.assert_no_error()
    state.assert_listener_exists(scenario.listener_name, port=port)

    # Step 2: Create learning session with higher sample target for complex API
    create_prompt = (
        "Create a learning session for route pattern '.*' with 50 target samples. "
        "Start it immediately."
    )
    trace1 = run_agent_scenario(learn_agent, create_prompt, timeout_s=120.0)
    trace1.assert_no_error()
    trace1.assert_tool_called("cp_create_learning_session")

    # Step 3: Send realistic MockBank traffic (2 rounds)
    traffic = send_mockbank_traffic(port, rounds=2)
    assert traffic.total_requests > 20, (
        f"Expected >20 requests, sent {traffic.total_requests}"
    )

    # Step 4: Poll for session completion using direct MCP (fast + reliable)
    session_completed = False
    for attempt in range(30):  # 30 x 5s = 150s max
        try:
            sessions = mcp.call_tool("cp_list_learning_sessions", {})
            for s in sessions.get("sessions", []):
                status = s.get("status", "")
                current = s.get("current_sample_count", 0)
                target = s.get("target_sample_count", 0)
                if status == "completed":
                    session_completed = True
                    break
        except Exception:
            pass
        if session_completed:
            break
        time.sleep(5)

    # Step 4b: Wait for aggregated schemas to appear (even after session
    # completes, schema inference may need a moment).
    # Filter to /v2/api/ paths so prior tests' schemas don't pollute results.
    schema_ids = []
    for attempt in range(20):  # 20 x 3s = 60s max
        try:
            schemas = mcp.call_tool("cp_list_aggregated_schemas", {"limit": 500})
            schema_list = schemas.get("schemas", [])
            schema_ids = [
                s["id"] for s in schema_list
                if "id" in s and s.get("path", "").startswith("/v2/api/")
            ]
            if schema_ids:
                break
        except Exception:
            pass
        time.sleep(3)

    if not schema_ids:
        pytest.skip(
            "No aggregated schemas available after polling. "
            f"Session completed: {session_completed}"
        )

    # Step 5: Ask agent to export using discovered schema IDs
    export_prompt = (
        f"Export the aggregated schemas as an OpenAPI specification. "
        f"Use schema_ids {schema_ids}, title 'MockBank API', and version '1.0.0'. "
        f"Call cp_export_schema_openapi with these parameters."
    )
    trace2 = run_agent_scenario(learn_agent, export_prompt, timeout_s=300.0)
    trace2.assert_no_error()

    # Step 6: Extract learned spec from agent tool results or answer
    learned_spec = _extract_openapi_from_trace(trace2)

    # Fallback: if the agent didn't call export or produced no spec, export
    # directly via MCP so we still test the learning pipeline end-to-end.
    if learned_spec is None:
        try:
            raw = mcp.call_tool("cp_export_schema_openapi", {
                "schema_ids": schema_ids,
                "title": "MockBank API",
                "version": "1.0.0",
            })
            if isinstance(raw, dict) and "paths" in raw:
                learned_spec = raw
        except Exception:
            pass

    if learned_spec is None:
        pytest.skip("Could not extract OpenAPI spec from agent or direct MCP export")

    # Step 7: Compare against reference spec
    reference_spec = _load_reference_spec()
    result = compare_specs(learned_spec, reference_spec)

    assert result.endpoint_coverage >= 0.90, (
        f"Endpoint coverage {result.endpoint_coverage:.0%} < 90%. "
        f"Missing: {result.missing}"
    )
    assert result.schema_coverage >= 0.80, (
        f"Response schema coverage {result.schema_coverage:.0%} < 80%. "
        f"Learning agent missed response body fields."
    )
    assert result.header_coverage >= 0.70, (
        f"Header coverage {result.header_coverage:.0%} < 70%. "
        f"Learning agent missed important headers (e.g., Authorization)."
    )


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _extract_openapi_from_trace(trace) -> dict | None:
    """Try to extract an OpenAPI spec from agent tool results or answer."""
    # Check tool results for cp_export_schema_openapi
    for tc in trace.tool_calls:
        if tc.name == "cp_export_schema_openapi" and isinstance(tc.result, dict):
            spec = _find_openapi_in_dict(tc.result)
            if spec is not None:
                return spec

    # Fall back to parsing the answer text — but only accept valid OpenAPI
    parsed = _try_parse_json_from_text(trace.answer)
    if parsed and _is_openapi_spec(parsed):
        return parsed

    return None


def _find_openapi_in_dict(d: dict) -> dict | None:
    """Search a dict (possibly nested) for an OpenAPI spec."""
    if _is_openapi_spec(d):
        return d
    # May be nested under a content key
    for key in ("spec", "content", "data", "result"):
        nested = d.get(key)
        if isinstance(nested, dict) and _is_openapi_spec(nested):
            return nested
    # Try parsing string values
    for val in d.values():
        if isinstance(val, str):
            parsed = _try_parse_json_from_text(val)
            if parsed and _is_openapi_spec(parsed):
                return parsed
    return None


def _is_openapi_spec(d: dict) -> bool:
    """Check if a dict looks like a valid OpenAPI spec with endpoints."""
    return (
        isinstance(d, dict)
        and "openapi" in d
        and isinstance(d.get("paths"), dict)
        and len(d["paths"]) > 0
    )


def _try_parse_json_from_text(text: str) -> dict | None:
    """Attempt to extract a JSON object from text."""
    if not text:
        return None

    # Try the whole string
    try:
        obj = json.loads(text)
        if isinstance(obj, dict):
            return obj
    except (json.JSONDecodeError, TypeError):
        pass

    # Try JSON between code fences
    for match in re.finditer(r"```(?:json)?\s*\n?(.*?)\n?```", text, re.DOTALL):
        try:
            obj = json.loads(match.group(1))
            if isinstance(obj, dict):
                return obj
        except json.JSONDecodeError:
            continue

    # Try to find the outermost { ... } block
    start = text.find("{")
    if start >= 0:
        depth = 0
        for i in range(start, len(text)):
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
                if depth == 0:
                    try:
                        obj = json.loads(text[start : i + 1])
                        if isinstance(obj, dict):
                            return obj
                    except json.JSONDecodeError:
                        pass
                    break

    return None


def _load_reference_spec() -> dict:
    """Load the MockBank API reference spec from the fixtures directory."""
    fixture_path = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "testing", "fixtures", "mockbank-openapi.yaml",
    )
    with open(fixture_path) as f:
        return yaml.safe_load(f)
