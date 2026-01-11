//! DTOs for MCP route handlers

use crate::services::mcp_service;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// MCP status response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpStatusResponse {
    /// Whether the route is ready for MCP enablement
    pub ready: bool,
    /// Whether MCP is currently enabled on the route
    pub enabled: bool,
    /// List of missing required fields
    pub missing_fields: Vec<String>,
    /// The tool name if MCP is enabled
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Recommended source for schema information
    pub recommended_source: String,
}

impl From<mcp_service::McpStatusResponse> for McpStatusResponse {
    fn from(s: mcp_service::McpStatusResponse) -> Self {
        Self {
            ready: s.ready,
            enabled: s.enabled,
            missing_fields: s.missing_fields,
            tool_name: s.tool_name,
            recommended_source: s.recommended_source,
        }
    }
}

/// Request body for enabling MCP on a route
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EnableMcpRequestBody {
    /// Optional custom tool name (defaults to api_{operation_id})
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Optional custom description (defaults to route summary)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional schema source identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_source: Option<String>,
    /// Summary for the route (used to create metadata if missing)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// HTTP method for the route (used to create metadata if missing)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_method: Option<String>,
}

/// Schema refresh result response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RefreshSchemaResponse {
    /// Whether the refresh was successful
    pub success: bool,
    /// Message describing the result
    pub message: String,
}

impl From<mcp_service::RefreshSchemaResult> for RefreshSchemaResponse {
    fn from(r: mcp_service::RefreshSchemaResult) -> Self {
        Self { success: r.success, message: r.message }
    }
}

/// Request body for bulk enabling MCP
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BulkMcpEnableRequest {
    /// List of route IDs to enable MCP on
    pub route_ids: Vec<String>,
}

/// Response for bulk enable operation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BulkMcpEnableResponse {
    /// Results for each route
    pub results: Vec<BulkEnableResult>,
    /// Number of routes successfully enabled
    pub succeeded: u32,
    /// Number of routes that failed
    pub failed: u32,
}

/// Result for a single route in bulk enable
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BulkEnableResult {
    /// Route ID
    pub route_id: String,
    /// Whether enablement was successful
    pub success: bool,
    /// The tool name if successful
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Request body for bulk disabling MCP
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BulkMcpDisableRequest {
    /// List of route IDs to disable MCP on
    pub route_ids: Vec<String>,
}

/// Response for bulk disable operation
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BulkMcpDisableResponse {
    /// Results for each route
    pub results: Vec<BulkDisableResult>,
    /// Number of routes successfully disabled
    pub succeeded: u32,
    /// Number of routes that failed
    pub failed: u32,
}

/// Result for a single route in bulk disable
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BulkDisableResult {
    /// Route ID
    pub route_id: String,
    /// Whether disablement was successful
    pub success: bool,
    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
