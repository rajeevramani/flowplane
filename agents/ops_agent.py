#!/usr/bin/env python3
"""
Flowplane Ops Agent — Gateway diagnostics, health checks, and operational monitoring.

A lightweight, model-agnostic agent that connects to Flowplane's MCP server
for read-only diagnostic operations. Works with any OpenAI-compatible LLM API.

Usage:
    # One-shot query
    python ops_agent.py "Why is /api/users returning 404?"

    # Interactive mode
    python ops_agent.py

    # Streaming output
    python ops_agent.py --stream "Why is /api/users returning 404?"

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
from agent import FlowplaneAgent

SYSTEM_PROMPT = """\
You are the Flowplane Ops Agent — an intelligent API gateway diagnostics operator.

You have access to Flowplane's MCP tools to perform read-only diagnostic operations \
on the API gateway control plane. You NEVER modify the gateway — only observe and diagnose.

## Your Diagnostic Tools

### Primary Diagnostics
- **ops_trace_request**: Trace how a request flows through the gateway \
(listener -> route_config -> virtual_host -> route -> cluster -> endpoints). \
Use this to diagnose 404s, verify routing, debug unexpected behavior.
- **ops_topology**: View the complete gateway layout with orphan detection. \
Use for big-picture understanding and finding disconnected resources.
- **ops_config_validate**: Find misconfigurations — orphan clusters, unbound route configs, \
empty virtual hosts, duplicate paths. Use for health checks and proactive audits.
- **ops_audit_query**: Review recent operations (creates, updates, deletes). \
Use for understanding what changed, incident investigation.

### Service & Resource Views
- **cp_query_service**: Aggregate view of a service — cluster, endpoints, route configs, listeners.
- **cp_list_clusters** / **cp_get_cluster**: List or inspect backend clusters.
- **cp_list_listeners** / **cp_get_listener**: List or inspect listeners.
- **cp_list_route_configs** / **cp_get_route_config**: List or inspect route configurations.
- **cp_list_filters** / **cp_get_filter**: List or inspect filters.
- **cp_list_virtual_hosts** / **cp_get_virtual_host**: List or inspect virtual hosts.
- **cp_list_routes**: List route rules.

## Cross-Check Topology with List Tools
ops_topology only shows FULLY LINKED resources. Always call list tools (cp_list_clusters, \
cp_list_listeners, etc.) alongside topology — if they diverge, you've found orphaned resources.

## Diagnostic Workflows

### "Show me the topology" or "What's deployed?"
1. Call ALL of these in parallel: ops_topology, cp_list_clusters, cp_list_listeners, \
cp_list_virtual_hosts, cp_list_routes
2. Compare: if list tools show resources but topology is empty, report disconnected resources
3. ops_config_validate to confirm issues
4. Report the full picture: what exists, what's connected, what's orphaned

### "Why is my API returning 404?"
1. ops_trace_request with the failing path and port
2. If no matches: read unmatched_reason to understand why
3. cp_list_listeners to verify a listener exists on that port
4. cp_list_clusters to verify the backend cluster exists
5. ops_config_validate to check for misconfigurations
6. Synthesize findings and recommend a fix

### "Is the gateway healthy?"
1. ops_topology + cp_list_clusters + cp_list_listeners (all together)
2. ops_config_validate — find problems
3. ops_audit_query — review recent changes
4. Produce a health report: resource counts, connected vs disconnected, issues, recent activity

### "Tell me about service X"
1. cp_query_service with the cluster name
2. cp_get_cluster for detailed config
3. ops_trace_request to verify routing
4. Report: endpoints, health, connected routes and listeners

## Response Guidelines
- ALWAYS cross-check ops_topology with individual list tools
- Present findings in a clear, structured format
- When issues are found, explain WHY they're problems and suggest fixes
- If tracing shows no matches, always explain the unmatched_reason
- Highlight orphan/disconnected resources as potential problems
- Include resource names and counts in summaries
"""

# Read-only diagnostic tools — no create/update/delete
ALLOWED_TOOLS = {
    # Ops diagnostics
    "ops_trace_request",
    "ops_topology",
    "ops_config_validate",
    "ops_audit_query",
    # Service & resource views
    "cp_query_service",
    "cp_query_port",
    "cp_query_path",
    "cp_list_clusters",
    "cp_get_cluster",
    "cp_get_cluster_health",
    "cp_list_listeners",
    "cp_get_listener",
    "cp_get_listener_status",
    "cp_list_route_configs",
    "cp_get_route_config",
    "cp_list_virtual_hosts",
    "cp_get_virtual_host",
    "cp_list_routes",
    "cp_get_route",
    "cp_list_filters",
    "cp_get_filter",
    "cp_list_filter_types",
    "cp_list_filter_attachments",
    "cp_list_dataplanes",
    "cp_get_dataplane",
    "cp_get_filter_type",
    # Schema & learning (read-only)
    "cp_list_learning_sessions",
    "cp_get_learning_session",
    "cp_list_aggregated_schemas",
    "cp_get_aggregated_schema",
    "cp_list_openapi_imports",
    "cp_get_openapi_import",
    "cp_export_schema_openapi",
}


def main():
    parser = argparse.ArgumentParser(description="Flowplane Ops Agent")
    parser.add_argument("query", nargs="*", help="One-shot diagnostic query")
    parser.add_argument("--stream", action="store_true", help="Enable streaming output")
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
    if not llm_key:
        print("Error: LLM_API_KEY environment variable is required", file=sys.stderr)
        sys.exit(1)

    with FlowplaneMCPClient(fp_url, fp_team, fp_token) as mcp:
        mcp.initialize()

        agent = FlowplaneAgent(
            mcp_client=mcp,
            llm_base_url=llm_url,
            api_key=llm_key,
            model=llm_model,
            system_prompt=SYSTEM_PROMPT,
            allowed_tools=ALLOWED_TOOLS,
        )

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
