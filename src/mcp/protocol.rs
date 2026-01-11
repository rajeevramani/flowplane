//! MCP Protocol Types
//!
//! JSON-RPC 2.0 and MCP message types based on MCP specification (version 2024-11-05).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// JSON-RPC 2.0 Request
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<JsonRpcId>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 Response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<JsonRpcId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    String(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// MCP error codes
pub mod error_codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// MCP Initialize Request
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequest {
    pub protocol_version: String,
    pub capabilities: Capabilities,
    pub client_info: ClientInfo,
}

/// Backward compatibility alias
pub type InitializeParams = InitializeRequest;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// MCP Initialize Response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub protocol_version: String,
    pub capabilities: Capabilities,
    pub server_info: ServerInfo,
}

/// Backward compatibility alias
pub type InitializeResult = InitializeResponse;

/// MCP Initialized Notification
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InitializedNotification {
    pub method: String,
}

impl Default for InitializedNotification {
    fn default() -> Self {
        Self { method: "notifications/initialized".to_string() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// MCP Capabilities for both client and server
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct Capabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceCapabilities>,
}

/// Backward compatibility aliases
pub type ClientCapabilities = Capabilities;
pub type ServerCapabilities = Capabilities;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilities {
    pub list_changed: Option<bool>,
}

/// Backward compatibility alias
pub type ToolsCapability = ToolCapabilities;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceCapabilities {
    pub subscribe: Option<bool>,
    pub list_changed: Option<bool>,
}

/// Backward compatibility alias
pub type ResourcesCapability = ResourceCapabilities;

/// MCP Tool Definition
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// MCP Tools List Response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ToolsListResult {
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// MCP Tool Call Parameters
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ToolCallRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// Backward compatibility alias
pub type ToolCallParams = ToolCallRequest;

/// MCP Tool Call Result
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ToolCallResult {
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
}

/// MCP Resource
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// MCP Resources List Response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResourcesListResult {
    pub resources: Vec<Resource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// MCP Resource Read Parameters
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResourceReadParams {
    pub uri: String,
}

/// MCP Resource Read Response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResourceReadResult {
    pub contents: Vec<ResourceContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::String("test-1".to_string())),
            method: "initialize".to_string(),
            params: serde_json::json!({"test": "value"}),
        };

        let json = serde_json::to_string(&request).expect("Failed to serialize");
        let deserialized: JsonRpcRequest =
            serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.jsonrpc, "2.0");
        assert_eq!(deserialized.method, "initialize");
        assert_eq!(deserialized.id, Some(JsonRpcId::String("test-1".to_string())));
    }

    #[test]
    fn test_json_rpc_response_serialization() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Number(42)),
            result: Some(serde_json::json!({"success": true})),
            error: None,
        };

        let json = serde_json::to_string(&response).expect("Failed to serialize");
        let deserialized: JsonRpcResponse =
            serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(deserialized.jsonrpc, "2.0");
        assert_eq!(deserialized.id, Some(JsonRpcId::Number(42)));
        assert!(deserialized.result.is_some());
        assert!(deserialized.error.is_none());
    }

    #[test]
    fn test_initialize_request_deserialization() {
        let json = r#"{
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }"#;

        let request: InitializeRequest = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(request.protocol_version, "2024-11-05");
        assert_eq!(request.client_info.name, "test-client");
        assert_eq!(request.client_info.version, "1.0.0");
    }

    #[test]
    fn test_initialized_notification_default() {
        let notification = InitializedNotification::default();
        assert_eq!(notification.method, "notifications/initialized");
    }

    #[test]
    fn test_tool_definition() {
        let tool = Tool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        };

        let serialized = serde_json::to_value(&tool).expect("Failed to serialize");
        assert_eq!(serialized["name"], "test_tool");
        assert_eq!(serialized["description"], "A test tool");
    }

    #[test]
    fn test_tool_call_request() {
        let request = ToolCallRequest {
            name: "test_tool".to_string(),
            arguments: Some(serde_json::json!({"arg": "value"})),
        };

        let serialized = serde_json::to_value(&request).expect("Failed to serialize");
        assert_eq!(serialized["name"], "test_tool");
        assert!(serialized["arguments"].is_object());
    }

    #[test]
    fn test_content_block_serialization() {
        let block = ContentBlock::Text { text: "Hello".to_string() };
        let serialized = serde_json::to_value(&block).expect("Failed to serialize");
        assert_eq!(serialized["type"], "text");
        assert_eq!(serialized["text"], "Hello");
    }

    #[test]
    fn test_resource_definition() {
        let resource = Resource {
            uri: "flowplane://clusters/prod".to_string(),
            name: "Production Cluster".to_string(),
            description: Some("Prod config".to_string()),
            mime_type: Some("application/json".to_string()),
        };

        assert_eq!(resource.uri, "flowplane://clusters/prod");
        assert_eq!(resource.name, "Production Cluster");
    }
}
