"""
Lightweight model-agnostic agent loop for Flowplane.

Works with any OpenAI-compatible API (Anthropic, Ollama, Groq, Together, vLLM, etc.)
via configurable base_url. Connects to Flowplane's MCP server for tool execution.

Includes:
- Guardrails: pre/post-call hooks, auto-preflight, dataplane injection
- Streaming: event-based output for real-time tool call visibility
- ConversationMemory: sliding-window history for multi-turn chat sessions

Usage:
    from mcp_client import FlowplaneMCPClient
    from agent import FlowplaneAgent, Guardrails, ConversationMemory

    mcp = FlowplaneMCPClient("http://localhost:8080", "platform-admin", "fp_pat_...")
    mcp.initialize()

    guardrails = Guardrails(mcp)
    guardrails.enable_auto_preflight()
    guardrails.enable_dataplane_injection()

    agent = FlowplaneAgent(
        mcp_client=mcp,
        llm_base_url="https://api.openai.com/v1",
        api_key="sk-...",
        model="gpt-4o",
        system_prompt="You are a gateway diagnostics agent.",
        guardrails=guardrails,
    )
    answer = agent.run("Show me the gateway topology")
"""

from __future__ import annotations

import json
import sys
from collections.abc import Callable, Generator
from typing import Any

from openai import OpenAI
from mcp_client import FlowplaneMCPClient


def mcp_to_openai_tools(mcp_tools: list[dict], allowed: set[str] | None = None) -> list[dict]:
    """Convert MCP tool definitions to OpenAI function-calling format.

    MCP inputSchema is already JSON Schema — it maps 1:1 to OpenAI parameters.
    Optionally filter to only allowed tool names.
    """
    result = []
    for tool in mcp_tools:
        if allowed and tool["name"] not in allowed:
            continue
        result.append({
            "type": "function",
            "function": {
                "name": tool["name"],
                "description": tool.get("description", ""),
                "parameters": tool.get("inputSchema", {"type": "object", "properties": {}}),
            },
        })
    return result


# ---------------------------------------------------------------------------
# Guardrails
# ---------------------------------------------------------------------------

class Guardrails:
    """Pre- and post-call hooks that sit between the agent loop and MCP execution.

    Features:
        - Pre-call hook registry: modify or reject tool calls before execution
        - Auto-preflight: run dev_preflight_check before any cp_create_* call
        - Dataplane injection: ensure dataplaneId is present for cp_create_listener
        - Post-call hooks: record created resources in a deployment manifest
    """

    def __init__(self, mcp_client: FlowplaneMCPClient):
        self.mcp = mcp_client
        self._pre_hooks: list[Callable[[str, dict], dict | None]] = []
        self._post_hooks: list[Callable[[str, dict, dict], None]] = []
        self._preflight_done: bool = False
        self._auto_preflight: bool = False
        self._dataplane_injection: bool = False
        self._name_dedup: bool = False
        self._port_validation: bool = False
        self._port_range: tuple[int, int] = (10000, 10020)
        self.manifest: dict[str, list[dict]] = {
            "clusters": [],
            "route_configs": [],
            "listeners": [],
            "virtual_hosts": [],
            "routes": [],
            "filters": [],
            "dataplanes": [],
        }

    # -- Configuration ------------------------------------------------------

    def enable_auto_preflight(self) -> Guardrails:
        """Auto-run dev_preflight_check before any cp_create_* call."""
        self._auto_preflight = True
        return self

    def enable_dataplane_injection(self) -> Guardrails:
        """Auto-inject dataplaneId into cp_create_listener if missing."""
        self._dataplane_injection = True
        return self

    def enable_name_dedup(self) -> Guardrails:
        """Check for existing resources before cp_create_* calls.

        Queries the corresponding cp_list_* tool to see if the name already
        exists. If it does, appends a short suffix (port number or counter)
        to make the name unique and injects a ``_dedup_warning`` key so the
        LLM can report what happened.
        """
        self._name_dedup = True
        return self

    def enable_port_validation(self, low: int = 10000, high: int = 10020) -> Guardrails:
        """Warn when cp_create_listener targets a port outside the expected range.

        Also calls cp_query_port to check whether the port is already in use.
        The call is *not* blocked — a warning is injected into the args so the
        LLM can surface it to the user.
        """
        self._port_validation = True
        self._port_range = (low, high)
        return self

    def add_pre_hook(self, hook: Callable[[str, dict], dict | None]) -> None:
        """Register a pre-call hook.  Return modified args, or None to reject."""
        self._pre_hooks.append(hook)

    def add_post_hook(self, hook: Callable[[str, dict, dict], None]) -> None:
        """Register a post-call hook (tool_name, args, result)."""
        self._post_hooks.append(hook)

    def reset_turn(self) -> None:
        """Reset per-turn state (e.g. preflight flag). Call at the start of each run()."""
        self._preflight_done = False

    # -- Execution ----------------------------------------------------------

    def before_call(self, tool_name: str, args: dict) -> dict:
        """Run all pre-call logic. Returns (possibly modified) args.

        Raises ``GuardrailReject`` if a hook returns None.
        """
        # Auto-preflight for create calls
        if self._auto_preflight and tool_name.startswith("cp_create_") and not self._preflight_done:
            try:
                self.mcp.call_tool("dev_preflight_check", args)
            except Exception:
                pass  # best-effort; don't block the call
            self._preflight_done = True

        # Dataplane injection for listener creation
        if self._dataplane_injection and tool_name == "cp_create_listener":
            if "dataplaneId" not in args or not args["dataplaneId"]:
                args = dict(args)  # don't mutate original
                dp_id = self._resolve_dataplane_id()
                if dp_id:
                    args["dataplaneId"] = dp_id

        # Name dedup for create calls
        if self._name_dedup and tool_name.startswith("cp_create_") and args.get("name"):
            args = self._dedup_name(tool_name, args)

        # Port validation for listener creation
        if self._port_validation and tool_name == "cp_create_listener":
            args = self._validate_port(args)

        # Custom pre-hooks
        for hook in self._pre_hooks:
            result = hook(tool_name, args)
            if result is None:
                raise GuardrailReject(f"Pre-hook rejected call to {tool_name}")
            args = result

        return args

    def after_call(self, tool_name: str, args: dict, result: dict) -> None:
        """Run all post-call logic, including manifest tracking."""
        # Track created resources
        if tool_name.startswith("cp_create_"):
            resource_type = tool_name.removeprefix("cp_create_")
            # Pluralise to match manifest keys
            key = resource_type + "s" if not resource_type.endswith("s") else resource_type
            if key in self.manifest:
                entry = {"name": args.get("name", ""), "args": args}
                # Try to capture ID from result
                for id_field in ("id", "Id", "ID"):
                    if id_field in (result or {}):
                        entry["id"] = result[id_field]
                        break
                self.manifest[key].append(entry)

        # Custom post-hooks
        for hook in self._post_hooks:
            try:
                hook(tool_name, args, result)
            except Exception:
                pass

    # -- Internals ----------------------------------------------------------

    # Map cp_create_* tool names to the corresponding cp_list_* tool and
    # the response key that holds the list of items.
    _CREATE_TO_LIST: dict[str, tuple[str, str]] = {
        "cp_create_cluster": ("cp_list_clusters", "clusters"),
        "cp_create_route_config": ("cp_list_route_configs", "route_configs"),
        "cp_create_listener": ("cp_list_listeners", "listeners"),
        "cp_create_virtual_host": ("cp_list_virtual_hosts", "virtual_hosts"),
        "cp_create_route": ("cp_list_routes", "routes"),
        "cp_create_filter": ("cp_list_filters", "filters"),
        "cp_create_dataplane": ("cp_list_dataplanes", "dataplanes"),
    }

    def _dedup_name(self, tool_name: str, args: dict) -> dict:
        """Check if the proposed name already exists; suffix it if so."""
        mapping = self._CREATE_TO_LIST.get(tool_name)
        if not mapping:
            return args

        list_tool, list_key = mapping
        try:
            resp = self.mcp.call_tool(list_tool, {})
            items = resp.get(list_key) or resp.get("items") or []
            if isinstance(resp, list):
                items = resp
        except Exception:
            return args  # best-effort

        existing_names = {item.get("name", "") for item in items}
        proposed = args["name"]
        if proposed not in existing_names:
            return args

        # Generate a unique name by appending a counter
        args = dict(args)
        for i in range(2, 100):
            candidate = f"{proposed}-{i}"
            if candidate not in existing_names:
                args["name"] = candidate
                args["_dedup_warning"] = (
                    f"Name '{proposed}' already exists. "
                    f"Renamed to '{candidate}'."
                )
                break
        return args

    def _validate_port(self, args: dict) -> dict:
        """Check port availability and range for listener creation."""
        port = args.get("port")
        if port is None:
            return args

        args = dict(args)
        warnings: list[str] = []
        low, high = self._port_range

        if not (low <= port <= high):
            warnings.append(
                f"Port {port} is outside the recommended Envoy range "
                f"({low}-{high}). Envoy may not expose this port."
            )

        # Check if port is already in use
        try:
            resp = self.mcp.call_tool("cp_query_port", {"port": port})
            # If the response indicates the port is in use, warn
            if resp.get("in_use") or resp.get("listener"):
                listener_name = ""
                if isinstance(resp.get("listener"), dict):
                    listener_name = resp["listener"].get("name", "")
                elif isinstance(resp.get("listener"), str):
                    listener_name = resp["listener"]
                warnings.append(
                    f"Port {port} is already in use"
                    + (f" by listener '{listener_name}'" if listener_name else "")
                    + "."
                )
        except Exception:
            pass  # best-effort

        if warnings:
            args["_port_warnings"] = warnings

        return args

    def _resolve_dataplane_id(self) -> str | None:
        """List dataplanes; return first ID or create a default one."""
        try:
            dps = self.mcp.call_tool("cp_list_dataplanes", {})
            items = dps.get("dataplanes") or dps.get("items") or []
            if isinstance(dps, list):
                items = dps
            if items:
                return items[0].get("id") or items[0].get("Id")
            # None exist — create a default
            created = self.mcp.call_tool("cp_create_dataplane", {"name": "default-dp"})
            dp_id = created.get("id") or created.get("Id")
            self.manifest["dataplanes"].append({"name": "default-dp", "id": dp_id})
            return dp_id
        except Exception:
            return None


class GuardrailReject(Exception):
    """Raised when a guardrail pre-hook rejects a tool call."""


# ---------------------------------------------------------------------------
# Conversation Memory
# ---------------------------------------------------------------------------

class ConversationMemory:
    """Sliding-window message history for multi-turn chat sessions.

    Keeps the system prompt outside the window so it's never evicted.
    """

    def __init__(self, max_history: int = 20):
        self.max_history = max_history
        self._messages: list[dict] = []
        self._manifest: dict[str, list[dict]] | None = None

    @property
    def messages(self) -> list[dict]:
        """Return current message history."""
        return list(self._messages)

    @property
    def deployment_context(self) -> str:
        """Summary of resources created this session (from guardrails manifest)."""
        if not self._manifest:
            return ""
        parts = []
        for kind, items in self._manifest.items():
            if items:
                names = ", ".join(i.get("name", i.get("id", "?")) for i in items)
                parts.append(f"{kind}: [{names}]")
        return "Deployed this session: " + "; ".join(parts) if parts else ""

    def link_manifest(self, manifest: dict[str, list[dict]]) -> None:
        """Link to a Guardrails manifest for deployment_context."""
        self._manifest = manifest

    def add(self, message: dict) -> None:
        """Append a message, trimming oldest if over limit."""
        self._messages.append(message)
        while len(self._messages) > self.max_history:
            self._messages.pop(0)

    def add_many(self, messages: list[dict]) -> None:
        """Append multiple messages."""
        for m in messages:
            self.add(m)

    def reset(self) -> None:
        """Clear all history."""
        self._messages.clear()

    def to_messages(self, system_prompt: str) -> list[dict]:
        """Build a full messages list with system prompt prepended.

        Injects deployment context if available.
        """
        ctx = self.deployment_context
        sys_content = system_prompt
        if ctx:
            sys_content += f"\n\n## Session Context\n{ctx}"
        return [{"role": "system", "content": sys_content}] + self.messages


# ---------------------------------------------------------------------------
# Agent
# ---------------------------------------------------------------------------

class FlowplaneAgent:
    """Model-agnostic agent loop with optional guardrails and streaming."""

    def __init__(
        self,
        mcp_client: FlowplaneMCPClient,
        llm_base_url: str,
        api_key: str,
        model: str,
        system_prompt: str,
        allowed_tools: set[str] | None = None,
        max_turns: int = 15,
        guardrails: Guardrails | None = None,
    ):
        self.mcp = mcp_client
        self.llm = OpenAI(base_url=llm_base_url, api_key=api_key)
        self.model = model
        self.system_prompt = system_prompt
        self.max_turns = max_turns
        self.guardrails = guardrails

        # Load and convert tools from MCP server
        mcp_tools = self.mcp.list_tools()
        self.tools = mcp_to_openai_tools(mcp_tools, allowed_tools)
        self.tool_names = {t["function"]["name"] for t in self.tools}

    # -- Stateless single-shot run ------------------------------------------

    def run(self, user_message: str) -> str:
        """Run the agentic loop: send message -> LLM -> tool calls -> execute -> repeat."""
        messages = [
            {"role": "system", "content": self.system_prompt},
            {"role": "user", "content": user_message},
        ]
        if self.guardrails:
            self.guardrails.reset_turn()

        for turn in range(self.max_turns):
            response = self.llm.chat.completions.create(
                model=self.model,
                messages=messages,
                tools=self.tools if self.tools else None,
            )

            choice = response.choices[0]
            message = choice.message

            if not message.tool_calls:
                return message.content or ""

            messages.append(message)

            for tool_call in message.tool_calls:
                fn_name = tool_call.function.name
                try:
                    fn_args = json.loads(tool_call.function.arguments)
                except json.JSONDecodeError:
                    fn_args = {}

                result_str = self._execute_tool(fn_name, fn_args)

                messages.append({
                    "role": "tool",
                    "tool_call_id": tool_call.id,
                    "content": result_str,
                })

        return messages[-1].get("content", "Max turns reached without final answer.")

    # -- Streaming run ------------------------------------------------------

    def run_stream(
        self, user_message: str, memory: ConversationMemory | None = None
    ) -> Generator[dict[str, Any], None, None]:
        """Run the agentic loop, yielding events for each step.

        Events:
            {"type": "thinking", "content": "..."}   — assistant reasoning text
            {"type": "tool_call", "name": "...", "args": {...}}
            {"type": "tool_result", "name": "...", "result": {...}}
            {"type": "answer", "content": "..."}      — final response
        """
        if memory is not None:
            memory.add({"role": "user", "content": user_message})
            messages = memory.to_messages(self.system_prompt)
        else:
            messages = [
                {"role": "system", "content": self.system_prompt},
                {"role": "user", "content": user_message},
            ]

        if self.guardrails:
            self.guardrails.reset_turn()

        for turn in range(self.max_turns):
            response = self.llm.chat.completions.create(
                model=self.model,
                messages=messages,
                tools=self.tools if self.tools else None,
            )

            choice = response.choices[0]
            message = choice.message

            if not message.tool_calls:
                content = message.content or ""
                if memory is not None:
                    memory.add({"role": "assistant", "content": content})
                yield {"type": "answer", "content": content}
                return

            # Emit thinking if the assistant sent text alongside tool calls
            if message.content:
                yield {"type": "thinking", "content": message.content}

            messages.append(message)

            for tool_call in message.tool_calls:
                fn_name = tool_call.function.name
                try:
                    fn_args = json.loads(tool_call.function.arguments)
                except json.JSONDecodeError:
                    fn_args = {}

                yield {"type": "tool_call", "name": fn_name, "args": fn_args}

                result_str = self._execute_tool(fn_name, fn_args)
                try:
                    result_parsed = json.loads(result_str)
                except json.JSONDecodeError:
                    result_parsed = result_str

                yield {"type": "tool_result", "name": fn_name, "result": result_parsed}

                messages.append({
                    "role": "tool",
                    "tool_call_id": tool_call.id,
                    "content": result_str,
                })

        yield {"type": "answer", "content": "Max turns reached without final answer."}

    # -- Tool execution (shared) --------------------------------------------

    def _execute_tool(self, fn_name: str, fn_args: dict) -> str:
        """Execute a single tool call through MCP, applying guardrails if present."""
        print(f"  -> {fn_name}({json.dumps(fn_args, separators=(',', ':'))})", file=sys.stderr)

        if fn_name not in self.tool_names:
            return json.dumps({"error": f"Unknown tool: {fn_name}"})

        # Guardrail pre-call
        if self.guardrails:
            try:
                fn_args = self.guardrails.before_call(fn_name, fn_args)
            except GuardrailReject as e:
                return json.dumps({"error": str(e)})

        try:
            result = self.mcp.call_tool(fn_name, fn_args)
            result_str = json.dumps(result, separators=(",", ":"))
        except Exception as e:
            return json.dumps({"error": str(e)})

        # Guardrail post-call
        if self.guardrails:
            self.guardrails.after_call(fn_name, fn_args, result)

        return result_str

    # -- Interactive chat ---------------------------------------------------

    def chat(self, stream: bool = False) -> None:
        """Interactive chat loop with conversation memory.

        Args:
            stream: If True, print tool calls and results in real-time.
        """
        print(f"Flowplane Agent ({self.model}) | {len(self.tools)} tools loaded")
        print("Type 'quit' to exit.\n")

        memory = ConversationMemory()
        if self.guardrails:
            memory.link_manifest(self.guardrails.manifest)

        while True:
            try:
                user_input = input("you> ").strip()
            except (EOFError, KeyboardInterrupt):
                print()
                break

            if user_input.lower() in ("quit", "exit", "q"):
                break
            if not user_input:
                continue

            if stream:
                answer = ""
                for event in self.run_stream(user_input, memory=memory):
                    if event["type"] == "thinking":
                        print(f"  [thinking] {event['content']}", file=sys.stderr)
                    elif event["type"] == "tool_call":
                        print(
                            f"  ⚡ {event['name']}({json.dumps(event['args'], separators=(',', ':'))})",
                            file=sys.stderr,
                        )
                    elif event["type"] == "tool_result":
                        preview = json.dumps(event["result"], separators=(",", ":"))
                        if len(preview) > 200:
                            preview = preview[:200] + "…"
                        print(f"  ✓ {event['name']} → {preview}", file=sys.stderr)
                    elif event["type"] == "answer":
                        answer = event["content"]
                print(f"\nagent> {answer}\n")
            else:
                # Non-streaming: use memory by running through run_stream but collecting silently
                answer = ""
                for event in self.run_stream(user_input, memory=memory):
                    if event["type"] == "answer":
                        answer = event["content"]
                print(f"\nagent> {answer}\n")
