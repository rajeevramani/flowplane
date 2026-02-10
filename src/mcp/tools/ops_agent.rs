//! Ops Agent MCP Tools
//!
//! Read-only diagnostic tools for tracing requests, viewing topology, and
//! validating configuration. These tools query the database directly
//! (no xds_state needed) via `ReportingRepository`.

use crate::domain::OrgId;
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::storage::repositories::ReportingRepository;
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

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Request path to trace (e.g., '/api/users')"
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

EXAMPLE:
{ "scope": "listener", "name": "http-8080" }

RETURNS:
- rows: Flattened topology rows (listener → route_config → virtual_host → route)
- orphan_clusters: Clusters with no route_configs referencing them
- orphan_route_configs: Route configs with no listener bound
- summary: Counts for listeners, route_configs, clusters, routes, orphans
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

    let port = args.get("port").and_then(|v| v.as_i64());

    let repo = ReportingRepository::new(db_pool.clone());
    let result = repo
        .trace_request_path(team, path, port)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to trace request path: {}", e)))?;

    let output = json!({
        "success": true,
        "path": path,
        "port": port,
        "match_count": result.matches.len(),
        "matches": result.matches,
        "endpoints": result.endpoints,
        "message": if result.matches.is_empty() {
            format!("No routes match path '{}'{}", path, port.map(|p| format!(" on port {}", p)).unwrap_or_default())
        } else {
            format!("Found {} route(s) matching path '{}'{}", result.matches.len(), path, port.map(|p| format!(" on port {}", p)).unwrap_or_default())
        }
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

    let repo = ReportingRepository::new(db_pool.clone());
    let result = repo
        .full_topology(team, scope, name, limit)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to get topology: {}", e)))?;

    let output = json!({
        "success": true,
        "scope": scope.unwrap_or("full"),
        "name": name,
        "rows": result.rows,
        "orphan_clusters": result.orphan_clusters,
        "orphan_route_configs": result.orphan_route_configs,
        "summary": result.summary,
        "truncated": result.truncated,
        "message": format!(
            "Topology: {} listeners, {} route_configs, {} clusters, {} routes ({} orphan clusters, {} orphan route_configs){}",
            result.summary.listener_count,
            result.summary.route_config_count,
            result.summary.cluster_count,
            result.summary.route_count,
            result.summary.orphan_cluster_count,
            result.summary.orphan_route_config_count,
            if result.truncated { " [TRUNCATED]" } else { "" }
        )
    });

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
    async fn test_execute_ops_topology_happy_path() {
        let db = test_db("ops_topo_happy").await;

        let result = execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(!output["rows"].as_array().unwrap().is_empty());
        assert!(output["summary"]["listener_count"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_execute_ops_topology_scoped() {
        let db = test_db("ops_topo_scoped").await;

        let result = execute_ops_topology(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"scope": "listener", "name": "http-8080"}),
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

        let result = execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({"limit": 1}))
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

        let result = execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({}))
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
}
