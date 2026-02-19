# Flowplane Agents

Lightweight, model-agnostic AI agents that connect to Flowplane's MCP server for API gateway deployment and diagnostics. They use any OpenAI-compatible LLM API and include guardrails, streaming output, and conversation memory.

## Quick Start

```bash
# From the project root - start Flowplane and seed demo data
make up && make seed

# Set environment (use the org-admin token from seed output)
export FLOWPLANE_URL=http://localhost:8080
export FLOWPLANE_TEAM=engineering
export FLOWPLANE_TOKEN=<org-admin token from seed output>
export LLM_BASE_URL=<your LLM endpoint>  # e.g., http://localhost:11434/v1 for Ollama
export LLM_API_KEY=<your LLM key>
export LLM_MODEL=<model name>  # e.g., gpt-4o, qwen3-coder-next:cloud

# Install dependencies and run
pip install -r agents/requirements.txt
python agents/dev_agent.py "Expose httpbin at localhost:8000 on path / at port 10001"
```

## Prerequisites

- Python 3.11+
- Install dependencies:

```bash
pip install -r requirements.txt
```

## Dev Agent

The dev agent handles gateway deployment tasks -- creating clusters, routes, and listeners.

**One-shot mode:**

```bash
python dev_agent.py "Expose httpbin at localhost:8000 on path / at port 10001"
```

**Interactive mode:**

```bash
python dev_agent.py
```

**Streaming mode:**

```bash
python dev_agent.py --stream "Expose httpbin at localhost:8000 on path / at port 10001"
```

**Verify a deployment:**

```bash
python dev_agent.py --verify --path /api/orders --port 10001
```

## Ops Agent

The ops agent handles diagnostics, topology inspection, and troubleshooting.

**One-shot mode:**

```bash
python ops_agent.py "Why is /api/users returning 404?"
```

**Interactive mode:**

```bash
python ops_agent.py
```

**Streaming mode:**

```bash
python ops_agent.py --stream "Show me the gateway topology"
```

## Learn Agent

The learn agent handles API schema discovery -- creating learning sessions to capture traffic, monitoring progress, and exporting OpenAPI specs.

**One-shot mode:**

```bash
python learn_agent.py "Learn the API on orders-routes and export OpenAPI"
```

**Interactive mode:**

```bash
python learn_agent.py
```

**Streaming mode:**

```bash
python learn_agent.py --stream "What schemas have been discovered?"
```

**Check session status (no LLM needed):**

```bash
python learn_agent.py --status
python learn_agent.py --status --session-id <uuid>
```

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `FLOWPLANE_URL` | Flowplane API base URL | `http://localhost:8080` |
| `FLOWPLANE_TEAM` | Team context for MCP | (required) |
| `FLOWPLANE_TOKEN` | PAT token (required) | - |
| `LLM_BASE_URL` | LLM API endpoint | `https://api.openai.com/v1` |
| `LLM_API_KEY` | LLM API key (required) | - |
| `LLM_MODEL` | Model name | `gpt-4o` |

> After running `make seed`, the default team is `engineering` under the `acme-corp` organization.

## Tested Models

The agents use the OpenAI function-calling format. Tested with GPT-4o and Qwen 3 (via Ollama).

## Example Output

A one-shot dev agent run (abbreviated):

```
$ python dev_agent.py "Expose httpbin at localhost:8000 on path / at port 10001"
  -> dev_preflight_check({"name":"httpbin","port":10001})
  -> cp_create_cluster({"name":"httpbin","endpoints":[{"address":"localhost","port":8000}]})
  -> cp_create_route_config({"name":"httpbin-rc","clusterName":"httpbin"})
  -> cp_create_listener({"name":"httpbin-listener","port":10001,"routeConfigName":"httpbin-rc"})

Deployment complete! httpbin is now accessible at http://localhost:10001/
```

## Running Tests

```bash
# Unit tests (no infrastructure needed)
make test-agents

# Full integration tests with Envoy (requires docker)
make test-agents-envoy
```
