//! Routes MCP Tools
//!
//! Control Plane tools for managing routes.

use crate::domain::RouteConfigId;
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::services::RouteService;
use crate::storage::RouteConfigRepository;
use crate::xds::route::RouteConfig;
use crate::xds::XdsState;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::instrument;

/// Tool definition for listing routes
pub fn cp_list_routes_tool() -> Tool {
    Tool {
        name: "cp_list_routes".to_string(),
        description: r#"List all routes with their metadata and configuration.

RESOURCE ORDER: Routes are part of Route Configurations (order 2 of 4).
Routes are created within route configs, which depend on clusters.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
                         │
                    contains routes

PURPOSE: Discover existing routes to:
- Understand current API endpoint configuration
- Find routes that reference specific clusters
- Plan new route additions
- Check route ordering (rule_order determines matching priority)

RETURNS: Array of route objects with:
- id: Internal route identifier
- name: Route name (used for filter attachment)
- route_config: Parent route configuration name
- virtual_host: Virtual host this route belongs to
- path_pattern: URL path pattern (prefix, exact, regex, template)
- match_type: Type of path matching
- rule_order: Priority order (lower numbers match first)
- metadata: {operation_id, summary, description, tags} from OpenAPI if imported

HIERARCHY:
  Route Config → Virtual Hosts → Routes
  - Route configs contain multiple virtual hosts
  - Virtual hosts match domains and contain routes
  - Routes match paths and forward to clusters

RELATED TOOLS: cp_create_route_config (create), cp_get_cluster (verify targets)"#
            .to_string(),
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
                    "description": "Filter by route configuration name to see routes in a specific config"
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

// =============================================================================
// CRUD Tools (Create, Update, Delete)
// =============================================================================

/// Returns the MCP tool definition for creating a route config.
pub fn cp_create_route_config_tool() -> Tool {
    Tool {
        name: "cp_create_route_config".to_string(),
        description: r#"Create a new route configuration in the Flowplane control plane.

RESOURCE ORDER: Route configs are order 2 of 4.
PREREQUISITE: Clusters referenced in route actions MUST exist first.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
       ▲                   │
       │                   │
  create first      you are here

CREATION WORKFLOW:
1. First, create clusters for your backend services (cp_create_cluster)
2. Then, create this route config referencing those clusters
3. Finally, create a listener to expose the routes (cp_create_listener)

STRUCTURE:
Route Config → Virtual Hosts → Routes
- virtualHosts: Domain-level grouping (use domains: ["*"] to match all)
- routes: Path-level matching with forwarding actions

PATH MATCH TYPES:
- prefix: Matches if path starts with value (e.g., "/api" matches "/api/users")
- exact: Matches only the exact path (e.g., "/health" matches only "/health")
- regex: Regex pattern matching (e.g., "/users/[0-9]+" matches "/users/123")
- template: Path template with variables (e.g., "/api/v1/users/{user_id}")

ACTION TYPES:
- forward: Send to a single cluster
- weighted: Split traffic across multiple clusters (A/B testing, canary)
- redirect: HTTP redirect to different host/path

PREFIX REWRITE (important for backends like httpbin):
Use "prefixRewrite" in action to rewrite the URL path before forwarding.
Example: Match "/api/get" and rewrite to "/get" for httpbin backend.

COMPLETE EXAMPLE WITH PREFIX REWRITE (for httpbin backend):
{
  "name": "httpbin-routes",
  "virtualHosts": [{
    "name": "default",
    "domains": ["*"],
    "routes": [
      {
        "name": "httpbin-get",
        "match": {"path": {"type": "prefix", "value": "/api/get"}},
        "action": {"type": "forward", "cluster": "httpbin-cluster", "prefixRewrite": "/get"}
      },
      {
        "name": "httpbin-post",
        "match": {"path": {"type": "prefix", "value": "/api/post"}},
        "action": {"type": "forward", "cluster": "httpbin-cluster", "prefixRewrite": "/post"}
      },
      {
        "name": "httpbin-anything",
        "match": {"path": {"type": "prefix", "value": "/api/anything"}},
        "action": {"type": "forward", "cluster": "httpbin-cluster", "prefixRewrite": "/anything"}
      },
      {
        "name": "exact-health",
        "match": {"path": {"type": "exact", "value": "/health"}},
        "action": {"type": "forward", "cluster": "httpbin-cluster", "prefixRewrite": "/status/200"}
      }
    ]
  }]
}

EXAMPLE WITH WEIGHTED ROUTING (canary/A-B testing):
{
  "name": "canary-routes",
  "virtualHosts": [{
    "name": "default",
    "domains": ["*"],
    "routes": [{
      "name": "canary-deploy",
      "match": {"path": {"type": "prefix", "value": "/api"}},
      "action": {
        "type": "weighted",
        "clusters": [
          {"name": "api-stable", "weight": 90},
          {"name": "api-canary", "weight": 10}
        ]
      }
    }]
  }]
}

Authorization: Requires cp:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique name for the route config (e.g., 'api-routes', 'admin-routes')"
                },
                "virtualHosts": {
                    "type": "array",
                    "description": "List of virtual hosts defining domain matching and route rules",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Virtual host name"
                            },
                            "domains": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Domains to match (e.g., ['*'], ['api.example.com'])"
                            },
                            "routes": {
                                "type": "array",
                                "description": "Route rules for this virtual host",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": {"type": "string", "description": "Route rule name"},
                                        "match": {
                                            "type": "object",
                                            "description": "Path and header matching configuration",
                                            "properties": {
                                                "path": {
                                                    "type": "object",
                                                    "description": "Path matching rule",
                                                    "properties": {
                                                        "type": {"type": "string", "enum": ["prefix", "exact", "regex", "template"]},
                                                        "value": {"type": "string", "description": "Path value for prefix/exact/regex"},
                                                        "template": {"type": "string", "description": "Path template (e.g., '/api/v1/users/{user_id}')"}
                                                    },
                                                    "required": ["type"]
                                                },
                                                "headers": {
                                                    "type": "array",
                                                    "description": "Optional header matching",
                                                    "items": {
                                                        "type": "object",
                                                        "properties": {
                                                            "name": {"type": "string"},
                                                            "value": {"type": "string"},
                                                            "regex": {"type": "string"},
                                                            "present": {"type": "boolean"}
                                                        },
                                                        "required": ["name"]
                                                    }
                                                }
                                            },
                                            "required": ["path"]
                                        },
                                        "action": {
                                            "type": "object",
                                            "description": "Route action (forward, weighted, or redirect)",
                                            "properties": {
                                                "type": {"type": "string", "enum": ["forward", "weighted", "redirect"]},
                                                "cluster": {"type": "string", "description": "Target cluster for forward action"},
                                                "timeoutSeconds": {"type": "integer", "description": "Request timeout"},
                                                "prefixRewrite": {"type": "string", "description": "Rewrite path prefix"},
                                                "clusters": {
                                                    "type": "array",
                                                    "description": "Weighted clusters for traffic splitting",
                                                    "items": {
                                                        "type": "object",
                                                        "properties": {
                                                            "name": {"type": "string"},
                                                            "weight": {"type": "integer"}
                                                        },
                                                        "required": ["name", "weight"]
                                                    }
                                                },
                                                "hostRedirect": {"type": "string"},
                                                "pathRedirect": {"type": "string"},
                                                "responseCode": {"type": "integer"}
                                            },
                                            "required": ["type"]
                                        }
                                    },
                                    "required": ["match", "action"]
                                }
                            }
                        },
                        "required": ["name", "domains", "routes"]
                    }
                }
            },
            "required": ["name", "virtualHosts"]
        }),
    }
}

/// Returns the MCP tool definition for updating a route config.
pub fn cp_update_route_config_tool() -> Tool {
    Tool {
        name: "cp_update_route_config".to_string(),
        description: r#"Update an existing route configuration.

IMPORTANT: This is a FULL REPLACEMENT, not a partial update.
Provide the complete new virtualHosts configuration.

PREREQUISITES:
- All clusters referenced in the new configuration must exist
- Route config name cannot be changed

SAFE TO UPDATE: Route config updates take effect immediately.
Connected listeners will serve the new routes.

COMMON USE CASES:
- Add new routes to an existing config
- Modify path matching rules
- Change cluster targets
- Update timeout settings
- Implement traffic splitting (weighted clusters)

WORKFLOW:
1. Use cp_list_routes to see current routes
2. Prepare complete new virtualHosts array
3. Call this tool with the full new configuration

TIP: Copy current config, modify it, then submit the complete updated version.

Authorization: Requires cp:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the route config to update"
                },
                "virtualHosts": {
                    "type": "array",
                    "description": "New virtual hosts configuration (replaces existing)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "domains": {"type": "array", "items": {"type": "string"}},
                            "routes": {"type": "array", "items": {"type": "object"}}
                        },
                        "required": ["name", "domains", "routes"]
                    }
                }
            },
            "required": ["name", "virtualHosts"]
        }),
    }
}

/// Returns the MCP tool definition for deleting a route config.
pub fn cp_delete_route_config_tool() -> Tool {
    Tool {
        name: "cp_delete_route_config".to_string(),
        description: r#"Delete a route configuration from the Flowplane control plane.

DELETION ORDER: Delete in REVERSE order of creation.
Delete listeners referencing this route config FIRST.

ORDER: [Listeners] ─► [Route Configs] ─► [Clusters/Filters]

PREREQUISITES FOR DELETION:
- No listeners may reference this route config
- If listeners are bound, update or delete them first

WILL FAIL IF:
- Route config name is "default-gateway-route" (system route)
- Listeners are bound to this route config (listener_route_configs table)

WORKFLOW:
1. Use cp_list_listeners to find listeners using this route config
2. Update or delete those listeners first
3. Then delete the route config

CASCADING EFFECTS:
When deleted, all contained virtual hosts and routes are also deleted.
This is safe if no listeners reference the config.

Required Parameters:
- name: Name of the route config to delete

Authorization: Requires cp:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the route config to delete"
                }
            },
            "required": ["name"]
        }),
    }
}

/// Execute the cp_create_route_config tool.
#[instrument(skip(_db_pool, xds_state, args), fields(team = %team), name = "mcp_execute_create_route_config")]
pub async fn execute_create_route_config(
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

    let virtual_hosts = args.get("virtualHosts").ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: virtualHosts".to_string())
    })?;

    // 2. Summarize the configuration for storage
    let (path_prefix, cluster_summary) = summarize_virtual_hosts(virtual_hosts);

    // 3. Transform from MCP-friendly format to internal format
    // MCP uses: {"type": "prefix", "value": "/api"}, {"type": "forward", "cluster": "x"}
    // Internal uses: {"Prefix": "/api"}, {"Cluster": {"name": "x"}}
    let transformed_virtual_hosts = transform_virtual_hosts_for_internal(virtual_hosts);

    // 4. Build full configuration for xDS
    let configuration = json!({
        "name": name,
        "virtual_hosts": transformed_virtual_hosts
    });

    tracing::debug!(
        team = %team,
        route_config_name = %name,
        path_prefix = %path_prefix,
        cluster_summary = %cluster_summary,
        "Creating route config via MCP"
    );

    // 5. Create via service layer
    let route_service = RouteService::new(xds_state.clone());
    let created = route_service
        .create_route(
            name.to_string(),
            path_prefix.clone(),
            cluster_summary.clone(),
            configuration.clone(),
            Some(team.to_string()),
        )
        .await
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("already exists") || err_str.contains("UNIQUE constraint") {
                McpError::Conflict(format!("Route config '{}' already exists", name))
            } else if err_str.contains("validation") {
                McpError::ValidationError(err_str)
            } else {
                McpError::InternalError(format!("Failed to create route config: {}", e))
            }
        })?;

    // 6. Sync route hierarchy to normalized tables (virtual_hosts, routes)
    // This is needed for the UI to display routes correctly
    if let Some(ref sync_service) = xds_state.route_hierarchy_sync_service {
        // Parse the stored configuration back to RouteConfig
        let xds_config: RouteConfig =
            serde_json::from_value(configuration.clone()).map_err(|e| {
                McpError::InternalError(format!("Failed to parse route config for sync: {}", e))
            })?;

        let route_config_id = RouteConfigId::from_string(created.id.to_string());
        if let Err(err) = sync_service.sync(&route_config_id, &xds_config).await {
            tracing::error!(
                error = %err,
                route_config_id = %created.id,
                "Failed to sync route hierarchy after MCP creation"
            );
            // Continue anyway - the route config was created, hierarchy sync is optional
        }
    }

    // 7. Format success response
    let output = json!({
        "success": true,
        "routeConfig": {
            "id": created.id.to_string(),
            "name": created.name,
            "pathPrefix": created.path_prefix,
            "clusterTargets": created.cluster_name,
            "team": created.team,
            "version": created.version,
            "createdAt": created.created_at.to_rfc3339(),
        },
        "message": format!(
            "Route config '{}' created successfully. xDS configuration has been refreshed.",
            created.name
        ),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config_name = %created.name,
        route_config_id = %created.id,
        "Successfully created route config via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Helper function to summarize virtual hosts for storage.
fn summarize_virtual_hosts(virtual_hosts: &Value) -> (String, String) {
    let mut paths = Vec::new();
    let mut clusters = std::collections::HashSet::new();

    if let Some(hosts) = virtual_hosts.as_array() {
        for host in hosts {
            if let Some(routes) = host.get("routes").and_then(|r| r.as_array()) {
                for route in routes {
                    // Extract path
                    if let Some(path) = route.get("match").and_then(|m| m.get("path")) {
                        if let Some(value) = path.get("value").and_then(|v| v.as_str()) {
                            paths.push(value.to_string());
                        } else if let Some(template) = path.get("template").and_then(|t| t.as_str())
                        {
                            paths.push(template.to_string());
                        }
                    }

                    // Extract cluster
                    if let Some(action) = route.get("action") {
                        if let Some(cluster) = action.get("cluster").and_then(|c| c.as_str()) {
                            clusters.insert(cluster.to_string());
                        }
                        if let Some(weighted) = action.get("clusters").and_then(|c| c.as_array()) {
                            for wc in weighted {
                                if let Some(name) = wc.get("name").and_then(|n| n.as_str()) {
                                    clusters.insert(name.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let path_prefix = paths.first().cloned().unwrap_or_else(|| "/".to_string());
    let cluster_summary = if clusters.is_empty() {
        "none".to_string()
    } else {
        clusters.into_iter().collect::<Vec<_>>().join(", ")
    };

    (path_prefix, cluster_summary)
}

/// Transform virtual hosts from MCP-friendly format to internal format.
///
/// MCP schema uses:
/// - path: `{"type": "prefix", "value": "/api"}`
/// - action: `{"type": "forward", "cluster": "x", "prefixRewrite": "/y"}`
///
/// Internal format uses Rust enum serialization:
/// - path: `{"Prefix": "/api"}`
/// - action: `{"Cluster": {"name": "x", "prefix_rewrite": "/y"}}`
fn transform_virtual_hosts_for_internal(virtual_hosts: &Value) -> Value {
    let Some(hosts) = virtual_hosts.as_array() else {
        return virtual_hosts.clone();
    };

    let transformed_hosts: Vec<Value> = hosts
        .iter()
        .map(|host| {
            let mut new_host = host.clone();
            if let Some(routes) = host.get("routes").and_then(|r| r.as_array()) {
                let transformed_routes: Vec<Value> = routes.iter().map(transform_route).collect();
                new_host["routes"] = json!(transformed_routes);
            }
            new_host
        })
        .collect();

    json!(transformed_hosts)
}

/// Transform a single route from MCP format to internal format.
fn transform_route(route: &Value) -> Value {
    let mut new_route = serde_json::Map::new();

    // Copy name if present
    if let Some(name) = route.get("name") {
        new_route.insert("name".to_string(), name.clone());
    }

    // Transform match.path
    if let Some(match_obj) = route.get("match") {
        let mut new_match = serde_json::Map::new();

        if let Some(path) = match_obj.get("path") {
            new_match.insert("path".to_string(), transform_path(path));
        }

        // Copy headers if present
        if let Some(headers) = match_obj.get("headers") {
            new_match.insert("headers".to_string(), headers.clone());
        }

        // Copy query_parameters if present
        if let Some(qp) = match_obj.get("query_parameters") {
            new_match.insert("query_parameters".to_string(), qp.clone());
        }

        new_route.insert("match".to_string(), json!(new_match));
    }

    // Transform action
    if let Some(action) = route.get("action") {
        new_route.insert("action".to_string(), transform_action(action));
    }

    // Copy typed_per_filter_config if present
    if let Some(tpfc) = route.get("typed_per_filter_config") {
        new_route.insert("typed_per_filter_config".to_string(), tpfc.clone());
    }

    json!(new_route)
}

/// Transform path from MCP format to internal enum format.
/// `{"type": "prefix", "value": "/api"}` → `{"Prefix": "/api"}`
fn transform_path(path: &Value) -> Value {
    let path_type = path.get("type").and_then(|t| t.as_str()).unwrap_or("prefix");

    match path_type {
        "prefix" => {
            let value = path.get("value").and_then(|v| v.as_str()).unwrap_or("/");
            json!({"Prefix": value})
        }
        "exact" => {
            let value = path.get("value").and_then(|v| v.as_str()).unwrap_or("/");
            json!({"Exact": value})
        }
        "regex" => {
            let value = path.get("value").and_then(|v| v.as_str()).unwrap_or(".*");
            json!({"Regex": value})
        }
        "template" => {
            let value = path
                .get("template")
                .or_else(|| path.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("/");
            json!({"Template": value})
        }
        _ => {
            // If already in internal format (e.g., {"Prefix": "/api"}), pass through
            path.clone()
        }
    }
}

/// Transform action from MCP format to internal enum format.
/// `{"type": "forward", "cluster": "x"}` → `{"Cluster": {"name": "x"}}`
fn transform_action(action: &Value) -> Value {
    let action_type = action.get("type").and_then(|t| t.as_str()).unwrap_or("forward");

    match action_type {
        "forward" => {
            let mut cluster_obj = serde_json::Map::new();

            if let Some(cluster) = action.get("cluster").and_then(|c| c.as_str()) {
                cluster_obj.insert("name".to_string(), json!(cluster));
            }
            if let Some(timeout) = action.get("timeoutSeconds").and_then(|t| t.as_u64()) {
                cluster_obj.insert("timeout".to_string(), json!(timeout));
            }
            if let Some(prefix_rewrite) = action.get("prefixRewrite").and_then(|p| p.as_str()) {
                cluster_obj.insert("prefix_rewrite".to_string(), json!(prefix_rewrite));
            }
            if let Some(path_rewrite) = action.get("pathTemplateRewrite").and_then(|p| p.as_str()) {
                cluster_obj.insert("path_template_rewrite".to_string(), json!(path_rewrite));
            }
            if let Some(retry) = action.get("retryPolicy") {
                cluster_obj.insert("retry_policy".to_string(), retry.clone());
            }

            json!({"Cluster": cluster_obj})
        }
        "weighted" => {
            let mut weighted_obj = serde_json::Map::new();

            if let Some(clusters) = action.get("clusters").and_then(|c| c.as_array()) {
                weighted_obj.insert("clusters".to_string(), json!(clusters));
            }
            if let Some(total_weight) = action.get("totalWeight").and_then(|t| t.as_u64()) {
                weighted_obj.insert("total_weight".to_string(), json!(total_weight as u32));
            }

            json!({"WeightedClusters": weighted_obj})
        }
        "redirect" => {
            let mut redirect_obj = serde_json::Map::new();

            if let Some(host) = action.get("hostRedirect").and_then(|h| h.as_str()) {
                redirect_obj.insert("host_redirect".to_string(), json!(host));
            }
            if let Some(path) = action.get("pathRedirect").and_then(|p| p.as_str()) {
                redirect_obj.insert("path_redirect".to_string(), json!(path));
            }
            if let Some(code) = action.get("responseCode").and_then(|c| c.as_u64()) {
                redirect_obj.insert("response_code".to_string(), json!(code as u32));
            }

            json!({"Redirect": redirect_obj})
        }
        _ => {
            // If already in internal format, pass through
            action.clone()
        }
    }
}

/// Execute the cp_update_route_config tool.
#[instrument(skip(db_pool, xds_state, args), fields(team = %team), name = "mcp_execute_update_route_config")]
pub async fn execute_update_route_config(
    db_pool: &SqlitePool,
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse route config name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    let virtual_hosts = args.get("virtualHosts").ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: virtualHosts".to_string())
    })?;

    tracing::debug!(
        team = %team,
        route_config_name = %name,
        "Updating route config via MCP"
    );

    // 2. Get existing route config
    let repo = RouteConfigRepository::new(db_pool.clone());
    let existing = repo.get_by_name(name).await.map_err(|e| {
        if e.to_string().contains("not found") {
            McpError::ResourceNotFound(format!("Route config '{}' not found", name))
        } else {
            McpError::InternalError(format!("Failed to get route config: {}", e))
        }
    })?;

    // 3. Verify team ownership
    if !team.is_empty() {
        if let Some(route_team) = &existing.team {
            if route_team != team {
                return Err(McpError::Forbidden(format!(
                    "Cannot update route config '{}' owned by team '{}'",
                    name, route_team
                )));
            }
        }
    }

    // 4. Summarize the new configuration
    let (path_prefix, cluster_summary) = summarize_virtual_hosts(virtual_hosts);

    // 5. Transform from MCP-friendly format to internal format
    let transformed_virtual_hosts = transform_virtual_hosts_for_internal(virtual_hosts);

    // 6. Build full configuration for xDS
    let configuration = json!({
        "name": name,
        "virtual_hosts": transformed_virtual_hosts
    });

    // 7. Update via service layer
    let route_service = RouteService::new(xds_state.clone());
    let updated = route_service
        .update_route(name, path_prefix, cluster_summary, configuration.clone())
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to update route config: {}", e)))?;

    // 8. Sync route hierarchy to normalized tables (virtual_hosts, routes)
    if let Some(ref sync_service) = xds_state.route_hierarchy_sync_service {
        let xds_config: RouteConfig = serde_json::from_value(configuration).map_err(|e| {
            McpError::InternalError(format!("Failed to parse route config for sync: {}", e))
        })?;

        let route_config_id = RouteConfigId::from_string(updated.id.to_string());
        if let Err(err) = sync_service.sync(&route_config_id, &xds_config).await {
            tracing::error!(
                error = %err,
                route_config_id = %updated.id,
                "Failed to sync route hierarchy after MCP update"
            );
        }
    }

    // 9. Format success response
    let output = json!({
        "success": true,
        "routeConfig": {
            "id": updated.id.to_string(),
            "name": updated.name,
            "pathPrefix": updated.path_prefix,
            "clusterTargets": updated.cluster_name,
            "team": updated.team,
            "version": updated.version,
            "updatedAt": updated.updated_at.to_rfc3339(),
        },
        "message": format!(
            "Route config '{}' updated successfully. xDS configuration has been refreshed.",
            updated.name
        ),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config_name = %updated.name,
        route_config_id = %updated.id,
        "Successfully updated route config via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_delete_route_config tool.
#[instrument(skip(db_pool, xds_state, args), fields(team = %team), name = "mcp_execute_delete_route_config")]
pub async fn execute_delete_route_config(
    db_pool: &SqlitePool,
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse route config name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        route_config_name = %name,
        "Deleting route config via MCP"
    );

    // 2. Get existing route config to verify ownership
    let repo = RouteConfigRepository::new(db_pool.clone());
    let existing = repo.get_by_name(name).await.map_err(|e| {
        if e.to_string().contains("not found") {
            McpError::ResourceNotFound(format!("Route config '{}' not found", name))
        } else {
            McpError::InternalError(format!("Failed to get route config: {}", e))
        }
    })?;

    // 3. Verify team ownership
    if !team.is_empty() {
        if let Some(route_team) = &existing.team {
            if route_team != team {
                return Err(McpError::Forbidden(format!(
                    "Cannot delete route config '{}' owned by team '{}'",
                    name, route_team
                )));
            }
        }
    }

    // 4. Delete via service layer
    let route_service = RouteService::new(xds_state.clone());
    route_service.delete_route(name).await.map_err(|e| {
        let err_str = e.to_string();
        if err_str.contains("default gateway") {
            McpError::Forbidden(err_str)
        } else {
            McpError::InternalError(format!("Failed to delete route config: {}", e))
        }
    })?;

    // 5. Format success response
    let output = json!({
        "success": true,
        "message": format!(
            "Route config '{}' deleted successfully. xDS configuration has been refreshed.",
            name
        ),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config_name = %name,
        "Successfully deleted route config via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}
