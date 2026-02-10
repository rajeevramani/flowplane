//! MCP Tools Module
//!
//! Provides Control Plane (CP) tools for querying Flowplane configuration via MCP protocol.
//! Each tool allows AI assistants to inspect and query the control plane state.

use crate::domain::OrgId;

pub mod clusters;
pub mod dataplanes;
pub mod devops_agent;
pub mod filter_types;
pub mod filters;
pub mod learning;
pub mod listeners;
pub mod openapi;
pub mod routes;
pub mod schemas;
pub mod virtual_hosts;

// Re-export tool definitions for convenience
pub use clusters::{cp_get_cluster_health_tool, cp_get_cluster_tool, cp_list_clusters_tool};
pub use clusters::{execute_get_cluster, execute_get_cluster_health, execute_list_clusters};

// Re-export cluster CRUD tools
pub use clusters::{cp_create_cluster_tool, cp_delete_cluster_tool, cp_update_cluster_tool};
pub use clusters::{execute_create_cluster, execute_delete_cluster, execute_update_cluster};

pub use filters::{cp_get_filter_tool, cp_list_filters_tool};
pub use filters::{execute_get_filter, execute_list_filters};

// Re-export filter CRUD tools
pub use filters::{cp_create_filter_tool, cp_delete_filter_tool, cp_update_filter_tool};
pub use filters::{execute_create_filter, execute_delete_filter, execute_update_filter};

// Re-export filter attachment tools
pub use filters::{cp_attach_filter_tool, cp_detach_filter_tool, cp_list_filter_attachments_tool};
pub use filters::{execute_attach_filter, execute_detach_filter, execute_list_filter_attachments};

pub use listeners::{
    cp_get_listener_status_tool, cp_get_listener_tool, cp_list_listeners_tool, cp_query_port_tool,
};
pub use listeners::{
    execute_get_listener, execute_get_listener_status, execute_list_listeners, execute_query_port,
};

// Re-export listener CRUD tools
pub use listeners::{cp_create_listener_tool, cp_delete_listener_tool, cp_update_listener_tool};
pub use listeners::{execute_create_listener, execute_delete_listener, execute_update_listener};

pub use routes::{cp_list_routes_tool, cp_query_path_tool};
pub use routes::{execute_list_routes, execute_query_path};

// Re-export route config CRUD tools
pub use routes::{
    cp_create_route_config_tool, cp_delete_route_config_tool, cp_update_route_config_tool,
};
pub use routes::{
    execute_create_route_config, execute_delete_route_config, execute_update_route_config,
};

// Re-export individual route CRUD tools
pub use routes::{
    cp_create_route_tool, cp_delete_route_tool, cp_get_route_tool, cp_update_route_tool,
};
pub use routes::{
    execute_create_route, execute_delete_route, execute_get_route, execute_update_route,
};

// Re-export virtual host tools
pub use virtual_hosts::{
    cp_create_virtual_host_tool, cp_delete_virtual_host_tool, cp_get_virtual_host_tool,
    cp_list_virtual_hosts_tool, cp_update_virtual_host_tool,
};
pub use virtual_hosts::{
    execute_create_virtual_host, execute_delete_virtual_host, execute_get_virtual_host,
    execute_list_virtual_hosts, execute_update_virtual_host,
};

// Re-export aggregated schema tools
pub use schemas::{cp_get_aggregated_schema_tool, cp_list_aggregated_schemas_tool};
pub use schemas::{execute_get_aggregated_schema, execute_list_aggregated_schemas};

// Re-export learning session tools
pub use learning::{
    cp_create_learning_session_tool, cp_delete_learning_session_tool, cp_get_learning_session_tool,
    cp_list_learning_sessions_tool,
};
pub use learning::{
    execute_create_learning_session, execute_delete_learning_session, execute_get_learning_session,
    execute_list_learning_sessions,
};

// Re-export OpenAPI import tools
pub use openapi::{cp_get_openapi_import_tool, cp_list_openapi_imports_tool};
pub use openapi::{execute_get_openapi_import, execute_list_openapi_imports};

// Re-export dataplane tools
pub use dataplanes::{
    cp_create_dataplane_tool, cp_delete_dataplane_tool, cp_get_dataplane_tool,
    cp_list_dataplanes_tool, cp_update_dataplane_tool,
};
pub use dataplanes::{
    execute_create_dataplane, execute_delete_dataplane, execute_get_dataplane,
    execute_list_dataplanes, execute_update_dataplane,
};

// Re-export filter type tools
pub use filter_types::{cp_get_filter_type_tool, cp_list_filter_types_tool};
pub use filter_types::{execute_get_filter_type, execute_list_filter_types};

// Re-export DevOps agent tools
pub use devops_agent::devops_get_deployment_status_tool;
pub use devops_agent::execute_devops_get_deployment_status;

use crate::mcp::error::McpError;
use crate::mcp::protocol::{Tool, ToolCallResult};
use crate::storage::DbPool;
use serde_json::Value;

/// Get all available MCP tools.
///
/// Returns a vector of all tool definitions that can be exposed to MCP clients.
/// Includes both read-only tools (cp_list_*, cp_get_*) and CRUD tools (cp_create_*, cp_update_*, cp_delete_*).
pub fn get_all_tools() -> Vec<Tool> {
    vec![
        // Read-only tools
        cp_list_clusters_tool(),
        cp_get_cluster_tool(),
        cp_get_cluster_health_tool(),
        cp_list_listeners_tool(),
        cp_get_listener_tool(),
        cp_query_port_tool(),
        cp_get_listener_status_tool(),
        cp_list_routes_tool(),
        cp_get_route_tool(),
        cp_query_path_tool(),
        cp_list_filters_tool(),
        cp_get_filter_tool(),
        cp_list_virtual_hosts_tool(),
        cp_get_virtual_host_tool(),
        cp_list_aggregated_schemas_tool(),
        cp_get_aggregated_schema_tool(),
        cp_list_learning_sessions_tool(),
        cp_get_learning_session_tool(),
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
        // Individual route CRUD tools
        cp_create_route_tool(),
        cp_update_route_tool(),
        cp_delete_route_tool(),
        // Virtual host CRUD tools
        cp_create_virtual_host_tool(),
        cp_update_virtual_host_tool(),
        cp_delete_virtual_host_tool(),
        // Filter CRUD tools
        cp_create_filter_tool(),
        cp_update_filter_tool(),
        cp_delete_filter_tool(),
        // Filter attachment tools
        cp_attach_filter_tool(),
        cp_detach_filter_tool(),
        cp_list_filter_attachments_tool(),
        // Learning session tools
        cp_create_learning_session_tool(),
        cp_delete_learning_session_tool(),
        // OpenAPI import tools
        cp_list_openapi_imports_tool(),
        cp_get_openapi_import_tool(),
        // Dataplane CRUD tools
        cp_list_dataplanes_tool(),
        cp_get_dataplane_tool(),
        cp_create_dataplane_tool(),
        cp_update_dataplane_tool(),
        cp_delete_dataplane_tool(),
        // Filter type tools
        cp_list_filter_types_tool(),
        cp_get_filter_type_tool(),
        // DevOps agent workflow tools
        devops_get_deployment_status_tool(),
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
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    match tool_name {
        // Note: Cluster, Listener, and Filter tools are handled in handler.rs
        // using the internal API layer with XdsState.
        // Direct db_pool tools query tables directly for efficiency.
        "cp_list_routes" => execute_list_routes(db_pool, team, org_id, args).await,
        // Query-first tools (direct db_pool access for efficiency)
        "cp_query_port" => execute_query_port(db_pool, team, org_id, args).await,
        "cp_query_path" => execute_query_path(db_pool, team, org_id, args).await,
        _ => Err(McpError::ToolNotFound(format!("Unknown tool: {}", tool_name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_all_tools() {
        let tools = get_all_tools();
        // 14 read-only tools + 18 CRUD tools + 3 filter attachment + 2 learning session + 2 openapi + 5 dataplane + 2 filter types + 1 devops + 2 query-first + 2 status = 51 total
        assert_eq!(tools.len(), 51);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

        // Read-only tools
        assert!(tool_names.contains(&"cp_list_clusters"));
        assert!(tool_names.contains(&"cp_get_cluster"));
        assert!(tool_names.contains(&"cp_list_listeners"));
        assert!(tool_names.contains(&"cp_get_listener"));
        assert!(tool_names.contains(&"cp_list_routes"));
        assert!(tool_names.contains(&"cp_get_route"));
        assert!(tool_names.contains(&"cp_list_filters"));
        assert!(tool_names.contains(&"cp_get_filter"));
        assert!(tool_names.contains(&"cp_list_virtual_hosts"));
        assert!(tool_names.contains(&"cp_get_virtual_host"));
        assert!(tool_names.contains(&"cp_list_aggregated_schemas"));
        assert!(tool_names.contains(&"cp_get_aggregated_schema"));

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

        // Individual route CRUD tools
        assert!(tool_names.contains(&"cp_create_route"));
        assert!(tool_names.contains(&"cp_update_route"));
        assert!(tool_names.contains(&"cp_delete_route"));

        // Virtual host CRUD tools
        assert!(tool_names.contains(&"cp_create_virtual_host"));
        assert!(tool_names.contains(&"cp_update_virtual_host"));
        assert!(tool_names.contains(&"cp_delete_virtual_host"));

        // Filter CRUD tools
        assert!(tool_names.contains(&"cp_create_filter"));
        assert!(tool_names.contains(&"cp_update_filter"));
        assert!(tool_names.contains(&"cp_delete_filter"));

        // Filter attachment tools
        assert!(tool_names.contains(&"cp_attach_filter"));
        assert!(tool_names.contains(&"cp_detach_filter"));
        assert!(tool_names.contains(&"cp_list_filter_attachments"));

        // Learning session tools
        assert!(tool_names.contains(&"cp_list_learning_sessions"));
        assert!(tool_names.contains(&"cp_get_learning_session"));
        assert!(tool_names.contains(&"cp_create_learning_session"));
        assert!(tool_names.contains(&"cp_delete_learning_session"));
    }

    #[tokio::test]
    async fn test_execute_tool_unknown() {
        use crate::storage::test_helpers::TestDatabase;

        let _db = TestDatabase::new("mcp_tools_mod").await;
        let pool = _db.pool.clone();

        let result =
            execute_tool("unknown_tool", &pool, "test-team", None, serde_json::json!({})).await;

        assert!(result.is_err());
        if let Err(McpError::ToolNotFound(msg)) = result {
            assert!(msg.contains("Unknown tool"));
        } else {
            panic!("Expected ToolNotFound error");
        }
    }
}
