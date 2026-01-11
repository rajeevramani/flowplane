//! Routes MCP Tools
//!
//! Control Plane tools for managing routes.

use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tracing::instrument;

/// Tool definition for listing routes
pub fn cp_list_routes_tool() -> Tool {
    Tool {
        name: "cp_list_routes".to_string(),
        description: "List all routes with metadata. Routes represent path patterns and HTTP methods for traffic matching.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of routes to return (default: 100, max: 1000)",
                    "minimum": 1,
                    "maximum": 1000
                },
                "offset": {
                    "type": "integer",
                    "description": "Number of routes to skip (for pagination)",
                    "minimum": 0
                },
                "route_config": {
                    "type": "string",
                    "description": "Filter by route configuration name"
                }
            }
        }),
    }
}

/// Execute list routes operation
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_list_routes")]
pub async fn execute_list_routes(
    db_pool: &SqlitePool,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let limit = args["limit"].as_i64().unwrap_or(100).min(1000) as i32;
    let offset = args["offset"].as_i64().unwrap_or(0) as i32;
    let route_config_filter = args["route_config"].as_str();

    // Build query with optional route_config filter
    let query = if route_config_filter.is_some() {
        "SELECT r.id, r.virtual_host_id, r.name, r.path_pattern, r.match_type, r.rule_order, \
                rm.operation_id, rm.summary, rm.description, rm.tags, \
                vh.name as virtual_host_name, rc.name as route_config_name, \
                r.created_at, r.updated_at \
         FROM routes r \
         INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
         INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
         LEFT JOIN route_metadata rm ON r.id = rm.route_id \
         WHERE rc.team = $1 AND rc.name = $2 \
         ORDER BY rc.name, vh.name, r.rule_order \
         LIMIT $3 OFFSET $4"
    } else {
        "SELECT r.id, r.virtual_host_id, r.name, r.path_pattern, r.match_type, r.rule_order, \
                rm.operation_id, rm.summary, rm.description, rm.tags, \
                vh.name as virtual_host_name, rc.name as route_config_name, \
                r.created_at, r.updated_at \
         FROM routes r \
         INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
         INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
         LEFT JOIN route_metadata rm ON r.id = rm.route_id \
         WHERE rc.team = $1 \
         ORDER BY rc.name, vh.name, r.rule_order \
         LIMIT $2 OFFSET $3"
    };

    #[derive(sqlx::FromRow)]
    #[allow(dead_code)]
    struct RouteRow {
        id: String,
        #[sqlx(rename = "virtual_host_id")]
        _virtual_host_id: String,
        name: String,
        path_pattern: String,
        match_type: String,
        rule_order: i32,
        operation_id: Option<String>,
        summary: Option<String>,
        description: Option<String>,
        tags: Option<String>,
        virtual_host_name: String,
        route_config_name: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let routes = if let Some(rc_name) = route_config_filter {
        sqlx::query_as::<_, RouteRow>(query)
            .bind(team)
            .bind(rc_name)
            .bind(limit)
            .bind(offset)
            .fetch_all(db_pool)
            .await
    } else {
        sqlx::query_as::<_, RouteRow>(query)
            .bind(team)
            .bind(limit)
            .bind(offset)
            .fetch_all(db_pool)
            .await
    }
    .map_err(|e| {
        tracing::error!(error = %e, team = %team, "Failed to list routes");
        McpError::DatabaseError(e)
    })?;

    let result = json!({
        "routes": routes.iter().map(|r| {
            // Parse tags if available
            let tags: Option<Vec<String>> = r.tags.as_ref()
                .and_then(|t| serde_json::from_str(t).ok());

            json!({
                "id": r.id,
                "name": r.name,
                "route_config": r.route_config_name,
                "virtual_host": r.virtual_host_name,
                "path_pattern": r.path_pattern,
                "match_type": r.match_type,
                "rule_order": r.rule_order,
                "metadata": {
                    "operation_id": r.operation_id,
                    "summary": r.summary,
                    "description": r.description,
                    "tags": tags
                },
                "created_at": r.created_at.to_rfc3339(),
                "updated_at": r.updated_at.to_rfc3339()
            })
        }).collect::<Vec<_>>(),
        "count": routes.len(),
        "limit": limit,
        "offset": offset
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}
