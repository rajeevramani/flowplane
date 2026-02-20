//! Learning Session MCP Tools
//!
//! Control Plane tools for managing API learning sessions.

use crate::domain::OrgId;
use crate::internal_api::{
    CreateLearningSessionInternalRequest, InternalAuthContext, LearningSessionOperations,
    ListLearningSessionsRequest,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::mcp::response_builders::{build_create_response, build_delete_response};
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Tool definition for listing learning sessions
pub fn cp_list_learning_sessions_tool() -> Tool {
    Tool::new(
        "cp_list_learning_sessions",
        r#"List learning sessions for API schema discovery. Filter by status to find active, pending, or completed sessions.

PURPOSE: View all learning sessions to track API schema discovery progress.

LEARNING SESSION LIFECYCLE:
1. Pending - Session created but not yet activated
2. Active - Actively collecting traffic samples
3. Completing - Target sample count reached, generating schema
4. Completed - Schema generation finished, results available
5. Cancelled - Session manually cancelled
6. Failed - Session encountered an error

WHEN TO USE:
- Monitor active learning sessions
- Check status of schema discovery
- Find completed sessions with generated schemas
- List pending sessions waiting to be activated

FILTERING:
- status: Filter by session status (pending, active, completing, completed, cancelled, failed)
- limit: Maximum number of results (1-100)
- offset: Pagination offset

RETURNS: Array of learning session objects with:
- id: Session UUID
- team: Owning team
- route_pattern: Regex pattern matched for learning
- cluster_name: Optional cluster filter
- http_methods: Optional HTTP method filters
- status: Current session status
- target_sample_count: Target number of samples
- current_sample_count: Samples collected so far
- created_at, started_at, completed_at: Lifecycle timestamps

RELATED TOOLS: cp_get_learning_session (details), cp_create_learning_session (new session)"#,
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "description": "Filter by session status",
                    "enum": ["pending", "active", "completing", "completed", "cancelled", "failed"]
                },
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

/// Tool definition for getting a specific learning session
pub fn cp_get_learning_session_tool() -> Tool {
    Tool::new(
        "cp_get_learning_session",
        r#"Get detailed information about a specific learning session by ID.

PURPOSE: Retrieve complete session details including progress, configuration, and metadata.

RETURNS:
- id: Session UUID
- team: Owning team
- route_pattern: Regex pattern for traffic matching
- cluster_name: Optional cluster filter
- http_methods: Optional HTTP method filters (e.g., ["GET", "POST"])
- status: Current lifecycle status
- target_sample_count: Target number of samples to collect
- current_sample_count: Number of samples collected so far
- created_at: When session was created
- started_at: When session became active (if applicable)
- ends_at: Projected completion time (if applicable)
- completed_at: When session completed (if applicable)
- triggered_by: What triggered the session (manual, auto)
- deployment_version: Version tag when created
- error_message: Error details if status is 'failed'

WHEN TO USE:
- Check progress of an active session
- Verify session configuration
- Investigate failed sessions
- Get completion timestamp for schema lookup

RELATED TOOLS: cp_list_learning_sessions (discovery), cp_create_learning_session (new session)"#,
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Learning session UUID"
                }
            },
            "required": ["id"]
        }),
    )
}

/// Tool definition for creating a learning session
pub fn cp_create_learning_session_tool() -> Tool {
    Tool::new(
        "cp_create_learning_session",
        r#"Create a new learning session to discover API schemas from live traffic.

PURPOSE: Start collecting traffic samples to automatically generate OpenAPI schemas.

WORKFLOW:
1. Define route pattern (regex) to match endpoints
2. Set target sample count (how many requests to capture)
3. Optionally filter by cluster and/or HTTP methods
4. Session starts collecting traffic samples
5. When target is reached, schema is automatically generated
6. Retrieve generated schema using cp_list_schemas

REQUIRED PARAMETERS:
- route_pattern: Regex pattern to match routes (e.g., "^/api/users.*", "^/v1/orders/.*")
- target_sample_count: Number of samples to collect (1-100000)

OPTIONAL PARAMETERS:
- cluster_name: Filter to specific cluster only
- http_methods: Array of HTTP methods to include (e.g., ["GET", "POST"])
- auto_start: Whether to start immediately (default: true)

ROUTE PATTERN EXAMPLES:
- "^/api/users.*" - All user endpoints
- "^/v1/.*" - All v1 API endpoints
- "^/api/orders/[0-9]+$" - Specific order detail endpoint

SAMPLE COUNT GUIDANCE:
- Simple CRUD: 10-50 samples
- Complex APIs: 100-500 samples
- High variance: 500+ samples

AUTO_START:
- true (default): Session immediately starts collecting samples
- false: Session created in 'pending' state, activate manually later

AFTER CREATION:
- Session becomes 'active' and starts capturing traffic
- Monitor progress with cp_get_learning_session
- When complete, find generated schemas with cp_list_schemas

Authorization: Requires cp:write scope.
"#,
        json!({
            "type": "object",
            "properties": {
                "route_pattern": {
                    "type": "string",
                    "description": "Regex pattern to match routes for learning"
                },
                "target_sample_count": {
                    "type": "integer",
                    "description": "Number of traffic samples to collect",
                    "minimum": 1,
                    "maximum": 100000
                },
                "cluster_name": {
                    "type": "string",
                    "description": "Optional cluster name to filter traffic"
                },
                "http_methods": {
                    "type": "array",
                    "description": "Optional array of HTTP methods to include",
                    "items": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
                    }
                },
                "auto_start": {
                    "type": "boolean",
                    "description": "Whether to automatically start the session (default: true)"
                }
            },
            "required": ["route_pattern", "target_sample_count"]
        }),
    )
}

/// Tool definition for deleting a learning session
pub fn cp_delete_learning_session_tool() -> Tool {
    Tool::new(
        "cp_delete_learning_session",
        r#"Delete (cancel) a learning session.

PURPOSE: Cancel an active or pending learning session. Completed sessions cannot be deleted.

PREREQUISITES FOR DELETION:
- Session must be in 'pending' or 'active' state
- Cannot delete completed, completing, failed, or already cancelled sessions

BEHAVIOR:
- For pending sessions: Simply removes the session
- For active sessions: Stops sample collection, unregisters from access log service, removes session
- Collected samples are discarded (schemas not generated)

WHEN TO USE:
- Cancel a session that's no longer needed
- Stop a session collecting incorrect traffic
- Clean up test sessions

CANNOT DELETE:
- Completed sessions (use them as historical record)
- Failed sessions (keep for debugging)
- Sessions in 'completing' state (wait for completion)

Required Parameters:
- id: Session UUID to delete

WORKFLOW TO DELETE:
1. Use cp_get_learning_session to verify current status
2. If active/pending, call this tool
3. Session is cancelled and removed

Authorization: Requires cp:write scope.
"#,
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Learning session UUID to delete"
                }
            },
            "required": ["id"]
        }),
    )
}

/// Execute list learning sessions operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_learning_sessions")]
pub async fn execute_list_learning_sessions(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let status = args.get("status").and_then(|v| v.as_str()).map(String::from);
    let limit = args.get("limit").and_then(|v| v.as_i64());
    let offset = args.get("offset").and_then(|v| v.as_i64());

    // Use internal API layer
    let ops = LearningSessionOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = ListLearningSessionsRequest { status, limit, offset };

    let sessions = ops.list(req, &auth).await?;

    let result = json!({
        "sessions": sessions.iter().map(|s| {
            json!({
                "id": s.id,
                "team": s.team,
                "route_pattern": s.route_pattern,
                "cluster_name": s.cluster_name,
                "http_methods": s.http_methods,
                "status": s.status.to_string(),
                "target_sample_count": s.target_sample_count,
                "current_sample_count": s.current_sample_count,
                "created_at": s.created_at.to_rfc3339(),
                "started_at": s.started_at.map(|dt| dt.to_rfc3339()),
                "ends_at": s.ends_at.map(|dt| dt.to_rfc3339()),
                "completed_at": s.completed_at.map(|dt| dt.to_rfc3339()),
                "triggered_by": s.triggered_by,
                "deployment_version": s.deployment_version,
                "error_message": s.error_message
            })
        }).collect::<Vec<_>>(),
        "count": sessions.len()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

/// Execute get learning session operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_learning_session")]
pub async fn execute_get_learning_session(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: id".to_string()))?;

    // Use internal API layer
    let ops = LearningSessionOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let session = ops.get(id, &auth).await?;

    let result = json!({
        "id": session.id,
        "team": session.team,
        "route_pattern": session.route_pattern,
        "cluster_name": session.cluster_name,
        "http_methods": session.http_methods,
        "status": session.status.to_string(),
        "target_sample_count": session.target_sample_count,
        "current_sample_count": session.current_sample_count,
        "created_at": session.created_at.to_rfc3339(),
        "started_at": session.started_at.map(|dt| dt.to_rfc3339()),
        "ends_at": session.ends_at.map(|dt| dt.to_rfc3339()),
        "completed_at": session.completed_at.map(|dt| dt.to_rfc3339()),
        "triggered_by": session.triggered_by,
        "deployment_version": session.deployment_version,
        "configuration_snapshot": session.configuration_snapshot,
        "error_message": session.error_message,
        "updated_at": session.updated_at.to_rfc3339()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

/// Execute create learning session operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_create_learning_session")]
pub async fn execute_create_learning_session(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let route_pattern = args.get("route_pattern").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: route_pattern".to_string())
    })?;

    let target_sample_count =
        args.get("target_sample_count").and_then(|v| v.as_i64()).ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: target_sample_count".to_string())
        })?;

    // 2. Parse optional fields
    let cluster_name = args.get("cluster_name").and_then(|v| v.as_str()).map(String::from);
    let http_methods = args.get("http_methods").and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter().filter_map(|item| item.as_str().map(String::from)).collect::<Vec<_>>()
        })
    });
    let auto_start = args.get("auto_start").and_then(|v| v.as_bool());

    tracing::debug!(
        team = %team,
        route_pattern = %route_pattern,
        target_sample_count = %target_sample_count,
        "Creating learning session via MCP"
    );

    // 3. Validate target_sample_count range
    if !(1..=100000).contains(&target_sample_count) {
        return Err(McpError::InvalidParams(
            "target_sample_count must be between 1 and 100000".to_string(),
        ));
    }

    // 4. Use internal API layer
    let ops = LearningSessionOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = CreateLearningSessionInternalRequest {
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        route_pattern: route_pattern.to_string(),
        cluster_name,
        http_methods,
        target_sample_count,
        auto_start,
    };

    let result = ops.create(req, &auth).await?;

    // 5. Format success response (minimal token-efficient format)
    let output =
        build_create_response("learning_session", &result.data.route_pattern, &result.data.id);

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        session_id = %result.data.id,
        status = %result.data.status,
        "Successfully created learning session via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute delete learning session operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_delete_learning_session")]
pub async fn execute_delete_learning_session(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse session ID
    let id = args["id"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: id".to_string()))?;

    tracing::debug!(team = %team, session_id = %id, "Deleting learning session via MCP");

    // 2. Use internal API layer
    let ops = LearningSessionOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    ops.delete(id, &auth).await?;

    // 3. Format success response (minimal token-efficient format)
    let output = build_delete_response();

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, session_id = %id, "Successfully deleted learning session via MCP");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_learning_sessions_tool_definition() {
        let tool = cp_list_learning_sessions_tool();
        assert_eq!(tool.name, "cp_list_learning_sessions");
        assert!(tool.description.as_ref().unwrap().contains("learning session"));
        assert!(tool.description.as_ref().unwrap().contains("schema discovery"));
    }

    #[test]
    fn test_cp_get_learning_session_tool_definition() {
        let tool = cp_get_learning_session_tool();
        assert_eq!(tool.name, "cp_get_learning_session");
        assert!(tool.description.as_ref().unwrap().contains("detailed information"));

        // Check required field
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("id")));
    }

    #[test]
    fn test_cp_create_learning_session_tool_definition() {
        let tool = cp_create_learning_session_tool();
        assert_eq!(tool.name, "cp_create_learning_session");
        assert!(tool.description.as_ref().unwrap().contains("Create"));
        assert!(tool.description.as_ref().unwrap().contains("traffic"));

        // Check required fields
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("route_pattern")));
        assert!(required.contains(&json!("target_sample_count")));
    }

    #[test]
    fn test_cp_delete_learning_session_tool_definition() {
        let tool = cp_delete_learning_session_tool();
        assert_eq!(tool.name, "cp_delete_learning_session");
        assert!(tool.description.as_ref().unwrap().contains("Delete"));
        assert!(tool.description.as_ref().unwrap().contains("cancel"));

        // Check required field
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("id")));
    }

    #[test]
    fn test_tool_names_are_unique() {
        let tools = [
            cp_list_learning_sessions_tool(),
            cp_get_learning_session_tool(),
            cp_create_learning_session_tool(),
            cp_delete_learning_session_tool(),
        ];

        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let mut unique_names = names.clone();
        unique_names.sort();
        unique_names.dedup();

        assert_eq!(names.len(), unique_names.len(), "Tool names must be unique");
    }
}
