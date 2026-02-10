//! Ops Agent MCP Tools
//!
//! Read-only diagnostic tools for tracing requests, viewing topology, and
//! validating configuration. These tools query the database directly
//! (no xds_state needed) via `ReportingRepository`.

use crate::domain::OrgId;
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::storage::repositories::{AuditLogFilters, AuditLogRepository, ReportingRepository};
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
{ "scope": "listener", "name": "http-8080", "include_details": true }

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
                "include_details": {
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
        r#"Validate gateway configuration and detect problems.

PURPOSE: Find misconfigurations, orphaned resources, and potential issues.

USE CASES:
- Pre-deployment validation
- Find orphan clusters (backends with no traffic)
- Find unbound route_configs (routing rules with no listener)
- Detect empty endpoint pools
- Health check after changes

PARAMETERS: None required. All checks run automatically for the current team.

RETURNS:
- valid: Boolean — true if no issues found
- issues: Array of detected problems, each with:
  - severity: "warning" or "error"
  - category: "orphan_cluster", "orphan_route_config", etc.
  - message: Human-readable description
  - resource: Affected resource name
- summary: Counts for total issues, warnings, errors

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
                "resource_type": {
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

/// Execute ops_trace_request: trace a request path through the gateway.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_trace_request")]
pub async fn execute_ops_trace_request(
    db_pool: &DbPool,
    team: &str,
    _org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("'path' is required".to_string()))?;

    // Input validation (R6): max 2048 chars, must start with '/', reject null bytes
    if path.len() > 2048 {
        return Err(McpError::InvalidParams(
            "'path' exceeds maximum length of 2048 characters".to_string(),
        ));
    }
    if !path.starts_with('/') {
        return Err(McpError::InvalidParams("'path' must start with '/'".to_string()));
    }
    if path.contains('\0') {
        return Err(McpError::InvalidParams("'path' must not contain null bytes".to_string()));
    }

    let port = args.get("port").and_then(|v| v.as_i64());

    let repo = ReportingRepository::new(db_pool.clone());
    let result = repo
        .trace_request_path(team, path, port)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to trace request path: {}", e)))?;

    let port_desc = port.map(|p| format!(" on port {}", p)).unwrap_or_default();

    let (message, unmatched_reason) = if result.matches.is_empty() {
        let reason = format!("No route matches path '{}'{} for the current team", path, port_desc);
        (format!("No routes match path '{}'{}", path, port_desc), Some(reason))
    } else {
        (
            format!(
                "Found {} route(s) matching path '{}'{}",
                result.matches.len(),
                path,
                port_desc
            ),
            None,
        )
    };

    let output = json!({
        "success": true,
        "path": path,
        "port": port,
        "match_count": result.matches.len(),
        "matches": result.matches,
        "endpoints": result.endpoints,
        "unmatched_reason": unmatched_reason,
        "message": message
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute ops_topology: view the full gateway topology.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_topology")]
pub async fn execute_ops_topology(
    db_pool: &DbPool,
    team: &str,
    _org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let scope = args.get("scope").and_then(|v| v.as_str());
    let name = args.get("name").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_i64());
    let include_details = args.get("include_details").and_then(|v| v.as_bool()).unwrap_or(false);

    let repo = ReportingRepository::new(db_pool.clone());
    let result = repo
        .full_topology(team, scope, name, limit)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to get topology: {}", e)))?;

    let message = format!(
        "Topology: {} listeners, {} route_configs, {} clusters, {} routes ({} orphan clusters, {} orphan route_configs){}",
        result.summary.listener_count,
        result.summary.route_config_count,
        result.summary.cluster_count,
        result.summary.route_count,
        result.summary.orphan_cluster_count,
        result.summary.orphan_route_config_count,
        if result.truncated { " [TRUNCATED]" } else { "" }
    );

    // When include_details=false (default), omit rows for token efficiency (<120 tokens).
    // Orphans are always included since they're the most actionable part.
    let output = if include_details {
        json!({
            "success": true,
            "scope": scope.unwrap_or("full"),
            "name": name,
            "rows": result.rows,
            "orphan_clusters": result.orphan_clusters,
            "orphan_route_configs": result.orphan_route_configs,
            "summary": result.summary,
            "truncated": result.truncated,
            "message": message
        })
    } else {
        json!({
            "success": true,
            "scope": scope.unwrap_or("full"),
            "name": name,
            "orphan_clusters": result.orphan_clusters,
            "orphan_route_configs": result.orphan_route_configs,
            "summary": result.summary,
            "truncated": result.truncated,
            "message": message
        })
    };

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute ops_config_validate: validate gateway configuration.
#[instrument(skip(db_pool, _args), fields(team = %team), name = "mcp_execute_ops_config_validate")]
pub async fn execute_ops_config_validate(
    db_pool: &DbPool,
    team: &str,
    _org_id: Option<&OrgId>,
    _args: Value,
) -> Result<ToolCallResult, McpError> {
    let repo = ReportingRepository::new(db_pool.clone());

    // Get topology with orphan detection
    let topology = repo
        .full_topology(team, None, None, Some(200))
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to validate config: {}", e)))?;

    let mut issues: Vec<Value> = Vec::new();

    // Check for orphan clusters
    for orphan in &topology.orphan_clusters {
        issues.push(json!({
            "severity": "warning",
            "category": "orphan_cluster",
            "message": format!("Cluster '{}' (service: {}) is not referenced by any route_config — it receives no traffic", orphan.name, orphan.service_name),
            "resource": orphan.name
        }));
    }

    // Check for orphan route configs
    for orphan in &topology.orphan_route_configs {
        issues.push(json!({
            "severity": "warning",
            "category": "orphan_route_config",
            "message": format!("Route config '{}' (path: {}, cluster: {}) is not bound to any listener — its routes are unreachable", orphan.name, orphan.path_prefix, orphan.cluster_name),
            "resource": orphan.name
        }));
    }

    // Check for listeners with no route configs
    for row in &topology.rows {
        if row.route_config_name.is_none() {
            // Check if this listener is already reported (avoid duplicates)
            let already_reported = issues.iter().any(|i| {
                i.get("category").and_then(|c| c.as_str()) == Some("empty_listener")
                    && i.get("resource").and_then(|r| r.as_str()) == Some(&row.listener_name)
            });
            if !already_reported {
                issues.push(json!({
                    "severity": "warning",
                    "category": "empty_listener",
                    "message": format!("Listener '{}' ({}:{}) has no route configs bound — it cannot route traffic", row.listener_name, row.listener_address, row.listener_port.map(|p| p.to_string()).unwrap_or_else(|| "?".to_string())),
                    "resource": row.listener_name
                }));
            }
        }
    }

    let warning_count = issues.iter().filter(|i| i["severity"] == "warning").count();
    let error_count = issues.iter().filter(|i| i["severity"] == "error").count();
    let valid = issues.is_empty();

    let output = json!({
        "success": true,
        "valid": valid,
        "issues": issues,
        "summary": {
            "total_issues": issues.len(),
            "warnings": warning_count,
            "errors": error_count,
            "listeners": topology.summary.listener_count,
            "route_configs": topology.summary.route_config_count,
            "clusters": topology.summary.cluster_count,
            "routes": topology.summary.route_count
        },
        "message": if valid {
            "Configuration is valid — no issues detected".to_string()
        } else {
            format!("Found {} issue(s): {} warning(s), {} error(s)", issues.len(), warning_count, error_count)
        }
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute ops_audit_query: query recent audit log entries (PII-stripped).
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_audit_query")]
pub async fn execute_ops_audit_query(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let resource_type = args.get("resource_type").and_then(|v| v.as_str()).map(|s| s.to_string());
    let action = args.get("action").and_then(|v| v.as_str()).map(|s| s.to_string());
    let limit =
        args.get("limit").and_then(|v| v.as_i64()).map(|v| (v as i32).min(100)).or(Some(20));

    // Force team_id and org_id from session — cannot be overridden by caller
    let filters = AuditLogFilters {
        resource_type,
        action,
        user_id: None, // No user_id filter exposed — PII protection
        org_id: org_id.map(|o| o.to_string()),
        team_id: Some(team.to_string()),
        start_date: None,
        end_date: None,
    };

    let repo = AuditLogRepository::new(db_pool.clone());
    let entries = repo
        .query_logs(Some(filters), limit, Some(0))
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to query audit logs: {}", e)))?;

    // Convert to PII-stripped summaries
    let summaries: Vec<Value> = entries
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "resource_type": entry.resource_type,
                "resource_id": entry.resource_id,
                "resource_name": entry.resource_name,
                "action": entry.action,
                "created_at": entry.created_at.to_rfc3339()
            })
        })
        .collect();

    let count = summaries.len();
    let output = json!({
        "success": true,
        "entries": summaries,
        "count": count,
        "message": format!("Found {} audit log entries", count)
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(schema["properties"]["include_details"].is_object());

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
        let tools = vec![ops_trace_request_tool(), ops_topology_tool(), ops_config_validate_tool()];

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
            execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({"include_details": true}))
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
            json!({"scope": "listener", "name": "http-8080", "include_details": true}),
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
            json!({"limit": 1, "include_details": true}),
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
            execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({"include_details": true}))
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
        assert!(schema["properties"]["resource_type"].is_object());
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
            json!({"action": "delete", "resource_type": "listeners"}),
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
}
