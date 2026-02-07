//! OpenAPI Import MCP Tools
//!
//! Control Plane tools for viewing OpenAPI import records.

use crate::internal_api::{InternalAuthContext, ListOpenApiImportsRequest, OpenApiOperations};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Tool definition for listing OpenAPI imports
pub fn cp_list_openapi_imports_tool() -> Tool {
    Tool::new(
        "cp_list_openapi_imports",
        r#"List OpenAPI import records to track which specs have been imported into the Control Plane.

PURPOSE: View all OpenAPI specifications that have been imported, including metadata about when they were imported and what resources were created.

WHAT IS AN OPENAPI IMPORT?
When you import an OpenAPI spec (via cp_import_openapi or the REST API), the Control Plane:
1. Creates listeners, route configs, routes, and clusters from the spec
2. Stores metadata about the import for tracking and management
3. Links all created resources back to this import record

IMPORT RECORD FIELDS:
- id: Unique import ID (UUID)
- spec_name: Name/title from OpenAPI spec
- spec_version: Version from OpenAPI spec (if provided)
- spec_checksum: SHA-256 checksum of the source spec for change detection
- team: Team that owns this import
- listener_name: Name of the listener created for this spec
- imported_at: Timestamp when the spec was first imported
- updated_at: Timestamp of last update/reimport

WHEN TO USE:
- Check if a spec has already been imported
- Find import records to get their IDs for cp_get_openapi_import
- List all specs imported by a team
- Verify import metadata after importing a spec

FILTERING:
- limit: Maximum number of results (1-100)
- offset: Pagination offset

TEAM SCOPING:
- Non-admin users only see imports for their allowed teams
- Admin users see all imports across all teams

RETURNS: Array of import metadata objects sorted by imported_at (newest first).

RELATED TOOLS: cp_get_openapi_import (detailed view of single import)"#,
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "minimum": 1,
                    "maximum": 100
                },
                "offset": {
                    "type": "integer",
                    "description": "Number of results to skip for pagination",
                    "minimum": 0
                }
            }
        }),
    )
}

/// Tool definition for getting a specific OpenAPI import
pub fn cp_get_openapi_import_tool() -> Tool {
    Tool::new(
        "cp_get_openapi_import",
        r#"Get detailed information about a specific OpenAPI import by ID.

PURPOSE: Retrieve complete import metadata including the full spec content, checksums, and resource linkages.

RETURNS:
- id: Unique import ID (UUID)
- spec_name: Name/title from the OpenAPI specification
- spec_version: Version string from the spec (e.g., "1.0.0")
- spec_checksum: SHA-256 checksum of the original spec content
- team: Team that owns this import
- source_content: Full OpenAPI spec content (JSON/YAML as stored)
- listener_name: Name of the listener created for this spec
- imported_at: When the spec was first imported (RFC3339 timestamp)
- updated_at: When the import was last updated (RFC3339 timestamp)

WHEN TO USE:
- View full details of an import found via cp_list_openapi_imports
- Check if a spec has changed (compare checksums)
- Retrieve the original spec content for reference
- Verify which listener was created for a spec

USE CASES:
1. Import Verification: After importing, verify the metadata was stored correctly
2. Change Detection: Compare spec_checksum before reimporting to detect changes
3. Spec Retrieval: Get the original spec content without re-uploading
4. Resource Tracking: Find which listener was created for this spec

REQUIRED PARAMETERS:
- id: OpenAPI import UUID (from cp_list_openapi_imports)

TEAM SCOPING:
- Users can only access imports for their allowed teams
- Attempting to access another team's import returns NotFound error

RELATED TOOLS: cp_list_openapi_imports (discover imports)"#,
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "OpenAPI import UUID"
                }
            },
            "required": ["id"]
        }),
    )
}

/// Execute list OpenAPI imports operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_openapi_imports")]
pub async fn execute_list_openapi_imports(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32);
    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32);

    // Validate limit range
    if let Some(lim) = limit {
        if !(1..=100).contains(&lim) {
            return Err(McpError::InvalidParams("limit must be between 1 and 100".to_string()));
        }
    }

    // Validate offset range
    if let Some(off) = offset {
        if off < 0 {
            return Err(McpError::InvalidParams("offset must be >= 0".to_string()));
        }
    }

    // Use internal API layer
    let ops = OpenApiOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = ListOpenApiImportsRequest { limit, offset };

    let imports = ops.list(req, &auth).await?;

    let result = json!({
        "imports": imports.iter().map(|i| {
            json!({
                "id": i.id,
                "spec_name": i.spec_name,
                "spec_version": i.spec_version,
                "spec_checksum": i.spec_checksum,
                "team": i.team,
                "listener_name": i.listener_name,
                "imported_at": i.imported_at.to_rfc3339(),
                "updated_at": i.updated_at.to_rfc3339()
            })
        }).collect::<Vec<_>>(),
        "count": imports.len()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

/// Execute get OpenAPI import operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_openapi_import")]
pub async fn execute_get_openapi_import(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: id".to_string()))?;

    // Use internal API layer
    let ops = OpenApiOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let import = ops.get(id, &auth).await?;

    let result = json!({
        "id": import.id,
        "spec_name": import.spec_name,
        "spec_version": import.spec_version,
        "spec_checksum": import.spec_checksum,
        "team": import.team,
        "source_content": import.source_content,
        "listener_name": import.listener_name,
        "imported_at": import.imported_at.to_rfc3339(),
        "updated_at": import.updated_at.to_rfc3339()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_openapi_imports_tool_definition() {
        let tool = cp_list_openapi_imports_tool();
        assert_eq!(tool.name, "cp_list_openapi_imports");
        assert!(tool.description.as_ref().unwrap().contains("OpenAPI import"));
        assert!(tool.description.as_ref().unwrap().contains("import records"));

        // Check that limit and offset are supported
        assert!(tool.input_schema["properties"]["limit"].is_object());
        assert!(tool.input_schema["properties"]["offset"].is_object());

        // Check that no fields are required
        assert!(
            tool.input_schema.get("required").is_none()
                || tool.input_schema["required"].as_array().unwrap().is_empty()
        );
    }

    #[test]
    fn test_cp_get_openapi_import_tool_definition() {
        let tool = cp_get_openapi_import_tool();
        assert_eq!(tool.name, "cp_get_openapi_import");
        assert!(tool.description.as_ref().unwrap().contains("detailed information"));
        assert!(tool.description.as_ref().unwrap().contains("OpenAPI import"));

        // Check required field
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("id")));
    }

    #[test]
    fn test_tool_names_are_unique() {
        let tools = [cp_list_openapi_imports_tool(), cp_get_openapi_import_tool()];

        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let mut unique_names = names.clone();
        unique_names.sort();
        unique_names.dedup();

        assert_eq!(names.len(), unique_names.len(), "Tool names must be unique");
    }

    #[test]
    fn test_tool_descriptions_are_comprehensive() {
        // List tool should explain pagination
        let list_tool = cp_list_openapi_imports_tool();
        assert!(list_tool.description.as_ref().unwrap().contains("limit"));
        assert!(list_tool.description.as_ref().unwrap().contains("offset"));
        assert!(list_tool.description.as_ref().unwrap().contains("WHEN TO USE"));

        // Get tool should explain all fields
        let get_tool = cp_get_openapi_import_tool();
        assert!(get_tool.description.as_ref().unwrap().contains("spec_name"));
        assert!(get_tool.description.as_ref().unwrap().contains("spec_version"));
        assert!(get_tool.description.as_ref().unwrap().contains("spec_checksum"));
        assert!(get_tool.description.as_ref().unwrap().contains("source_content"));
        assert!(get_tool.description.as_ref().unwrap().contains("listener_name"));
    }

    #[test]
    fn test_list_tool_schema_validation() {
        let tool = cp_list_openapi_imports_tool();
        let schema = &tool.input_schema;

        // Check limit constraints
        let limit = &schema["properties"]["limit"];
        assert_eq!(limit["type"], "integer");
        assert_eq!(limit["minimum"], 1);
        assert_eq!(limit["maximum"], 100);

        // Check offset constraints
        let offset = &schema["properties"]["offset"];
        assert_eq!(offset["type"], "integer");
        assert_eq!(offset["minimum"], 0);
    }

    #[test]
    fn test_get_tool_schema_validation() {
        let tool = cp_get_openapi_import_tool();
        let schema = &tool.input_schema;

        // Check id is required
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "id");

        // Check id is a string
        let id = &schema["properties"]["id"];
        assert_eq!(id["type"], "string");
    }
}
