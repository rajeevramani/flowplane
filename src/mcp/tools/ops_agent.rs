//! Ops Agent MCP Tools
//!
//! Read-only diagnostic tools for tracing requests, viewing topology, and
//! validating configuration. These tools query the database directly
//! (no xds_state needed) via `ReportingRepository`.

use crate::domain::OrgId;
use crate::mcp::error::McpError;
use crate::mcp::protocol::{Tool, ToolCallResult};
use crate::services::ops_service::{self, OpsServiceError};
use crate::storage::DbPool;
use serde_json::{json, Value};
use tracing::instrument;

// =============================================================================
// TOOL DEFINITIONS
// =============================================================================

/// Ops tool: trace a request path through the full gateway chain.
pub fn ops_trace_request_tool() -> Tool {
    Tool::new(
        "ops_trace_request",
        r#"Trace how a request flows through the gateway: listener → route_config → virtual_host → route → cluster → endpoints.

PURPOSE: Diagnose routing issues by showing every resource a request touches.

USE CASES:
- Verify a path reaches the correct backend
- Debug 404s or unexpected routing
- Understand which listeners handle a request

PARAMETERS:
- path (required): Request path to trace (e.g., "/api/users")
- port (optional): Listener port filter (e.g., 8080)

EXAMPLE:
{ "path": "/api/orders", "port": 8080 }

RETURNS:
- matches: Array of trace rows (listener → route_config → virtual_host → route → cluster)
- endpoints: Array of backend endpoints for the matched clusters
- unmatched_reason: When no matches found, explains why (e.g., "no route matches path")

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Request path to trace (must start with '/', max 2048 chars)",
                    "maxLength": 2048
                },
                "port": {
                    "type": "integer",
                    "description": "Optional listener port filter"
                }
            },
            "required": ["path"]
        }),
    )
}

/// Ops tool: view the full gateway topology.
pub fn ops_topology_tool() -> Tool {
    Tool::new(
        "ops_topology",
        r#"View the full gateway topology: listeners, route_configs, virtual_hosts, routes, and orphan detection.

PURPOSE: Understand the complete gateway layout and find disconnected resources.

USE CASES:
- Get a bird's-eye view of the gateway
- Find orphan clusters (not referenced by any route_config)
- Find orphan route_configs (not bound to any listener)
- Scope to a specific listener, cluster, or route_config

PARAMETERS:
- scope (optional): Filter scope — "listener", "cluster", or "route_config". Omit for full topology.
- name (optional): Resource name filter (used with scope)
- limit (optional): Max rows per level (default: 50)
- include_details (optional): Include full topology rows (default: false). When false, returns summary only (<120 tokens).

EXAMPLE:
{ "scope": "listener", "name": "http-8080", "includeDetails": true }

RETURNS:
- summary: Counts for listeners, route_configs, clusters, routes, orphans (always included)
- orphan_clusters: Clusters with no route_configs referencing them (always included)
- orphan_route_configs: Route configs with no listener bound (always included)
- rows: Flattened topology rows (only when include_details=true)
- truncated: Whether the result was limited

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "scope": {
                    "type": "string",
                    "description": "Filter scope: 'listener', 'cluster', or 'route_config'. Omit for full topology.",
                    "enum": ["listener", "cluster", "route_config"]
                },
                "name": {
                    "type": "string",
                    "description": "Resource name filter (used with scope)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max rows per level (default: 50)",
                    "default": 50
                },
                "includeDetails": {
                    "type": "boolean",
                    "description": "Include full topology rows (default: false — summary only for token efficiency)",
                    "default": false
                }
            }
        }),
    )
}

/// Ops tool: validate gateway configuration and detect problems.
pub fn ops_config_validate_tool() -> Tool {
    Tool::new(
        "ops_config_validate",
        r#"Validate gateway configuration and detect Envoy proto constraint violations.

PURPOSE: Find misconfigurations that Envoy would reject (NACK), orphaned resources, and connectivity issues — before or after deployment.

USE CASES:
- Pre-deployment validation — catch proto violations before Envoy NACKs
- Post-deployment check — correlate issues with recent xDS NACKs
- Find orphan clusters (backends with no traffic)
- Find unbound route_configs (routing rules with no listener)
- Detect empty endpoint pools or invalid endpoints

CHECKS PERFORMED:
1. Connectivity: orphan clusters, orphan route_configs, empty listeners
2. Proto violations: empty endpoints, invalid endpoint format, health check thresholds, health check timeout/interval, empty HTTP health check path
3. xDS delivery: recent NACK events from Envoy dataplanes

PARAMETERS: None required. All checks run automatically for the current team.

RETURNS:
- valid: Boolean — true if no issues found
- issues: Array of detected problems, each with:
  - severity: "warning" or "error"
  - category: "orphan_cluster", "orphan_route_config", "empty_listener", "proto_violation", "xds_delivery"
  - message: Human-readable description
  - resource: Affected resource name
- summary: Counts for total issues, warnings, errors, and resource counts
- next_step: Suggested action based on findings

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {}
        }),
    )
}

/// Ops tool: query recent audit log entries (PII-stripped summaries).
pub fn ops_audit_query_tool() -> Tool {
    Tool::new(
        "ops_audit_query",
        r#"Query recent audit log entries for the current team.

PURPOSE: Review recent configuration changes and operations for troubleshooting or compliance.

SECURITY CONSTRAINTS:
- Team and org context are forced from the authenticated session — cannot be overridden
- Returns PII-stripped summaries only (no client_ip, user_agent, old/new configuration)
- Requires audit:read scope — cp:read does NOT grant access

PARAMETERS:
- resource_type (optional): Filter by resource type (e.g., "clusters", "listeners", "routes")
- action (optional): Filter by action (e.g., "create", "update", "delete")
- limit (optional): Max results (default: 20, max: 100)

EXAMPLE:
{ "resource_type": "clusters", "action": "delete", "limit": 10 }

RETURNS:
- entries: Array of audit summaries (id, resource_type, resource_id, resource_name, action, created_at)
- count: Number of entries returned
- message: Summary description

Authorization: Requires audit:read scope (NOT covered by cp:read)."#,
        json!({
            "type": "object",
            "properties": {
                "resourceType": {
                    "type": "string",
                    "description": "Filter by resource type (e.g., 'clusters', 'listeners', 'routes')"
                },
                "action": {
                    "type": "string",
                    "description": "Filter by action (e.g., 'create', 'update', 'delete')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default: 20, max: 100)",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20
                }
            }
        }),
    )
}

// =============================================================================
// EXECUTE FUNCTIONS
// =============================================================================

// validate_team_in_org is shared — use super::validate_team_in_org
use super::validate_team_in_org;

/// Convert OpsServiceError to McpError.
fn ops_err_to_mcp(e: OpsServiceError) -> McpError {
    match e {
        OpsServiceError::InvalidParam(msg) => McpError::InvalidParams(msg),
        OpsServiceError::Internal(msg) => McpError::InternalError(msg),
    }
}

/// Execute ops_trace_request: trace a request path through the gateway.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_trace_request")]
pub async fn execute_ops_trace_request(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("'path' is required".to_string()))?;
    let port = args.get("port").and_then(|v| v.as_i64());

    let result =
        ops_service::trace_request(db_pool, team, path, port).await.map_err(ops_err_to_mcp)?;

    let output = json!({
        "success": true,
        "path": result.path,
        "port": result.port,
        "match_count": result.match_count,
        "matches": result.matches,
        "endpoints": result.endpoints,
        "unmatched_reason": result.unmatched_reason,
        "message": result.message
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult::text(text))
}

/// Execute ops_topology: view the full gateway topology.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_topology")]
pub async fn execute_ops_topology(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let scope = args.get("scope").and_then(|v| v.as_str());
    let name = args.get("name").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_i64());
    let include_details = args.get("includeDetails").and_then(|v| v.as_bool()).unwrap_or(false);

    let result = ops_service::topology(db_pool, team, scope, name, limit, include_details)
        .await
        .map_err(ops_err_to_mcp)?;

    let mut output = json!({
        "success": true,
        "scope": result.scope,
        "name": result.name,
        "orphan_clusters": result.orphan_clusters,
        "orphan_route_configs": result.orphan_route_configs,
        "summary": result.summary,
        "truncated": result.truncated,
        "message": result.message
    });

    if let Some(rows) = &result.rows {
        output["rows"] = json!(rows);
    }

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult::text(text))
}

/// Execute ops_config_validate: validate gateway configuration.
#[instrument(skip(db_pool, _args), fields(team = %team), name = "mcp_execute_ops_config_validate")]
pub async fn execute_ops_config_validate(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    _args: Value,
) -> Result<ToolCallResult, McpError> {
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let result = ops_service::config_validate(db_pool, team).await.map_err(ops_err_to_mcp)?;

    let valid = result.valid;
    let output = json!({
        "success": true,
        "valid": valid,
        "issues": result.issues,
        "summary": result.summary,
        "message": if valid {
            "Configuration is valid — no issues detected".to_string()
        } else {
            format!("Found {} issue(s): {} error(s), {} warning(s) ({} proto violations, {} recent NACKs)",
                result.summary.total_issues, result.summary.errors, result.summary.warnings,
                result.summary.proto_violations, result.summary.recent_nacks)
        },
        "next_step": result.next_step
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult::text(text))
}

/// Execute ops_audit_query: query recent audit log entries (PII-stripped).
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_audit_query")]
pub async fn execute_ops_audit_query(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let resource_type = args.get("resourceType").and_then(|v| v.as_str());
    let action = args.get("action").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_i64());

    let result = ops_service::audit_query(
        db_pool,
        team,
        org_id.map(|o| o.as_str()),
        resource_type,
        action,
        limit,
    )
    .await
    .map_err(ops_err_to_mcp)?;

    let output = json!({
        "success": true,
        "entries": result.entries,
        "count": result.count,
        "message": result.message
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult::text(text))
}

/// Ops tool: view xDS delivery status per dataplane.
pub fn ops_xds_delivery_status_tool() -> Tool {
    Tool::new(
        "ops_xds_delivery_status",
        r#"Show xDS config delivery status for each dataplane: which resource types were accepted (ACK) or rejected (NACK) by Envoy.

PURPOSE: Quickly assess whether Envoy dataplanes are running the latest configuration or rejecting it.

USE CASES:
- After deploying config changes, verify Envoy accepted them
- Diagnose why a dataplane isn't picking up new routes/clusters
- Find dataplanes with persistent NACK errors

PARAMETERS:
- dataplane_name (optional): Filter to a specific dataplane. Omit to see all dataplanes.

EXAMPLE:
{ "dataplane_name": "my-dataplane" }

RETURNS:
- dataplanes: Array of dataplane status objects, each containing:
  - name: Dataplane name
  - resource_types: Object with CDS/RDS/LDS keys, each showing status (ACK or NACK), error details if NACK
- summary: Total dataplanes, how many have NACKs
- next_step: Suggested action based on findings

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "dataplaneName": {
                    "type": "string",
                    "description": "Filter to a specific dataplane name. Omit for all dataplanes."
                }
            }
        }),
    )
}

/// Ops tool: query recent NACK event history.
pub fn ops_nack_history_tool() -> Tool {
    Tool::new(
        "ops_nack_history",
        r#"Query recent xDS NACK events — configuration rejections from Envoy dataplanes.

PURPOSE: Investigate why Envoy rejected configuration changes by viewing the full error history.

USE CASES:
- After ops_xds_delivery_status shows NACKs, drill into the error details
- Filter by dataplane to isolate issues to a specific instance
- Filter by resource type (CDS/RDS/LDS/EDS) to narrow down the problem
- View recent NACKs after deploying config changes

PARAMETERS:
- limit (optional): Max events to return (default: 10, max: 100)
- dataplane_name (optional): Filter to a specific dataplane
- type_url (optional): Filter by resource type. Accepts short forms: "CDS", "RDS", "LDS", "EDS" or full type URLs.
- since (optional): ISO 8601 timestamp — only show events after this time (e.g., "2026-02-25T00:00:00Z")

EXAMPLES:
{ "dataplane_name": "my-dp", "limit": 5 }
{ "type_url": "CDS", "since": "2026-02-25T00:00:00Z" }

RETURNS:
- events: Array of NACK events with timestamp, dataplane_name, resource_type, error_message, error_code, resource_names, version_rejected
- count: Number of events returned
- message: Summary description

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Max events to return (default: 10, max: 100)",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 10
                },
                "dataplaneName": {
                    "type": "string",
                    "description": "Filter to a specific dataplane name"
                },
                "typeUrl": {
                    "type": "string",
                    "description": "Filter by resource type: 'CDS', 'RDS', 'LDS', 'EDS', or full type URL"
                },
                "since": {
                    "type": "string",
                    "description": "ISO 8601 timestamp — only show events after this time"
                }
            }
        }),
    )
}

// =============================================================================
// EXECUTE: ops_xds_delivery_status
// =============================================================================

/// Execute ops_xds_delivery_status: show per-dataplane xDS delivery health.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_xds_delivery_status")]
pub async fn execute_ops_xds_delivery_status(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let dataplane_filter = args.get("dataplaneName").and_then(|v| v.as_str());

    let result = ops_service::xds_delivery_status(db_pool, team, dataplane_filter)
        .await
        .map_err(ops_err_to_mcp)?;

    let output = json!({
        "success": true,
        "dataplanes": result.dataplanes,
        "summary": result.summary,
        "message": result.message,
        "next_step": result.next_step
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult::text(text))
}

// =============================================================================
// EXECUTE: ops_nack_history
// =============================================================================

/// Execute ops_nack_history: query recent NACK events.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_nack_history")]
pub async fn execute_ops_nack_history(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let limit = args.get("limit").and_then(|v| v.as_i64());
    let dataplane_name = args.get("dataplaneName").and_then(|v| v.as_str());
    let type_url_filter = args.get("typeUrl").and_then(|v| v.as_str());
    let since_str = args.get("since").and_then(|v| v.as_str());

    let result =
        ops_service::nack_history(db_pool, team, dataplane_name, type_url_filter, since_str, limit)
            .await
            .map_err(ops_err_to_mcp)?;

    let output = json!({
        "success": true,
        "events": result.events,
        "count": result.count,
        "message": result.message
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult::text(text))
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::protocol::ContentBlock;
    use crate::storage::test_helpers::{seed_reporting_data, TestDatabase, TEAM_A_ID, TEAM_B_ID};

    // Helper: create a TestDatabase with reporting seed data
    async fn test_db(name: &str) -> TestDatabase {
        let db = TestDatabase::new(name).await;
        seed_reporting_data(&db.pool).await;
        db
    }

    // ========================================================================
    // Tool definition tests
    // ========================================================================

    #[test]
    fn test_ops_trace_request_tool_definition() {
        let tool = ops_trace_request_tool();
        assert_eq!(tool.name, "ops_trace_request");
        assert!(tool.description.as_ref().unwrap().contains("Trace"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["port"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "path");
    }

    #[test]
    fn test_ops_topology_tool_definition() {
        let tool = ops_topology_tool();
        assert_eq!(tool.name, "ops_topology");
        assert!(tool.description.as_ref().unwrap().contains("topology"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["scope"].is_object());
        assert!(schema["properties"]["name"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        assert!(schema["properties"]["includeDetails"].is_object());

        // No required params
        assert!(
            schema["required"].is_null()
                || schema["required"].as_array().map(|a| a.is_empty()).unwrap_or(true)
        );
    }

    #[test]
    fn test_ops_config_validate_tool_definition() {
        let tool = ops_config_validate_tool();
        assert_eq!(tool.name, "ops_config_validate");
        assert!(tool.description.as_ref().unwrap().contains("Validate"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn test_all_ops_tools_have_valid_schemas() {
        let tools = vec![
            ops_trace_request_tool(),
            ops_topology_tool(),
            ops_config_validate_tool(),
            ops_xds_delivery_status_tool(),
        ];

        for tool in tools {
            assert!(!tool.name.is_empty(), "Tool name should not be empty");
            assert!(
                tool.description.as_ref().is_some_and(|d| !d.is_empty()),
                "Tool '{}' description should not be empty",
                tool.name
            );
            assert!(
                tool.input_schema.is_object(),
                "Tool '{}' schema should be an object",
                tool.name
            );
            assert_eq!(
                tool.input_schema["type"], "object",
                "Tool '{}' schema type should be 'object'",
                tool.name
            );
        }
    }

    // ========================================================================
    // Execute: ops_trace_request
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_trace_request_happy_path() {
        let db = test_db("ops_trace_happy").await;

        let result = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/api/orders", "port": 8080}),
        )
        .await
        .expect("should succeed");

        assert!(result.is_error.is_none());
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(output["match_count"].as_i64().unwrap() > 0);
        assert!(!output["matches"].as_array().unwrap().is_empty());
        assert!(!output["endpoints"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_no_match() {
        let db = test_db("ops_trace_no_match").await;

        let result = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/nonexistent/path"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert_eq!(output["match_count"], 0);
        assert!(output["matches"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_missing_path() {
        let db = test_db("ops_trace_missing_path").await;

        let result = execute_ops_trace_request(&db.pool, TEAM_A_ID, None, json!({})).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("path")),
            other => panic!("expected InvalidParams, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_path_validation() {
        let db = test_db("ops_trace_validate").await;

        // Must start with '/'
        let result =
            execute_ops_trace_request(&db.pool, TEAM_A_ID, None, json!({"path": "no-slash"})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("start with '/'")),
            other => panic!("expected InvalidParams for no-slash, got: {:?}", other),
        }

        // Max 2048 chars
        let long_path = format!("/{}", "a".repeat(2048));
        let result =
            execute_ops_trace_request(&db.pool, TEAM_A_ID, None, json!({"path": long_path})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("2048")),
            other => panic!("expected InvalidParams for long path, got: {:?}", other),
        }

        // Reject null bytes
        let result =
            execute_ops_trace_request(&db.pool, TEAM_A_ID, None, json!({"path": "/api/\0bad"}))
                .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("null")),
            other => panic!("expected InvalidParams for null byte, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_unmatched_reason() {
        let db = test_db("ops_trace_unmatched").await;

        let result = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/nonexistent/path"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["match_count"], 0);
        assert!(
            output["unmatched_reason"].is_string(),
            "should have unmatched_reason when no match"
        );
        let reason = output["unmatched_reason"].as_str().unwrap();
        assert!(reason.contains("No route matches"), "reason should explain the miss");
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_team_isolation() {
        let db = test_db("ops_trace_team_iso").await;

        // team-a should not see team-b's payments route
        let result = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/api/payments", "port": 9090}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["match_count"], 0, "team-a must not see team-b routes");

        // team-b should see its own payments route
        let result_b = execute_ops_trace_request(
            &db.pool,
            TEAM_B_ID,
            None,
            json!({"path": "/api/payments", "port": 9090}),
        )
        .await
        .expect("should succeed");

        let text_b = match &result_b.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output_b: Value = serde_json::from_str(text_b).expect("should be valid JSON");
        assert!(output_b["match_count"].as_i64().unwrap() > 0, "team-b should see its own routes");
    }

    // ========================================================================
    // Execute: ops_topology
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_topology_summary_only() {
        let db = test_db("ops_topo_summary").await;

        // Default: include_details=false, returns summary only (no rows)
        let result = execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(output["summary"]["listener_count"].as_i64().unwrap() > 0);
        assert!(output.get("rows").is_none(), "summary mode should not include rows");
        // Orphans are always included
        assert!(output.get("orphan_clusters").is_some());
    }

    #[tokio::test]
    async fn test_execute_ops_topology_with_details() {
        let db = test_db("ops_topo_details").await;

        let result =
            execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({"includeDetails": true}))
                .await
                .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(!output["rows"].as_array().unwrap().is_empty(), "detail mode should include rows");
        assert!(output["summary"]["listener_count"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_execute_ops_topology_scoped() {
        let db = test_db("ops_topo_scoped").await;

        let result = execute_ops_topology(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"scope": "listener", "name": "http-8080", "includeDetails": true}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["scope"], "listener");

        // All rows should be for the specified listener
        for row in output["rows"].as_array().unwrap() {
            assert_eq!(row["listener_name"], "http-8080");
        }
    }

    #[tokio::test]
    async fn test_execute_ops_topology_with_limit() {
        let db = test_db("ops_topo_limit").await;

        let result = execute_ops_topology(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"limit": 1, "includeDetails": true}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert!(output["rows"].as_array().unwrap().len() <= 1);
        assert_eq!(output["truncated"], true);
    }

    #[tokio::test]
    async fn test_execute_ops_topology_team_isolation() {
        let db = test_db("ops_topo_team_iso").await;

        let result =
            execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({"includeDetails": true}))
                .await
                .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");

        // team-a should not see team-b's listener
        let listener_names: Vec<&str> = output["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["listener_name"].as_str())
            .collect();
        assert!(!listener_names.contains(&"http-9090"), "team-a must not see team-b listener");
    }

    // ========================================================================
    // Execute: ops_config_validate
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_config_validate_detects_orphans() {
        let db = test_db("ops_validate_orphans").await;

        let result = execute_ops_config_validate(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert_eq!(output["valid"], false, "should detect issues");

        let issues = output["issues"].as_array().unwrap();

        // Should detect orphan cluster
        let orphan_cluster_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "orphan_cluster").collect();
        assert!(!orphan_cluster_issues.is_empty(), "should detect orphan cluster");
        assert!(orphan_cluster_issues.iter().any(|i| i["resource"] == "orphan-cluster"));

        // Should detect orphan route config
        let orphan_rc_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "orphan_route_config").collect();
        assert!(!orphan_rc_issues.is_empty(), "should detect orphan route config");
        assert!(orphan_rc_issues.iter().any(|i| i["resource"] == "orphan-rc"));
    }

    #[tokio::test]
    async fn test_execute_ops_config_validate_empty_team() {
        let db = TestDatabase::new("ops_validate_empty").await;

        let result = execute_ops_config_validate(&db.pool, "nonexistent-team", None, json!({}))
            .await
            .expect("should succeed on empty data");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        // No resources means no issues (or only global resource issues)
    }

    #[tokio::test]
    async fn test_execute_ops_config_validate_team_isolation() {
        let db = test_db("ops_validate_team_iso").await;

        // team-b should NOT see team-a's orphan cluster
        let result = execute_ops_config_validate(&db.pool, TEAM_B_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");

        let issues = output["issues"].as_array().unwrap();
        let orphan_names: Vec<&str> = issues
            .iter()
            .filter(|i| i["category"] == "orphan_cluster")
            .filter_map(|i| i["resource"].as_str())
            .collect();
        assert!(
            !orphan_names.contains(&"orphan-cluster"),
            "team-b must not see team-a's orphan-cluster"
        );
    }

    // ========================================================================
    // Tool definition: ops_audit_query
    // ========================================================================

    #[test]
    fn test_ops_audit_query_tool_definition() {
        let tool = ops_audit_query_tool();
        assert_eq!(tool.name, "ops_audit_query");
        assert!(tool.description.as_ref().unwrap().contains("audit"));
        assert!(tool.description.as_ref().unwrap().contains("audit:read"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["resourceType"].is_object());
        assert!(schema["properties"]["action"].is_object());
        assert!(schema["properties"]["limit"].is_object());

        // No required params
        assert!(
            schema["required"].is_null()
                || schema["required"].as_array().map(|a| a.is_empty()).unwrap_or(true)
        );
    }

    // ========================================================================
    // Execute: ops_audit_query
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_audit_query_empty() {
        let db = TestDatabase::new("ops_audit_empty").await;

        let result = execute_ops_audit_query(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed on empty audit logs");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert_eq!(output["count"], 0);
        assert!(output["entries"].as_array().unwrap().is_empty());
    }

    /// Helper: insert a basic audit log entry for testing.
    async fn insert_audit_entry(
        pool: &crate::storage::DbPool,
        resource_type: &str,
        resource_name: &str,
        action: &str,
        team_id: &str,
    ) {
        sqlx::query(
            "INSERT INTO audit_log (resource_type, resource_id, resource_name, action, \
             user_id, client_ip, user_agent, org_id, team_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())",
        )
        .bind(resource_type)
        .bind(resource_name) // use name as id for simplicity
        .bind(resource_name)
        .bind(action)
        .bind(None::<&str>) // user_id
        .bind(None::<&str>) // client_ip
        .bind(None::<&str>) // user_agent
        .bind(None::<&str>) // org_id
        .bind(team_id)
        .execute(pool)
        .await
        .expect("insert audit entry");
    }

    /// Helper: insert an audit log entry with PII fields for testing PII stripping.
    async fn insert_audit_entry_with_pii(
        pool: &crate::storage::DbPool,
        resource_type: &str,
        resource_name: &str,
        action: &str,
        team_id: &str,
    ) {
        sqlx::query(
            "INSERT INTO audit_log (resource_type, resource_id, resource_name, action, \
             user_id, client_ip, user_agent, org_id, team_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())",
        )
        .bind(resource_type)
        .bind(resource_name)
        .bind(resource_name)
        .bind(action)
        .bind("user-secret") // PII: user_id
        .bind("10.0.0.1") // PII: client_ip
        .bind("curl/7.0") // PII: user_agent
        .bind(None::<&str>) // org_id
        .bind(team_id)
        .execute(pool)
        .await
        .expect("insert audit entry with pii");
    }

    #[tokio::test]
    async fn test_execute_ops_audit_query_with_data() {
        let db = TestDatabase::new("ops_audit_data").await;

        // Insert audit log entry with PII fields
        insert_audit_entry_with_pii(&db.pool, "clusters", "my-cluster", "create", TEAM_A_ID).await;

        let result = execute_ops_audit_query(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(output["count"].as_i64().unwrap() >= 1);

        // Verify PII is stripped — no client_ip, user_agent, user_id, old/new config
        let entry = &output["entries"][0];
        assert!(entry.get("client_ip").is_none(), "must NOT contain client_ip");
        assert!(entry.get("user_agent").is_none(), "must NOT contain user_agent");
        assert!(entry.get("user_id").is_none(), "must NOT contain user_id");
        assert!(entry.get("old_configuration").is_none(), "must NOT contain old_configuration");
        assert!(entry.get("new_configuration").is_none(), "must NOT contain new_configuration");

        // Should contain summary fields
        assert_eq!(entry["resource_type"], "clusters");
        assert_eq!(entry["action"], "create");
        assert_eq!(entry["resource_name"], "my-cluster");
    }

    #[tokio::test]
    async fn test_execute_ops_audit_query_team_isolation() {
        let db = TestDatabase::new("ops_audit_iso").await;

        // team-a event
        insert_audit_entry(&db.pool, "clusters", "team-a-cluster", "create", TEAM_A_ID).await;

        // team-b event
        insert_audit_entry(&db.pool, "clusters", "team-b-cluster", "delete", TEAM_B_ID).await;

        // team-a should only see its own entry
        let result = execute_ops_audit_query(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let entries = output["entries"].as_array().unwrap();
        for entry in entries {
            assert_ne!(
                entry["resource_name"], "team-b-cluster",
                "team-a must NOT see team-b's audit entries"
            );
        }
    }

    #[tokio::test]
    async fn test_execute_ops_audit_query_with_filters() {
        let db = TestDatabase::new("ops_audit_filter").await;

        // Insert multiple events
        for action in &["create", "update", "delete"] {
            insert_audit_entry(&db.pool, "listeners", "test-listener", action, TEAM_A_ID).await;
        }

        // Filter by action
        let result = execute_ops_audit_query(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"action": "delete", "resourceType": "listeners"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let entries = output["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["action"], "delete");
    }

    // ========================================================================
    // Tool definition: ops_nack_history
    // ========================================================================

    #[test]
    fn test_ops_nack_history_tool_definition() {
        let tool = ops_nack_history_tool();
        assert_eq!(tool.name, "ops_nack_history");
        assert!(tool.description.as_ref().unwrap().contains("NACK"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["limit"].is_object());
        assert!(schema["properties"]["dataplaneName"].is_object());
        assert!(schema["properties"]["typeUrl"].is_object());
        assert!(schema["properties"]["since"].is_object());

        // No required params
        assert!(
            schema["required"].is_null()
                || schema["required"].as_array().map(|a| a.is_empty()).unwrap_or(true)
        );
    }

    // ========================================================================
    // Execute: ops_xds_delivery_status
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_xds_delivery_status_empty() {
        let db = test_db("ops_delivery_empty").await;

        let result = execute_ops_xds_delivery_status(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");
        assert!(output["success"].as_bool().unwrap_or(false));
        assert!(output["dataplanes"].as_array().unwrap_or(&vec![]).is_empty());
    }

    #[tokio::test]
    async fn test_execute_ops_xds_delivery_status_with_nacks() {
        let db = test_db("ops_delivery_nacks").await;

        // Create a dataplane for team-a
        sqlx::query(
            "INSERT INTO dataplanes (id, name, team, created_at, updated_at) \
             VALUES ($1, $2, $3, NOW(), NOW())",
        )
        .bind("dp-status-id")
        .bind("dp-status")
        .bind(TEAM_A_ID)
        .execute(&db.pool)
        .await
        .expect("insert dataplane");

        // Insert a NACK event for that dataplane
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-status",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "threshold missing",
        )
        .await;

        let result = execute_ops_xds_delivery_status(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");
        assert!(output["success"].as_bool().unwrap_or(false));

        let dataplanes = output["dataplanes"].as_array().expect("dataplanes array");
        assert!(!dataplanes.is_empty(), "should have at least one dataplane");

        // Find dp-status in the results
        let dp =
            dataplanes.iter().find(|d| d["name"] == "dp-status").expect("should find dp-status");
        let resource_types = &dp["resource_types"];
        assert!(resource_types["CDS"].is_object(), "should have CDS entry");
        assert_eq!(resource_types["CDS"]["status"], "NACK");
    }

    #[tokio::test]
    async fn test_execute_ops_xds_delivery_status_team_isolation() {
        let db = test_db("ops_delivery_iso").await;

        // Create dataplanes for both teams
        sqlx::query(
            "INSERT INTO dataplanes (id, name, team, created_at, updated_at) \
             VALUES ($1, $2, $3, NOW(), NOW())",
        )
        .bind("dp-iso-a-id")
        .bind("dp-iso-a")
        .bind(TEAM_A_ID)
        .execute(&db.pool)
        .await
        .expect("insert dp-a");

        sqlx::query(
            "INSERT INTO dataplanes (id, name, team, created_at, updated_at) \
             VALUES ($1, $2, $3, NOW(), NOW())",
        )
        .bind("dp-iso-b-id")
        .bind("dp-iso-b")
        .bind(TEAM_B_ID)
        .execute(&db.pool)
        .await
        .expect("insert dp-b");

        // Insert NACKs for team-b
        insert_nack_event(
            &db.pool,
            TEAM_B_ID,
            "dp-iso-b",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "secret error",
        )
        .await;

        // Team-a should only see its own dataplane, not team-b's NACKs
        let result = execute_ops_xds_delivery_status(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");

        let dataplanes = output["dataplanes"].as_array().expect("dataplanes array");
        // Team-a should see dp-iso-a but NOT dp-iso-b
        assert!(
            dataplanes.iter().all(|d| d["name"] != "dp-iso-b"),
            "should not see team-b dataplanes"
        );
    }

    #[tokio::test]
    async fn test_execute_ops_xds_delivery_status_filter_by_dataplane() {
        let db = test_db("ops_delivery_filter").await;

        // Create two dataplanes for team-a
        for (id, name) in [("dp-f1-id", "dp-filter-1"), ("dp-f2-id", "dp-filter-2")] {
            sqlx::query(
                "INSERT INTO dataplanes (id, name, team, created_at, updated_at) \
                 VALUES ($1, $2, $3, NOW(), NOW())",
            )
            .bind(id)
            .bind(name)
            .bind(TEAM_A_ID)
            .execute(&db.pool)
            .await
            .expect("insert dataplane");
        }

        let result = execute_ops_xds_delivery_status(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"dataplaneName": "dp-filter-1"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");

        let dataplanes = output["dataplanes"].as_array().expect("dataplanes array");
        assert_eq!(dataplanes.len(), 1, "should only show filtered dataplane");
        assert_eq!(dataplanes[0]["name"], "dp-filter-1");
    }

    // ========================================================================
    // Execute: ops_nack_history
    // ========================================================================

    /// Helper: insert a NACK event for testing.
    async fn insert_nack_event(
        pool: &crate::storage::DbPool,
        team: &str,
        dataplane_name: &str,
        type_url: &str,
        error_message: &str,
    ) {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO xds_nack_events (id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())",
        )
        .bind(&id)
        .bind(team)
        .bind(dataplane_name)
        .bind(type_url)
        .bind("v1")
        .bind("nonce-1")
        .bind(13_i64) // INTERNAL error code
        .bind(error_message)
        .bind(None::<&str>)
        .bind(Some(r#"["my-cluster"]"#))
        .execute(pool)
        .await
        .expect("insert nack event");
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_empty() {
        let db = TestDatabase::new("ops_nack_hist_empty").await;

        let result = execute_ops_nack_history(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed on empty");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert_eq!(output["count"], 0);
        assert!(output["events"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_returns_events() {
        let db = test_db("ops_nack_hist_data").await;

        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "missing threshold",
        )
        .await;

        let result = execute_ops_nack_history(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(output["count"].as_i64().unwrap() >= 1);

        let event = &output["events"][0];
        assert_eq!(event["dataplane_name"], "dp-1");
        assert_eq!(event["resource_type"], "CDS");
        assert_eq!(event["error_message"], "missing threshold");
        assert!(event["resource_names"].is_array());
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_filter_by_dataplane() {
        let db = test_db("ops_nack_hist_dp").await;

        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-alpha",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "error alpha",
        )
        .await;
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-beta",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "error beta",
        )
        .await;

        let result = execute_ops_nack_history(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"dataplaneName": "dp-alpha"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let events = output["events"].as_array().unwrap();
        for event in events {
            assert_eq!(event["dataplane_name"], "dp-alpha");
        }
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_filter_by_type_url_short() {
        let db = test_db("ops_nack_hist_type").await;

        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "cds error",
        )
        .await;
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.listener.v3.Listener",
            "lds error",
        )
        .await;

        // Filter by "CDS" short form
        let result = execute_ops_nack_history(&db.pool, TEAM_A_ID, None, json!({"typeUrl": "CDS"}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let events = output["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["resource_type"], "CDS");
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_team_isolation() {
        let db = test_db("ops_nack_hist_iso").await;

        // team-a NACK
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-a",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "team-a error",
        )
        .await;

        // team-b NACK
        insert_nack_event(
            &db.pool,
            TEAM_B_ID,
            "dp-b",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "team-b error",
        )
        .await;

        // team-a should only see its own events
        let result = execute_ops_nack_history(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let events = output["events"].as_array().unwrap();
        for event in events {
            assert_ne!(event["dataplane_name"], "dp-b", "team-a must NOT see team-b's NACK events");
        }
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_invalid_since() {
        let db = TestDatabase::new("ops_nack_hist_bad_since").await;

        let result = execute_ops_nack_history(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"since": "not-a-timestamp"}),
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("Invalid 'since' timestamp")),
            other => panic!("expected InvalidParams, got: {:?}", other),
        }
    }

    // ========================================================================
    // Enhanced ops_config_validate: proto violations + xds_delivery
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_config_validate_detects_health_check_violations() {
        let db = test_db("ops_validate_hc").await;

        // Insert a cluster with missing thresholds into team-a
        let config_missing_thresholds = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": "/health"}]}"#;
        sqlx::query(
            "INSERT INTO clusters (id, name, service_name, configuration, version, team, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 1, $5, NOW(), NOW())",
        )
        .bind("hc-bad-cluster-id")
        .bind("hc-bad-cluster")
        .bind("hc-svc")
        .bind(config_missing_thresholds)
        .bind(TEAM_A_ID)
        .execute(&db.pool)
        .await
        .expect("insert cluster");

        let result = execute_ops_config_validate(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");

        let issues = output["issues"].as_array().unwrap();
        let proto_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "proto_violation").collect();
        assert!(
            proto_issues.len() >= 2,
            "should detect missing healthy_threshold and unhealthy_threshold"
        );
        assert!(proto_issues.iter().all(|i| i["severity"] == "error"));
        assert!(proto_issues.iter().any(|i| i["resource"] == "hc-bad-cluster"));
    }

    #[tokio::test]
    async fn test_execute_ops_config_validate_includes_recent_nacks() {
        let db = test_db("ops_validate_nacks").await;

        // Insert a NACK event for team-a
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "test nack error",
        )
        .await;

        let result = execute_ops_config_validate(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");

        let issues = output["issues"].as_array().unwrap();
        let delivery_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "xds_delivery").collect();
        assert!(!delivery_issues.is_empty(), "should include recent NACK as xds_delivery warning");
        assert!(delivery_issues
            .iter()
            .any(|i| i["message"].as_str().unwrap_or("").contains("test nack error")));
    }
}
