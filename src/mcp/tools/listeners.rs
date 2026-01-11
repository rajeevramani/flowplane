//! MCP Tools for Listener Control Plane Operations
//!
//! Provides tools for querying and inspecting listener configurations via the MCP protocol.

use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::storage::repositories::listener::ListenerRepository;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tracing::instrument;

/// Returns the MCP tool definition for listing listeners.
///
/// This tool supports pagination via `limit` and `offset` parameters.
pub fn cp_list_listeners_tool() -> Tool {
    Tool {
        name: "cp_list_listeners".to_string(),
        description: "List all listeners in the Flowplane control plane. Returns listener configurations with names, addresses, ports, protocols, and metadata. Supports pagination via limit and offset parameters.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of listeners to return (default: 50, max: 1000)",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 50
                },
                "offset": {
                    "type": "integer",
                    "description": "Number of listeners to skip for pagination (default: 0)",
                    "minimum": 0,
                    "default": 0
                }
            }
        }),
    }
}

/// Returns the MCP tool definition for getting a listener by name.
///
/// Requires a `name` parameter to identify the listener.
pub fn cp_get_listener_tool() -> Tool {
    Tool {
        name: "cp_get_listener".to_string(),
        description: "Get detailed information about a specific listener by name. Returns the listener's complete configuration including address, port, protocol, filter chains, and metadata.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the listener to retrieve"
                }
            },
            "required": ["name"]
        }),
    }
}

/// Execute the cp_list_listeners tool.
///
/// Lists listeners with pagination, returning pretty-printed JSON output.
///
/// # Arguments
///
/// * `db_pool` - Database connection pool
/// * `team` - Team identifier for multi-tenancy filtering
/// * `args` - Tool arguments containing optional `limit` and `offset`
///
/// # Returns
///
/// A `ToolCallResult` with listener list as pretty-printed JSON text.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_list_listeners")]
pub async fn execute_list_listeners(
    db_pool: &SqlitePool,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(50));

    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(0));

    tracing::debug!(
        team = %team,
        limit = ?limit,
        offset = ?offset,
        "Listing listeners for team"
    );

    let repo = ListenerRepository::new(db_pool.clone());

    // For team-based queries, use list_by_teams to enforce multi-tenancy
    let listeners = if team.is_empty() {
        repo.list(limit, offset).await
    } else {
        repo.list_by_teams(&[team.to_string()], true, limit, offset).await
    }
    .map_err(|e| McpError::DatabaseError(sqlx::Error::Protocol(e.to_string())))?;

    // Build output with listener summaries
    let listener_summaries: Vec<Value> = listeners
        .iter()
        .map(|listener| {
            let mut summary = json!({
                "name": listener.name,
                "address": listener.address,
                "port": listener.port,
                "protocol": listener.protocol,
                "version": listener.version,
                "source": listener.source,
                "team": listener.team,
                "created_at": listener.created_at.to_rfc3339(),
                "updated_at": listener.updated_at.to_rfc3339(),
            });

            // Parse configuration to extract description/tags if present
            if let Ok(config) = serde_json::from_str::<Value>(&listener.configuration) {
                if let Some(description) = config.get("description") {
                    summary["description"] = description.clone();
                }
                if let Some(tags) = config.get("tags") {
                    summary["tags"] = tags.clone();
                }
            }

            summary
        })
        .collect();

    let output = json!({
        "listeners": listener_summaries,
        "count": listeners.len(),
        "limit": limit,
        "offset": offset,
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        listener_count = listeners.len(),
        "Successfully listed listeners"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_get_listener tool.
///
/// Retrieves a specific listener by name, returning detailed configuration.
///
/// # Arguments
///
/// * `db_pool` - Database connection pool
/// * `team` - Team identifier for access control
/// * `args` - Tool arguments containing required `name` field
///
/// # Returns
///
/// A `ToolCallResult` with listener details as pretty-printed JSON, or
/// `ResourceNotFound` error if the listener doesn't exist.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_get_listener")]
pub async fn execute_get_listener(
    db_pool: &SqlitePool,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        listener_name = %name,
        "Getting listener by name"
    );

    let repo = ListenerRepository::new(db_pool.clone());
    let listener = repo.get_by_name(name).await.map_err(|e| {
        if e.to_string().contains("not found") {
            McpError::ResourceNotFound(format!("Listener '{}' not found", name))
        } else {
            McpError::DatabaseError(sqlx::Error::Protocol(e.to_string()))
        }
    })?;

    // Verify team access if team is specified
    if !team.is_empty() {
        if let Some(listener_team) = &listener.team {
            if listener_team != team {
                return Err(McpError::ResourceNotFound(format!("Listener '{}' not found", name)));
            }
        }
    }

    // Parse configuration JSON for pretty output
    let configuration: Value =
        serde_json::from_str(&listener.configuration).map_err(McpError::SerializationError)?;

    let output = json!({
        "id": listener.id.to_string(),
        "name": listener.name,
        "address": listener.address,
        "port": listener.port,
        "protocol": listener.protocol,
        "configuration": configuration,
        "version": listener.version,
        "source": listener.source,
        "team": listener.team,
        "import_id": listener.import_id,
        "created_at": listener.created_at.to_rfc3339(),
        "updated_at": listener.updated_at.to_rfc3339(),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        listener_name = %name,
        "Successfully retrieved listener"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseConfig;
    use crate::storage::create_pool;
    use crate::storage::repositories::listener::CreateListenerRequest;

    async fn setup_test_db() -> SqlitePool {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            max_connections: 5,
            min_connections: 1,
            connect_timeout_seconds: 5,
            idle_timeout_seconds: 0,
            auto_migrate: false,
        };
        let pool = create_pool(&config).await.expect("Failed to create pool");

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await.expect("Failed to run migrations");

        pool
    }

    /// Create a test team in the database
    async fn create_test_team(pool: &SqlitePool, team_name: &str) {
        let team_id = format!("team-{}", uuid::Uuid::new_v4());
        sqlx::query("INSERT INTO teams (id, name, display_name, status) VALUES ($1, $2, $3, $4)")
            .bind(&team_id)
            .bind(team_name)
            .bind(format!("Test {}", team_name))
            .bind("active")
            .execute(pool)
            .await
            .expect("Failed to create test team");
    }

    #[tokio::test]
    async fn test_cp_list_listeners_tool_definition() {
        let tool = cp_list_listeners_tool();
        assert_eq!(tool.name, "cp_list_listeners");
        assert!(tool.description.contains("List all listeners"));
        assert!(tool.input_schema.get("properties").is_some());
    }

    #[tokio::test]
    async fn test_cp_get_listener_tool_definition() {
        let tool = cp_get_listener_tool();
        assert_eq!(tool.name, "cp_get_listener");
        assert!(tool.description.contains("Get detailed information"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[tokio::test]
    async fn test_execute_list_listeners_empty() {
        let pool = setup_test_db().await;
        let args = json!({});

        let result = execute_list_listeners(&pool, "test-team", args).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert_eq!(tool_result.content.len(), 1);

        if let ContentBlock::Text { text } = &tool_result.content[0] {
            let output: Value = serde_json::from_str(text).unwrap();
            assert_eq!(output["count"], 0);
        } else {
            panic!("Expected text content block");
        }
    }

    #[tokio::test]
    async fn test_execute_list_listeners_with_data() {
        let pool = setup_test_db().await;

        // Create the team first (required by foreign key constraint)
        create_test_team(&pool, "test-team").await;

        let repo = ListenerRepository::new(pool.clone());

        // Create test listener
        repo.create(CreateListenerRequest {
            name: "test-listener-1".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8080),
            protocol: Some("HTTP".to_string()),
            configuration: json!({"filter_chains": []}),
            team: Some("test-team".to_string()),
            import_id: None,
        })
        .await
        .expect("Failed to create listener");

        let args = json!({"limit": 10, "offset": 0});
        let result = execute_list_listeners(&pool, "test-team", args).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        if let ContentBlock::Text { text } = &tool_result.content[0] {
            let output: Value = serde_json::from_str(text).unwrap();
            assert_eq!(output["count"], 1);
            assert_eq!(output["listeners"][0]["name"], "test-listener-1");
            assert_eq!(output["listeners"][0]["port"], 8080);
        }
    }

    #[tokio::test]
    async fn test_execute_get_listener_not_found() {
        let pool = setup_test_db().await;
        let args = json!({"name": "non-existent-listener"});

        let result = execute_get_listener(&pool, "test-team", args).await;
        assert!(result.is_err());

        if let Err(McpError::ResourceNotFound(msg)) = result {
            assert!(msg.contains("not found"));
        } else {
            panic!("Expected ResourceNotFound error");
        }
    }

    #[tokio::test]
    async fn test_execute_get_listener_success() {
        let pool = setup_test_db().await;

        // Create the team first (required by foreign key constraint)
        create_test_team(&pool, "test-team").await;

        let repo = ListenerRepository::new(pool.clone());

        // Create test listener
        repo.create(CreateListenerRequest {
            name: "test-listener".to_string(),
            address: "127.0.0.1".to_string(),
            port: Some(9090),
            protocol: Some("HTTPS".to_string()),
            configuration: json!({"filter_chains": [], "description": "Test listener"}),
            team: Some("test-team".to_string()),
            import_id: None,
        })
        .await
        .expect("Failed to create listener");

        let args = json!({"name": "test-listener"});
        let result = execute_get_listener(&pool, "test-team", args).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        if let ContentBlock::Text { text } = &tool_result.content[0] {
            let output: Value = serde_json::from_str(text).unwrap();
            assert_eq!(output["name"], "test-listener");
            assert_eq!(output["address"], "127.0.0.1");
            assert_eq!(output["port"], 9090);
            assert_eq!(output["protocol"], "HTTPS");
            assert_eq!(output["configuration"]["description"], "Test listener");
        }
    }

    #[tokio::test]
    async fn test_execute_get_listener_missing_name() {
        let pool = setup_test_db().await;
        let args = json!({});

        let result = execute_get_listener(&pool, "test-team", args).await;
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("Missing required parameter: name"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }
}
