// MCP Protocol Types for JSON-RPC 2.0 and Model Context Protocol

// ============================================================================
// JSON-RPC 2.0 Types
// ============================================================================

export interface JsonRpcRequest {
	jsonrpc: '2.0';
	id: string | number;
	method: string;
	params?: unknown;
}

export interface JsonRpcResponse {
	jsonrpc: '2.0';
	id: string | number | null;
	result?: unknown;
	error?: JsonRpcError;
}

export interface JsonRpcError {
	code: number;
	message: string;
	data?: unknown;
}

// ============================================================================
// MCP Protocol Types
// ============================================================================

export interface McpServerInfo {
	name: string;
	version: string;
}

export interface McpCapabilities {
	tools?: { listChanged?: boolean };
	resources?: { subscribe?: boolean; listChanged?: boolean };
	prompts?: { listChanged?: boolean };
	logging?: Record<string, never>;
}

export interface McpInitializeResult {
	protocolVersion: string;
	serverInfo: McpServerInfo;
	capabilities: McpCapabilities;
}

// ============================================================================
// Frontend Connection Status Types
// ============================================================================

export interface McpConnectionStatus {
	connected: boolean;
	serverInfo?: McpServerInfo;
	protocolVersion?: string;
	lastPing?: Date;
	error?: string;
}

export interface McpPingResult {
	success: boolean;
	latencyMs: number;
	error?: string;
}
