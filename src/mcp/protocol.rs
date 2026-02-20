//! MCP Protocol Types
//!
//! JSON-RPC 2.0 and MCP message types based on MCP specification (version 2025-11-25).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Supported MCP protocol version (2025-11-25 only)
///
/// As of this version, we only support MCP 2025-11-25 (Streamable HTTP transport).
/// Older protocol versions are not supported. Use mcp-remote bridge for clients
/// that don't yet support 2025-11-25 (e.g., Claude Desktop using 2025-06-18).
pub const PROTOCOL_VERSION: &str = "2025-11-25";

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, ToSchema)]
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

/// Client information provided during initialization (MCP 2025-11-25)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    /// Required client name
    pub name: String,

    /// Required client version
    pub version: String,

    /// Optional human-readable display title
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Optional client description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional display icons
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,

    /// Optional website URL for client information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
}

/// MCP Initialize Response (MCP 2025-11-25)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub protocol_version: String,
    pub capabilities: Capabilities,
    pub server_info: ServerInfo,

    /// Optional server-specific instructions for the client
    ///
    /// These instructions provide guidance on how to use the server,
    /// what tools are available, and any special considerations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
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

/// Server information provided during initialization (MCP 2025-11-25)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// Required server name
    pub name: String,

    /// Required server version
    pub version: String,

    /// Optional human-readable display title
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Optional server description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional display icons
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,

    /// Optional website URL for server information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
}

/// MCP Capabilities for both client and server (MCP 2025-11-25)
///
/// This struct handles both client and server capabilities:
/// - Server capabilities: tools, resources, prompts, logging, completions, tasks
/// - Client capabilities: roots, sampling
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    // Server capabilities
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completions: Option<CompletionsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<TasksCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<ExperimentalCapabilities>,

    // Client capabilities (MCP 2025-11-25)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<serde_json::Value>,
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

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptsCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct LoggingCapability {}

/// Experimental capabilities (MCP 2025-11-25)
///
/// The spec defines this as `{ [key: string]: object }` - a map of experimental
/// feature names to their configuration objects. Using serde_json::Value for
/// maximum flexibility in accepting any experimental capabilities from clients.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct ExperimentalCapabilities(pub serde_json::Value);

/// Client capability for providing filesystem roots (MCP 2025-11-25)
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RootsCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Client capability for LLM sampling (MCP 2025-11-25)
///
/// Indicates client support for server-initiated sampling requests,
/// including context inclusion and tool use during sampling.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SamplingCapability {
    /// Whether the client supports context inclusion via includeContext parameter
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,

    /// Whether the client supports tool use via tools and toolChoice parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<serde_json::Value>,
}

// -----------------------------------------------------------------------------
// Completions and Tasks Capabilities (MCP 2025-11-25)
// -----------------------------------------------------------------------------

/// Completions capability for server-side autocompletion (MCP 2025-11-25)
///
/// The spec defines this as just `completions?: object` - an empty object
/// whose presence indicates the server supports argument autocompletion.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct CompletionsCapability {}

/// Tasks capability for client/server task management (MCP 2025-11-25)
///
/// Capability flags can be either booleans or objects with nested configuration.
/// Using serde_json::Value to accept both formats from different MCP clients.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TasksCapability {
    /// Task listing capability (bool or object)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<serde_json::Value>,

    /// Task cancellation capability (bool or object)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel: Option<serde_json::Value>,

    /// Task request capabilities
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<TaskRequests>,
}

/// Task request capability configuration (MCP 2025-11-25)
///
/// Specifies which types of task requests a client/server supports.
/// - Server capabilities use: `tools: { call?: object }`
/// - Client capabilities use: `sampling: { createMessage?: object }`, `elicitation: { create?: object }`
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskRequests {
    /// Server: Tool call capabilities for tasks (e.g., `{ call: {} }`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<serde_json::Value>,

    /// Client: Sampling request capabilities (e.g., `{ createMessage: {} }`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<serde_json::Value>,

    /// Client: Elicitation request capabilities (e.g., `{ create: {} }`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<serde_json::Value>,
}

// -----------------------------------------------------------------------------
// Icon and Annotation Types (MCP 2025-11-25)
// -----------------------------------------------------------------------------

/// Icon for display in user interfaces
///
/// Icons can be used for tools, servers, and clients to provide visual
/// representation in MCP host UIs.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Icon {
    /// URL or data URI of the icon resource
    pub src: String,

    /// MIME type of the icon (e.g., "image/png", "image/svg+xml")
    pub mime_type: String,

    /// Optional size hints (e.g., ["48x48", "any"])
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<String>>,

    /// Optional theme variant ("light" or "dark")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

/// Tool behavioral hints and metadata (MCP 2025-11-25)
///
/// Annotations provide hints about tool behavior that clients can use
/// for UI presentation and decision-making.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ToolAnnotations {
    /// Hints about tool behavior (free-form JSON, untrusted unless from trusted server)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<serde_json::Value>,
}

/// MCP Tool Definition (MCP 2025-11-25)
///
/// Enhanced tool definition with optional fields for title, icons,
/// output schema, and behavioral annotations.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    /// Unique identifier for the tool (1-128 characters)
    pub name: String,

    /// Optional human-readable display title (different from name)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Human-readable description (optional per MCP 2025-11-25)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// JSON Schema for input validation
    pub input_schema: serde_json::Value,

    /// Optional JSON Schema for output structure
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,

    /// Optional display icons
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,

    /// Optional behavioral annotations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

impl Tool {
    /// Create a new tool with required fields, defaulting optional fields to None
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            title: None,
            description: Some(description.into()),
            input_schema,
            output_schema: None,
            icons: None,
            annotations: None,
        }
    }
}

impl Default for Tool {
    fn default() -> Self {
        Self {
            name: String::new(),
            title: None,
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: None,
            icons: None,
            annotations: None,
        }
    }
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

// -----------------------------------------------------------------------------
// Prompts API Types
// -----------------------------------------------------------------------------

/// MCP Prompt definition
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

/// Prompt argument definition
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// Prompts list response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PromptsListResult {
    pub prompts: Vec<Prompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Prompt get request parameters
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PromptGetParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// Prompt get response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PromptGetResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub messages: Vec<PromptMessage>,
}

/// Prompt message in a prompt template
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum PromptMessage {
    User { content: PromptContent },
    Assistant { content: PromptContent },
}

/// Prompt content (text or image)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PromptContent {
    Text { text: String },
    Image { data: String, mime_type: String },
}

// -----------------------------------------------------------------------------
// MCP Connections API Types
// -----------------------------------------------------------------------------

/// Type of MCP connection/session
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionType {
    /// SSE streaming connection
    Sse,
    /// HTTP-only session (stateless)
    Http,
}

/// Connection information for listing active MCP connections and sessions
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    /// Unique connection identifier
    pub connection_id: String,
    /// Team the connection belongs to
    pub team: String,
    /// ISO 8601 timestamp when connection was established
    pub created_at: String,
    /// ISO 8601 timestamp of last activity
    pub last_activity: String,
    /// Current log level filter for this connection
    pub log_level: String,
    /// Client information (name, version) if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_info: Option<ClientInfo>,
    /// Negotiated protocol version if initialized
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    /// Whether the connection has completed initialization
    pub initialized: bool,
    /// Type of connection (sse or http)
    pub connection_type: ConnectionType,
}

/// Response for connections list request
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsListResult {
    /// List of active connections
    pub connections: Vec<ConnectionInfo>,
    /// Total number of connections for the team
    pub total_count: usize,
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
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }"#;

        let request: InitializeRequest = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(request.protocol_version, "2025-11-25");
        assert_eq!(request.client_info.name, "test-client");
        assert_eq!(request.client_info.version, "1.0.0");
    }

    #[test]
    fn test_initialized_notification_default() {
        let notification = InitializedNotification::default();
        assert_eq!(notification.method, "notifications/initialized");
    }

    #[test]
    fn test_tool_definition_minimal() {
        // Test Tool with only required fields
        let tool = Tool {
            name: "test_tool".to_string(),
            title: None,
            description: Some("A test tool".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: None,
            icons: None,
            annotations: None,
        };

        let serialized = serde_json::to_value(&tool).expect("Failed to serialize");
        assert_eq!(serialized["name"], "test_tool");
        assert_eq!(serialized["description"], "A test tool");
        // Optional fields should not be present in serialized output
        assert!(serialized.get("title").is_none());
        assert!(serialized.get("outputSchema").is_none());
        assert!(serialized.get("icons").is_none());
        assert!(serialized.get("annotations").is_none());
    }

    #[test]
    fn test_tool_definition_full() {
        // Test Tool with all fields (MCP 2025-11-25)
        let tool = Tool {
            name: "test_tool".to_string(),
            title: Some("Test Tool".to_string()),
            description: Some("A test tool with all fields".to_string()),
            input_schema: serde_json::json!({"type": "object", "properties": {"arg": {"type": "string"}}}),
            output_schema: Some(
                serde_json::json!({"type": "object", "properties": {"result": {"type": "string"}}}),
            ),
            icons: Some(vec![Icon {
                src: "data:image/png;base64,ABC123".to_string(),
                mime_type: "image/png".to_string(),
                sizes: Some(vec!["48x48".to_string()]),
                theme: Some("light".to_string()),
            }]),
            annotations: Some(ToolAnnotations {
                hints: Some(serde_json::json!({"category": "test", "priority": "low"})),
            }),
        };

        let serialized = serde_json::to_value(&tool).expect("Failed to serialize");
        assert_eq!(serialized["name"], "test_tool");
        assert_eq!(serialized["title"], "Test Tool");
        assert_eq!(serialized["description"], "A test tool with all fields");
        assert!(serialized["inputSchema"].is_object());
        assert!(serialized["outputSchema"].is_object());
        assert!(serialized["icons"].is_array());
        assert!(serialized["annotations"].is_object());
    }

    #[test]
    fn test_icon_serialization() {
        let icon = Icon {
            src: "https://example.com/icon.png".to_string(),
            mime_type: "image/png".to_string(),
            sizes: Some(vec!["32x32".to_string(), "48x48".to_string()]),
            theme: Some("dark".to_string()),
        };

        let serialized = serde_json::to_value(&icon).expect("Failed to serialize");
        assert_eq!(serialized["src"], "https://example.com/icon.png");
        assert_eq!(serialized["mimeType"], "image/png");
        assert_eq!(serialized["sizes"][0], "32x32");
        assert_eq!(serialized["theme"], "dark");
    }

    #[test]
    fn test_tool_annotations_serialization() {
        let annotations = ToolAnnotations {
            hints: Some(serde_json::json!({"destructive": true, "confirmation": "required"})),
        };

        let serialized = serde_json::to_value(&annotations).expect("Failed to serialize");
        assert!(serialized["hints"]["destructive"].as_bool().unwrap());
        assert_eq!(serialized["hints"]["confirmation"], "required");
    }

    #[test]
    fn test_tool_deserialization_minimal() {
        // Test deserializing a Tool with only required fields (backward compat)
        let json = r#"{"name":"test","inputSchema":{"type":"object"}}"#;
        let tool: Tool = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(tool.name, "test");
        assert!(tool.description.is_none());
        assert!(tool.title.is_none());
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

    // -------------------------------------------------------------------------
    // Phase 1.3: Completions and Tasks Capability Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_completions_capability_serialization() {
        // Test CompletionsCapability (empty object per MCP 2025-11-25 spec)
        let capability = CompletionsCapability::default();
        let serialized = serde_json::to_value(&capability).expect("Failed to serialize");
        assert!(serialized.is_object());
        // Empty object - presence indicates support
        assert_eq!(serialized, serde_json::json!({}));
    }

    #[test]
    fn test_tasks_capability_serialization() {
        // Test minimal TasksCapability
        let capability = TasksCapability::default();
        let serialized = serde_json::to_value(&capability).expect("Failed to serialize");
        assert!(serialized.is_object());

        // Test server-style tasks capability (tools.call)
        let capability = TasksCapability {
            list: Some(serde_json::json!({})),
            cancel: Some(serde_json::json!({})),
            requests: Some(TaskRequests {
                tools: Some(serde_json::json!({"call": {}})),
                sampling: None,
                elicitation: None,
            }),
        };
        let serialized = serde_json::to_value(&capability).expect("Failed to serialize");
        assert!(serialized["list"].is_object());
        assert!(serialized["cancel"].is_object());
        assert!(serialized["requests"]["tools"]["call"].is_object());

        // Test client-style tasks capability (sampling.createMessage, elicitation.create)
        let capability = TasksCapability {
            list: Some(serde_json::json!({})),
            cancel: Some(serde_json::json!({})),
            requests: Some(TaskRequests {
                tools: None,
                sampling: Some(serde_json::json!({"createMessage": {}})),
                elicitation: Some(serde_json::json!({"create": {}})),
            }),
        };
        let serialized = serde_json::to_value(&capability).expect("Failed to serialize");
        assert!(serialized["list"].is_object());
        assert!(serialized["cancel"].is_object());
        assert!(serialized["requests"]["sampling"]["createMessage"].is_object());
        assert!(serialized["requests"]["elicitation"]["create"].is_object());
    }

    #[test]
    fn test_enhanced_capabilities_serialization() {
        // Test Capabilities with completions and tasks fields
        let capabilities = Capabilities {
            tools: Some(ToolCapabilities { list_changed: Some(true) }),
            completions: Some(CompletionsCapability {}), // Empty object per spec
            tasks: Some(TasksCapability {
                list: Some(serde_json::json!({})),
                cancel: Some(serde_json::json!({})),
                requests: None,
            }),
            ..Default::default()
        };

        let serialized = serde_json::to_value(&capabilities).expect("Failed to serialize");
        assert!(serialized["tools"]["listChanged"].as_bool().unwrap());
        // Completions is an empty object (presence indicates support)
        assert!(serialized["completions"].is_object());
        assert!(serialized["tasks"]["list"].is_object());
        assert!(serialized["tasks"]["cancel"].is_object());
        // Optional fields should not be serialized
        assert!(serialized.get("resources").is_none());
    }

    // -------------------------------------------------------------------------
    // Phase 1.4: Enhanced Info Struct Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_enhanced_client_info_serialization() {
        // Test minimal ClientInfo (backward compatible)
        let client_info = ClientInfo {
            name: "test-client".to_string(),
            version: "1.0.0".to_string(),
            title: None,
            description: None,
            icons: None,
            website_url: None,
        };
        let serialized = serde_json::to_value(&client_info).expect("Failed to serialize");
        assert_eq!(serialized["name"], "test-client");
        assert_eq!(serialized["version"], "1.0.0");
        assert!(serialized.get("title").is_none());

        // Test full ClientInfo with all new fields
        let client_info = ClientInfo {
            name: "test-client".to_string(),
            version: "1.0.0".to_string(),
            title: Some("Test Client".to_string()),
            description: Some("A test MCP client".to_string()),
            icons: Some(vec![Icon {
                src: "https://example.com/icon.png".to_string(),
                mime_type: "image/png".to_string(),
                sizes: Some(vec!["32x32".to_string()]),
                theme: None,
            }]),
            website_url: Some("https://example.com".to_string()),
        };
        let serialized = serde_json::to_value(&client_info).expect("Failed to serialize");
        assert_eq!(serialized["title"], "Test Client");
        assert_eq!(serialized["description"], "A test MCP client");
        assert_eq!(serialized["websiteUrl"], "https://example.com");
        assert!(serialized["icons"].is_array());
    }

    #[test]
    fn test_enhanced_server_info_serialization() {
        // Test minimal ServerInfo (backward compatible)
        let server_info = ServerInfo {
            name: "flowplane-mcp".to_string(),
            version: "0.0.3".to_string(),
            title: None,
            description: None,
            icons: None,
            website_url: None,
        };
        let serialized = serde_json::to_value(&server_info).expect("Failed to serialize");
        assert_eq!(serialized["name"], "flowplane-mcp");
        assert_eq!(serialized["version"], "0.0.3");
        assert!(serialized.get("title").is_none());

        // Test full ServerInfo with all new fields
        let server_info = ServerInfo {
            name: "flowplane-mcp".to_string(),
            version: "0.0.3".to_string(),
            title: Some("Flowplane MCP Server".to_string()),
            description: Some("Envoy control plane MCP server".to_string()),
            icons: Some(vec![Icon {
                src: "data:image/svg+xml;base64,PHN2Zy8+".to_string(),
                mime_type: "image/svg+xml".to_string(),
                sizes: Some(vec!["any".to_string()]),
                theme: Some("light".to_string()),
            }]),
            website_url: Some("https://flowplane.dev".to_string()),
        };
        let serialized = serde_json::to_value(&server_info).expect("Failed to serialize");
        assert_eq!(serialized["title"], "Flowplane MCP Server");
        assert_eq!(serialized["description"], "Envoy control plane MCP server");
        assert_eq!(serialized["websiteUrl"], "https://flowplane.dev");
        assert!(serialized["icons"].is_array());
    }

    #[test]
    fn test_enhanced_initialize_response_serialization() {
        // Test InitializeResponse without instructions (backward compatible)
        let response = InitializeResponse {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: Capabilities::default(),
            server_info: ServerInfo {
                name: "flowplane-mcp".to_string(),
                version: "0.0.3".to_string(),
                title: None,
                description: None,
                icons: None,
                website_url: None,
            },
            instructions: None,
        };
        let serialized = serde_json::to_value(&response).expect("Failed to serialize");
        assert_eq!(serialized["protocolVersion"], PROTOCOL_VERSION);
        assert!(serialized.get("instructions").is_none());

        // Test InitializeResponse with instructions
        let response = InitializeResponse {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: Capabilities::default(),
            server_info: ServerInfo {
                name: "flowplane-mcp".to_string(),
                version: "0.0.3".to_string(),
                title: None,
                description: None,
                icons: None,
                website_url: None,
            },
            instructions: Some("Use list_clusters to see available clusters.".to_string()),
        };
        let serialized = serde_json::to_value(&response).expect("Failed to serialize");
        assert_eq!(serialized["instructions"], "Use list_clusters to see available clusters.");
    }

    #[test]
    fn test_client_info_deserialization_backward_compatible() {
        // Test deserializing old-style ClientInfo (only name and version)
        let json = r#"{"name":"old-client","version":"0.1.0"}"#;
        let client_info: ClientInfo = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(client_info.name, "old-client");
        assert_eq!(client_info.version, "0.1.0");
        assert!(client_info.title.is_none());
        assert!(client_info.description.is_none());
        assert!(client_info.icons.is_none());
        assert!(client_info.website_url.is_none());
    }

    #[test]
    fn test_server_info_deserialization_backward_compatible() {
        // Test deserializing old-style ServerInfo (only name and version)
        let json = r#"{"name":"old-server","version":"1.0.0"}"#;
        let server_info: ServerInfo = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(server_info.name, "old-server");
        assert_eq!(server_info.version, "1.0.0");
        assert!(server_info.title.is_none());
        assert!(server_info.description.is_none());
        assert!(server_info.icons.is_none());
        assert!(server_info.website_url.is_none());
    }

    // -------------------------------------------------------------------------
    // MCP 2025-11-25 Sampling Capability Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_sampling_capability_with_context_and_tools() {
        // Test deserializing SamplingCapability with context and tools (MCP Inspector format)
        let json = r#"{
            "protocolVersion": "2025-11-25",
            "capabilities": {
                "sampling": {
                    "context": {},
                    "tools": {}
                }
            },
            "clientInfo": {
                "name": "mcp-inspector",
                "version": "1.0.0"
            }
        }"#;

        let request: InitializeRequest = serde_json::from_str(json)
            .expect("Should deserialize MCP 2025-11-25 sampling capability");
        assert_eq!(request.protocol_version, "2025-11-25");
        assert!(request.capabilities.sampling.is_some());
        let sampling = request.capabilities.sampling.unwrap();
        assert!(sampling.context.is_some());
        assert!(sampling.tools.is_some());
    }

    #[test]
    fn test_sampling_capability_empty() {
        // Test deserializing SamplingCapability as empty object (backward compatible)
        let json = r#"{
            "protocolVersion": "2025-11-25",
            "capabilities": {
                "sampling": {}
            },
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }"#;

        let request: InitializeRequest =
            serde_json::from_str(json).expect("Should deserialize empty sampling capability");
        assert!(request.capabilities.sampling.is_some());
        let sampling = request.capabilities.sampling.unwrap();
        assert!(sampling.context.is_none());
        assert!(sampling.tools.is_none());
    }

    #[test]
    fn test_sampling_capability_partial() {
        // Test deserializing SamplingCapability with only tools (partial)
        let json = r#"{
            "protocolVersion": "2025-11-25",
            "capabilities": {
                "sampling": {
                    "tools": {}
                }
            },
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }"#;

        let request: InitializeRequest =
            serde_json::from_str(json).expect("Should deserialize partial sampling capability");
        let sampling = request.capabilities.sampling.unwrap();
        assert!(sampling.context.is_none());
        assert!(sampling.tools.is_some());
    }

    #[test]
    fn test_mcp_inspector_payload() {
        // Test exact payload from MCP Inspector v0.19.0
        let json = r#"{
            "protocolVersion": "2025-11-25",
            "capabilities": {
                "sampling": {},
                "elicitation": {},
                "roots": {"listChanged": true},
                "tasks": {
                    "list": {},
                    "cancel": {},
                    "requests": {
                        "sampling": {"createMessage": {}},
                        "elicitation": {"create": {}}
                    }
                }
            },
            "clientInfo": {
                "name": "inspector-client",
                "version": "0.19.0"
            }
        }"#;

        let request: InitializeRequest =
            serde_json::from_str(json).expect("Should deserialize MCP Inspector payload");
        assert_eq!(request.protocol_version, "2025-11-25");
        assert_eq!(request.client_info.name, "inspector-client");
        assert!(request.capabilities.sampling.is_some());
        assert!(request.capabilities.elicitation.is_some());
        assert!(request.capabilities.roots.is_some());
        assert!(request.capabilities.tasks.is_some());

        let tasks = request.capabilities.tasks.unwrap();
        assert!(tasks.list.is_some());
        assert!(tasks.cancel.is_some());
        assert!(tasks.requests.is_some());

        let requests = tasks.requests.unwrap();
        assert!(requests.sampling.is_some());
        assert!(requests.elicitation.is_some());
    }
}
