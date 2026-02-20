"""
Lightweight Flowplane MCP Client.

Connects to Flowplane's MCP server over Streamable HTTP (2025-11-25 protocol).
Handles session initialization, tool discovery, and tool execution.

Usage:
    client = FlowplaneMCPClient("http://localhost:8080", "platform-admin", "fp_pat_...")
    client.initialize()
    tools = client.list_tools()
    result = client.call_tool("ops_topology", {"scope": "full"})
"""

import json
import httpx


class FlowplaneMCPClient:
    def __init__(self, base_url: str, team: str, token: str):
        self.endpoint = f"{base_url}/api/v1/mcp/cp?team={team}"
        self.token = token
        self.session_id: str | None = None
        self._request_id = 0
        self._http = httpx.Client(timeout=60.0)

    def _next_id(self) -> int:
        self._request_id += 1
        return self._request_id

    def _headers(self) -> dict:
        h = {
            "Content-Type": "application/json",
            "Accept": "application/json",
            "Authorization": f"Bearer {self.token}",
        }
        if self.session_id:
            h["MCP-Session-Id"] = self.session_id
        return h

    def _rpc(self, method: str, params: dict | None = None) -> dict:
        body = {
            "jsonrpc": "2.0",
            "id": self._next_id(),
            "method": method,
            "params": params or {},
        }
        resp = self._http.post(self.endpoint, headers=self._headers(), json=body)
        resp.raise_for_status()

        # Capture session ID from headers
        sid = resp.headers.get("mcp-session-id")
        if sid:
            self.session_id = sid

        data = resp.json()
        if "error" in data:
            raise RuntimeError(f"MCP error: {data['error']['message']}")
        return data.get("result", {})

    def initialize(self) -> dict:
        """Initialize MCP session. Must be called before any other method."""
        result = self._rpc("initialize", {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "flowplane-agent", "version": "1.0.0"},
        })
        return result

    def list_tools(self) -> list[dict]:
        """List all available MCP tools with their schemas."""
        result = self._rpc("tools/list")
        return result.get("tools", [])

    def call_tool(self, name: str, arguments: dict | None = None) -> dict:
        """Execute an MCP tool and return parsed JSON result."""
        result = self._rpc("tools/call", {"name": name, "arguments": arguments or {}})
        # Extract text content from MCP response
        for block in result.get("content", []):
            if block.get("type") == "text":
                try:
                    return json.loads(block["text"])
                except json.JSONDecodeError:
                    return {"text": block["text"]}
        return result

    def call_tool_raw(self, name: str, arguments: dict | None = None) -> str:
        """Execute an MCP tool and return raw text result."""
        result = self._rpc("tools/call", {"name": name, "arguments": arguments or {}})
        for block in result.get("content", []):
            if block.get("type") == "text":
                return block["text"]
        return json.dumps(result)

    def close(self):
        self._http.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
