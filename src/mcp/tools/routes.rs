//! Routes MCP Tools
//!
//! Control Plane tools for managing routes.

use crate::internal_api::routes::transform_virtual_hosts_for_internal;
use crate::internal_api::{
    CreateRouteConfigRequest, InternalAuthContext, RouteConfigOperations, UpdateRouteConfigRequest,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
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
/// Note: This queries the individual routes table directly for UI display,
/// not route_configs. It needs direct db_pool access.
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
    let auth = InternalAuthContext::from_mcp(team);

    let req = CreateRouteConfigRequest {
        name: name.to_string(),
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        config: configuration,
    };

    let result = ops.create(req, &auth).await?;

    // 5. Format success response
    let output = json!({
        "success": true,
        "routeConfig": {
            "id": result.data.id.to_string(),
            "name": result.data.name,
            "pathPrefix": result.data.path_prefix,
            "clusterTargets": result.data.cluster_name,
            "team": result.data.team,
            "version": result.data.version,
            "createdAt": result.data.created_at.to_rfc3339(),
        },
        "message": result.message.unwrap_or_else(|| format!(
            "Route config '{}' created successfully. xDS configuration has been refreshed.",
            result.data.name
        )),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

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
    let auth = InternalAuthContext::from_mcp(team);

    let req = UpdateRouteConfigRequest { config: configuration };

    let result = ops.update(name, req, &auth).await?;

    // 5. Format success response
    let output = json!({
        "success": true,
        "routeConfig": {
            "id": result.data.id.to_string(),
            "name": result.data.name,
            "pathPrefix": result.data.path_prefix,
            "clusterTargets": result.data.cluster_name,
            "team": result.data.team,
            "version": result.data.version,
            "updatedAt": result.data.updated_at.to_rfc3339(),
        },
        "message": result.message.unwrap_or_else(|| format!(
            "Route config '{}' updated successfully. xDS configuration has been refreshed.",
            result.data.name
        )),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

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
    let auth = InternalAuthContext::from_mcp(team);

    let result = ops.delete(name, &auth).await?;

    // 3. Format success response
    let output = json!({
        "success": true,
        "message": result.message.unwrap_or_else(|| format!(
            "Route config '{}' deleted successfully. xDS configuration has been refreshed.",
            name
        )),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config_name = %name,
        "Successfully deleted route config via MCP"
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
        assert!(tool.description.contains("routes"));
    }

    #[test]
    fn test_cp_create_route_config_tool_definition() {
        let tool = cp_create_route_config_tool();
        assert_eq!(tool.name, "cp_create_route_config");
        assert!(tool.description.contains("Create"));

        // Check required fields in schema
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("virtualHosts")));
    }

    #[test]
    fn test_cp_update_route_config_tool_definition() {
        let tool = cp_update_route_config_tool();
        assert_eq!(tool.name, "cp_update_route_config");
        assert!(tool.description.contains("Update"));
    }

    #[test]
    fn test_cp_delete_route_config_tool_definition() {
        let tool = cp_delete_route_config_tool();
        assert_eq!(tool.name, "cp_delete_route_config");
        assert!(tool.description.contains("Delete"));
    }
}
