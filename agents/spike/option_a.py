"""Option A: Tool-calling agent for the architecture comparison spike.

Uses the standard OpenAI function-calling loop with Flowplane MCP tools.
Guardrails handle pre-flight, dataplane injection, name dedup, and port validation.
"""

import sys
from pathlib import Path

_AGENTS_DIR = Path(__file__).resolve().parent.parent
if str(_AGENTS_DIR) not in sys.path:
    sys.path.insert(0, str(_AGENTS_DIR))

import json

from openai import OpenAI

from agent import Guardrails, GuardrailReject, mcp_to_openai_tools
from mcp_client import FlowplaneMCPClient
from spike.shared import (
    PREAMBLE,
    TEST_PROMPT,
    AgentConfig,
    SkillLoader,
    SpikeTrace,
    run_traced,
)

# Same allowed tools as dev_agent.py
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


def run(config: AgentConfig, mcp: FlowplaneMCPClient) -> SpikeTrace:
    """Run the tool-calling agent and return a SpikeTrace."""
    # 1. Create Guardrails with all 4 features enabled
    guardrails = Guardrails(mcp)
    guardrails.enable_auto_preflight()
    guardrails.enable_dataplane_injection()
    guardrails.enable_name_dedup()
    guardrails.enable_port_validation()

    # 2. Load and convert MCP tools
    mcp_tools = mcp.list_tools()
    tools = mcp_to_openai_tools(mcp_tools, ALLOWED_TOOLS)
    tool_names = {t["function"]["name"] for t in tools}

    # 3. Build system prompt
    skill_text = SkillLoader.load("flowplane-api", refs=["routing-cookbook.md"])
    system_prompt = PREAMBLE + "\n\n" + skill_text

    # 4. Create OpenAI client from config
    llm = OpenAI(base_url=config.llm_base_url, api_key=config.llm_api_key)

    # 5. Reset guardrails turn state
    guardrails.reset_turn()

    # 6. Build execute_fn that applies guardrails before/after each MCP call
    def execute_fn(tool_name: str, args: dict) -> str:
        if tool_name not in tool_names:
            return json.dumps({"error": f"Unknown tool: {tool_name}"})

        try:
            args = guardrails.before_call(tool_name, args)
        except GuardrailReject as e:
            return json.dumps({"error": str(e)})

        try:
            result = mcp.call_tool(tool_name, args)
            result_str = json.dumps(result, separators=(",", ":"))
        except Exception as e:
            return json.dumps({"error": str(e)})

        guardrails.after_call(tool_name, args, result)
        return result_str

    # 7. Call run_traced and return SpikeTrace
    return run_traced(
        llm=llm,
        model=config.llm_model,
        system_prompt=system_prompt,
        user_message=TEST_PROMPT,
        tools=tools,
        execute_fn=execute_fn,
    )
