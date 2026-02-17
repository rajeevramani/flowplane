//! MCP Tools for Listener Control Plane Operations
//!
//! Provides tools for querying and inspecting listener configurations via the MCP protocol.
//!
//! The tools use the internal API layer (`ListenerOperations`) for unified
//! validation and team-based access control.

use crate::domain::OrgId;
use crate::internal_api::{
    CreateListenerRequest as InternalCreateRequest, InternalAuthContext, ListListenersRequest,
    ListenerOperations, UpdateListenerRequest as InternalUpdateRequest,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::mcp::response_builders::{
    build_delete_response, build_query_response, build_rich_create_response, build_update_response,
    ResourceRef,
};
use crate::storage::DbPool;
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
    Tool::new(
        "cp_list_listeners",
        r#"List all listeners in the Flowplane control plane.

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

RELATED TOOLS: cp_get_listener (details), cp_create_listener (create), cp_list_route_configs (routes)"#,
        json!({
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
    )
}

/// Returns the MCP tool definition for getting a listener by name.
///
/// Requires a `name` parameter to identify the listener.
pub fn cp_get_listener_tool() -> Tool {
    Tool::new(
        "cp_get_listener",
        r#"Get detailed information about a specific listener by name.

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

RELATED TOOLS: cp_list_listeners (discovery), cp_update_listener (modify), cp_create_listener (create)"#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the listener to retrieve"
                }
            },
            "required": ["name"]
        }),
    )
}

/// Returns the MCP tool definition for creating a listener.
pub fn cp_create_listener_tool() -> Tool {
    Tool::new(
        "cp_create_listener",
        r#"Create a new listener (network entry point) in the Flowplane control plane.

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

Authorization: Requires cp:write scope."#,
        json!({
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
                },
                "dataplaneId": {
                    "type": "string",
                    "description": "ID of the dataplane this listener belongs to. Required for MCP tool execution routing. Create a dataplane first if none exist."
                }
            },
            "required": ["name", "port", "dataplaneId"]
        }),
    )
}

/// Returns the MCP tool definition for updating a listener.
pub fn cp_update_listener_tool() -> Tool {
    Tool::new(
        "cp_update_listener",
        r#"Update an existing listener's configuration.

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
- routeConfigName: Name of the route configuration to bind (updates the HttpConnectionManager filter)
- address: New bind address
- port: New port number (1-65535)
- protocol: HTTP, HTTPS, or TCP
- filterChains: New filter chain configuration (REPLACES existing)
- dataplaneId: ID of dataplane to assign listener to (for MCP tool gateway routing)

NOTE: routeConfigName and filterChains are mutually exclusive. Use routeConfigName for simple
route binding. Use filterChains for advanced configurations (TLS, tracing, etc.).

TIP: Use cp_get_listener first to see current configuration.

Authorization: Requires cp:write scope."#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the listener to update (cannot be changed)"
                },
                "routeConfigName": {
                    "type": "string",
                    "description": "Name of the route configuration to bind to this listener. Updates the route_config_name in the HttpConnectionManager filter."
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
                },
                "dataplaneId": {
                    "type": "string",
                    "description": "ID of the dataplane to assign this listener to. The dataplane must exist and belong to the same team."
                }
            },
            "required": ["name"]
        }),
    )
}

/// Returns the MCP tool definition for deleting a listener.
pub fn cp_delete_listener_tool() -> Tool {
    Tool::new(
        "cp_delete_listener",
        r#"Delete a listener from the Flowplane control plane.

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

Authorization: Requires cp:write scope."#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the listener to delete"
                }
            },
            "required": ["name"]
        }),
    )
}

/// Returns the MCP tool definition for querying port availability.
///
/// This is a query-first tool that checks if a port is already in use.
pub fn cp_query_port_tool() -> Tool {
    Tool::new(
        "cp_query_port",
        r#"Check if a port is already in use by a listener.

PURPOSE: Query-first design - check state before creating listeners.

RETURNS:
- Not found: {"found": false} (port available)
- Found: {"found": true, "ref": {type, name, id}, "data": {address, route_config}}

EXAMPLE:
  cp_query_port({"port": 8080})
  → {"found": true, "ref": {"type": "listener", "name": "api-gateway", "id": "l-123"},
     "data": {"address": "0.0.0.0", "route_config": "api-routes"}}

USE CASE:
  Before creating listener: cp_query_port → if found, reuse or choose different port

TOKEN BUDGET: <80 tokens response

Authorization: Requires listeners:read or cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "port": {
                    "type": "integer",
                    "description": "Port number to check (1-65535)",
                    "minimum": 1,
                    "maximum": 65535
                }
            },
            "required": ["port"]
        }),
    )
}

/// Returns the MCP tool definition for getting listener status.
///
/// Returns status information including route config count.
pub fn cp_get_listener_status_tool() -> Tool {
    Tool::new(
        "cp_get_listener_status",
        r#"Get status information for a specific listener.

PURPOSE: Check listener health and configuration before modifications.

RETURNS: Status summary with:
- active_connections: Active connection count (placeholder for future metrics)
- route_config_count: Number of route configs attached
- address: Bind address
- port: Listen port
- protocol: Protocol type

TOKEN BUDGET: <80 tokens response

Authorization: Requires listeners:read or cp:read scope."#
            .to_string(),
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the listener to get status for"
                }
            },
            "required": ["name"]
        }),
    )
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
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(50));
    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(0));

    tracing::debug!(team = %team, limit = ?limit, offset = ?offset, "Listing listeners for team");

    // Use internal API layer
    let ops = ListenerOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;
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
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(team = %team, listener_name = %name, "Getting listener by name");

    // Use internal API layer
    let ops = ListenerOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;
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
    org_id: Option<&OrgId>,
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
    let dataplane_id = args
        .get("dataplaneId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            McpError::InvalidParams(
                "Missing required parameter: dataplaneId - create a dataplane first".to_string(),
            )
        })?
        .to_string();

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
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;
    let create_req = InternalCreateRequest {
        name: name.to_string(),
        address,
        port,
        protocol: Some(protocol),
        team: Some(team.to_string()),
        config,
        dataplane_id,
    };

    let result = ops.create(create_req, &auth).await?;
    let created = result.data;

    // 4. Format rich response — listener is the final deployment step
    let mut bound = json!({
        "address": args["address"].as_str().unwrap_or("0.0.0.0"),
        "port": port
    });
    if let Some(rc_name) = route_config_name {
        bound["route_config"] = json!(rc_name);
    }

    let output = build_rich_create_response(
        "listener",
        &created.name,
        created.id.as_ref(),
        Some(bound),
        None,
        Some("Deployment complete. Verify with ops_trace_request or devops_get_deployment_status."),
    );

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

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
    org_id: Option<&OrgId>,
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
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    // 3. Get existing listener to build config
    let existing = ops.get(name, &auth).await?;

    // 4. Parse existing configuration
    let mut config: ListenerConfig = serde_json::from_str(&existing.configuration)
        .map_err(|e| McpError::InvalidParams(format!("Failed to parse listener config: {}", e)))?;

    // 5. Apply updates to config
    let route_config_name = args.get("routeConfigName").and_then(|v| v.as_str());

    if let Some(fc_json) = args.get("filterChains") {
        config.filter_chains = serde_json::from_value(fc_json.clone())
            .map_err(|e| McpError::InvalidParams(format!("Invalid filterChains: {}", e)))?;
    } else if let Some(rc_name) = route_config_name {
        // Update the route_config_name in the existing HttpConnectionManager filter.
        // If no HCM filter exists yet, create a default filter chain with one.
        let mut found = false;
        for fc in &mut config.filter_chains {
            for filter in &mut fc.filters {
                if let FilterType::HttpConnectionManager { route_config_name: ref mut rcn, .. } =
                    filter.filter_type
                {
                    *rcn = Some(rc_name.to_string());
                    found = true;
                }
            }
        }
        if !found {
            // No existing HCM filter — create a default filter chain
            config.filter_chains = vec![FilterChainConfig {
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
            }];
        }
    }

    let address = args.get("address").and_then(|v| v.as_str()).map(|s| s.to_string());
    let port = args.get("port").and_then(|v| v.as_u64()).map(|p| p as u16);
    let protocol = args.get("protocol").and_then(|v| v.as_str()).map(|s| s.to_string());
    let dataplane_id = args.get("dataplaneId").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Update config address/port if provided
    if let Some(ref addr) = address {
        config.address = addr.clone();
    }
    if let Some(p) = port {
        config.port = p as u32;
    }

    let update_req = InternalUpdateRequest { address, port, protocol, config, dataplane_id };

    let result = ops.update(name, update_req, &auth).await?;
    let updated = result.data;

    // 6. Format success response (minimal token-efficient format)
    let output = build_update_response("listener", &updated.name, updated.id.as_ref());

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

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
    org_id: Option<&OrgId>,
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
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;
    ops.delete(name, &auth).await?;

    // 3. Format success response (minimal token-efficient format)
    let output = build_delete_response();

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, listener_name = %name, "Successfully deleted listener via MCP");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_query_port tool.
///
/// Query-first tool that checks if a port is already in use by a listener.
/// Returns minimal response format for token efficiency.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_query_port")]
pub async fn execute_query_port(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let port =
        args.get("port").and_then(|v| v.as_i64()).ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: port".to_string())
        })? as i32;

    tracing::debug!(team = %team, port = %port, "Querying port availability");

    // Query database for listener using this port
    #[derive(sqlx::FromRow)]
    struct PortQueryRow {
        id: String,
        name: String,
        address: String,
        route_config_name: Option<String>,
    }

    // Query listeners table with optional join to get route config name
    // The route_config_name comes from the listener_route_configs junction table
    let query = r#"
        SELECT l.id, l.name, l.address, rc.name as route_config_name
        FROM listeners l
        LEFT JOIN listener_route_configs lr ON l.id = lr.listener_id
        LEFT JOIN route_configs rc ON lr.route_config_id = rc.id
        WHERE l.port = $1 AND (l.team = $2 OR l.team IS NULL)
        LIMIT 1
    "#;

    let result = sqlx::query_as::<_, PortQueryRow>(query)
        .bind(port)
        .bind(team)
        .fetch_optional(db_pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, port = %port, "Failed to query port");
            McpError::DatabaseError(e)
        })?;

    let found = result.is_some();

    let response = if let Some(row) = result {
        let ref_ = ResourceRef::listener(&row.name, &row.id);
        let data = json!({
            "address": row.address,
            "route_config": row.route_config_name
        });
        build_query_response(true, Some(ref_), Some(data))
    } else {
        build_query_response(false, None, None)
    };

    let text = serde_json::to_string(&response).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, port = %port, found = found, "Port query completed");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_get_listener_status tool.
///
/// Returns status information for a listener including route config count.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_listener_status")]
pub async fn execute_get_listener_status(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(team = %team, listener_name = %name, "Getting listener status");

    // Verify listener exists
    let ops = ListenerOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;
    let listener = ops.get(name, &auth).await?;

    // Query route config count
    let pool = xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Database not available".to_string()))?
        .pool();

    let route_count_query = r#"
        SELECT COUNT(*) as count
        FROM listener_route_configs
        WHERE listener_id = $1
    "#;

    #[derive(sqlx::FromRow)]
    struct CountRow {
        count: i64,
    }

    let count_row = sqlx::query_as::<_, CountRow>(route_count_query)
        .bind(&listener.id)
        .fetch_one(pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener.id, "Failed to query route count");
            McpError::DatabaseError(e)
        })?;

    // Build response
    let data = json!({
        "active_connections": 0,  // Placeholder for future metrics integration
        "route_config_count": count_row.count,
        "address": listener.address,
        "port": listener.port,
        "protocol": listener.protocol
    });

    let response = build_query_response(
        true,
        Some(ResourceRef::listener(&listener.name, listener.id.to_string())),
        Some(data),
    );

    let text = serde_json::to_string(&response).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        listener_name = %name,
        route_count = count_row.count,
        "Successfully retrieved listener status"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_listeners_tool_definition() {
        let tool = cp_list_listeners_tool();
        assert_eq!(tool.name, "cp_list_listeners");
        assert!(tool.description.as_ref().unwrap().contains("List all listeners"));
        assert!(tool.input_schema.get("properties").is_some());
    }

    #[test]
    fn test_cp_get_listener_tool_definition() {
        let tool = cp_get_listener_tool();
        assert_eq!(tool.name, "cp_get_listener");
        assert!(tool.description.as_ref().unwrap().contains("Get detailed information"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[test]
    fn test_cp_create_listener_tool_definition() {
        let tool = cp_create_listener_tool();
        assert_eq!(tool.name, "cp_create_listener");
        assert!(tool.description.as_ref().unwrap().contains("Create a new listener"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[test]
    fn test_cp_update_listener_tool_definition() {
        let tool = cp_update_listener_tool();
        assert_eq!(tool.name, "cp_update_listener");
        assert!(tool.description.as_ref().unwrap().contains("Update an existing"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[test]
    fn test_cp_delete_listener_tool_definition() {
        let tool = cp_delete_listener_tool();
        assert_eq!(tool.name, "cp_delete_listener");
        assert!(tool.description.as_ref().unwrap().contains("Delete a listener"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[test]
    fn test_cp_query_port_tool_definition() {
        let tool = cp_query_port_tool();
        assert_eq!(tool.name, "cp_query_port");
        assert!(tool.description.as_ref().unwrap().contains("Check if a port is already in use"));
        assert!(tool.input_schema.get("required").is_some());

        // Verify required parameters
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("port")));
        assert_eq!(required.len(), 1); // Only port is required

        // Verify port constraints
        let port_schema = &tool.input_schema["properties"]["port"];
        assert_eq!(port_schema["type"], "integer");
        assert_eq!(port_schema["minimum"], 1);
        assert_eq!(port_schema["maximum"], 65535);
    }

    #[test]
    fn test_cp_get_listener_status_tool_definition() {
        let tool = cp_get_listener_status_tool();
        assert_eq!(tool.name, "cp_get_listener_status");
        assert!(tool.description.as_ref().unwrap().contains("status information"));
        assert!(tool.input_schema.get("required").is_some());

        // Verify required parameters
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
        assert_eq!(required.len(), 1); // Only name is required
    }
}
