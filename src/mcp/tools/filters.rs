//! Filters MCP Tools
//!
//! Control Plane tools for managing filters.

use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tracing::instrument;

/// Tool definition for listing filters
pub fn cp_list_filters_tool() -> Tool {
    Tool {
        name: "cp_list_filters".to_string(),
        description: "List all filters with their configuration. Filters provide features like authentication, rate limiting, CORS, etc.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "filter_type": {
                    "type": "string",
                    "description": "Filter by filter type (e.g., jwt_auth, oauth2, cors, rate_limit)"
                }
            }
        }),
    }
}

/// Tool definition for getting a specific filter
pub fn cp_get_filter_tool() -> Tool {
    Tool {
        name: "cp_get_filter".to_string(),
        description: "Get detailed information about a specific filter by name.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the filter to retrieve"
                }
            },
            "required": ["name"]
        }),
    }
}

/// Execute list filters operation
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_list_filters")]
pub async fn execute_list_filters(
    db_pool: &SqlitePool,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let filter_type = args["filter_type"].as_str();

    #[derive(sqlx::FromRow)]
    struct FilterRow {
        id: String,
        name: String,
        filter_type: String,
        description: Option<String>,
        configuration: String,
        version: i64,
        source: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let filters = if let Some(ft) = filter_type {
        sqlx::query_as::<_, FilterRow>(
            "SELECT id, name, filter_type, description, configuration, version, source, created_at, updated_at \
             FROM filters WHERE team = $1 AND filter_type = $2 ORDER BY name"
        )
        .bind(team)
        .bind(ft)
        .fetch_all(db_pool)
        .await
    } else {
        sqlx::query_as::<_, FilterRow>(
            "SELECT id, name, filter_type, description, configuration, version, source, created_at, updated_at \
             FROM filters WHERE team = $1 ORDER BY name"
        )
        .bind(team)
        .fetch_all(db_pool)
        .await
    }
    .map_err(|e| {
        tracing::error!(error = %e, team = %team, "Failed to list filters");
        McpError::DatabaseError(e)
    })?;

    let result = json!({
        "filters": filters.iter().map(|f| {
            // Parse configuration JSON, log warning on failure
            let config: Value = serde_json::from_str(&f.configuration).unwrap_or_else(|e| {
                tracing::warn!(filter_id = %f.id, error = %e, "Failed to parse filter configuration");
                json!({"_parse_error": format!("Failed to parse configuration: {}", e)})
            });

            json!({
                "id": f.id,
                "name": f.name,
                "filter_type": f.filter_type,
                "description": f.description,
                "configuration": config,
                "version": f.version,
                "source": f.source,
                "created_at": f.created_at.to_rfc3339(),
                "updated_at": f.updated_at.to_rfc3339()
            })
        }).collect::<Vec<_>>(),
        "count": filters.len()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

/// Execute get filter operation
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_get_filter")]
pub async fn execute_get_filter(
    db_pool: &SqlitePool,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    #[derive(sqlx::FromRow)]
    struct FilterRow {
        id: String,
        name: String,
        filter_type: String,
        description: Option<String>,
        configuration: String,
        version: i64,
        source: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let filter = sqlx::query_as::<_, FilterRow>(
        "SELECT id, name, filter_type, description, configuration, version, source, created_at, updated_at \
         FROM filters WHERE team = $1 AND name = $2"
    )
    .bind(team)
    .bind(name)
    .fetch_optional(db_pool)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, team = %team, filter_name = %name, "Failed to get filter");
        McpError::DatabaseError(e)
    })?;

    let filter =
        filter.ok_or_else(|| McpError::ResourceNotFound(format!("Filter '{}' not found", name)))?;

    // Parse configuration JSON, log warning on failure
    let config: Value = serde_json::from_str(&filter.configuration).unwrap_or_else(|e| {
        tracing::warn!(filter_id = %filter.id, error = %e, "Failed to parse filter configuration");
        json!({"_parse_error": format!("Failed to parse configuration: {}", e)})
    });

    // Get installations (listeners where this filter is installed)
    #[derive(sqlx::FromRow)]
    struct InstallationRow {
        listener_id: String,
        listener_name: String,
        listener_address: String,
        filter_order: i64,
    }

    let installations = sqlx::query_as::<_, InstallationRow>(
        "SELECT l.id as listener_id, l.name as listener_name, l.address as listener_address, lf.filter_order \
         FROM listener_filters lf \
         INNER JOIN listeners l ON lf.listener_id = l.id \
         WHERE lf.filter_id = $1 \
         ORDER BY l.name"
    )
    .bind(&filter.id)
    .fetch_all(db_pool)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, filter_id = %filter.id, "Failed to get filter installations");
        McpError::DatabaseError(e)
    })?;

    let result = json!({
        "id": filter.id,
        "name": filter.name,
        "filter_type": filter.filter_type,
        "description": filter.description,
        "configuration": config,
        "version": filter.version,
        "source": filter.source,
        "installations": installations.iter().map(|i| json!({
            "listener_id": i.listener_id,
            "listener_name": i.listener_name,
            "listener_address": i.listener_address,
            "order": i.filter_order
        })).collect::<Vec<_>>(),
        "created_at": filter.created_at.to_rfc3339(),
        "updated_at": filter.updated_at.to_rfc3339()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}
