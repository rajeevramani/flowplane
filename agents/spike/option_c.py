"""
Option C: Single-shot plan — LLM sees full tool schemas as documentation,
produces the complete tool sequence in one call, harness executes without
further LLM interaction.

Round trip budget: 2 (one to plan, one to review results).

Key difference from A and B: the tool schemas are embedded in the system
prompt as reference documentation, NOT registered as function-calling tools.
The only function-calling tool is submit_plan.
"""

import sys
from pathlib import Path

_AGENTS_DIR = Path(__file__).resolve().parent.parent
if str(_AGENTS_DIR) not in sys.path:
    sys.path.insert(0, str(_AGENTS_DIR))

import json
import time

from openai import OpenAI

from agent import Guardrails, mcp_to_openai_tools
from mcp_client import FlowplaneMCPClient
from spike.shared import (
    PREAMBLE,
    TEST_PROMPT,
    AgentConfig,
    SkillLoader,
    SpikeTrace,
    TestScenario,
)
from spike.option_a import ALLOWED_TOOLS
from spike.option_b import plan_executor

# ---------------------------------------------------------------------------
# Prompt additions
# ---------------------------------------------------------------------------

PLAN_PROMPT = """
## Your Task

You will receive a deployment request. Produce a COMPLETE ordered plan as a
single submit_plan call. Each step is {"tool": "<mcp_tool_name>", "params": {…}}.

The harness executes every step sequentially with guardrails (preflight,
dataplane injection, name dedup, port validation). It stops on the first error.

You get ONE chance — there is no back-and-forth. Include every step needed for
a working deployment, including verification at the end.

If the request is contradictory or impossible (e.g. "no rewrite" but the
source and destination paths differ), call reject_task with a clear reason
instead of producing a broken plan.

## Critical: How prefixRewrite Works

When a route uses prefix matching, `prefixRewrite` performs a **literal string
replacement** of the matched prefix portion of the request path. The matched
prefix is removed and the rewrite value is substituted in its place.

Formula: `result_path = prefixRewrite + request_path[len(match_prefix):]`

This means the rewrite value must be the COMPLETE desired backend path prefix,
not just the first segment. If you want requests to `/foo/bar` to arrive at
`/baz/bar` on the backend, the match prefix is `/foo/bar` and the prefixRewrite
is `/baz/bar` — NOT `/baz`.

## Critical: Updating Route Configs

cp_update_route_config performs a FULL REPLACEMENT of the virtualHosts array.
To add a new route, you must include ALL existing routes plus the new one.
The pre-queried state below shows the current route config details — preserve
existing routes when adding new ones.

## Tool Parameter Reference

Below are the exact JSON schemas for every available MCP tool. Use these
to produce correct parameters on the first attempt.

"""


def _build_tool_reference(mcp_tools: list[dict]) -> str:
    """Format MCP tool schemas as readable documentation for the system prompt."""
    lines = []
    for tool in mcp_tools:
        name = tool.get("name", "")
        if name not in ALLOWED_TOOLS:
            continue
        desc = tool.get("description", "")
        schema = tool.get("inputSchema", {})
        props = schema.get("properties", {})
        required = schema.get("required", [])

        lines.append(f"### {name}")
        if desc:
            # Truncate long descriptions to keep prompt size reasonable
            short_desc = desc[:300] + "..." if len(desc) > 300 else desc
            lines.append(short_desc)

        if props:
            lines.append("Parameters:")
            for pname, pschema in props.items():
                ptype = pschema.get("type", "any")
                pdesc = pschema.get("description", "")
                req = " (required)" if pname in required else ""
                lines.append(f"  - {pname}: {ptype}{req} — {pdesc}")

                # Show items schema for arrays
                if ptype == "array" and "items" in pschema:
                    items = pschema["items"]
                    if items.get("properties"):
                        for iprop, ischema in items["properties"].items():
                            lines.append(f"      .{iprop}: {ischema.get('type', 'any')} — {ischema.get('description', '')}")
        lines.append("")

    return "\n".join(lines)


# Tools the LLM can call: submit a plan OR reject the task
SUBMIT_PLAN_TOOL = [
    {
        "type": "function",
        "function": {
            "name": "submit_plan",
            "description": (
                "Submit the complete deployment plan. All steps will be executed "
                "sequentially with guardrails. You get ONE shot — include everything."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "description": "Ordered list of MCP tool calls.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": {
                                    "type": "string",
                                    "description": "MCP tool name (from the reference above).",
                                },
                                "params": {
                                    "type": "object",
                                    "description": "Tool parameters matching the schema in the reference.",
                                },
                            },
                            "required": ["tool", "params"],
                        },
                    },
                },
                "required": ["steps"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "reject_task",
            "description": (
                "Reject the task when the request is contradictory, impossible, "
                "or missing critical information. Explain why so the caller can fix it."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "description": "Clear explanation of why the task cannot be completed as requested.",
                    },
                },
                "required": ["reason"],
            },
        },
    },
]


# ---------------------------------------------------------------------------
# run
# ---------------------------------------------------------------------------

def run(
    config: AgentConfig,
    mcp: FlowplaneMCPClient,
    scenario: TestScenario | None = None,
) -> SpikeTrace:
    """Run the single-shot plan agent. Budget: 2 LLM calls.

    If scenario is provided, uses its prompt and pre_queries instead of
    the hardcoded TEST_PROMPT and default pre-queries.
    """
    trace = SpikeTrace()
    start = time.monotonic()

    prompt = scenario.prompt if scenario else TEST_PROMPT
    pre_queries = scenario.pre_queries if scenario else {
        "dataplanes": ("cp_list_dataplanes", {}),
        "listeners": ("cp_list_listeners", {}),
        "clusters": ("cp_list_clusters", {}),
        "port_10019": ("cp_query_port", {"port": 10019}),
    }

    guardrails = (
        Guardrails(mcp)
        .enable_auto_preflight()
        .enable_dataplane_injection()
        .enable_name_dedup()
        .enable_port_validation()
    )
    guardrails.reset_turn()

    # Build system prompt: skill + tool schemas as docs + plan instructions
    skill_text = SkillLoader.load("flowplane-api", refs=["routing-cookbook.md"])
    mcp_tools = mcp.list_tools()
    tool_ref = _build_tool_reference(mcp_tools)
    system_prompt = PREAMBLE + "\n\n" + skill_text + "\n\n" + PLAN_PROMPT + tool_ref

    llm = OpenAI(base_url=config.llm_base_url, api_key=config.llm_api_key)

    # --- Pre-query: gather runtime state so the LLM has real IDs ---
    label_prefix = f"[C:{scenario.name}]" if scenario else "[C]"
    print(f"  {label_prefix} Pre-query: gathering runtime state...", file=sys.stderr)
    state_context: dict[str, str] = {}
    for label, (tool, params) in pre_queries.items():
        try:
            result = mcp.call_tool(tool, params)
            state_context[label] = json.dumps(result, indent=2, default=str)
            print(f"       {label}: OK", file=sys.stderr)
        except Exception as e:
            state_context[label] = json.dumps({"error": str(e)})
            print(f"       {label}: ERROR {e}", file=sys.stderr)

    # For T5-type scenarios that need full route config details, auto-fetch them
    if scenario and "route_configs" in state_context:
        try:
            rc_data = json.loads(state_context["route_configs"])
            rc_items = rc_data.get("route_configs") or rc_data.get("items") or []
            for rc in rc_items:
                rc_name = rc.get("name", "")
                if rc_name:
                    detail = mcp.call_tool("cp_get_route_config", {"name": rc_name})
                    state_context[f"route_config_detail_{rc_name}"] = json.dumps(detail, indent=2, default=str)
                    print(f"       route_config_detail_{rc_name}: OK", file=sys.stderr)
        except Exception:
            pass  # best-effort enrichment

    state_block = "\n\n## Current Gateway State (pre-queried by harness)\n\n"
    for label, data in state_context.items():
        state_block += f"### {label}\n```json\n{data}\n```\n\n"
    state_block += (
        "Use the EXACT dataplaneId from the dataplanes list above for cp_create_listener. "
        "Do NOT guess or use placeholder values like 'default'.\n"
    )

    # --- Call 1: LLM produces the plan ---
    print(f"  {label_prefix} Call 1: Requesting plan from LLM...", file=sys.stderr)
    messages: list[dict] = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": prompt + state_block},
    ]

    response = llm.chat.completions.create(
        model=config.llm_model,
        messages=messages,
        tools=SUBMIT_PLAN_TOOL,
    )
    trace.turn_count += 1
    if response.usage:
        trace.prompt_tokens.append(response.usage.prompt_tokens or 0)
        trace.completion_tokens.append(response.usage.completion_tokens or 0)

    # Extract plan
    choice = response.choices[0]
    message = choice.message
    steps: list[dict] = []

    # Debug: dump raw response shape
    if message.tool_calls:
        for tc in message.tool_calls:
            raw_args = tc.function.arguments[:200]
            print(f"  {label_prefix} Raw tool_call: {tc.function.name}, args preview: {raw_args}...", file=sys.stderr)
    elif message.content:
        print(f"  {label_prefix} Raw content preview: {message.content[:300]}...", file=sys.stderr)

    if message.tool_calls:
        for tc in message.tool_calls:
            if tc.function.name == "reject_task":
                try:
                    args = json.loads(tc.function.arguments)
                    reason = args.get("reason", "No reason given")
                except json.JSONDecodeError:
                    reason = tc.function.arguments or "No reason given"
                print(f"  {label_prefix} REJECTED: {reason}", file=sys.stderr)
                trace.tool_calls.append({"name": "reject_task", "args": {"reason": reason}})
                trace.final_answer = f"Task rejected: {reason}"
                trace.elapsed_s = time.monotonic() - start
                return trace
            if tc.function.name == "submit_plan":
                try:
                    args = json.loads(tc.function.arguments)
                    steps = args.get("steps", [])
                    # LLMs sometimes double-encode: {"steps": "[{...}]"} instead of {"steps": [{...}]}
                    if isinstance(steps, str):
                        steps = json.loads(steps)
                except json.JSONDecodeError:
                    pass
                trace.tool_calls.append({"name": "submit_plan", "args": {"step_count": len(steps)}})
    elif message.content:
        # Fallback: model returned raw JSON in content
        try:
            parsed = json.loads(message.content)
            steps = parsed.get("steps", parsed) if isinstance(parsed, dict) else parsed
        except json.JSONDecodeError:
            content = message.content
            if "```" in content:
                start_idx = content.find("```")
                end_idx = content.rfind("```")
                if start_idx != end_idx:
                    block = content[start_idx:end_idx].split("\n", 1)[-1]
                    try:
                        parsed = json.loads(block)
                        steps = parsed.get("steps", parsed) if isinstance(parsed, dict) else parsed
                    except json.JSONDecodeError:
                        pass

    # Validate steps are dicts with "tool" keys
    valid_steps = [s for s in steps if isinstance(s, dict) and "tool" in s]
    if not valid_steps:
        raw_preview = json.dumps(steps[:5], default=str)[:500] if steps else "(empty)"
        trace.final_answer = (
            f"LLM produced {len(steps)} items but none are valid steps. "
            f"Preview: {raw_preview}"
        )
        if message.content:
            trace.final_answer += f"\n\nRaw content: {message.content[:500]}"
        trace.elapsed_s = time.monotonic() - start
        return trace
    steps = valid_steps

    print(f"  {label_prefix} Plan received: {len(steps)} steps", file=sys.stderr)
    for i, step in enumerate(steps):
        print(f"       {i + 1}. {step.get('tool', '?')}", file=sys.stderr)

    # --- Execute the plan ---
    print(f"  {label_prefix} Executing plan...", file=sys.stderr)
    results = plan_executor(steps, mcp, guardrails)

    for r in results:
        tool = r.get("tool", "?")
        step_params = steps[r["step"]].get("params", {}) if r["step"] < len(steps) else {}
        trace.tool_calls.append({"name": tool, "args": step_params})
        if "error" in r:
            print(f"       step {r['step']}: {tool} -> ERROR: {r['error']}", file=sys.stderr)
        else:
            print(f"       step {r['step']}: {tool} -> OK", file=sys.stderr)

    # --- Call 2: LLM reviews results and produces final answer ---
    print(f"  {label_prefix} Call 2: LLM reviewing results...", file=sys.stderr)
    results_summary = json.dumps(results, indent=2, default=str)
    messages.append(message)
    if message.tool_calls:
        messages.append({
            "role": "tool",
            "tool_call_id": message.tool_calls[0].id,
            "content": results_summary,
        })
    else:
        messages.append({
            "role": "user",
            "content": f"Plan execution results:\n{results_summary}\n\nSummarize what happened.",
        })

    response2 = llm.chat.completions.create(
        model=config.llm_model,
        messages=messages,
    )
    trace.turn_count += 1
    if response2.usage:
        trace.prompt_tokens.append(response2.usage.prompt_tokens or 0)
        trace.completion_tokens.append(response2.usage.completion_tokens or 0)

    trace.final_answer = response2.choices[0].message.content or ""
    trace.elapsed_s = time.monotonic() - start
    return trace
