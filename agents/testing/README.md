# Agent Integration Testing Framework

Integration tests that run Flowplane agents (dev, ops, learn) against a real control plane and assert on outcomes.

## Quick Start

```bash
# From project root:
make test-agents-up        # Start isolated test env
cd agents && FLOWPLANE_TEST_URL=http://localhost:8090 \
  LLM_API_KEY=sk-... \
  pytest tests/test_dev_create.py -v
make test-agents-down      # Tear down
```

Or all-in-one:

```bash
LLM_API_KEY=sk-... make test-agents
```

## Architecture

```
agents/
  testing/
    harness.py              # run_agent_scenario() wraps FlowplaneAgent.run_stream()
    cp_helpers.py           # CPBootstrapper (seeds CP) + CPStateHelper (assertions)
    fixtures.py             # APIScenario dataclass, unique name generation
    conftest.py             # Shared pytest fixtures
    docker-compose.test.yml # Isolated: postgres:5433, CP:8090, httpbin (internal)
    envoy-bootstrap.yml.tpl # Envoy bootstrap template (for traffic tests)
  tests/
    test_dev_create.py      # Dev agent: create simple API, multi-path, specific port
    test_dev_update.py      # Dev agent: add path to VH, add route to listener
    test_dev_filters.py     # Dev agent: rate limit, CORS, JWT, override
    test_ops_diagnose.py    # Ops agent: missing route, topology, config validation
    test_learning.py        # Learn agent: session lifecycle, OpenAPI export
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_TEST_URL` | `http://localhost:8090` | CP base URL |
| `LLM_BASE_URL` | `https://api.openai.com/v1` | LLM API endpoint |
| `LLM_API_KEY` | *(required)* | LLM API key |
| `LLM_MODEL` | `gpt-4o` | Model name |
| `WITH_ENVOY` | *(unset)* | Enable Envoy-dependent tests |
| `FLOWPLANE_SKIP_BOOTSTRAP` | *(unset)* | Skip CP bootstrap |

## Test Isolation

Each test gets a UUID-based prefix (e.g., `t1a2b3c4`). All resource names use this prefix. After each test, resources matching the prefix are auto-deleted. This avoids per-test org/team creation while preventing name collisions.

## Key Design Decisions

- **Assert on CP state, not conversation text**: Dev tests use `CPStateHelper` to call MCP list/get tools. Ops tests assert on which tools the agent called (from the trace).
- **Harness wraps `run_stream()`**: No agent code modifications. `ConversationTrace` captures all tool calls with timing.
- **Bootstrap mirrors seed-data.sh**: `CPBootstrapper` uses httpx with session cookies for the same auth flow.
- **Envoy is optional**: Most tests only need CP. Traffic tests are `@needs_envoy` and skipped without `WITH_ENVOY`.

## Running Subsets

```bash
# Single test
pytest tests/test_dev_create.py::test_create_simple_api -v

# Only ops tests
pytest tests/test_ops_diagnose.py -v

# Only learning tests (needs Envoy)
WITH_ENVOY=1 pytest tests/test_learning.py -v
```
