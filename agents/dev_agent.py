#!/usr/bin/env python3
"""
Flowplane Dev Agent — Deploy APIs through the gateway with guardrail-enforced workflows.

A lightweight, model-agnostic agent that connects to Flowplane's MCP server
to deploy, configure, and verify API gateway resources. Guardrails automatically
handle pre-flight checks, dataplane injection, and deployment tracking.

Usage:
    # One-shot deployment
    python dev_agent.py "Expose httpbin at localhost:8000 on path / at port 10001"

    # Interactive mode
    python dev_agent.py

    # Streaming output
    python dev_agent.py --stream "Expose httpbin at localhost:8000 on path / at port 10001"

    # Verify a deployment
    python dev_agent.py --verify --path /api/orders --port 10001

Environment variables:
    FLOWPLANE_URL    - Flowplane API base URL (default: http://localhost:8080)
    FLOWPLANE_TEAM   - Team context (default: platform-admin)
    FLOWPLANE_TOKEN  - PAT token (required)
    LLM_BASE_URL     - LLM API endpoint (default: https://api.openai.com/v1)
    LLM_API_KEY      - LLM API key (required)
    LLM_MODEL        - Model name (default: gpt-4o)
"""

import argparse
import json
import os
import sys

from mcp_client import FlowplaneMCPClient
from agent import FlowplaneAgent, Guardrails, ConversationMemory

SYSTEM_PROMPT = """\
You are the Flowplane Dev Agent — an intelligent API deployment operator.

You deploy backend services through the Flowplane API gateway using MCP tools. \
Guardrails automatically enforce pre-flight checks and dataplane injection, so \
focus on choosing the right resources and configurations.

## Terminology & Resource Model
- **Dataplane** — an Envoy instance that runs the gateway (auto-managed by guardrails)
- **Cluster** — upstream backend service with endpoints (host:port)
- **Route Configuration** — path matching and forwarding rules (references a cluster)
- **Virtual Host** — domain grouping within a route config
- **Route** — individual path rule within a virtual host
- **Filter** (optional) — policies like rate limiting, CORS, auth
- **Listener** — entry point (address:port), binds to a route config

## Smart Defaults
- Listen address: 0.0.0.0
- Protocol: HTTP
- Load balancing: ROUND_ROBIN
- Virtual host domains: ["*"]
- Match type: prefix

## Error Handling
- ALREADY_EXISTS: use the update tool or choose a different name
- NOT_FOUND: create the prerequisite first (clusters before routes, etc.)
- CONFLICT: check existing resource, suggest resolution

## Response Guidelines
- Report each step as you complete it
- After deployment, run verification (query_service + config_validate + trace_request)
- Present a deployment summary with all resource names and the final trace result
- Include the final access URL (e.g., http://localhost:10001/api/orders)
- If any step fails, stop and report the issue with a suggested fix
"""

# Deployment + diagnostic tools
ALLOWED_TOOLS = {
    # Pre-flight
    "dev_preflight_check",
    "cp_query_port",
    "cp_query_path",
    # Create
    "cp_create_cluster",
    "cp_create_route_config",
    "cp_create_virtual_host",
    "cp_create_route",
    "cp_create_listener",
    "cp_create_filter",
    "cp_attach_filter",
    # Update
    "cp_update_cluster",
    "cp_update_route_config",
    "cp_update_listener",
    "cp_update_filter",
    "cp_update_virtual_host",
    "cp_update_route",
    # Read
    "cp_list_clusters",
    "cp_get_cluster",
    "cp_get_cluster_health",
    "cp_list_listeners",
    "cp_get_listener",
    "cp_list_route_configs",
    "cp_get_route_config",
    "cp_list_virtual_hosts",
    "cp_get_virtual_host",
    "cp_list_routes",
    "cp_get_route",
    "cp_list_filters",
    "cp_get_filter",
    "cp_list_filter_types",
    "cp_get_filter_type",
    "cp_list_filter_attachments",
    # Dataplanes
    "cp_list_dataplanes",
    "cp_get_dataplane",
    "cp_create_dataplane",
    # Verify
    "cp_query_service",
    "ops_trace_request",
    "ops_topology",
    "ops_config_validate",
}


def _build_agent(mcp: FlowplaneMCPClient, llm_url: str, llm_key: str, llm_model: str) -> FlowplaneAgent:
    """Build the dev agent with guardrails enabled."""
    guardrails = Guardrails(mcp)
    guardrails.enable_auto_preflight().enable_dataplane_injection()

    return FlowplaneAgent(
        mcp_client=mcp,
        llm_base_url=llm_url,
        api_key=llm_key,
        model=llm_model,
        system_prompt=SYSTEM_PROMPT,
        allowed_tools=ALLOWED_TOOLS,
        guardrails=guardrails,
    )


def _run_verify(mcp: FlowplaneMCPClient, path: str, port: int) -> None:
    """Run config validation and request tracing for a given path/port."""
    print(f"Verifying path={path} port={port} ...\n")

    print("── ops_config_validate ──")
    try:
        result = mcp.call_tool("ops_config_validate", {})
        print(json.dumps(result, indent=2))
    except Exception as e:
        print(f"Error: {e}")

    print(f"\n── ops_trace_request (port={port}, path={path}) ──")
    try:
        result = mcp.call_tool("ops_trace_request", {"port": port, "path": path})
        print(json.dumps(result, indent=2))
    except Exception as e:
        print(f"Error: {e}")


def main():
    parser = argparse.ArgumentParser(description="Flowplane Dev Agent")
    parser.add_argument("query", nargs="*", help="One-shot deployment query")
    parser.add_argument("--stream", action="store_true", help="Enable streaming output")
    parser.add_argument("--verify", action="store_true", help="Run verification for a deployment")
    parser.add_argument("--path", default="/", help="Path for --verify (default: /)")
    parser.add_argument("--port", type=int, default=10000, help="Port for --verify (default: 10000)")
    args = parser.parse_args()

    fp_url = os.environ.get("FLOWPLANE_URL", "http://localhost:8080")
    fp_team = os.environ.get("FLOWPLANE_TEAM", "platform-admin")
    fp_token = os.environ.get("FLOWPLANE_TOKEN")
    llm_url = os.environ.get("LLM_BASE_URL", "https://api.openai.com/v1")
    llm_key = os.environ.get("LLM_API_KEY")
    llm_model = os.environ.get("LLM_MODEL", "gpt-4o")

    if not fp_token:
        print("Error: FLOWPLANE_TOKEN environment variable is required", file=sys.stderr)
        sys.exit(1)

    with FlowplaneMCPClient(fp_url, fp_team, fp_token) as mcp:
        mcp.initialize()

        # --verify mode: no LLM needed
        if args.verify:
            _run_verify(mcp, args.path, args.port)
            return

        if not llm_key:
            print("Error: LLM_API_KEY environment variable is required", file=sys.stderr)
            sys.exit(1)

        agent = _build_agent(mcp, llm_url, llm_key, llm_model)

        if args.query:
            query = " ".join(args.query)
            if args.stream:
                for event in agent.run_stream(query):
                    if event["type"] == "tool_call":
                        print(f"  ⚡ {event['name']}({json.dumps(event['args'], separators=(',', ':'))})", file=sys.stderr)
                    elif event["type"] == "tool_result":
                        preview = json.dumps(event["result"], separators=(",", ":"))
                        if len(preview) > 200:
                            preview = preview[:200] + "…"
                        print(f"  ✓ {event['name']} → {preview}", file=sys.stderr)
                    elif event["type"] == "answer":
                        print(event["content"])
            else:
                print(agent.run(query))
        else:
            agent.chat(stream=args.stream)


if __name__ == "__main__":
    main()
