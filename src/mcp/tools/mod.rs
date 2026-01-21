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

// Re-export cluster CRUD tools
pub use clusters::{cp_create_cluster_tool, cp_delete_cluster_tool, cp_update_cluster_tool};
pub use clusters::{execute_create_cluster, execute_delete_cluster, execute_update_cluster};

pub use filters::{cp_get_filter_tool, cp_list_filters_tool};
pub use filters::{execute_get_filter, execute_list_filters};

// Re-export filter CRUD tools
pub use filters::{cp_create_filter_tool, cp_delete_filter_tool, cp_update_filter_tool};
pub use filters::{execute_create_filter, execute_delete_filter, execute_update_filter};

pub use listeners::{cp_get_listener_tool, cp_list_listeners_tool};
pub use listeners::{execute_get_listener, execute_list_listeners};

// Re-export listener CRUD tools
pub use listeners::{cp_create_listener_tool, cp_delete_listener_tool, cp_update_listener_tool};
pub use listeners::{execute_create_listener, execute_delete_listener, execute_update_listener};

pub use routes::cp_list_routes_tool;
pub use routes::execute_list_routes;

// Re-export route config CRUD tools
pub use routes::{
    cp_create_route_config_tool, cp_delete_route_config_tool, cp_update_route_config_tool,
};
pub use routes::{
    execute_create_route_config, execute_delete_route_config, execute_update_route_config,
};

use crate::mcp::error::McpError;
use crate::mcp::protocol::{Tool, ToolCallResult};
use serde_json::Value;
use sqlx::SqlitePool;

/// Get all available MCP tools.
///
/// Returns a vector of all tool definitions that can be exposed to MCP clients.
/// Includes both read-only tools (cp_list_*, cp_get_*) and CRUD tools (cp_create_*, cp_update_*, cp_delete_*).
pub fn get_all_tools() -> Vec<Tool> {
    vec![
        // Read-only tools
        cp_list_clusters_tool(),
        cp_get_cluster_tool(),
        cp_list_listeners_tool(),
        cp_get_listener_tool(),
        cp_list_routes_tool(),
        cp_list_filters_tool(),
        cp_get_filter_tool(),
        // Cluster CRUD tools
        cp_create_cluster_tool(),
        cp_update_cluster_tool(),
        cp_delete_cluster_tool(),
        // Listener CRUD tools
        cp_create_listener_tool(),
        cp_update_listener_tool(),
        cp_delete_listener_tool(),
        // Route config CRUD tools
        cp_create_route_config_tool(),
        cp_update_route_config_tool(),
        cp_delete_route_config_tool(),
        // Filter CRUD tools
        cp_create_filter_tool(),
        cp_update_filter_tool(),
        cp_delete_filter_tool(),
    ]
}

/// Execute a tool by name (non-cluster tools only).
///
/// Routes tool execution to the appropriate handler based on tool name.
/// Note: Cluster operations are handled separately via the handler because
/// they use the internal API layer which requires XdsState.
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
        // Note: Cluster tools (cp_list_clusters, cp_get_cluster, cp_create_cluster,
        // cp_update_cluster, cp_delete_cluster) are handled in handler.rs using
        // the internal API layer with XdsState.
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
        // 7 read-only tools + 12 CRUD tools = 19 total
        assert_eq!(tools.len(), 19);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

        // Read-only tools
        assert!(tool_names.contains(&"cp_list_clusters"));
        assert!(tool_names.contains(&"cp_get_cluster"));
        assert!(tool_names.contains(&"cp_list_listeners"));
        assert!(tool_names.contains(&"cp_get_listener"));
        assert!(tool_names.contains(&"cp_list_routes"));
        assert!(tool_names.contains(&"cp_list_filters"));
        assert!(tool_names.contains(&"cp_get_filter"));

        // Cluster CRUD tools
        assert!(tool_names.contains(&"cp_create_cluster"));
        assert!(tool_names.contains(&"cp_update_cluster"));
        assert!(tool_names.contains(&"cp_delete_cluster"));

        // Listener CRUD tools
        assert!(tool_names.contains(&"cp_create_listener"));
        assert!(tool_names.contains(&"cp_update_listener"));
        assert!(tool_names.contains(&"cp_delete_listener"));

        // Route config CRUD tools
        assert!(tool_names.contains(&"cp_create_route_config"));
        assert!(tool_names.contains(&"cp_update_route_config"));
        assert!(tool_names.contains(&"cp_delete_route_config"));

        // Filter CRUD tools
        assert!(tool_names.contains(&"cp_create_filter"));
        assert!(tool_names.contains(&"cp_update_filter"));
        assert!(tool_names.contains(&"cp_delete_filter"));
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
