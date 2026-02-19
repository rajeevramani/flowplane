"""Shared pytest fixtures for agent integration tests.

Provides:
- bootstrap_result: session-scoped CP bootstrap (admin, org, team, dataplane, token)
- mcp_client: session-scoped FlowplaneMCPClient for state assertions
- state: session-scoped CPStateHelper
- test_prefix: function-scoped unique prefix for resource names
- dev_agent / ops_agent / learn_agent: function-scoped agent instances
- cleanup_test_resources: autouse cleanup after each test
- _cassette_client: function-scoped LLM record/replay cassette

Environment variables:
    FLOWPLANE_TEST_URL      CP base URL (default: http://localhost:8090)
    LLM_BASE_URL            LLM API endpoint (default: https://api.openai.com/v1)
    LLM_API_KEY             LLM API key (required for agent tests, optional in replay)
    LLM_MODEL               Model name (default: gpt-4o)
    AGENT_TEST_MODE         "live" (default), "record", "replay", or "replay-or-live"
    WITH_ENVOY              Set to enable Envoy-dependent tests
    FLOWPLANE_SKIP_BOOTSTRAP  Set to skip bootstrap (use existing CP state)
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

# Ensure agents/ is on the import path
_agents_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _agents_dir not in sys.path:
    sys.path.insert(0, _agents_dir)

from mcp_client import FlowplaneMCPClient
from agent import FlowplaneAgent, Guardrails

from .cp_helpers import CPBootstrapper, CPStateHelper
from .fixtures import unique_prefix, reset_port_tracking
from .replay import CassetteOpenAIClient


# ---------------------------------------------------------------------------
# Environment helpers
# ---------------------------------------------------------------------------

def _test_url() -> str:
    return os.environ.get("FLOWPLANE_TEST_URL", "http://localhost:8090")


def _llm_base_url() -> str:
    return os.environ.get("LLM_BASE_URL", "https://api.openai.com/v1")


def _llm_api_key() -> str:
    key = os.environ.get("LLM_API_KEY", "")
    if not key:
        mode = _agent_test_mode()
        # Pure replay never calls LLM — dummy key is fine.
        # replay-or-live might fall back to live, but we can't know yet at
        # this point.  Return a dummy so agent construction doesn't skip;
        # if the test actually goes live with this dummy key, the OpenAI
        # client will fail with an auth error (clear signal to set the key).
        if mode in ("replay", "replay-or-live"):
            return "replay-no-key-needed"
        pytest.skip("LLM_API_KEY not set — skipping agent test")
    return key


def _llm_model() -> str:
    return os.environ.get("LLM_MODEL", "gpt-4o")


def _agent_test_mode() -> str:
    """Return the agent test mode: 'live', 'record', 'replay', or 'replay-or-live'."""
    return os.environ.get("AGENT_TEST_MODE", "live")


def _cassette_path(request: pytest.FixtureRequest) -> Path:
    """Derive cassette file path from the test's file and function name.

    Layout: agents/testing/cassettes/{test_file_stem}/{test_function_name}.json
    """
    test_file = Path(request.fspath)
    test_name = request.node.name
    cassettes_dir = Path(__file__).parent / "cassettes"
    return cassettes_dir / test_file.stem / f"{test_name}.json"


# ---------------------------------------------------------------------------
# Agent builder helpers (mirrors dev_agent.py, ops_agent.py, learn_agent.py)
# ---------------------------------------------------------------------------

def _build_dev_agent(mcp: FlowplaneMCPClient) -> FlowplaneAgent:
    """Build a dev agent matching dev_agent.py:_build_agent()."""
    from dev_agent import SYSTEM_PROMPT, ALLOWED_TOOLS

    guardrails = Guardrails(mcp)
    guardrails.enable_auto_preflight().enable_dataplane_injection()
    guardrails.enable_name_dedup().enable_port_validation()

    return FlowplaneAgent(
        mcp_client=mcp,
        llm_base_url=_llm_base_url(),
        api_key=_llm_api_key(),
        model=_llm_model(),
        system_prompt=SYSTEM_PROMPT,
        allowed_tools=ALLOWED_TOOLS,
        guardrails=guardrails,
    )


def _build_ops_agent(mcp: FlowplaneMCPClient) -> FlowplaneAgent:
    """Build an ops agent matching ops_agent.py."""
    from ops_agent import SYSTEM_PROMPT, ALLOWED_TOOLS

    return FlowplaneAgent(
        mcp_client=mcp,
        llm_base_url=_llm_base_url(),
        api_key=_llm_api_key(),
        model=_llm_model(),
        system_prompt=SYSTEM_PROMPT,
        allowed_tools=ALLOWED_TOOLS,
    )


def _build_learn_agent(mcp: FlowplaneMCPClient) -> FlowplaneAgent:
    """Build a learn agent matching learn_agent.py."""
    from learn_agent import SYSTEM_PROMPT, ALLOWED_TOOLS

    return FlowplaneAgent(
        mcp_client=mcp,
        llm_base_url=_llm_base_url(),
        api_key=_llm_api_key(),
        model=_llm_model(),
        system_prompt=SYSTEM_PROMPT,
        allowed_tools=ALLOWED_TOOLS,
    )


def _wrap_agent_llm(agent: FlowplaneAgent, cassette: CassetteOpenAIClient) -> None:
    """Replace the agent's LLM client with the cassette.

    The cassette's real_client is set during fixture creation (record mode),
    so we only need to swap the agent's LLM reference.
    """
    agent.llm = cassette


# ---------------------------------------------------------------------------
# Session-scoped fixtures (shared across all tests)
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def bootstrap_result():
    """Bootstrap the CP once per session: admin, org, team, dataplane, token."""
    if os.environ.get("FLOWPLANE_SKIP_BOOTSTRAP"):
        pytest.skip("FLOWPLANE_SKIP_BOOTSTRAP set — skipping bootstrap")

    bootstrapper = CPBootstrapper(_test_url())
    bootstrapper.wait_for_cp(timeout_s=60.0)
    result = bootstrapper.bootstrap()

    # If Envoy tests are enabled, generate bootstrap config and start Envoy
    if os.environ.get("WITH_ENVOY"):
        bootstrapper.generate_envoy_bootstrap(result.dataplane_id)
        bootstrapper.start_envoy(timeout_s=60.0)

    return result


@pytest.fixture(scope="session")
def mcp_client(bootstrap_result):
    """Session-scoped MCP client using the bootstrapped org-admin token."""
    client = FlowplaneMCPClient(
        bootstrap_result.base_url,
        bootstrap_result.team,
        bootstrap_result.token,
    )
    client.initialize()
    yield client
    client.close()


@pytest.fixture(scope="session")
def state(mcp_client):
    """Session-scoped CPStateHelper for assertions."""
    return CPStateHelper(mcp_client)


# ---------------------------------------------------------------------------
# Function-scoped fixtures (fresh per test)
# ---------------------------------------------------------------------------

@pytest.fixture
def _cassette_client(request):
    """Function-scoped LLM cassette for record/replay modes.

    Returns None in live mode (or replay-or-live fallback), a CassetteOpenAIClient
    in record/replay.
    """
    reset_port_tracking()

    mode = _agent_test_mode()
    if mode == "live":
        yield None
        return

    path = _cassette_path(request)

    if mode == "record":
        from openai import OpenAI
        real_client = OpenAI(base_url=_llm_base_url(), api_key=_llm_api_key())
        cassette = CassetteOpenAIClient(
            mode="record",
            cassette_path=path,
            real_client=real_client,
        )
        request.addfinalizer(cassette.save)
        yield cassette
    elif mode == "replay":
        cassette = CassetteOpenAIClient(mode="replay", cassette_path=path)
        cassette.load()
        yield cassette
    elif mode == "replay-or-live":
        if path.exists():
            cassette = CassetteOpenAIClient(mode="replay", cassette_path=path)
            cassette.load()
            yield cassette
        else:
            # No cassette — fall back to live (returns None like live mode)
            yield None
    else:
        raise ValueError(f"Unknown AGENT_TEST_MODE: {mode!r}")


@pytest.fixture
def test_prefix(_cassette_client):
    """Unique 8-char prefix for resource names in this test."""
    cassette = _cassette_client
    if cassette is None:
        return unique_prefix()

    if cassette.mode == "replay":
        return cassette.prefix

    # record mode: generate a new prefix and store it on the cassette
    prefix = unique_prefix()
    cassette.prefix = prefix
    cassette.model = _llm_model()
    return prefix


@pytest.fixture
def dev_agent(mcp_client, _cassette_client):
    """Fresh dev agent instance for this test."""
    agent = _build_dev_agent(mcp_client)
    if _cassette_client is not None:
        _wrap_agent_llm(agent, _cassette_client)
    return agent


@pytest.fixture
def ops_agent(mcp_client, _cassette_client):
    """Fresh ops agent instance for this test."""
    agent = _build_ops_agent(mcp_client)
    if _cassette_client is not None:
        _wrap_agent_llm(agent, _cassette_client)
    return agent


@pytest.fixture
def learn_agent(mcp_client, _cassette_client):
    """Fresh learn agent instance for this test."""
    agent = _build_learn_agent(mcp_client)
    if _cassette_client is not None:
        _wrap_agent_llm(agent, _cassette_client)
    return agent


@pytest.fixture(autouse=True)
def cleanup_test_resources(request, state, test_prefix):
    """After each test, delete any resources created with the test prefix."""
    yield
    state.delete_resources_with_prefix(test_prefix)


# ---------------------------------------------------------------------------
# Marks
# ---------------------------------------------------------------------------

needs_envoy = pytest.mark.skipif(
    not os.environ.get("WITH_ENVOY"),
    reason="WITH_ENVOY not set — skipping Envoy-dependent test",
)
