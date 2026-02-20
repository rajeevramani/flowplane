//! Dataplane MCP Tools
//!
//! Control Plane tools for managing dataplanes (Envoy instances).

use crate::domain::OrgId;
use crate::internal_api::{
    CreateDataplaneInternalRequest, DataplaneOperations, InternalAuthContext,
    ListDataplanesInternalRequest, UpdateDataplaneInternalRequest,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::mcp::response_builders::{
    build_delete_response, build_rich_create_response, build_update_response,
};
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Tool definition for listing dataplanes
pub fn cp_list_dataplanes_tool() -> Tool {
    Tool::new(
        "cp_list_dataplanes",
        r#"List dataplanes (Envoy instances) for managing API gateways.

PURPOSE: View all configured dataplanes to understand your gateway topology.

WHAT IS A DATAPLANE:
A dataplane represents an Envoy proxy instance that handles traffic routing, load balancing,
and request/response processing. Each dataplane can have multiple listeners, routes, and filters.

WHEN TO USE:
- Discover available dataplanes in your infrastructure
- Check dataplane configurations before creating listeners
- Verify team-owned dataplanes for multi-tenancy
- List dataplanes to find gateway hosts for MCP tool execution

FILTERING:
- limit: Maximum number of results (1-1000, default: 100)
- offset: Pagination offset

RETURNS: Array of dataplane objects with:
- id: Unique dataplane ID
- team: Owning team
- name: Dataplane name (unique within team)
- gateway_host: Optional gateway host URL
- description: Optional description
- created_at, updated_at: Lifecycle timestamps

RELATED TOOLS: cp_get_dataplane (details), cp_create_dataplane (new dataplane)"#,
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "minimum": 1,
                    "maximum": 1000
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

/// Tool definition for getting a specific dataplane
pub fn cp_get_dataplane_tool() -> Tool {
    Tool::new(
        "cp_get_dataplane",
        r#"Get detailed information about a specific dataplane by team and name.

PURPOSE: Retrieve complete dataplane details including configuration and metadata.

RETURNS:
- id: Unique dataplane ID
- team: Owning team
- name: Dataplane name
- gateway_host: Optional gateway host URL for MCP tool execution
- description: Optional human-readable description
- created_at: When dataplane was created
- updated_at: When dataplane was last modified

WHEN TO USE:
- Check configuration of a specific dataplane
- Verify dataplane exists before creating resources
- Get gateway_host for API calls or tool execution
- Review dataplane metadata

REQUIRED PARAMETERS:
- team: Team that owns the dataplane
- name: Dataplane name

RELATED TOOLS: cp_list_dataplanes (discovery), cp_create_dataplane (new dataplane)"#,
        json!({
            "type": "object",
            "properties": {
                "team": {
                    "type": "string",
                    "description": "Team that owns the dataplane"
                },
                "name": {
                    "type": "string",
                    "description": "Dataplane name"
                }
            },
            "required": ["team", "name"]
        }),
    )
}

/// Tool definition for creating a dataplane
pub fn cp_create_dataplane_tool() -> Tool {
    Tool::new(
        "cp_create_dataplane",
        r#"Create a new dataplane (Envoy instance) for routing traffic.

PURPOSE: Set up a new dataplane to host listeners, routes, and filters.

WORKFLOW:
1. Create a dataplane with a unique name within your team
2. Optionally specify a gateway_host URL for external access
3. Add listeners to the dataplane using cp_create_listener
4. Configure routes and filters for traffic management
5. Dataplane becomes active and ready to process requests

REQUIRED PARAMETERS:
- team: Team that will own this dataplane
- name: Dataplane name (must be unique within team)

OPTIONAL PARAMETERS:
- gateway_host: Gateway host URL (e.g., "https://api.example.com")
- description: Human-readable description

NAMING CONVENTIONS:
- Use descriptive names: "production-gateway", "staging-api", "dev-proxy"
- Names must be unique within a team
- Use lowercase with hyphens for consistency

GATEWAY_HOST:
- Optional URL where the dataplane is accessible
- Used for MCP tools that need to make API calls
- Can be added or updated later if not known at creation time

EXAMPLE DATAPLANES:
- "production-gateway" - Main production API gateway
- "staging-envoy" - Staging environment gateway
- "canary-proxy" - Canary deployment testing

Authorization: Requires cp:write scope and team access.
"#,
        json!({
            "type": "object",
            "properties": {
                "team": {
                    "type": "string",
                    "description": "Team that owns this dataplane"
                },
                "name": {
                    "type": "string",
                    "description": "Dataplane name (unique within team)"
                },
                "gateway_host": {
                    "type": "string",
                    "description": "Optional gateway host URL (e.g., https://api.example.com)"
                },
                "description": {
                    "type": "string",
                    "description": "Optional human-readable description"
                }
            },
            "required": ["team", "name"]
        }),
    )
}

/// Tool definition for updating a dataplane
pub fn cp_update_dataplane_tool() -> Tool {
    Tool::new(
        "cp_update_dataplane",
        r#"Update an existing dataplane's configuration.

PURPOSE: Modify dataplane settings like gateway_host or description.

WHAT CAN BE UPDATED:
- gateway_host: Change or set the gateway host URL
- description: Update the human-readable description

WHAT CANNOT BE CHANGED:
- team: Dataplane ownership is immutable
- name: Use delete + create to rename

REQUIRED PARAMETERS:
- team: Team that owns the dataplane
- name: Dataplane name

OPTIONAL PARAMETERS (at least one required):
- gateway_host: New gateway host URL
- description: New description

WHEN TO USE:
- Update gateway_host when URL changes
- Improve descriptions for better documentation
- Correct configuration mistakes

EXAMPLE USE CASES:
- Change gateway_host from staging to production URL
- Add description to undocumented dataplane
- Update gateway_host after infrastructure changes

Authorization: Requires cp:write scope and team access.
"#,
        json!({
            "type": "object",
            "properties": {
                "team": {
                    "type": "string",
                    "description": "Team that owns the dataplane"
                },
                "name": {
                    "type": "string",
                    "description": "Dataplane name to update"
                },
                "gateway_host": {
                    "type": "string",
                    "description": "New gateway host URL"
                },
                "description": {
                    "type": "string",
                    "description": "New description"
                }
            },
            "required": ["team", "name"]
        }),
    )
}

/// Tool definition for deleting a dataplane
pub fn cp_delete_dataplane_tool() -> Tool {
    Tool::new(
        "cp_delete_dataplane",
        r#"Delete a dataplane and all associated resources.

PURPOSE: Remove a dataplane that is no longer needed.

WARNING: DESTRUCTIVE OPERATION
- Deletes the dataplane permanently
- May cascade delete associated listeners, routes, and filters
- Cannot be undone - verify before deleting

PREREQUISITES FOR DELETION:
- Dataplane should have no active listeners
- All routes should be removed or migrated
- Backup any critical configurations before deletion

REQUIRED PARAMETERS:
- team: Team that owns the dataplane
- name: Dataplane name to delete

WHEN TO USE:
- Decommission old or unused dataplanes
- Clean up test/development dataplanes
- Remove misconfigured dataplanes

WORKFLOW TO DELETE:
1. Use cp_get_dataplane to verify current state
2. Remove or migrate any listeners/routes if needed
3. Call this tool to delete the dataplane
4. Verify deletion with cp_list_dataplanes

CANNOT DELETE:
- Dataplanes that are actively routing traffic (check listeners first)
- Dataplanes with dependent resources (remove dependencies first)

Authorization: Requires cp:write scope and team access.
"#,
        json!({
            "type": "object",
            "properties": {
                "team": {
                    "type": "string",
                    "description": "Team that owns the dataplane"
                },
                "name": {
                    "type": "string",
                    "description": "Dataplane name to delete"
                }
            },
            "required": ["team", "name"]
        }),
    )
}

/// Execute list dataplanes operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_dataplanes")]
pub async fn execute_list_dataplanes(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32);
    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32);

    // Use internal API layer
    let ops = DataplaneOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = ListDataplanesInternalRequest { limit, offset };

    let dataplanes = ops.list(req, &auth).await?;

    let result = json!({
        "dataplanes": dataplanes.iter().map(|dp| {
            json!({
                "id": dp.id.to_string(),
                "team": dp.team,
                "name": dp.name,
                "gateway_host": dp.gateway_host,
                "description": dp.description,
                "created_at": dp.created_at.to_rfc3339(),
                "updated_at": dp.updated_at.to_rfc3339()
            })
        }).collect::<Vec<_>>(),
        "count": dataplanes.len()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

/// Execute get dataplane operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_dataplane")]
pub async fn execute_get_dataplane(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let dataplane_team = args["team"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: team".to_string()))?;

    let name = args["name"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    // Use internal API layer
    let ops = DataplaneOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let dataplane = ops.get(dataplane_team, name, &auth).await?;

    let result = json!({
        "id": dataplane.id.to_string(),
        "team": dataplane.team,
        "name": dataplane.name,
        "gateway_host": dataplane.gateway_host,
        "description": dataplane.description,
        "created_at": dataplane.created_at.to_rfc3339(),
        "updated_at": dataplane.updated_at.to_rfc3339()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

/// Execute create dataplane operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_create_dataplane")]
pub async fn execute_create_dataplane(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let dataplane_team = args
        .get("team")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: team".to_string()))?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    // 2. Parse optional fields
    let gateway_host = args.get("gateway_host").and_then(|v| v.as_str()).map(String::from);
    let description = args.get("description").and_then(|v| v.as_str()).map(String::from);

    tracing::debug!(
        team = %team,
        dataplane_team = %dataplane_team,
        dataplane_name = %name,
        "Creating dataplane via MCP"
    );

    // 3. Use internal API layer
    let ops = DataplaneOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = CreateDataplaneInternalRequest {
        team: dataplane_team.to_string(),
        name: name.to_string(),
        gateway_host,
        description,
    };

    let result = ops.create(req, &auth).await?;

    // 4. Format rich response with team context and next-step guidance
    let output = build_rich_create_response(
        "dataplane",
        &result.data.name,
        result.data.id.as_ref(),
        Some(json!({"team": dataplane_team})),
        None,
        Some("Create clusters with cp_create_cluster for backend services"),
    );

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        dataplane_team = %dataplane_team,
        dataplane_name = %name,
        dataplane_id = %result.data.id,
        "Successfully created dataplane via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute update dataplane operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_update_dataplane")]
pub async fn execute_update_dataplane(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let dataplane_team = args["team"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: team".to_string()))?;

    let name = args["name"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    // 2. Parse optional fields (at least one should be present)
    let gateway_host = args.get("gateway_host").and_then(|v| v.as_str()).map(String::from);
    let description = args.get("description").and_then(|v| v.as_str()).map(String::from);

    if gateway_host.is_none() && description.is_none() {
        return Err(McpError::InvalidParams(
            "At least one field (gateway_host or description) must be provided for update"
                .to_string(),
        ));
    }

    tracing::debug!(
        team = %team,
        dataplane_team = %dataplane_team,
        dataplane_name = %name,
        "Updating dataplane via MCP"
    );

    // 3. Use internal API layer
    let ops = DataplaneOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = UpdateDataplaneInternalRequest { gateway_host, description };

    let result = ops.update(dataplane_team, name, req, &auth).await?;

    // 4. Format success response (minimal token-efficient format)
    let output = build_update_response("dataplane", &result.data.name, result.data.id.as_ref());

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        dataplane_team = %dataplane_team,
        dataplane_name = %name,
        "Successfully updated dataplane via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute delete dataplane operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_delete_dataplane")]
pub async fn execute_delete_dataplane(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let dataplane_team = args["team"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: team".to_string()))?;

    let name = args["name"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        dataplane_team = %dataplane_team,
        dataplane_name = %name,
        "Deleting dataplane via MCP"
    );

    // 2. Use internal API layer
    let ops = DataplaneOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    ops.delete(dataplane_team, name, &auth).await?;

    // 3. Format success response (minimal token-efficient format)
    let output = build_delete_response();

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        dataplane_team = %dataplane_team,
        dataplane_name = %name,
        "Successfully deleted dataplane via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_dataplanes_tool_definition() {
        let tool = cp_list_dataplanes_tool();
        assert_eq!(tool.name, "cp_list_dataplanes");
        assert!(tool.description.as_ref().unwrap().contains("dataplane"));
        assert!(tool.description.as_ref().unwrap().contains("Envoy"));
    }

    #[test]
    fn test_cp_get_dataplane_tool_definition() {
        let tool = cp_get_dataplane_tool();
        assert_eq!(tool.name, "cp_get_dataplane");
        assert!(tool.description.as_ref().unwrap().contains("detailed information"));

        // Check required fields
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("team")));
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_cp_create_dataplane_tool_definition() {
        let tool = cp_create_dataplane_tool();
        assert_eq!(tool.name, "cp_create_dataplane");
        assert!(tool.description.as_ref().unwrap().contains("Create"));
        assert!(tool.description.as_ref().unwrap().contains("Envoy"));

        // Check required fields
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("team")));
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_cp_update_dataplane_tool_definition() {
        let tool = cp_update_dataplane_tool();
        assert_eq!(tool.name, "cp_update_dataplane");
        assert!(tool.description.as_ref().unwrap().contains("Update"));

        // Check required fields
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("team")));
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_cp_delete_dataplane_tool_definition() {
        let tool = cp_delete_dataplane_tool();
        assert_eq!(tool.name, "cp_delete_dataplane");
        assert!(tool.description.as_ref().unwrap().contains("Delete"));
        assert!(tool.description.as_ref().unwrap().contains("DESTRUCTIVE"));

        // Check required fields
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("team")));
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_tool_names_are_unique() {
        let tools = [
            cp_list_dataplanes_tool(),
            cp_get_dataplane_tool(),
            cp_create_dataplane_tool(),
            cp_update_dataplane_tool(),
            cp_delete_dataplane_tool(),
        ];

        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let mut unique_names = names.clone();
        unique_names.sort();
        unique_names.dedup();

        assert_eq!(names.len(), unique_names.len(), "Tool names must be unique");
    }

    #[test]
    fn test_tool_descriptions_mention_authorization() {
        let create_tool = cp_create_dataplane_tool();
        let update_tool = cp_update_dataplane_tool();
        let delete_tool = cp_delete_dataplane_tool();

        assert!(create_tool.description.as_ref().unwrap().contains("Authorization"));
        assert!(update_tool.description.as_ref().unwrap().contains("Authorization"));
        assert!(delete_tool.description.as_ref().unwrap().contains("Authorization"));
    }

    #[test]
    fn test_delete_tool_has_warning() {
        let tool = cp_delete_dataplane_tool();
        assert!(tool.description.as_ref().unwrap().contains("WARNING"));
        assert!(tool.description.as_ref().unwrap().contains("DESTRUCTIVE"));
    }
}
