//! Request and response types for MCP tools API

use crate::api::handlers::pagination::default_limit;
use crate::domain::mcp::SchemaSource;
use crate::domain::{McpToolCategory, McpToolSourceType};
use crate::mcp::protocol::Tool;
use crate::storage::McpToolData;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Query parameters for listing MCP tools
#[derive(Debug, Clone, Deserialize, ToSchema, IntoParams, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListMcpToolsQuery {
    /// Filter by category (control_plane or gateway_api)
    #[serde(default)]
    pub category: Option<McpToolCategory>,

    /// Filter by enabled status
    #[serde(default)]
    pub enabled: Option<bool>,

    /// Search by tool name (case-insensitive partial match)
    #[serde(default)]
    pub search: Option<String>,

    /// Maximum number of tools to return (default: 50)
    #[serde(default = "default_limit")]
    pub limit: i64,

    /// Offset for pagination (default: 0)
    #[serde(default)]
    pub offset: i64,
}

/// MCP tool response DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpToolResponse {
    /// Unique identifier for the tool
    pub id: String,

    /// Team that owns this tool
    pub team: String,

    /// Tool name (e.g., "api_getUser", "create_cluster")
    pub name: String,

    /// Human-readable description of what the tool does
    pub description: Option<String>,

    /// Category: control_plane or gateway_api
    pub category: McpToolCategory,

    /// Source type: builtin, openapi, learned, or manual
    pub source_type: McpToolSourceType,

    /// Whether this is a built-in tool (cannot be edited/disabled)
    #[serde(default)]
    pub is_builtin: bool,

    /// JSON Schema for tool input parameters
    pub input_schema: serde_json::Value,

    /// JSON Schema for expected output (optional)
    pub output_schema: Option<serde_json::Value>,

    /// Reference to learned schema if enriched from learning (optional)
    pub learned_schema_id: Option<i64>,

    /// Source of the schema information (optional)
    pub schema_source: Option<SchemaSource>,

    /// Route ID for gateway_api tools (required for gateway_api category)
    pub route_id: Option<String>,

    /// HTTP method for gateway_api tools (GET, POST, PUT, DELETE, etc.)
    pub http_method: Option<String>,

    /// HTTP path pattern for gateway_api tools
    pub http_path: Option<String>,

    /// Target cluster name for gateway_api tools
    pub cluster_name: Option<String>,

    /// Envoy listener port for execution
    pub listener_port: Option<i64>,

    /// Whether this tool is enabled (true) or disabled (false)
    pub enabled: bool,

    /// Confidence score (1.0 for OpenAPI, 0.0-1.0 for learned)
    pub confidence: Option<f64>,

    /// Timestamp when the tool was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp when the tool was last updated
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<McpToolData> for McpToolResponse {
    fn from(data: McpToolData) -> Self {
        Self {
            id: data.id.to_string(),
            team: data.team,
            name: data.name,
            description: data.description,
            category: data.category,
            source_type: data.source_type,
            is_builtin: false, // Database tools are not built-in
            input_schema: data.input_schema,
            output_schema: data.output_schema,
            learned_schema_id: data.learned_schema_id,
            schema_source: data.schema_source.and_then(|s| s.parse().ok()),
            route_id: data.route_id.map(|id| id.to_string()),
            http_method: data.http_method,
            http_path: data.http_path,
            cluster_name: data.cluster_name,
            listener_port: data.listener_port,
            enabled: data.enabled,
            confidence: data.confidence,
            created_at: data.created_at,
            updated_at: data.updated_at,
        }
    }
}

impl McpToolResponse {
    /// Create an McpToolResponse from a built-in CP Tool definition.
    ///
    /// CP tools are hardcoded in the MCP handler and always enabled.
    pub fn from_builtin_tool(tool: &Tool, team: &str) -> Self {
        let now = Utc::now();
        Self {
            id: format!("builtin:{}", tool.name),
            team: team.to_string(),
            name: tool.name.clone(),
            description: tool.description.clone(),
            category: McpToolCategory::ControlPlane,
            source_type: McpToolSourceType::Builtin,
            is_builtin: true,
            input_schema: tool.input_schema.clone(),
            output_schema: None, // CP tools don't have output schemas
            learned_schema_id: None,
            schema_source: None,
            route_id: None,
            http_method: None,
            http_path: None,
            cluster_name: None,
            listener_port: None,
            enabled: true,         // CP tools are always enabled
            confidence: Some(1.0), // Built-in tools have 100% confidence
            created_at: now,
            updated_at: now,
        }
    }
}

/// Request body for updating an MCP tool
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMcpToolBody {
    /// Tool name (e.g., "api_getUser")
    pub name: Option<String>,

    /// Human-readable description
    pub description: Option<String>,

    /// Category: control_plane or gateway_api
    pub category: Option<McpToolCategory>,

    /// HTTP method for gateway_api tools (GET, POST, PUT, DELETE, etc.)
    pub http_method: Option<String>,

    /// HTTP path pattern for gateway_api tools
    pub http_path: Option<String>,

    /// Envoy listener port for gateway_api tool execution
    pub listener_port: Option<i64>,

    /// Target cluster name for gateway_api tools
    pub cluster_name: Option<String>,

    /// JSON Schema for tool input parameters
    pub input_schema: Option<serde_json::Value>,

    /// JSON Schema for expected output (optional)
    pub output_schema: Option<serde_json::Value>,

    /// Whether this tool is enabled
    pub enabled: Option<bool>,
}

// === Learned Schema Types ===

/// Request body for applying learned schema to a route
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplyLearnedSchemaRequest {
    /// Force override even if current source is OpenAPI
    #[serde(default)]
    pub force: Option<bool>,
}

/// Response for applying learned schema
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplyLearnedSchemaResponse {
    /// Whether the operation was successful
    pub success: bool,

    /// Previous source type before applying
    pub previous_source: String,

    /// ID of the learned schema that was applied
    pub learned_schema_id: i64,

    /// Confidence score of the applied schema
    pub confidence: f64,

    /// Number of samples used to learn the schema
    pub sample_count: i64,
}

/// Response for checking learned schema availability
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckLearnedSchemaResponse {
    /// Whether a learned schema is available
    pub available: bool,

    /// Learned schema information if available
    pub schema: Option<LearnedSchemaInfoResponse>,

    /// Current source type of the route metadata
    pub current_source: String,

    /// Whether the learned schema can be applied (confidence >= 0.8)
    pub can_apply: bool,

    /// Whether force flag is required (current source is OpenAPI)
    pub requires_force: bool,
}

/// Information about a learned schema
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LearnedSchemaInfoResponse {
    /// Schema ID
    pub id: i64,

    /// Confidence score (0.0 to 1.0)
    pub confidence: f64,

    /// Number of samples used to learn the schema
    pub sample_count: i64,

    /// Schema version
    pub version: i64,

    /// When the schema was last observed (ISO 8601 format)
    pub last_observed: String,
}
