//! MCP Tools for Listener Control Plane Operations
//!
//! Provides tools for querying and inspecting listener configurations via the MCP protocol.
//!
//! The tools use the internal API layer (`ListenerOperations`) for unified
//! validation and team-based access control.

use crate::internal_api::{
    CreateListenerRequest as InternalCreateRequest, InternalAuthContext, ListListenersRequest,
    ListenerOperations, UpdateListenerRequest as InternalUpdateRequest,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::xds::filters::http::{HttpFilterConfigEntry, HttpFilterKind};
use crate::xds::listener::{FilterChainConfig, FilterConfig, FilterType, ListenerConfig};
use crate::xds::XdsState;
use serde_json::{json, Value};
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

// =============================================================================
// Execute Functions - Using Internal API Layer
// =============================================================================

/// Execute the cp_list_listeners tool.
///
/// Lists listeners with pagination, returning pretty-printed JSON output.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_listeners")]
pub async fn execute_list_listeners(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(50));
    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(0));

    tracing::debug!(team = %team, limit = ?limit, offset = ?offset, "Listing listeners for team");

    // Use internal API layer
    let ops = ListenerOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    let list_req = ListListenersRequest {
        limit,
        offset,
        include_defaults: true, /* MCP includes defaults */
    };

    let result = ops.list(list_req, &auth).await?;

    // Build output with listener summaries
    let listener_summaries: Vec<Value> = result
        .listeners
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
        "count": result.count,
        "limit": limit,
        "offset": offset,
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, listener_count = result.count, "Successfully listed listeners");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_get_listener tool.
///
/// Retrieves a specific listener by name, returning detailed configuration.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_listener")]
pub async fn execute_get_listener(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(team = %team, listener_name = %name, "Getting listener by name");

    // Use internal API layer
    let ops = ListenerOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    let listener = ops.get(name, &auth).await?;

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

    tracing::info!(team = %team, listener_name = %name, "Successfully retrieved listener");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_create_listener tool.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_create_listener")]
pub async fn execute_create_listener(
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
    let route_config_name = args.get("routeConfigName").and_then(|v| v.as_str());

    let filter_chains = if let Some(fc_json) = args.get("filterChains") {
        serde_json::from_value(fc_json.clone())
            .map_err(|e| McpError::InvalidParams(format!("Invalid filterChains: {}", e)))?
    } else if let Some(rc_name) = route_config_name {
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
        vec![]
    };

    let config = ListenerConfig {
        name: name.to_string(),
        address: address.clone(),
        port: port as u32,
        filter_chains,
    };

    // 3. Create via internal API layer
    let ops = ListenerOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    let create_req = InternalCreateRequest {
        name: name.to_string(),
        address,
        port,
        protocol: Some(protocol),
        team: Some(team.to_string()),
        config,
    };

    let result = ops.create(create_req, &auth).await?;
    let created = result.data;

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
        "message": result.message.unwrap_or_else(|| format!(
            "Listener '{}' created successfully. xDS configuration has been refreshed.",
            created.name
        )),
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
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_update_listener")]
pub async fn execute_update_listener(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse listener name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(team = %team, listener_name = %name, "Updating listener via MCP");

    // 2. Use internal API layer
    let ops = ListenerOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    // 3. Get existing listener to build config
    let existing = ops.get(name, &auth).await?;

    // 4. Parse existing configuration
    let mut config: ListenerConfig = serde_json::from_str(&existing.configuration)
        .map_err(|e| McpError::InvalidParams(format!("Failed to parse listener config: {}", e)))?;

    // 5. Apply updates to config
    if let Some(fc_json) = args.get("filterChains") {
        config.filter_chains = serde_json::from_value(fc_json.clone())
            .map_err(|e| McpError::InvalidParams(format!("Invalid filterChains: {}", e)))?;
    }

    let address = args.get("address").and_then(|v| v.as_str()).map(|s| s.to_string());
    let port = args.get("port").and_then(|v| v.as_u64()).map(|p| p as u16);
    let protocol = args.get("protocol").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Update config address/port if provided
    if let Some(ref addr) = address {
        config.address = addr.clone();
    }
    if let Some(p) = port {
        config.port = p as u32;
    }

    let update_req = InternalUpdateRequest { address, port, protocol, config };

    let result = ops.update(name, update_req, &auth).await?;
    let updated = result.data;

    // 6. Format success response
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
        "message": result.message.unwrap_or_else(|| format!(
            "Listener '{}' updated successfully. xDS configuration has been refreshed.",
            updated.name
        )),
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
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_delete_listener")]
pub async fn execute_delete_listener(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse listener name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(team = %team, listener_name = %name, "Deleting listener via MCP");

    // 2. Delete via internal API layer
    let ops = ListenerOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    let result = ops.delete(name, &auth).await?;

    // 3. Format success response
    let output = json!({
        "success": true,
        "message": result.message.unwrap_or_else(|| format!(
            "Listener '{}' deleted successfully. xDS configuration has been refreshed.",
            name
        )),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, listener_name = %name, "Successfully deleted listener via MCP");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_listeners_tool_definition() {
        let tool = cp_list_listeners_tool();
        assert_eq!(tool.name, "cp_list_listeners");
        assert!(tool.description.contains("List all listeners"));
        assert!(tool.input_schema.get("properties").is_some());
    }

    #[test]
    fn test_cp_get_listener_tool_definition() {
        let tool = cp_get_listener_tool();
        assert_eq!(tool.name, "cp_get_listener");
        assert!(tool.description.contains("Get detailed information"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[test]
    fn test_cp_create_listener_tool_definition() {
        let tool = cp_create_listener_tool();
        assert_eq!(tool.name, "cp_create_listener");
        assert!(tool.description.contains("Create a new listener"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[test]
    fn test_cp_update_listener_tool_definition() {
        let tool = cp_update_listener_tool();
        assert_eq!(tool.name, "cp_update_listener");
        assert!(tool.description.contains("Update an existing"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[test]
    fn test_cp_delete_listener_tool_definition() {
        let tool = cp_delete_listener_tool();
        assert_eq!(tool.name, "cp_delete_listener");
        assert!(tool.description.contains("Delete a listener"));
        assert!(tool.input_schema.get("required").is_some());
    }
}
