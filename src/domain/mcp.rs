//! MCP (Model Context Protocol) domain types
//!
//! This module defines the domain types related to MCP functionality,
//! including tool categories, source types, and tool/metadata definitions.

use crate::domain::id::{McpToolId, RouteId, RouteMetadataId, TeamId};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

/// MCP tool category
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpToolCategory {
    /// Control plane tools for managing gateway configuration
    ControlPlane,
    /// Gateway API tools for proxying to upstream services
    GatewayApi,
}

impl fmt::Display for McpToolCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ControlPlane => write!(f, "control_plane"),
            Self::GatewayApi => write!(f, "gateway_api"),
        }
    }
}

impl FromStr for McpToolCategory {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "control_plane" => Ok(Self::ControlPlane),
            "gateway_api" => Ok(Self::GatewayApi),
            _ => Err(format!("Invalid McpToolCategory: {}", s)),
        }
    }
}

/// MCP tool source type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpToolSourceType {
    /// Built-in control plane tools
    Builtin,
    /// Generated from OpenAPI spec
    Openapi,
    /// Learned from traffic
    Learned,
    /// Manually created
    Manual,
}

impl fmt::Display for McpToolSourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin => write!(f, "builtin"),
            Self::Openapi => write!(f, "openapi"),
            Self::Learned => write!(f, "learned"),
            Self::Manual => write!(f, "manual"),
        }
    }
}

impl FromStr for McpToolSourceType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "builtin" => Ok(Self::Builtin),
            "openapi" => Ok(Self::Openapi),
            "learned" => Ok(Self::Learned),
            "manual" => Ok(Self::Manual),
            _ => Err(format!("Invalid McpToolSourceType: {}", s)),
        }
    }
}

/// Route metadata source type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteMetadataSourceType {
    /// Extracted from OpenAPI specification
    Openapi,
    /// Manually added
    Manual,
    /// Learned from traffic patterns
    Learned,
}

/// Source of schema information for MCP tools
///
/// Tracks where the schema information originated from:
/// - OpenApi: Extracted from OpenAPI specification
/// - Learned: Derived from observed API traffic
/// - Manual: Manually entered by user
/// - Mixed: Combination of multiple sources (e.g., OpenAPI enriched with learning)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchemaSource {
    /// Schema from OpenAPI specification
    Openapi,
    /// Schema from learned API patterns
    Learned,
    /// Manually entered schema
    Manual,
    /// Mixed sources (e.g., OpenAPI enriched with learning)
    Mixed,
}

impl fmt::Display for RouteMetadataSourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Openapi => write!(f, "openapi"),
            Self::Manual => write!(f, "manual"),
            Self::Learned => write!(f, "learned"),
        }
    }
}

impl FromStr for RouteMetadataSourceType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "openapi" => Ok(Self::Openapi),
            "manual" => Ok(Self::Manual),
            "learned" => Ok(Self::Learned),
            _ => Err(format!("Invalid RouteMetadataSourceType: {}", s)),
        }
    }
}

impl fmt::Display for SchemaSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Openapi => write!(f, "openapi"),
            Self::Learned => write!(f, "learned"),
            Self::Manual => write!(f, "manual"),
            Self::Mixed => write!(f, "mixed"),
        }
    }
}

impl FromStr for SchemaSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "openapi" => Ok(Self::Openapi),
            "learned" => Ok(Self::Learned),
            "manual" => Ok(Self::Manual),
            "mixed" => Ok(Self::Mixed),
            _ => Err(format!("Invalid SchemaSource: {}", s)),
        }
    }
}

/// MCP tool definition
///
/// Represents a tool that can be executed by AI assistants through the MCP protocol.
/// Tools can be control plane operations (managing gateway config) or gateway API
/// operations (proxying to upstream services).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct McpTool {
    /// Unique identifier for the tool
    pub id: McpToolId,

    /// Team that owns this tool (for multi-tenancy isolation)
    pub team: TeamId,

    /// Tool name (e.g., "api_getUser", "create_cluster")
    pub name: String,

    /// Human-readable description of what the tool does
    pub description: Option<String>,

    /// Category: control_plane or gateway_api
    pub category: McpToolCategory,

    /// Source type: builtin, openapi, learned, or manual
    pub source_type: McpToolSourceType,

    /// JSON Schema for tool input parameters
    pub input_schema: String,

    /// JSON Schema for expected output (optional)
    pub output_schema: Option<String>,

    /// Reference to learned schema if enriched from learning (optional)
    pub learned_schema_id: Option<i64>,

    /// Source of the schema information (optional)
    pub schema_source: Option<SchemaSource>,

    /// Route ID for gateway_api tools (required for gateway_api category)
    pub route_id: Option<RouteId>,

    /// HTTP method for gateway_api tools (GET, POST, PUT, DELETE, etc.)
    pub http_method: Option<String>,

    /// HTTP path pattern for gateway_api tools
    pub http_path: Option<String>,

    /// Target cluster name for gateway_api tools
    pub cluster_name: Option<String>,

    /// Envoy listener port for execution
    pub listener_port: Option<i32>,

    /// Whether this tool is enabled (true) or disabled (false)
    pub enabled: bool,

    /// Confidence score (1.0 for OpenAPI, 0.0-1.0 for learned)
    pub confidence: Option<f64>,

    /// Timestamp when the tool was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp when the tool was last updated
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Route metadata extracted from OpenAPI specs or learning
///
/// Stores OpenAPI metadata for routes to enable MCP tool generation and
/// provide rich context for AI assistants. Can be enriched with information
/// from API learning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RouteMetadata {
    /// Unique identifier for the metadata
    pub id: RouteMetadataId,

    /// Route this metadata belongs to
    pub route_id: RouteId,

    /// OpenAPI operation ID (e.g., "getUser", "createOrder")
    pub operation_id: Option<String>,

    /// Short summary of the operation
    pub summary: Option<String>,

    /// Detailed description of the operation
    pub description: Option<String>,

    /// OpenAPI tags (comma-separated)
    pub tags: Option<String>,

    /// HTTP method (GET, POST, PUT, DELETE, etc.)
    pub http_method: Option<String>,

    /// JSON Schema for request body
    pub request_body_schema: Option<String>,

    /// JSON Schema for response bodies (status code -> schema mapping)
    pub response_schemas: Option<String>,

    /// Reference to learned schema if enriched from learning (optional)
    pub learning_schema_id: Option<i64>,

    /// Whether this metadata was enriched from learning data
    pub enriched_from_learning: bool,

    /// Source type: openapi, manual, or learned
    pub source_type: RouteMetadataSourceType,

    /// Confidence score for learned metadata
    pub confidence: Option<f64>,

    /// Timestamp when the metadata was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp when the metadata was last updated
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_tool_category_display() {
        assert_eq!(McpToolCategory::ControlPlane.to_string(), "control_plane");
        assert_eq!(McpToolCategory::GatewayApi.to_string(), "gateway_api");
    }

    #[test]
    fn test_mcp_tool_category_from_str() {
        assert_eq!(
            "control_plane".parse::<McpToolCategory>().unwrap(),
            McpToolCategory::ControlPlane
        );
        assert_eq!("gateway_api".parse::<McpToolCategory>().unwrap(), McpToolCategory::GatewayApi);
        assert!("invalid".parse::<McpToolCategory>().is_err());
    }

    #[test]
    fn test_mcp_tool_source_type_display() {
        assert_eq!(McpToolSourceType::Builtin.to_string(), "builtin");
        assert_eq!(McpToolSourceType::Openapi.to_string(), "openapi");
        assert_eq!(McpToolSourceType::Learned.to_string(), "learned");
        assert_eq!(McpToolSourceType::Manual.to_string(), "manual");
    }

    #[test]
    fn test_route_metadata_source_type_display() {
        assert_eq!(RouteMetadataSourceType::Openapi.to_string(), "openapi");
        assert_eq!(RouteMetadataSourceType::Manual.to_string(), "manual");
        assert_eq!(RouteMetadataSourceType::Learned.to_string(), "learned");
    }

    #[test]
    fn test_schema_source_display() {
        assert_eq!(SchemaSource::Openapi.to_string(), "openapi");
        assert_eq!(SchemaSource::Learned.to_string(), "learned");
        assert_eq!(SchemaSource::Manual.to_string(), "manual");
        assert_eq!(SchemaSource::Mixed.to_string(), "mixed");
    }

    #[test]
    fn test_schema_source_from_str() {
        assert_eq!("openapi".parse::<SchemaSource>().unwrap(), SchemaSource::Openapi);
        assert_eq!("learned".parse::<SchemaSource>().unwrap(), SchemaSource::Learned);
        assert_eq!("manual".parse::<SchemaSource>().unwrap(), SchemaSource::Manual);
        assert_eq!("mixed".parse::<SchemaSource>().unwrap(), SchemaSource::Mixed);
        assert!("invalid".parse::<SchemaSource>().is_err());
    }

    #[test]
    fn test_mcp_tool_structure() {
        let tool = McpTool {
            id: McpToolId::new(),
            team: TeamId::new(),
            name: "api_getUser".to_string(),
            description: Some("Get user by ID".to_string()),
            category: McpToolCategory::GatewayApi,
            source_type: McpToolSourceType::Openapi,
            input_schema: r#"{"type":"object","properties":{"id":{"type":"string"}}}"#.to_string(),
            output_schema: Some(
                r#"{"type":"object","properties":{"name":{"type":"string"}}}"#.to_string(),
            ),
            learned_schema_id: None,
            schema_source: Some(SchemaSource::Openapi),
            route_id: Some(RouteId::new()),
            http_method: Some("GET".to_string()),
            http_path: Some("/users/{id}".to_string()),
            cluster_name: Some("user-service".to_string()),
            listener_port: Some(8080),
            enabled: true,
            confidence: Some(1.0),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        assert_eq!(tool.name, "api_getUser");
        assert_eq!(tool.category, McpToolCategory::GatewayApi);
        assert!(tool.enabled);
    }

    #[test]
    fn test_route_metadata_structure() {
        let metadata = RouteMetadata {
            id: RouteMetadataId::new(),
            route_id: RouteId::new(),
            operation_id: Some("getUser".to_string()),
            summary: Some("Get user by ID".to_string()),
            description: Some("Returns a single user".to_string()),
            tags: Some("users".to_string()),
            http_method: Some("GET".to_string()),
            request_body_schema: None,
            response_schemas: Some(
                r#"{"200":{"type":"object","properties":{"id":{"type":"string"}}}}"#.to_string(),
            ),
            learning_schema_id: None,
            enriched_from_learning: false,
            source_type: RouteMetadataSourceType::Openapi,
            confidence: Some(1.0),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        assert_eq!(metadata.operation_id.as_deref(), Some("getUser"));
        assert_eq!(metadata.source_type, RouteMetadataSourceType::Openapi);
        assert!(!metadata.enriched_from_learning);
    }

    #[test]
    fn test_mcp_tool_category_serde() {
        let category = McpToolCategory::ControlPlane;
        let json = serde_json::to_string(&category).expect("Failed to serialize");
        assert_eq!(json, "\"control_plane\"");

        let deserialized: McpToolCategory =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(category, deserialized);
    }

    #[test]
    fn test_schema_source_serde() {
        let source = SchemaSource::Mixed;
        let json = serde_json::to_string(&source).expect("Failed to serialize");
        assert_eq!(json, "\"mixed\"");

        let deserialized: SchemaSource =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(source, deserialized);
    }
}
