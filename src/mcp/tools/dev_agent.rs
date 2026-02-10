//! Dev Agent MCP Tools
//!
//! Development workflow tools for pre-creation validation and preflight checks.
//! These tools query the database directly (no xds_state needed) to detect
//! potential conflicts before resource creation.

use crate::domain::OrgId;
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::storage::DbPool;
use serde_json::{json, Value};
use tracing::instrument;

// =============================================================================
// TOOL DEFINITIONS
// =============================================================================

/// Dev tool: comprehensive pre-creation validation.
pub fn dev_preflight_check_tool() -> Tool {
    Tool::new(
        "dev_preflight_check",
        r#"Run pre-creation validation before deploying resources.

PURPOSE: Catch conflicts and issues BEFORE creating resources. Use this as the first step
in any deployment workflow to avoid partial failures.

CHECKS PERFORMED:
- Port conflicts: Is the listen port already in use?
- Path conflicts: Is the request path already routed?
- Name collisions: Do any proposed resource names already exist?
- Cluster name: Does a cluster with this name already exist?

PARAMETERS:
- path (optional): Request path to check for routing conflicts (e.g., "/api/orders")
- listen_port (optional): Port number to check availability
- cluster_name (optional): Cluster name to check for duplicates
- route_config_name (optional): Route config name to check for duplicates
- listener_name (optional): Listener name to check for duplicates

EXAMPLE:
{ "path": "/api/orders", "listen_port": 8080, "cluster_name": "orders-svc" }

RETURNS:
- ready: Boolean — true if all checks pass
- checks: Array of individual check results
- message: Summary of findings

SECURITY: Port conflict responses do not reveal which team owns a conflicting port.

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Request path to check for routing conflicts"
                },
                "listen_port": {
                    "type": "integer",
                    "description": "Port number to check availability"
                },
                "cluster_name": {
                    "type": "string",
                    "description": "Cluster name to check for duplicates"
                },
                "route_config_name": {
                    "type": "string",
                    "description": "Route config name to check for duplicates"
                },
                "listener_name": {
                    "type": "string",
                    "description": "Listener name to check for duplicates"
                }
            }
        }),
    )
}

// =============================================================================
// EXECUTE FUNCTIONS
// =============================================================================

/// Execute dev_preflight_check: validate before creating resources.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_dev_preflight_check")]
pub async fn execute_dev_preflight_check(
    db_pool: &DbPool,
    team: &str,
    _org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let mut checks: Vec<Value> = Vec::new();
    let mut ready = true;

    // Check port availability
    if let Some(port) = args.get("listen_port").and_then(|v| v.as_i64()) {
        let port_check = check_port_available(db_pool, port).await?;
        if !port_check["pass"].as_bool().unwrap_or(false) {
            ready = false;
        }
        checks.push(port_check);
    }

    // Check path conflicts (within team scope)
    if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
        let path_check = check_path_conflict(db_pool, team, path).await?;
        if !path_check["pass"].as_bool().unwrap_or(false) {
            ready = false;
        }
        checks.push(path_check);
    }

    // Check cluster name collision
    if let Some(name) = args.get("cluster_name").and_then(|v| v.as_str()) {
        let name_check = check_name_exists(db_pool, "clusters", "name", name).await?;
        if !name_check["pass"].as_bool().unwrap_or(false) {
            ready = false;
        }
        checks.push(name_check);
    }

    // Check route config name collision
    if let Some(name) = args.get("route_config_name").and_then(|v| v.as_str()) {
        let name_check = check_name_exists(db_pool, "route_configs", "name", name).await?;
        if !name_check["pass"].as_bool().unwrap_or(false) {
            ready = false;
        }
        checks.push(name_check);
    }

    // Check listener name collision
    if let Some(name) = args.get("listener_name").and_then(|v| v.as_str()) {
        let name_check = check_name_exists(db_pool, "listeners", "name", name).await?;
        if !name_check["pass"].as_bool().unwrap_or(false) {
            ready = false;
        }
        checks.push(name_check);
    }

    if checks.is_empty() {
        return Err(McpError::InvalidParams(
            "At least one check parameter is required (path, listen_port, cluster_name, route_config_name, or listener_name)".to_string(),
        ));
    }

    let failed_count = checks.iter().filter(|c| !c["pass"].as_bool().unwrap_or(true)).count();

    let output = json!({
        "success": true,
        "ready": ready,
        "checks": checks,
        "summary": {
            "total_checks": checks.len(),
            "passed": checks.len() - failed_count,
            "failed": failed_count
        },
        "message": if ready {
            "All preflight checks passed — safe to proceed with resource creation".to_string()
        } else {
            format!("{} preflight check(s) failed — resolve conflicts before creating resources", failed_count)
        }
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

// =============================================================================
// HELPER FUNCTIONS
// =============================================================================

/// Check if a port is available across ALL teams (security: don't reveal which team owns it).
async fn check_port_available(db_pool: &DbPool, port: i64) -> Result<Value, McpError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT COUNT(*) FROM listeners WHERE port = $1")
        .bind(port)
        .fetch_optional(db_pool)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to check port: {}", e)))?;

    let count = row.map(|r| r.0).unwrap_or(0);
    let available = count == 0;

    Ok(json!({
        "check": "port_available",
        "port": port,
        "pass": available,
        "message": if available {
            format!("Port {} is available", port)
        } else {
            // Security: do NOT reveal which team owns the port
            format!("Port {} is already in use", port)
        }
    }))
}

/// Check if a path is already routed within the team scope.
async fn check_path_conflict(db_pool: &DbPool, team: &str, path: &str) -> Result<Value, McpError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*) FROM routes r \
         JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
         JOIN route_configs rc ON vh.route_config_id = rc.id \
         WHERE r.path_pattern = $1 AND (rc.team = $2 OR rc.team IS NULL)",
    )
    .bind(path)
    .bind(team)
    .fetch_optional(db_pool)
    .await
    .map_err(|e| McpError::InternalError(format!("Failed to check path: {}", e)))?;

    let count = row.map(|r| r.0).unwrap_or(0);
    let available = count == 0;

    Ok(json!({
        "check": "path_available",
        "path": path,
        "pass": available,
        "message": if available {
            format!("Path '{}' is not currently routed", path)
        } else {
            format!("Path '{}' is already routed — use ops_trace_request to see the full chain", path)
        }
    }))
}

/// Check if a resource name already exists in a table.
async fn check_name_exists(
    db_pool: &DbPool,
    table: &str,
    column: &str,
    name: &str,
) -> Result<Value, McpError> {
    // Use parameterized query — table/column are compile-time constants (not user input)
    let query = format!("SELECT COUNT(*) FROM {} WHERE {} = $1", table, column);
    let row: Option<(i64,)> = sqlx::query_as(&query)
        .bind(name)
        .fetch_optional(db_pool)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to check name: {}", e)))?;

    let count = row.map(|r| r.0).unwrap_or(0);
    let available = count == 0;

    Ok(json!({
        "check": format!("{}_name_available", table),
        "name": name,
        "pass": available,
        "message": if available {
            format!("{} '{}' is available", table.replace('_', " "), name)
        } else {
            format!("{} '{}' already exists", table.replace('_', " "), name)
        }
    }))
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
    fn test_dev_preflight_check_tool_definition() {
        let tool = dev_preflight_check_tool();
        assert_eq!(tool.name, "dev_preflight_check");
        assert!(tool.description.as_ref().unwrap().contains("pre-creation"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["listen_port"].is_object());
        assert!(schema["properties"]["cluster_name"].is_object());
    }

    // ========================================================================
    // Execute tests
    // ========================================================================

    #[tokio::test]
    async fn test_preflight_all_clear() {
        let db = test_db("preflight_clear").await;

        let result = execute_dev_preflight_check(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/api/unused", "listen_port": 55555, "cluster_name": "new-cluster"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");
        assert_eq!(output["ready"], true);
        assert_eq!(output["summary"]["failed"], 0);
    }

    #[tokio::test]
    async fn test_preflight_port_conflict() {
        let db = test_db("preflight_port").await;

        // Port 8080 is used by team-a's listener
        let result =
            execute_dev_preflight_check(&db.pool, TEAM_A_ID, None, json!({"listen_port": 8080}))
                .await
                .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");
        assert_eq!(output["ready"], false);

        let port_check = &output["checks"][0];
        assert_eq!(port_check["pass"], false);
        assert!(port_check["message"].as_str().unwrap().contains("already in use"));
    }

    #[tokio::test]
    async fn test_preflight_port_conflict_cross_tenant_sanitized() {
        let db = test_db("preflight_xten").await;

        // team-a queries about port 9090 which belongs to team-b
        let result =
            execute_dev_preflight_check(&db.pool, TEAM_A_ID, None, json!({"listen_port": 9090}))
                .await
                .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");
        assert_eq!(output["ready"], false);

        // Must say "in use" but NOT reveal team-b
        let msg = output["checks"][0]["message"].as_str().unwrap();
        assert!(msg.contains("already in use"), "should say port is in use");
        assert!(!msg.contains("team-b"), "must NOT reveal team-b");
        assert!(!msg.contains(TEAM_B_ID), "must NOT reveal team-b ID");
    }

    #[tokio::test]
    async fn test_preflight_path_conflict() {
        let db = test_db("preflight_path").await;

        // /api/orders is routed for team-a
        let result =
            execute_dev_preflight_check(&db.pool, TEAM_A_ID, None, json!({"path": "/api/orders"}))
                .await
                .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");
        assert_eq!(output["ready"], false);

        let path_check = &output["checks"][0];
        assert_eq!(path_check["pass"], false);
        assert!(path_check["message"].as_str().unwrap().contains("already routed"));
    }

    #[tokio::test]
    async fn test_preflight_name_collision() {
        let db = test_db("preflight_name").await;

        // "orders-svc" cluster already exists
        let result = execute_dev_preflight_check(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"cluster_name": "orders-svc"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");
        assert_eq!(output["ready"], false);
        assert_eq!(output["checks"][0]["pass"], false);
    }

    #[tokio::test]
    async fn test_preflight_no_params() {
        let db = test_db("preflight_empty").await;

        let result = execute_dev_preflight_check(&db.pool, TEAM_A_ID, None, json!({})).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("At least one")),
            other => panic!("expected InvalidParams, got: {:?}", other),
        }
    }
}
