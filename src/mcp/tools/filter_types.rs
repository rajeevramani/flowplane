//! Filter Type MCP Tools
//!
//! Control Plane tools for discovering available filter types and their schemas.

use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Tool definition for listing filter types
pub fn cp_list_filter_types_tool() -> Tool {
    Tool::new(
        "cp_list_filter_types",
        r#"List all available filter types with their configuration schemas.

PURPOSE: Discover what filter types can be used when creating filters.

FILTER TYPES are templates that define:
- What the filter does (e.g., JWT authentication, rate limiting, CORS)
- How to configure it (JSON Schema for validation)
- Where it can attach (Route, Listener, Cluster)
- Envoy HTTP filter name for xDS generation
- Per-route configuration behavior

RETURNS: Array of filter type objects with:
- name: Filter type identifier (e.g., "jwt_auth", "cors", "rate_limit")
- display_name: Human-readable name
- description: What the filter does
- version: Schema version
- envoy_filter_name: Underlying Envoy HTTP filter name
- attachment_points: Where filter can attach (Route, Listener, Cluster)
- requires_listener_config: Whether listener-level config is required
- per_route_behavior: How filter behaves per-route (full_config, reference_only, disable_only, not_supported)
- is_implemented: Whether filter is fully implemented
- source: "built_in" or "custom"
- config_schema: JSON Schema for filter configuration

WHEN TO USE:
- Before creating a filter to understand available options
- To get the config schema for validation
- To understand filter capabilities and attachment points

FILTERING:
- Returns all filter types (both built-in and custom)
- Use is_implemented field to filter production-ready filters
- Use source field to distinguish built-in from custom filters

RELATED TOOLS: cp_get_filter_type (detailed info), cp_create_filter (use a filter type)"#,
        json!({
            "type": "object",
            "properties": {}
        }),
    )
}

/// Tool definition for getting a specific filter type
pub fn cp_get_filter_type_tool() -> Tool {
    Tool::new(
        "cp_get_filter_type",
        r#"Get detailed information about a specific filter type by name.

PURPOSE: Retrieve complete schema definition and metadata for a filter type.

USE CASES:
- Understand how to configure a specific filter
- Get JSON Schema for validation before creating a filter
- Check filter capabilities (attachment points, per-route behavior)
- Verify filter implementation status

REQUIRED PARAMETERS:
- name: Filter type name (e.g., "jwt_auth", "cors", "local_rate_limit")

RETURNS: Filter type object with:
- name: Filter type identifier
- display_name: Human-readable name (e.g., "JWT Authentication")
- description: Detailed explanation of what the filter does
- version: Schema version for compatibility tracking
- envoy_filter_name: Envoy HTTP filter name (e.g., "envoy.filters.http.jwt_authn")
- attachment_points: Valid attachment points (e.g., ["Route", "Listener"])
- requires_listener_config: Whether listener-level configuration is required
- per_route_behavior: How filter handles per-route config:
  * "full_config" - Full configuration override per route
  * "reference_only" - Reference to listener config by name
  * "disable_only" - Can only disable filter per route
  * "not_supported" - No per-route configuration
- is_implemented: true if filter is production-ready, false if planned
- source: "built_in" (shipped with CP) or "custom" (user-defined)
- config_schema: Complete JSON Schema for validating filter configuration

COMMON FILTER TYPES:
- jwt_auth: JWT authentication
- cors: Cross-Origin Resource Sharing
- local_rate_limit: Local in-memory rate limiting
- rate_limit: External distributed rate limiting
- ext_authz: External authorization service
- rbac: Role-based access control
- compressor: Response compression (gzip)
- oauth2: OAuth2 authentication
- header_mutation: Add/modify/remove headers
- custom_response: Custom error responses
- mcp: Model Context Protocol for AI gateway

EXAMPLE:
To create a JWT filter, first get the schema:
1. Call cp_get_filter_type with name="jwt_auth"
2. Review config_schema to understand required fields
3. Use cp_create_filter with validated configuration

RELATED TOOLS: cp_list_filter_types (discovery), cp_create_filter (use this type)"#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Filter type name (e.g., 'jwt_auth', 'cors', 'rate_limit')"
                }
            },
            "required": ["name"]
        }),
    )
}

/// Execute list filter types operation
#[instrument(skip(xds_state), name = "mcp_execute_list_filter_types")]
pub async fn execute_list_filter_types(
    xds_state: &Arc<XdsState>,
    _team: &str,
    _args: Value,
) -> Result<ToolCallResult, McpError> {
    // Access the filter schema registry
    let registry = xds_state.get_filter_schema_registry();

    // List all schemas
    let schemas = registry.list_all();

    // Transform to JSON response
    let filter_types: Vec<Value> = schemas
        .iter()
        .map(|schema| {
            json!({
                "name": schema.name,
                "display_name": schema.display_name,
                "description": schema.description,
                "version": schema.version,
                "envoy_filter_name": schema.envoy.http_filter_name,
                "attachment_points": schema.capabilities.attachment_points.iter()
                    .map(|ap| match ap {
                        crate::domain::filter::AttachmentPoint::Route => "Route",
                        crate::domain::filter::AttachmentPoint::Listener => "Listener",
                        crate::domain::filter::AttachmentPoint::Cluster => "Cluster",
                    })
                    .collect::<Vec<_>>(),
                "requires_listener_config": schema.capabilities.requires_listener_config,
                "per_route_behavior": match schema.capabilities.per_route_behavior {
                    crate::domain::filter::PerRouteBehavior::FullConfig => "full_config",
                    crate::domain::filter::PerRouteBehavior::ReferenceOnly => "reference_only",
                    crate::domain::filter::PerRouteBehavior::DisableOnly => "disable_only",
                    crate::domain::filter::PerRouteBehavior::NotSupported => "not_supported",
                },
                "is_implemented": schema.is_implemented,
                "source": match schema.source {
                    crate::domain::filter_schema::SchemaSource::BuiltIn => "built_in",
                    crate::domain::filter_schema::SchemaSource::Custom => "custom",
                },
                "config_schema": schema.config_schema,
            })
        })
        .collect();

    let result = json!({
        "filter_types": filter_types,
        "count": filter_types.len()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

/// Execute get filter type operation
#[instrument(skip(xds_state), name = "mcp_execute_get_filter_type")]
pub async fn execute_get_filter_type(
    xds_state: &Arc<XdsState>,
    _team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Parse filter type name
    let name = args["name"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    // Access the filter schema registry
    let registry = xds_state.get_filter_schema_registry();

    // Get the specific schema
    let schema = registry.get(name).ok_or_else(|| {
        McpError::ResourceNotFound(format!(
            "Filter type '{}' not found. Use cp_list_filter_types to see available filter types.",
            name
        ))
    })?;

    // Transform to JSON response
    let result = json!({
        "name": schema.name,
        "display_name": schema.display_name,
        "description": schema.description,
        "version": schema.version,
        "envoy_filter_name": schema.envoy.http_filter_name,
        "attachment_points": schema.capabilities.attachment_points.iter()
            .map(|ap| match ap {
                crate::domain::filter::AttachmentPoint::Route => "Route",
                crate::domain::filter::AttachmentPoint::Listener => "Listener",
                crate::domain::filter::AttachmentPoint::Cluster => "Cluster",
            })
            .collect::<Vec<_>>(),
        "requires_listener_config": schema.capabilities.requires_listener_config,
        "per_route_behavior": match schema.capabilities.per_route_behavior {
            crate::domain::filter::PerRouteBehavior::FullConfig => "full_config",
            crate::domain::filter::PerRouteBehavior::ReferenceOnly => "reference_only",
            crate::domain::filter::PerRouteBehavior::DisableOnly => "disable_only",
            crate::domain::filter::PerRouteBehavior::NotSupported => "not_supported",
        },
        "is_implemented": schema.is_implemented,
        "source": match schema.source {
            crate::domain::filter_schema::SchemaSource::BuiltIn => "built_in",
            crate::domain::filter_schema::SchemaSource::Custom => "custom",
        },
        "config_schema": schema.config_schema,
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_filter_types_tool_definition() {
        let tool = cp_list_filter_types_tool();
        assert_eq!(tool.name, "cp_list_filter_types");
        assert!(tool.description.as_ref().unwrap().contains("filter type"));
        assert!(tool.description.as_ref().unwrap().contains("configuration schemas"));
        assert!(tool.description.as_ref().unwrap().contains("WHEN TO USE"));

        // Should have no required parameters
        let schema = &tool.input_schema;
        assert!(schema["properties"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_cp_get_filter_type_tool_definition() {
        let tool = cp_get_filter_type_tool();
        assert_eq!(tool.name, "cp_get_filter_type");
        assert!(tool.description.as_ref().unwrap().contains("specific filter type"));
        assert!(tool.description.as_ref().unwrap().contains("name"));

        // Check required field
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));

        // Check name parameter exists
        let properties = tool.input_schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("name"));
        assert_eq!(properties["name"]["type"], "string");
    }

    #[test]
    fn test_tool_names_are_unique() {
        let tools = [cp_list_filter_types_tool(), cp_get_filter_type_tool()];

        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let mut unique_names = names.clone();
        unique_names.sort();
        unique_names.dedup();

        assert_eq!(names.len(), unique_names.len(), "Tool names must be unique");
    }

    #[test]
    fn test_tool_descriptions_have_purpose() {
        let tools = [cp_list_filter_types_tool(), cp_get_filter_type_tool()];

        for tool in &tools {
            assert!(
                tool.description.as_ref().unwrap().contains("PURPOSE:"),
                "Tool {} should have PURPOSE section",
                tool.name
            );
        }
    }

    #[test]
    fn test_tool_descriptions_have_related_tools() {
        let tools = [cp_list_filter_types_tool(), cp_get_filter_type_tool()];

        for tool in &tools {
            assert!(
                tool.description.as_ref().unwrap().contains("RELATED TOOLS:"),
                "Tool {} should have RELATED TOOLS section",
                tool.name
            );
        }
    }

    #[test]
    fn test_list_tool_has_empty_schema() {
        let tool = cp_list_filter_types_tool();
        let properties = tool.input_schema["properties"].as_object().unwrap();
        assert!(properties.is_empty(), "List tool should have no parameters");
    }

    #[test]
    fn test_get_tool_requires_name() {
        let tool = cp_get_filter_type_tool();
        let required = tool.input_schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1, "Get tool should have exactly one required parameter");
        assert!(required.contains(&json!("name")), "Get tool should require 'name' parameter");
    }
}
