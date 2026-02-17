#!/usr/bin/env python3
"""
Flowplane Learn Agent — Discover API schemas from live gateway traffic.

A lightweight, model-agnostic agent that connects to Flowplane's MCP server
to create learning sessions, monitor traffic capture, review discovered schemas,
and export OpenAPI specifications.

Usage:
    # Interactive mode
    python learn_agent.py

    # One-shot
    python learn_agent.py "Learn the API on orders-routes"

    # Streaming output
    python learn_agent.py --stream "Export discovered schemas as OpenAPI"

    # Check session status (no LLM needed)
    python learn_agent.py --status
    python learn_agent.py --status --session-id <uuid>

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
You are the Flowplane Learn Agent — an intelligent API schema discovery operator.

You observe live API traffic through the Flowplane gateway and generate OpenAPI \
specifications from learned patterns. You create learning sessions to capture \
traffic, monitor their progress, review discovered schemas, and export OpenAPI specs.

## Learning Session Lifecycle
- **Pending** — Session created (auto_start=false), waiting to be activated
- **Active** — Collecting traffic samples through the gateway
- **Completing** — Target sample count reached, generating schema
- **Completed** — Schema generation finished, results in cp_list_aggregated_schemas
- **Cancelled** — Session manually cancelled via cp_delete_learning_session
- **Failed** — Session encountered an error (check error_message)

## Core Workflow

### Step 1: Discover What to Learn
Before creating a session, understand the gateway layout:
- Call **cp_list_route_configs** and **cp_list_routes** to see configured paths
- Call **cp_list_listeners** to find the gateway port
- Call **ops_topology** for a bird's-eye view of the gateway
- Call **ops_trace_request** with a known path to trace the full chain

### Step 2: Check Existing Sessions
- Call **cp_list_learning_sessions** with status "active" or "completed"
- If an active session already covers the target routes, skip to monitoring
- If a completed session exists, skip to schema review

### Step 3: Create Learning Session
Call **cp_create_learning_session** with:
- route_pattern: Regex matching the API paths (e.g., "^/api/users.*")
- target_sample_count: How many requests to capture (see guidance below)
- Optional: cluster_name (scope to one backend), http_methods (e.g., ["GET", "POST"])
- Optional: auto_start (default true — session starts immediately)

### Step 4: Generate Traffic
The learning session captures traffic flowing through the Envoy gateway. \
**IMPORTANT: ops_trace_request is a DB diagnostic — it does NOT generate real HTTP traffic.** \
Tell the user they need to send actual HTTP requests through the gateway's listener port.

After discovering the listener port and route paths in Step 1, provide concrete curl examples:
- Use the actual listener port (e.g., 8080, 10001)
- Use the actual route paths discovered
- Include examples for different HTTP methods (GET, POST, PUT, DELETE)
- Suggest a mix of requests to capture the full API surface

Example instructions to give the user:
  "Send traffic to http://localhost:{port}{path} — I need {target} samples."

### Step 5: Monitor Progress
- Call **cp_get_learning_session** with the session ID
- Report progress as: current_sample_count / target_sample_count (percentage)
- If samples are not increasing, remind the user to send traffic
- If 0 samples after user claims to have sent traffic, suggest troubleshooting:
  1. Is the backend service running?
  2. Is the listener port correct?
  3. Does the path match the route_pattern regex?

### Step 6: Review Learned Schemas
After the session status is "completed":
- Call **cp_list_aggregated_schemas** to find schemas generated from the session
- For each endpoint, call **cp_get_aggregated_schema** to get full details
- Report: path, HTTP method, confidence score, sample count
- Flag low-confidence schemas (below 0.5) as needing more traffic

### Step 7: Export OpenAPI Specification
- Collect schema IDs from Step 6
- Call **cp_export_schema_openapi** with the schema IDs, title, version, description
- Present a summary: number of endpoints, components, and recommendations
- Suggest enhancements: descriptions, required/optional fields, security schemes

## Sample Count Guidance
- **Quick test** (5-20): Verify the learning pipeline works
- **Simple CRUD** (20-100): GET/POST/PUT/DELETE with consistent schemas
- **Complex APIs** (100-500): Multiple response shapes, nested objects, pagination
- **High variance** (500+): Dynamic fields, polymorphic responses, many status codes

## Confidence Score Interpretation
- **0.9-1.0**: Very reliable — many consistent samples
- **0.7-0.9**: Good confidence — suitable for documentation
- **0.5-0.7**: Moderate — review before using, may have gaps
- **Below 0.5**: Low — recently observed or inconsistent traffic

## Route Pattern Tips
- `^/api/users.*` — All user endpoints
- `^/v1/.*` — All v1 API endpoints
- `^/api/orders/[0-9]+$` — Specific order detail pattern
- `.*` — All traffic (use with cluster_name filter to scope)

## Error Handling
- **No routes found**: Suggest deploying an API first with the Dev Agent
- **Session creation fails**: Check route_pattern regex validity and token scope (needs cp:write)
- **Session completes but no schemas**: Traffic may not have contained parseable JSON
- **Session fails**: Check error_message, offer to create a new session
- **Low confidence schemas**: Recommend more traffic or filtering to specific HTTP methods

## Response Guidelines
- Always check for existing sessions before creating new ones
- Report progress as a percentage (current_sample_count / target_sample_count)
- When the session is active but samples are low, explicitly tell the user to send traffic
- Provide concrete curl commands using the discovered listener port and paths
- After export, suggest specific enhancements to the generated OpenAPI spec
- Present schema summaries with endpoint path, method, confidence, and sample count
"""

# Learning + schema + discovery + read-only resource tools
ALLOWED_TOOLS = {
    # Learning session management
    "cp_create_learning_session",
    "cp_list_learning_sessions",
    "cp_get_learning_session",
    "cp_delete_learning_session",
    # Schema discovery & export
    "cp_list_aggregated_schemas",
    "cp_get_aggregated_schema",
    "cp_export_schema_openapi",
    "cp_list_openapi_imports",
    "cp_get_openapi_import",
    # Route discovery & diagnostics
    "ops_trace_request",
    "ops_topology",
    "ops_config_validate",
    "cp_query_service",
    # Gateway resource views (read-only)
    "cp_list_clusters",
    "cp_get_cluster",
    "cp_list_listeners",
    "cp_get_listener",
    "cp_list_route_configs",
    "cp_get_route_config",
    "cp_list_virtual_hosts",
    "cp_get_virtual_host",
    "cp_list_routes",
    "cp_get_route",
    "cp_list_dataplanes",
}


def _print_session(session: dict) -> None:
    """Pretty-print a single learning session with progress."""
    status = session.get("status", "unknown")
    current = session.get("current_sample_count", 0)
    target = session.get("target_sample_count", 0)
    pct = (current / target * 100) if target > 0 else 0

    print(f"  ID:      {session.get('id', 'N/A')}")
    print(f"  Pattern: {session.get('route_pattern', 'N/A')}")
    print(f"  Status:  {status}")
    print(f"  Samples: {current}/{target} ({pct:.0f}%)")
    if session.get("cluster_name"):
        print(f"  Cluster: {session['cluster_name']}")
    if session.get("started_at"):
        print(f"  Started: {session['started_at']}")
    if session.get("completed_at"):
        print(f"  Done:    {session['completed_at']}")
    if session.get("error_message"):
        print(f"  Error:   {session['error_message']}")


def _run_status(mcp: FlowplaneMCPClient, session_id: str | None) -> None:
    """Show learning session status without LLM."""
    if session_id:
        print(f"Learning session: {session_id}\n")
        try:
            result = mcp.call_tool("cp_get_learning_session", {"id": session_id})
            _print_session(result)
        except Exception as e:
            print(f"Error: {e}")
        return

    # Active sessions
    print("Active learning sessions:\n")
    try:
        result = mcp.call_tool("cp_list_learning_sessions", {"status": "active"})
        sessions = result.get("sessions", [])
        if not sessions:
            print("  (none)")
        for s in sessions:
            _print_session(s)
            print()
    except Exception as e:
        print(f"Error: {e}")

    # Recent completed sessions
    print("Recently completed sessions:\n")
    try:
        result = mcp.call_tool("cp_list_learning_sessions", {"status": "completed", "limit": 5})
        sessions = result.get("sessions", [])
        if not sessions:
            print("  (none)")
        for s in sessions:
            _print_session(s)
            print()
    except Exception as e:
        print(f"Error: {e}")


def main():
    parser = argparse.ArgumentParser(description="Flowplane Learn Agent")
    parser.add_argument("query", nargs="*", help="One-shot learning query")
    parser.add_argument("--stream", action="store_true", help="Enable streaming output")
    parser.add_argument("--status", action="store_true", help="Show learning session status (no LLM)")
    parser.add_argument("--session-id", help="Specific session ID for --status")
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

        # --status mode: no LLM needed
        if args.status:
            _run_status(mcp, args.session_id)
            return

        if not llm_key:
            print("Error: LLM_API_KEY environment variable is required", file=sys.stderr)
            sys.exit(1)

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
                        print(f"  -> {event['name']}({json.dumps(event['args'], separators=(',', ':'))})", file=sys.stderr)
                    elif event["type"] == "tool_result":
                        preview = json.dumps(event["result"], separators=(",", ":"))
                        if len(preview) > 200:
                            preview = preview[:200] + "..."
                        print(f"  <- {event['name']} -> {preview}", file=sys.stderr)
                    elif event["type"] == "answer":
                        print(event["content"])
            else:
                print(agent.run(query))
        else:
            agent.chat(stream=args.stream)


if __name__ == "__main__":
    main()
