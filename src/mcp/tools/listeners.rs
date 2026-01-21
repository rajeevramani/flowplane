//! MCP Tools for Listener Control Plane Operations
//!
//! Provides tools for querying and inspecting listener configurations via the MCP protocol.

use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::services::ListenerService;
use crate::storage::repositories::listener::ListenerRepository;
use crate::xds::filters::http::{HttpFilterConfigEntry, HttpFilterKind};
use crate::xds::listener::{FilterChainConfig, FilterConfig, FilterType, ListenerConfig};
use crate::xds::XdsState;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::instrument;

/// Returns the MCP tool definition for listing listeners.
///
/// This tool supports pagination via `limit` and `offset` parameters.
pub fn cp_list_listeners_tool() -> Tool {
    Tool {
        name: "cp_list_listeners".to_string(),
        description: r#"List all listeners in the Flowplane control plane.

RESOURCE ORDER: Listeners are the final resource (order 4 of 4).
Create listeners AFTER clusters and route configurations exist.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
  [Filters]  ───────────┘                      ▲
       └───────────────────────────────────────┘
                                          you are here

PURPOSE: Discover existing network entry points to:
- See which ports are already in use
- Find listeners for specific services
- Understand the complete traffic path

RETURNS: Array of listener summaries with:
- name: Unique listener identifier
- address: Bind address (e.g., "0.0.0.0" for all interfaces)
- port: Listen port number
- protocol: HTTP, HTTPS, or TCP
- team: Owning team
- version: Configuration version

TRAFFIC FLOW:
  Client → [Listener:port] → [Route Config] → [Cluster] → Backend

RELATED TOOLS: cp_get_listener (details), cp_create_listener (create), cp_list_route_configs (routes)"#.to_string(),
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
        description: r#"Get detailed information about a specific listener by name.

PURPOSE: Retrieve complete listener configuration including filter chains and attached routes.

RETURNS:
- id: Internal listener identifier
- name: Unique listener name
- address: Bind address (IP or socket path)
- port: Listen port (null for Unix sockets)
- protocol: HTTP, HTTPS, or TCP
- configuration: Complete config including filter chains
- team: Owning team
- version: For optimistic locking

CONFIGURATION DETAILS:
- filterChains: Network filter pipeline (usually HTTP connection manager)
- routeConfigName: Route configuration bound to this listener (in filter chain)
- httpFilters: HTTP-level filters applied to requests

WHEN TO USE:
- Before updating a listener
- To verify which route config is attached
- To check filter chain configuration
- To understand complete traffic processing pipeline

TRAFFIC FLOW CONTEXT:
This listener receives traffic on address:port, processes through filter chains,
matches routes in the attached route config, and forwards to clusters.

RELATED TOOLS: cp_list_listeners (discovery), cp_update_listener (modify), cp_create_listener (create)"#.to_string(),
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

// =============================================================================
// CRUD Tools (Create, Update, Delete)
// =============================================================================

/// Returns the MCP tool definition for creating a listener.
pub fn cp_create_listener_tool() -> Tool {
    Tool {
        name: "cp_create_listener".to_string(),
        description: r#"Create a new listener (network entry point) in the Flowplane control plane.

RESOURCE ORDER: Listeners are the FINAL resource (order 4 of 4).
PREREQUISITE: Route configurations MUST exist first.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
  [Filters]  ───────────┘                      ▲
                                          you are here

COMPLETE API CREATION WORKFLOW:
1. cp_create_cluster - Create backend service(s)
2. cp_create_filter - Create filters if needed (optional)
3. cp_create_route_config - Create routes referencing clusters
4. cp_create_listener - Create listener bound to route config (THIS STEP)

A listener defines where Envoy accepts incoming traffic:
- address: Network interface to bind (0.0.0.0 for all)
- port: TCP port number
- routeConfigName: Name of the route configuration to bind (REQUIRED for traffic to flow)

After creation, xDS configuration is pushed to Envoy proxies.
Traffic flow: Client → Listener:port → RouteConfig → Cluster → Backend

SIMPLE EXAMPLE (RECOMMENDED for AI agents):
{
  "name": "api-listener",
  "port": 8080,
  "routeConfigName": "api-routes"
}

EXAMPLE WITH ALL OPTIONS:
{
  "name": "api-listener",
  "address": "0.0.0.0",
  "port": 8080,
  "protocol": "HTTP",
  "routeConfigName": "api-routes"
}

ADVANCED: For custom filter chains (TLS, tracing, etc.), use filterChains parameter instead of routeConfigName.

Authorization: Requires cp:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique name for the listener (e.g., 'api-gateway', 'admin-listener')"
                },
                "address": {
                    "type": "string",
                    "description": "IP address to bind (e.g., '0.0.0.0' for all interfaces, '127.0.0.1' for localhost)",
                    "default": "0.0.0.0"
                },
                "port": {
                    "type": "integer",
                    "description": "Port number to listen on (1-65535)",
                    "minimum": 1,
                    "maximum": 65535
                },
                "protocol": {
                    "type": "string",
                    "description": "Protocol type",
                    "enum": ["HTTP", "HTTPS", "TCP"],
                    "default": "HTTP"
                },
                "routeConfigName": {
                    "type": "string",
                    "description": "Name of the route configuration to bind to this listener. REQUIRED for traffic routing. The route config must exist before creating the listener."
                },
                "filterChains": {
                    "type": "array",
                    "description": "Advanced: Custom filter chain configurations. Use this instead of routeConfigName for complex setups (TLS, tracing, etc.).",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Filter chain name" },
                            "filters": {
                                "type": "array",
                                "description": "Network filters (typically http_connection_manager)",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "filter_type": { "type": "object" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "required": ["name", "port"]
        }),
    }
}

/// Returns the MCP tool definition for updating a listener.
pub fn cp_update_listener_tool() -> Tool {
    Tool {
        name: "cp_update_listener".to_string(),
        description: r#"Update an existing listener's configuration.

PURPOSE: Modify listener settings such as port, address, protocol, or filter chains.
Changes are automatically pushed to Envoy via xDS.

PARTIAL UPDATE: Only specified fields are updated. Unspecified fields retain current values.
EXCEPTION: filterChains is a FULL REPLACEMENT if provided.

COMMON USE CASES:
- Change listen port
- Update bind address
- Switch protocol (HTTP to HTTPS)
- Modify filter chain configuration
- Bind to a different route config

WARNING: Changing port or address may cause brief traffic disruption.

Required Parameters:
- name: Name of the listener to update (cannot be changed)

Optional Parameters:
- address: New bind address
- port: New port number (1-65535)
- protocol: HTTP, HTTPS, or TCP
- filterChains: New filter chain configuration (REPLACES existing)

TIP: Use cp_get_listener first to see current configuration.

Authorization: Requires cp:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the listener to update (cannot be changed)"
                },
                "address": {
                    "type": "string",
                    "description": "New IP address to bind"
                },
                "port": {
                    "type": "integer",
                    "description": "New port number (1-65535)",
                    "minimum": 1,
                    "maximum": 65535
                },
                "protocol": {
                    "type": "string",
                    "description": "New protocol type",
                    "enum": ["HTTP", "HTTPS", "TCP"]
                },
                "filterChains": {
                    "type": "array",
                    "description": "New filter chain configurations (REPLACES existing completely)"
                }
            },
            "required": ["name"]
        }),
    }
}

/// Returns the MCP tool definition for deleting a listener.
pub fn cp_delete_listener_tool() -> Tool {
    Tool {
        name: "cp_delete_listener".to_string(),
        description: r#"Delete a listener from the Flowplane control plane.

DELETION ORDER: Listeners are deleted FIRST in the reverse dependency order.

ORDER: [Listeners] ─► [Route Configs] ─► [Clusters/Filters]
            ▲
       delete first

SAFE TO DELETE: Listeners can be deleted without affecting route configs or clusters.
Deleting a listener just removes the network entry point.

WILL FAIL IF:
- Listener name is "default-gateway-listener" (system listener)

EFFECT:
- Traffic to this address:port will no longer be accepted
- Associated route config and clusters remain intact
- Can recreate the listener to restore traffic

WORKFLOW TO FULLY REMOVE AN API:
1. Delete listener (stops traffic) - THIS STEP
2. Delete route config (removes routing)
3. Delete clusters (removes backends)
4. Delete filters (removes policies)

Required Parameters:
- name: Name of the listener to delete

Authorization: Requires cp:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the listener to delete"
                }
            },
            "required": ["name"]
        }),
    }
}

/// Execute the cp_create_listener tool.
#[instrument(skip(_db_pool, xds_state, args), fields(team = %team), name = "mcp_execute_create_listener")]
pub async fn execute_create_listener(
    _db_pool: &SqlitePool,
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    let port =
        args.get("port").and_then(|v| v.as_u64()).ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: port".to_string())
        })? as u16;

    let address = args.get("address").and_then(|v| v.as_str()).unwrap_or("0.0.0.0").to_string();

    let protocol = args.get("protocol").and_then(|v| v.as_str()).unwrap_or("HTTP").to_string();

    tracing::debug!(
        team = %team,
        listener_name = %name,
        address = %address,
        port = %port,
        protocol = %protocol,
        "Creating listener via MCP"
    );

    // 2. Build ListenerConfig
    // Parse optional routeConfigName for simple binding
    let route_config_name = args.get("routeConfigName").and_then(|v| v.as_str());

    let filter_chains = if let Some(fc_json) = args.get("filterChains") {
        // Use custom filter chains if provided
        serde_json::from_value(fc_json.clone())
            .map_err(|e| McpError::InvalidParams(format!("Invalid filterChains: {}", e)))?
    } else if let Some(rc_name) = route_config_name {
        // Build default HTTP filter chain with route config binding
        vec![FilterChainConfig {
            name: Some("default".to_string()),
            filters: vec![FilterConfig {
                name: "envoy.filters.network.http_connection_manager".to_string(),
                filter_type: FilterType::HttpConnectionManager {
                    route_config_name: Some(rc_name.to_string()),
                    inline_route_config: None,
                    access_log: None,
                    tracing: None,
                    http_filters: vec![HttpFilterConfigEntry {
                        name: None,
                        is_optional: false,
                        disabled: false,
                        filter: HttpFilterKind::Router,
                    }],
                },
            }],
            tls_context: None,
        }]
    } else {
        // No filter chains and no route config - create empty (will need separate binding)
        vec![]
    };

    let config = ListenerConfig {
        name: name.to_string(),
        address: address.clone(),
        port: port as u32,
        filter_chains,
    };

    // 3. Create via service layer
    let listener_service = ListenerService::new(xds_state.clone());
    let created = listener_service
        .create_listener(name.to_string(), address, port, protocol, config, Some(team.to_string()))
        .await
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("already exists") || err_str.contains("UNIQUE constraint") {
                McpError::Conflict(format!("Listener '{}' already exists", name))
            } else if err_str.contains("validation") {
                McpError::ValidationError(err_str)
            } else {
                McpError::InternalError(format!("Failed to create listener: {}", e))
            }
        })?;

    // 4. Format success response
    let output = json!({
        "success": true,
        "listener": {
            "id": created.id.to_string(),
            "name": created.name,
            "address": created.address,
            "port": created.port,
            "protocol": created.protocol,
            "team": created.team,
            "version": created.version,
            "createdAt": created.created_at.to_rfc3339(),
        },
        "message": format!(
            "Listener '{}' created successfully. xDS configuration has been refreshed.",
            created.name
        ),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        listener_name = %created.name,
        listener_id = %created.id,
        "Successfully created listener via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_update_listener tool.
#[instrument(skip(db_pool, xds_state, args), fields(team = %team), name = "mcp_execute_update_listener")]
pub async fn execute_update_listener(
    db_pool: &SqlitePool,
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse listener name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        listener_name = %name,
        "Updating listener via MCP"
    );

    // 2. Get existing listener
    let repo = ListenerRepository::new(db_pool.clone());
    let existing = repo.get_by_name(name).await.map_err(|e| {
        if e.to_string().contains("not found") {
            McpError::ResourceNotFound(format!("Listener '{}' not found", name))
        } else {
            McpError::InternalError(format!("Failed to get listener: {}", e))
        }
    })?;

    // 3. Verify team ownership
    if !team.is_empty() {
        if let Some(listener_team) = &existing.team {
            if listener_team != team {
                return Err(McpError::Forbidden(format!(
                    "Cannot update listener '{}' owned by team '{}'",
                    name, listener_team
                )));
            }
        }
    }

    // 4. Parse existing configuration
    let mut config: ListenerConfig = serde_json::from_str(&existing.configuration)
        .map_err(|e| McpError::InternalError(format!("Failed to parse listener config: {}", e)))?;

    // 5. Apply updates
    let address = args
        .get("address")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| existing.address.clone());

    let port = args
        .get("port")
        .and_then(|v| v.as_u64())
        .map(|p| p as u16)
        .unwrap_or(existing.port.unwrap_or(80) as u16);

    let protocol = args
        .get("protocol")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| existing.protocol.clone());

    if let Some(fc_json) = args.get("filterChains") {
        config.filter_chains = serde_json::from_value(fc_json.clone())
            .map_err(|e| McpError::InvalidParams(format!("Invalid filterChains: {}", e)))?;
    }

    config.address = address.clone();
    config.port = port as u32;

    // 6. Update via service layer
    let listener_service = ListenerService::new(xds_state.clone());
    let updated = listener_service
        .update_listener(name, address, port, protocol, config)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to update listener: {}", e)))?;

    // 7. Format success response
    let output = json!({
        "success": true,
        "listener": {
            "id": updated.id.to_string(),
            "name": updated.name,
            "address": updated.address,
            "port": updated.port,
            "protocol": updated.protocol,
            "team": updated.team,
            "version": updated.version,
            "updatedAt": updated.updated_at.to_rfc3339(),
        },
        "message": format!(
            "Listener '{}' updated successfully. xDS configuration has been refreshed.",
            updated.name
        ),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        listener_name = %updated.name,
        listener_id = %updated.id,
        "Successfully updated listener via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_delete_listener tool.
#[instrument(skip(db_pool, xds_state, args), fields(team = %team), name = "mcp_execute_delete_listener")]
pub async fn execute_delete_listener(
    db_pool: &SqlitePool,
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse listener name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        listener_name = %name,
        "Deleting listener via MCP"
    );

    // 2. Get existing listener to verify ownership
    let repo = ListenerRepository::new(db_pool.clone());
    let existing = repo.get_by_name(name).await.map_err(|e| {
        if e.to_string().contains("not found") {
            McpError::ResourceNotFound(format!("Listener '{}' not found", name))
        } else {
            McpError::InternalError(format!("Failed to get listener: {}", e))
        }
    })?;

    // 3. Verify team ownership
    if !team.is_empty() {
        if let Some(listener_team) = &existing.team {
            if listener_team != team {
                return Err(McpError::Forbidden(format!(
                    "Cannot delete listener '{}' owned by team '{}'",
                    name, listener_team
                )));
            }
        }
    }

    // 4. Delete via service layer
    let listener_service = ListenerService::new(xds_state.clone());
    listener_service.delete_listener(name).await.map_err(|e| {
        let err_str = e.to_string();
        if err_str.contains("default gateway") {
            McpError::Forbidden(err_str)
        } else {
            McpError::InternalError(format!("Failed to delete listener: {}", e))
        }
    })?;

    // 5. Format success response
    let output = json!({
        "success": true,
        "message": format!(
            "Listener '{}' deleted successfully. xDS configuration has been refreshed.",
            name
        ),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        listener_name = %name,
        "Successfully deleted listener via MCP"
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
