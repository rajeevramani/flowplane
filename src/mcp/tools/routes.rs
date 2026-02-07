//! Routes MCP Tools
//!
//! Control Plane tools for managing routes.

use crate::internal_api::routes::transform_virtual_hosts_for_internal;
use crate::internal_api::{
    CreateRouteConfigRequest, InternalAuthContext, RouteConfigOperations, UpdateRouteConfigRequest,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::mcp::response_builders::{
    build_create_response, build_delete_response, build_query_response, build_update_response,
    ResourceRef,
};
use crate::storage::DbPool;
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Tool definition for listing routes
pub fn cp_list_routes_tool() -> Tool {
    Tool::new(
        "cp_list_routes",
        r#"List all routes with their metadata and configuration.

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

RELATED TOOLS: cp_create_route_config (create), cp_get_cluster (verify targets)"#,
        json!({
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
    )
}

/// Returns the MCP tool definition for querying path routing.
///
/// This is a query-first tool that checks if a path is already routed.
pub fn cp_query_path_tool() -> Tool {
    Tool::new(
        "cp_query_path",
        r#"Check if a path is already routed to a cluster.

PURPOSE: Query-first design - check if path exists before creating routes.

PARAMS:
- path: URL path to check (e.g., "/api/users")
- port: Optional port to scope search to specific listener

RETURNS:
- Not found: {"found": false} (path available)
- Found: {"found": true, "ref": {type, name, id}, "data": {cluster, route_config, match_type}}

EXAMPLE:
  cp_query_path({"path": "/api/users", "port": 8080})
  → {"found": true, "ref": {"type": "route", "name": "users-route", "id": "r-456"},
     "data": {"cluster": "user-svc", "route_config": "api-routes", "match_type": "prefix"}}

USE CASE:
  Before creating route: cp_query_path → if found, update existing or choose different path

PATH MATCHING:
- Checks exact matches first
- Then checks prefix matches (path starts with pattern)
- Returns first matching route in priority order

TOKEN BUDGET: <80 tokens response

Authorization: Requires routes:read or cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "URL path to check (e.g., '/api', '/users/{id}')"
                },
                "port": {
                    "type": "integer",
                    "description": "Optional port to scope search to specific listener",
                    "minimum": 1,
                    "maximum": 65535
                }
            },
            "required": ["path"]
        }),
    )
}

/// Execute list routes operation
/// Note: This queries the individual routes table directly for UI display,
/// not route_configs. It needs direct db_pool access.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_list_routes")]
pub async fn execute_list_routes(
    db_pool: &DbPool,
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

/// Execute the cp_query_path tool.
///
/// Query-first tool that checks if a path is already routed.
/// Returns minimal response format for token efficiency.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_query_path")]
pub async fn execute_query_path(
    db_pool: &DbPool,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let query_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: path".to_string()))?;

    let port = args.get("port").and_then(|v| v.as_i64()).map(|p| p as i32);

    tracing::debug!(team = %team, path = %query_path, port = ?port, "Querying path routing");

    // Query for matching routes
    // We need to check both exact matches and prefix matches
    #[derive(sqlx::FromRow)]
    struct PathQueryRow {
        route_id: String,
        route_name: String,
        path_pattern: String,
        match_type: String,
        route_config_name: String,
        cluster_name: String,
    }

    // Build query based on whether port is specified
    // The query joins routes -> virtual_hosts -> route_configs
    // and optionally to listener_routes -> listeners for port filtering
    let (query, result) = if let Some(p) = port {
        let query = r#"
            SELECT r.id as route_id, r.name as route_name, r.path_pattern, r.match_type,
                   rc.name as route_config_name, rc.cluster_name
            FROM routes r
            INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
            INNER JOIN route_configs rc ON vh.route_config_id = rc.id
            INNER JOIN listener_routes lr ON rc.id = lr.route_config_id
            INNER JOIN listeners l ON lr.listener_id = l.id
            WHERE (rc.team = $1 OR rc.team IS NULL)
              AND l.port = $2
              AND (
                  r.path_pattern = $3
                  OR (r.match_type = 'prefix' AND $3 LIKE r.path_pattern || '%')
                  OR (r.match_type = 'prefix' AND r.path_pattern LIKE $3 || '%')
              )
            ORDER BY r.rule_order ASC
            LIMIT 1
        "#;
        let res = sqlx::query_as::<_, PathQueryRow>(query)
            .bind(team)
            .bind(p)
            .bind(query_path)
            .fetch_optional(db_pool)
            .await;
        (query, res)
    } else {
        let query = r#"
            SELECT r.id as route_id, r.name as route_name, r.path_pattern, r.match_type,
                   rc.name as route_config_name, rc.cluster_name
            FROM routes r
            INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
            INNER JOIN route_configs rc ON vh.route_config_id = rc.id
            WHERE (rc.team = $1 OR rc.team IS NULL)
              AND (
                  r.path_pattern = $2
                  OR (r.match_type = 'prefix' AND $2 LIKE r.path_pattern || '%')
                  OR (r.match_type = 'prefix' AND r.path_pattern LIKE $2 || '%')
              )
            ORDER BY r.rule_order ASC
            LIMIT 1
        "#;
        let res = sqlx::query_as::<_, PathQueryRow>(query)
            .bind(team)
            .bind(query_path)
            .fetch_optional(db_pool)
            .await;
        (query, res)
    };

    let result = result.map_err(|e| {
        tracing::error!(error = %e, team = %team, path = %query_path, query = %query, "Failed to query path");
        McpError::DatabaseError(e)
    })?;

    let found = result.is_some();

    let response = if let Some(row) = result {
        let ref_ = ResourceRef::route(&row.route_name, &row.route_id);
        let data = json!({
            "cluster": row.cluster_name,
            "route_config": row.route_config_name,
            "match_type": row.match_type,
            "path_pattern": row.path_pattern
        });
        build_query_response(true, Some(ref_), Some(data))
    } else {
        build_query_response(false, None, None)
    };

    let text = serde_json::to_string(&response).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, path = %query_path, port = ?port, found = found, "Path query completed");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

// =============================================================================
// CRUD Tools (Create, Update, Delete)
// =============================================================================

/// Returns the MCP tool definition for creating a route config.
pub fn cp_create_route_config_tool() -> Tool {
    Tool::new(
        "cp_create_route_config",
        r#"Create a new route configuration in the Flowplane control plane.

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

Authorization: Requires cp:write scope."#,
        json!({
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
    )
}

/// Returns the MCP tool definition for updating a route config.
pub fn cp_update_route_config_tool() -> Tool {
    Tool::new(
        "cp_update_route_config",
        r#"Update an existing route configuration.

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

Authorization: Requires cp:write scope."#,
        json!({
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
    )
}

/// Returns the MCP tool definition for deleting a route config.
pub fn cp_delete_route_config_tool() -> Tool {
    Tool::new(
        "cp_delete_route_config",
        r#"Delete a route configuration from the Flowplane control plane.

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

Authorization: Requires cp:write scope."#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the route config to delete"
                }
            },
            "required": ["name"]
        }),
    )
}

/// Execute the cp_create_route_config tool using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_create_route_config")]
pub async fn execute_create_route_config(
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

    tracing::debug!(
        team = %team,
        route_config_name = %name,
        "Creating route config via MCP"
    );

    // 2. Transform from MCP-friendly format to internal format
    // MCP uses: {"type": "prefix", "value": "/api"}, {"type": "forward", "cluster": "x"}
    // Internal uses: {"Prefix": "/api"}, {"Cluster": {"name": "x"}}
    let transformed_virtual_hosts = transform_virtual_hosts_for_internal(virtual_hosts);

    // 3. Build full configuration for xDS
    let configuration = json!({
        "name": name,
        "virtual_hosts": transformed_virtual_hosts
    });

    // 4. Use internal API layer
    let ops = RouteConfigOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = CreateRouteConfigRequest {
        name: name.to_string(),
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        config: configuration,
    };

    let result = ops.create(req, &auth).await?;

    // 5. Format success response (minimal token-efficient format)
    let output = build_create_response("route_config", &result.data.name, result.data.id.as_ref());

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config_name = %result.data.name,
        route_config_id = %result.data.id,
        "Successfully created route config via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_update_route_config tool using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_update_route_config")]
pub async fn execute_update_route_config(
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

    // 2. Transform from MCP-friendly format to internal format
    let transformed_virtual_hosts = transform_virtual_hosts_for_internal(virtual_hosts);

    // 3. Build full configuration for xDS
    let configuration = json!({
        "name": name,
        "virtual_hosts": transformed_virtual_hosts
    });

    // 4. Use internal API layer
    let ops = RouteConfigOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = UpdateRouteConfigRequest { config: configuration };

    let result = ops.update(name, req, &auth).await?;

    // 5. Format success response (minimal token-efficient format)
    let output = build_update_response("route_config", &result.data.name, result.data.id.as_ref());

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config_name = %result.data.name,
        route_config_id = %result.data.id,
        "Successfully updated route config via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_delete_route_config tool using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_delete_route_config")]
pub async fn execute_delete_route_config(
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

    // 2. Use internal API layer
    let ops = RouteConfigOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    ops.delete(name, &auth).await?;

    // 3. Format success response (minimal token-efficient format)
    let output = build_delete_response();

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config_name = %name,
        "Successfully deleted route config via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

// =============================================================================
// Individual Route CRUD Tools
// =============================================================================

/// Returns the MCP tool definition for getting a route by hierarchy
pub fn cp_get_route_tool() -> Tool {
    Tool::new(
        "cp_get_route",
        r#"Get a specific route by hierarchy (route_config → virtual_host → route).

RESOURCE ORDER: Routes are within virtual hosts, which are within route configs (order 2 of 4).

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
                         │
                    contains virtual hosts
                         │
                    contains routes

HIERARCHY: route_config → virtual_host → route
Routes are path-level matching rules within a virtual host.

PURPOSE: Retrieve complete route details including:
- name: Route identifier (unique within virtual host)
- path_pattern: URL path pattern (e.g., "/api", "/users/{id}")
- match_type: Type of matching (prefix, exact, regex, template)
- rule_order: Priority order (lower values match first)
- virtual_host_id: Parent virtual host
- created_at/updated_at: Timestamps

MATCH TYPES:
- prefix: Matches if path starts with pattern (e.g., "/api" matches "/api/users")
- exact: Matches only the exact path
- regex: Regular expression matching
- template: Path template with variables (e.g., "/users/{id}")

Required Parameters:
- route_config: Name of the route configuration
- virtual_host: Name of the virtual host within the route config
- name: Name of the route

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "route_config": {
                    "type": "string",
                    "description": "Route configuration name"
                },
                "virtual_host": {
                    "type": "string",
                    "description": "Virtual host name within the route config"
                },
                "name": {
                    "type": "string",
                    "description": "Route name"
                }
            },
            "required": ["route_config", "virtual_host", "name"]
        }),
    )
}

/// Returns the MCP tool definition for creating a route
pub fn cp_create_route_tool() -> Tool {
    Tool::new(
        "cp_create_route",
        r#"Create a new route within a virtual host.

RESOURCE ORDER: Routes are within virtual hosts, which are within route configs (order 2 of 4).
PREREQUISITES: The route config and virtual host MUST exist first.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
                         │
                    contains virtual hosts
                         │
                    contains routes (you are here)

CREATION WORKFLOW:
1. First, ensure route config exists (cp_create_route_config)
2. Ensure virtual host exists within route config
3. Create this route within the virtual host
4. Update the route config to sync to xDS (route changes require route config update)

STRUCTURE:
Route Config → Virtual Hosts → Routes
- Routes define path-level matching and actions
- Multiple routes can exist in a virtual host
- Routes are evaluated in rule_order (lower numbers first)

MATCH TYPES:
- prefix: Matches if path starts with pattern (e.g., "/api" matches "/api/users")
- exact: Matches only the exact path
- regex: Regular expression matching
- template: Path template with variables (e.g., "/users/{id}")

ACTION TYPES (must be provided in action parameter):
- forward: {"type": "forward", "cluster": "cluster-name"}
- weighted: {"type": "weighted", "clusters": [{"name": "a", "weight": 90}, {"name": "b", "weight": 10}]}
- redirect: {"type": "redirect", "hostRedirect": "example.com", "responseCode": 301}

Required Parameters:
- route_config: Name of the route configuration
- virtual_host: Name of the virtual host
- name: Unique route name (within virtual host)
- path_pattern: URL path pattern (e.g., "/api", "/users/{id}")
- match_type: Type of matching (prefix, exact, regex, template)
- action: Route action (forward, weighted, redirect)

Optional Parameters:
- rule_order: Priority order (default: 100, lower values match first)

IMPORTANT: After creating routes, you must update the route config to sync changes to xDS.

Authorization: Requires cp:write scope."#,
        json!({
            "type": "object",
            "properties": {
                "route_config": {
                    "type": "string",
                    "description": "Route configuration name"
                },
                "virtual_host": {
                    "type": "string",
                    "description": "Virtual host name within the route config"
                },
                "name": {
                    "type": "string",
                    "description": "Unique route name (within virtual host)"
                },
                "path_pattern": {
                    "type": "string",
                    "description": "URL path pattern (e.g., '/api', '/users/{id}')"
                },
                "match_type": {
                    "type": "string",
                    "enum": ["prefix", "exact", "regex", "template"],
                    "description": "Type of path matching"
                },
                "action": {
                    "type": "object",
                    "description": "Route action (forward, weighted, or redirect)"
                },
                "rule_order": {
                    "type": "integer",
                    "description": "Priority order (default: 100, lower values match first)",
                    "default": 100
                }
            },
            "required": ["route_config", "virtual_host", "name", "path_pattern", "match_type", "action"]
        }),
    )
}

/// Returns the MCP tool definition for updating a route
pub fn cp_update_route_tool() -> Tool {
    Tool::new(
        "cp_update_route",
        r#"Update an existing route within a virtual host.

IMPORTANT: This is a PARTIAL update. Only provide fields you want to change.
Route name cannot be changed.

PREREQUISITES:
- Route config, virtual host, and route must exist
- All clusters referenced in new action must exist

SAFE TO UPDATE: Route updates modify the normalized routes table.
To sync changes to xDS, you must update the parent route config afterward.

COMMON USE CASES:
- Change path pattern or match type
- Update rule order for priority changes
- Modify action (change target cluster, add traffic splitting)

WORKFLOW:
1. Use cp_get_route to see current configuration
2. Call this tool with only the fields you want to change
3. Update the parent route config to sync to xDS

Required Parameters:
- route_config: Name of the route configuration
- virtual_host: Name of the virtual host
- name: Route name to update

Optional Parameters (provide only what you want to change):
- path_pattern: New URL path pattern
- match_type: New match type (prefix, exact, regex, template)
- rule_order: New priority order
- action: New route action

IMPORTANT: After updating routes, you must update the route config to sync changes to xDS.

Authorization: Requires cp:write scope."#,
        json!({
            "type": "object",
            "properties": {
                "route_config": {
                    "type": "string",
                    "description": "Route configuration name"
                },
                "virtual_host": {
                    "type": "string",
                    "description": "Virtual host name within the route config"
                },
                "name": {
                    "type": "string",
                    "description": "Route name to update"
                },
                "path_pattern": {
                    "type": "string",
                    "description": "New URL path pattern (optional)"
                },
                "match_type": {
                    "type": "string",
                    "enum": ["prefix", "exact", "regex", "template"],
                    "description": "New match type (optional)"
                },
                "rule_order": {
                    "type": "integer",
                    "description": "New priority order (optional)"
                },
                "action": {
                    "type": "object",
                    "description": "New route action (optional)"
                }
            },
            "required": ["route_config", "virtual_host", "name"]
        }),
    )
}

/// Returns the MCP tool definition for deleting a route
pub fn cp_delete_route_tool() -> Tool {
    Tool::new(
        "cp_delete_route",
        r#"Delete a route from a virtual host.

DELETION ORDER: Delete routes before deleting their parent virtual host or route config.

PREREQUISITES FOR DELETION:
- Route must exist
- Have write access to the parent route config

WORKFLOW:
1. Identify the route to delete by hierarchy (route_config → virtual_host → route)
2. Call this tool to delete the route
3. Update the parent route config to sync the deletion to xDS

CASCADING EFFECTS:
When deleted, the route is removed from the normalized routes table.
To sync the deletion to xDS, you must update the parent route config.

Required Parameters:
- route_config: Name of the route configuration
- virtual_host: Name of the virtual host
- name: Route name to delete

IMPORTANT: After deleting routes, you must update the route config to sync changes to xDS.

Authorization: Requires cp:write scope."#,
        json!({
            "type": "object",
            "properties": {
                "route_config": {
                    "type": "string",
                    "description": "Route configuration name"
                },
                "virtual_host": {
                    "type": "string",
                    "description": "Virtual host name within the route config"
                },
                "name": {
                    "type": "string",
                    "description": "Route name to delete"
                }
            },
            "required": ["route_config", "virtual_host", "name"]
        }),
    )
}

/// Execute the cp_get_route tool
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_route")]
pub async fn execute_get_route(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    use crate::internal_api::{InternalAuthContext, RouteOperations};

    let route_config = args.get("route_config").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: route_config".to_string())
    })?;

    let virtual_host = args.get("virtual_host").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: virtual_host".to_string())
    })?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        route_config = %route_config,
        virtual_host = %virtual_host,
        route_name = %name,
        "Getting route by hierarchy"
    );

    let ops = RouteOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let route = ops.get(route_config, virtual_host, name, &auth).await?;

    let output = json!({
        "id": route.id.to_string(),
        "name": route.name,
        "virtual_host_id": route.virtual_host_id.to_string(),
        "path_pattern": route.path_pattern,
        "match_type": route.match_type.to_string(),
        "rule_order": route.rule_order,
        "created_at": route.created_at.to_rfc3339(),
        "updated_at": route.updated_at.to_rfc3339(),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config = %route_config,
        virtual_host = %virtual_host,
        route_name = %name,
        "Successfully retrieved route"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_create_route tool
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_create_route")]
pub async fn execute_create_route(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    use crate::internal_api::{CreateRouteRequest, InternalAuthContext, RouteOperations};

    let route_config = args.get("route_config").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: route_config".to_string())
    })?;

    let virtual_host = args.get("virtual_host").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: virtual_host".to_string())
    })?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    let path_pattern = args.get("path_pattern").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: path_pattern".to_string())
    })?;

    let match_type = args.get("match_type").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: match_type".to_string())
    })?;

    let action = args
        .get("action")
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: action".to_string()))?;

    let rule_order = args.get("rule_order").and_then(|v| v.as_i64()).map(|v| v as i32);

    tracing::debug!(
        team = %team,
        route_config = %route_config,
        virtual_host = %virtual_host,
        route_name = %name,
        "Creating route via MCP"
    );

    let ops = RouteOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = CreateRouteRequest {
        route_config: route_config.to_string(),
        virtual_host: virtual_host.to_string(),
        name: name.to_string(),
        path_pattern: path_pattern.to_string(),
        match_type: match_type.to_string(),
        rule_order,
        action: action.clone(),
    };

    let result = ops.create(req, &auth).await?;

    // Format success response (minimal token-efficient format)
    let output = build_create_response("route", &result.data.name, result.data.id.as_ref());

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config = %route_config,
        virtual_host = %virtual_host,
        route_name = %result.data.name,
        route_id = %result.data.id,
        "Successfully created route via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_update_route tool
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_update_route")]
pub async fn execute_update_route(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    use crate::internal_api::{InternalAuthContext, RouteOperations, UpdateRouteRequest};

    let route_config = args.get("route_config").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: route_config".to_string())
    })?;

    let virtual_host = args.get("virtual_host").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: virtual_host".to_string())
    })?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    let path_pattern = args.get("path_pattern").and_then(|v| v.as_str()).map(|s| s.to_string());
    let match_type = args.get("match_type").and_then(|v| v.as_str()).map(|s| s.to_string());
    let rule_order = args.get("rule_order").and_then(|v| v.as_i64()).map(|v| v as i32);
    let action = args.get("action").cloned();

    tracing::debug!(
        team = %team,
        route_config = %route_config,
        virtual_host = %virtual_host,
        route_name = %name,
        "Updating route via MCP"
    );

    let ops = RouteOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = UpdateRouteRequest { path_pattern, match_type, rule_order, action };

    let result = ops.update(route_config, virtual_host, name, req, &auth).await?;

    // Format success response (minimal token-efficient format)
    let output = build_update_response("route", &result.data.name, result.data.id.as_ref());

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config = %route_config,
        virtual_host = %virtual_host,
        route_name = %result.data.name,
        route_id = %result.data.id,
        "Successfully updated route via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_delete_route tool
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_delete_route")]
pub async fn execute_delete_route(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    use crate::internal_api::{InternalAuthContext, RouteOperations};

    let route_config = args.get("route_config").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: route_config".to_string())
    })?;

    let virtual_host = args.get("virtual_host").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: virtual_host".to_string())
    })?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        route_config = %route_config,
        virtual_host = %virtual_host,
        route_name = %name,
        "Deleting route via MCP"
    );

    let ops = RouteOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    ops.delete(route_config, virtual_host, name, &auth).await?;

    // Format success response (minimal token-efficient format)
    let output = build_delete_response();

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config = %route_config,
        virtual_host = %virtual_host,
        route_name = %name,
        "Successfully deleted route via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_routes_tool_definition() {
        let tool = cp_list_routes_tool();
        assert_eq!(tool.name, "cp_list_routes");
        assert!(tool.description.as_ref().unwrap().contains("routes"));
    }

    #[test]
    fn test_cp_create_route_config_tool_definition() {
        let tool = cp_create_route_config_tool();
        assert_eq!(tool.name, "cp_create_route_config");
        assert!(tool.description.as_ref().unwrap().contains("Create"));

        // Check required fields in schema
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("virtualHosts")));
    }

    #[test]
    fn test_cp_update_route_config_tool_definition() {
        let tool = cp_update_route_config_tool();
        assert_eq!(tool.name, "cp_update_route_config");
        assert!(tool.description.as_ref().unwrap().contains("Update"));
    }

    #[test]
    fn test_cp_delete_route_config_tool_definition() {
        let tool = cp_delete_route_config_tool();
        assert_eq!(tool.name, "cp_delete_route_config");
        assert!(tool.description.as_ref().unwrap().contains("Delete"));
    }

    #[test]
    fn test_cp_get_route_tool_definition() {
        let tool = cp_get_route_tool();
        assert_eq!(tool.name, "cp_get_route");
        assert!(tool.description.as_ref().unwrap().contains("Get a specific route"));

        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("route_config")));
        assert!(required.contains(&json!("virtual_host")));
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_cp_create_route_tool_definition() {
        let tool = cp_create_route_tool();
        assert_eq!(tool.name, "cp_create_route");
        assert!(tool.description.as_ref().unwrap().contains("Create a new route"));

        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("route_config")));
        assert!(required.contains(&json!("virtual_host")));
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("path_pattern")));
        assert!(required.contains(&json!("match_type")));
        assert!(required.contains(&json!("action")));
    }

    #[test]
    fn test_cp_update_route_tool_definition() {
        let tool = cp_update_route_tool();
        assert_eq!(tool.name, "cp_update_route");
        assert!(tool.description.as_ref().unwrap().contains("Update an existing route"));

        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("route_config")));
        assert!(required.contains(&json!("virtual_host")));
        assert!(required.contains(&json!("name")));
        assert_eq!(required.len(), 3); // Only 3 required, rest optional
    }

    #[test]
    fn test_cp_delete_route_tool_definition() {
        let tool = cp_delete_route_tool();
        assert_eq!(tool.name, "cp_delete_route");
        assert!(tool.description.as_ref().unwrap().contains("Delete a route"));

        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("route_config")));
        assert!(required.contains(&json!("virtual_host")));
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_cp_query_path_tool_definition() {
        let tool = cp_query_path_tool();
        assert_eq!(tool.name, "cp_query_path");
        assert!(tool.description.as_ref().unwrap().contains("Check if a path is already routed"));
        assert!(tool.input_schema.get("required").is_some());

        // Verify required parameters
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("path")));
        assert_eq!(required.len(), 1); // Only path is required, port is optional

        // Verify properties
        let props = &tool.input_schema["properties"];
        assert_eq!(props["path"]["type"], "string");
        assert_eq!(props["port"]["type"], "integer");
        assert_eq!(props["port"]["minimum"], 1);
        assert_eq!(props["port"]["maximum"], 65535);
    }
}
