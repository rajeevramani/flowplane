//! MCP Tools Module
//!
//! Provides Control Plane (CP) tools for querying Flowplane configuration via MCP protocol.
//! Each tool allows AI assistants to inspect and query the control plane state.

pub mod clusters;
pub mod filters;
pub mod listeners;
pub mod routes;

// Re-export tool definitions for convenience
pub use clusters::{cp_get_cluster_tool, cp_list_clusters_tool};
pub use clusters::{execute_get_cluster, execute_list_clusters};

pub use filters::{cp_get_filter_tool, cp_list_filters_tool};
pub use filters::{execute_get_filter, execute_list_filters};

pub use listeners::{cp_get_listener_tool, cp_list_listeners_tool};
pub use listeners::{execute_get_listener, execute_list_listeners};

pub use routes::cp_list_routes_tool;
pub use routes::execute_list_routes;

use crate::mcp::error::McpError;
use crate::mcp::protocol::{Tool, ToolCallResult};
use serde_json::Value;
use sqlx::SqlitePool;

/// Get all available MCP tools.
///
/// Returns a vector of all tool definitions that can be exposed to MCP clients.
pub fn get_all_tools() -> Vec<Tool> {
    vec![
        cp_list_clusters_tool(),
        cp_get_cluster_tool(),
        cp_list_listeners_tool(),
        cp_get_listener_tool(),
        cp_list_routes_tool(),
        cp_list_filters_tool(),
        cp_get_filter_tool(),
    ]
}

/// Execute a tool by name.
///
/// Routes tool execution to the appropriate handler based on tool name.
///
/// # Arguments
///
/// * `tool_name` - Name of the tool to execute
/// * `db_pool` - Database connection pool
/// * `team` - Team identifier for multi-tenancy
/// * `args` - Tool arguments as JSON value
///
/// # Returns
///
/// Result containing the tool execution result or an error.
pub async fn execute_tool(
    tool_name: &str,
    db_pool: &SqlitePool,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    match tool_name {
        "cp_list_clusters" => execute_list_clusters(db_pool, team, args).await,
        "cp_get_cluster" => execute_get_cluster(db_pool, team, args).await,
        "cp_list_listeners" => execute_list_listeners(db_pool, team, args).await,
        "cp_get_listener" => execute_get_listener(db_pool, team, args).await,
        "cp_list_routes" => execute_list_routes(db_pool, team, args).await,
        "cp_list_filters" => execute_list_filters(db_pool, team, args).await,
        "cp_get_filter" => execute_get_filter(db_pool, team, args).await,
        _ => Err(McpError::ToolNotFound(format!("Unknown tool: {}", tool_name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_all_tools() {
        let tools = get_all_tools();
        assert_eq!(tools.len(), 7);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"cp_list_clusters"));
        assert!(tool_names.contains(&"cp_get_cluster"));
        assert!(tool_names.contains(&"cp_list_listeners"));
        assert!(tool_names.contains(&"cp_get_listener"));
        assert!(tool_names.contains(&"cp_list_routes"));
        assert!(tool_names.contains(&"cp_list_filters"));
        assert!(tool_names.contains(&"cp_get_filter"));
    }

    #[tokio::test]
    async fn test_execute_tool_unknown() {
        use crate::config::DatabaseConfig;
        use crate::storage::create_pool;

        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            max_connections: 5,
            min_connections: 1,
            connect_timeout_seconds: 5,
            idle_timeout_seconds: 0,
            auto_migrate: false,
        };
        let pool = create_pool(&config).await.expect("Failed to create pool");

        let result = execute_tool("unknown_tool", &pool, "test-team", serde_json::json!({})).await;

        assert!(result.is_err());
        if let Err(McpError::ToolNotFound(msg)) = result {
            assert!(msg.contains("Unknown tool"));
        } else {
            panic!("Expected ToolNotFound error");
        }
    }
}
