//! Filters MCP Tools
//!
//! Control Plane tools for managing filters.

use crate::internal_api::{
    CreateFilterRequest, FilterOperations, InternalAuthContext, ListFiltersRequest,
    UpdateFilterRequest,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Tool definition for listing filters
pub fn cp_list_filters_tool() -> Tool {
    Tool::new(
        "cp_list_filters",
        r#"List all filters available in the Flowplane control plane.

RESOURCE ORDER: Filters are independent resources (order 1 of 4).
Create filters BEFORE attaching them to listeners or routes.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
  [Filters]  ───────────┘     │                 │
       └──────────────────────┴─────────────────┘
       (filters can attach to routes or listeners)

PURPOSE: Discover available filters before:
- Attaching to a listener for global traffic processing
- Attaching to specific routes for per-endpoint policies

FILTER TYPES:
- jwt_auth: JWT token validation and authentication
- oauth2: OAuth 2.0 token introspection
- local_rate_limit: Request rate limiting
- cors: Cross-Origin Resource Sharing headers
- header_mutation: Add/remove/modify request/response headers
- ext_authz: External authorization service integration
- rbac: Role-based access control
- custom_response: Custom response generation
- compressor: Response compression (gzip, brotli)
- mcp: MCP-controlled dynamic filters

RETURNS: Array of filter objects with name, filter_type, configuration, and metadata.

RELATED TOOLS: cp_get_filter (details), cp_create_filter (create new)"#,
        json!({
            "type": "object",
            "properties": {
                "filter_type": {
                    "type": "string",
                    "description": "Filter by filter type (e.g., jwt_auth, oauth2, cors, rate_limit)",
                    "enum": ["jwt_auth", "oauth2", "local_rate_limit", "cors", "header_mutation", "ext_authz", "rbac", "custom_response", "compressor", "mcp"]
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of filters to return (1-1000, default: 100)",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 100
                },
                "offset": {
                    "type": "integer",
                    "description": "Offset for pagination (default: 0)",
                    "minimum": 0,
                    "default": 0
                }
            }
        }),
    )
}

/// Tool definition for getting a specific filter
pub fn cp_get_filter_tool() -> Tool {
    Tool::new(
        "cp_get_filter",
        r#"Get detailed information about a specific filter by name.

PURPOSE: Retrieve complete filter configuration and see where it's installed.

RETURNS:
- id: Internal filter identifier
- name: Unique filter name
- filter_type: Type of filter (jwt_auth, cors, rate_limit, etc.)
- description: Human-readable description
- configuration: Type-specific configuration object
- installations: Array of listeners/routes where this filter is attached
- version: For optimistic locking during updates

INSTALLATION INFO: Shows which listeners use this filter and in what order,
helping you understand the filter's scope and impact.

WHEN TO USE:
- Before updating filter configuration
- To see current settings and installation points
- To verify filter is properly attached
- Before deleting (check installations first)

CONFIGURATION VARIES BY TYPE:
- jwt_auth: {providers: [...], rules: [...]}
- cors: {allowOrigins: [...], allowMethods: [...], ...}
- rate_limit: {statPrefix, domain, rateLimits: [...]}
- header_mutation: {requestHeadersToAdd: [...], responseHeadersToAdd: [...]}

RELATED TOOLS: cp_list_filters (discovery), cp_update_filter (modify), cp_delete_filter (remove)"#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the filter to retrieve"
                }
            },
            "required": ["name"]
        }),
    )
}

/// Execute list filters operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_filters")]
pub async fn execute_list_filters(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let filter_type = args["filter_type"].as_str().map(|s| s.to_string());
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32);
    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32);

    // Use internal API layer
    let ops = FilterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    let req = ListFiltersRequest { filter_type, limit, offset, include_defaults: true };

    let response = ops.list(req, &auth).await?;

    let result = json!({
        "filters": response.filters.iter().map(|f| {
            // Parse configuration JSON
            let config: Value = serde_json::from_str(&f.configuration).unwrap_or_else(|e| {
                tracing::warn!(filter_id = %f.id, error = %e, "Failed to parse filter configuration");
                json!({"_parse_error": format!("Failed to parse configuration: {}", e)})
            });

            json!({
                "id": f.id.to_string(),
                "name": f.name,
                "filter_type": f.filter_type,
                "description": f.description,
                "configuration": config,
                "version": f.version,
                "source": f.source,
                "created_at": f.created_at.to_rfc3339(),
                "updated_at": f.updated_at.to_rfc3339()
            })
        }).collect::<Vec<_>>(),
        "count": response.count
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

/// Execute get filter operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_filter")]
pub async fn execute_get_filter(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    // Use internal API layer
    let ops = FilterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    let filter_with_installations = ops.get_with_installations(name, &auth).await?;
    let filter = &filter_with_installations.filter;

    // Parse configuration JSON
    let config: Value = serde_json::from_str(&filter.configuration).unwrap_or_else(|e| {
        tracing::warn!(filter_id = %filter.id, error = %e, "Failed to parse filter configuration");
        json!({"_parse_error": format!("Failed to parse configuration: {}", e)})
    });

    let result = json!({
        "id": filter.id.to_string(),
        "name": filter.name,
        "filter_type": filter.filter_type,
        "description": filter.description,
        "configuration": config,
        "version": filter.version,
        "source": filter.source,
        "listenerInstallations": filter_with_installations.listener_installations.iter().map(|i| json!({
            "listener_id": i.resource_id,
            "listener_name": i.resource_name,
            "order": i.order
        })).collect::<Vec<_>>(),
        "routeConfigInstallations": filter_with_installations.route_config_installations.iter().map(|i| json!({
            "route_config_id": i.resource_id,
            "route_config_name": i.resource_name
        })).collect::<Vec<_>>(),
        "created_at": filter.created_at.to_rfc3339(),
        "updated_at": filter.updated_at.to_rfc3339()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

// =============================================================================
// CRUD Tools (Create, Update, Delete)
// =============================================================================

/// Returns the MCP tool definition for creating a filter.
pub fn cp_create_filter_tool() -> Tool {
    Tool::new(
        "cp_create_filter",
        r#"Create a new filter in the Flowplane control plane.

RESOURCE ORDER: Filters are independent resources (order 1 of 4).
Create filters BEFORE attaching them to listeners or route configurations.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
  [Filters]  ───────────┘     │                 │
       └──────────────────────┴─────────────────┘

NEXT STEPS AFTER CREATING FILTER:
1. Attach to a listener for global traffic processing, OR
2. Attach to specific routes for per-endpoint policies

CONFIGURATION BY FILTER TYPE:

jwt_auth - JWT Authentication:
{
  "providers": [{
    "name": "my-provider",
    "issuer": "https://auth.example.com",
    "audiences": ["api"],
    "jwks": {"remoteJwks": {"uri": "https://auth.example.com/.well-known/jwks.json"}}
  }],
  "rules": [{"match": {"prefix": "/api"}, "requires": {"providerName": "my-provider"}}]
}

cors - Cross-Origin Resource Sharing:
{
  "allowOrigins": [{"exact": "https://app.example.com"}],
  "allowMethods": ["GET", "POST", "PUT", "DELETE"],
  "allowHeaders": ["Authorization", "Content-Type"],
  "maxAge": 86400
}

local_rate_limit - Request Rate Limiting:
{
  "statPrefix": "api_rate_limit",
  "tokenBucket": {"maxTokens": 100, "tokensPerFill": 100, "fillInterval": "60s"},
  "filterEnabled": {"defaultValue": {"numerator": 100, "denominator": "HUNDRED"}}
}

header_mutation - Header Modification:
{
  "requestHeadersToAdd": [{"header": {"key": "X-Custom", "value": "added"}, "append": false}],
  "requestHeadersToRemove": ["X-Remove-Me"],
  "responseHeadersToAdd": [{"header": {"key": "X-Response", "value": "added"}}]
}

ext_authz - External Authorization:
{
  "httpService": {
    "serverUri": {"uri": "http://auth-service:8080", "cluster": "auth-cluster"},
    "authorizationRequest": {"allowedHeaders": {"patterns": [{"exact": "Authorization"}]}}
  }
}

Authorization: Requires cp:write scope.
"#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique name for the filter (e.g., 'api-jwt-auth', 'cors-policy')"
                },
                "filterType": {
                    "type": "string",
                    "description": "Type of filter to create",
                    "enum": ["jwt_auth", "oauth2", "local_rate_limit", "cors", "header_mutation", "custom_response", "compressor", "ext_authz", "rbac", "mcp"]
                },
                "description": {
                    "type": "string",
                    "description": "Optional description of the filter's purpose"
                },
                "configuration": {
                    "type": "object",
                    "description": "Filter-specific configuration (see description for examples by filterType)"
                }
            },
            "required": ["name", "filterType", "configuration"]
        }),
    )
}

/// Returns the MCP tool definition for updating a filter.
pub fn cp_update_filter_tool() -> Tool {
    Tool::new(
        "cp_update_filter",
        r#"Update an existing filter's configuration.

PURPOSE: Modify filter settings. Changes are automatically pushed to Envoy via xDS.

IMPORTANT: Cannot change the filter type after creation. Create a new filter if you need a different type.

SAFE TO UPDATE: Updates to attached filters take effect immediately on all listeners/routes using the filter.

COMMON USE CASES:
- Update JWT provider configuration (new JWKS URI)
- Modify CORS allowed origins
- Adjust rate limit thresholds
- Add/remove header mutations
- Rename the filter

Required Parameters:
- name: Current name of the filter to update

Optional Parameters:
- newName: Rename the filter (updates all attachment references)
- description: New human-readable description
- configuration: New configuration object (must be valid for filter type)

TIP: Use cp_get_filter first to see current configuration and installations.

Authorization: Requires cp:write scope.
"#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Current name of the filter to update"
                },
                "newName": {
                    "type": "string",
                    "description": "New name for the filter (optional, updates all references)"
                },
                "description": {
                    "type": "string",
                    "description": "New description for the filter"
                },
                "configuration": {
                    "type": "object",
                    "description": "New filter configuration (must match existing filter type)"
                }
            },
            "required": ["name"]
        }),
    )
}

/// Returns the MCP tool definition for deleting a filter.
pub fn cp_delete_filter_tool() -> Tool {
    Tool::new(
        "cp_delete_filter",
        r#"Delete a filter from the Flowplane control plane.

PREREQUISITES FOR DELETION:
- Filter must NOT be attached to any listeners
- Filter must NOT be attached to any routes or route configs
- Detach the filter from all resources first

WILL FAIL IF:
- Filter is attached to any listener (listener_filters table)
- Filter is attached to any route config (route_config_filters table)
- Filter is attached to any route (route_filters table)

WORKFLOW TO DELETE AN ATTACHED FILTER:
1. Use cp_get_filter to see installations
2. Update listeners/routes to remove the filter attachment
3. Then delete the filter

Required Parameters:
- name: Name of the filter to delete

Authorization: Requires cp:write scope.
"#,
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the filter to delete"
                }
            },
            "required": ["name"]
        }),
    )
}

/// Execute the cp_create_filter tool using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_create_filter")]
pub async fn execute_create_filter(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    let filter_type = args.get("filterType").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: filterType".to_string())
    })?;

    let configuration = args.get("configuration").ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: configuration".to_string())
    })?;

    let description = args.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());

    tracing::debug!(
        team = %team,
        filter_name = %name,
        filter_type = %filter_type,
        "Creating filter via MCP"
    );

    // 2. Parse configuration into FilterConfig enum
    let config: crate::domain::FilterConfig = serde_json::from_value(configuration.clone())
        .map_err(|e| McpError::InvalidParams(format!("Invalid configuration: {}", e)))?;

    // 3. Use internal API layer
    let ops = FilterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    let req = CreateFilterRequest {
        name: name.to_string(),
        filter_type: filter_type.to_string(),
        description,
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        config,
    };

    let result = ops.create(req, &auth).await?;

    // 4. Format success response
    let output = json!({
        "success": true,
        "filter": {
            "id": result.data.id.to_string(),
            "name": result.data.name,
            "filterType": result.data.filter_type,
            "description": result.data.description,
            "team": result.data.team,
            "version": result.data.version,
            "createdAt": result.data.created_at.to_rfc3339(),
        },
        "message": result.message.unwrap_or_else(|| format!(
            "Filter '{}' created successfully. xDS configuration has been refreshed.",
            result.data.name
        )),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        filter_name = %result.data.name,
        filter_id = %result.data.id,
        "Successfully created filter via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_update_filter tool using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_update_filter")]
pub async fn execute_update_filter(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse filter name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        filter_name = %name,
        "Updating filter via MCP"
    );

    // 2. Parse optional updates
    let new_name = args.get("newName").and_then(|v| v.as_str()).map(|s| s.to_string());
    let new_description = args.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());
    let new_config = if let Some(config_json) = args.get("configuration") {
        Some(
            serde_json::from_value(config_json.clone())
                .map_err(|e| McpError::InvalidParams(format!("Invalid configuration: {}", e)))?,
        )
    } else {
        None
    };

    // 3. Use internal API layer
    let ops = FilterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    let req =
        UpdateFilterRequest { name: new_name, description: new_description, config: new_config };

    let result = ops.update(name, req, &auth).await?;

    // 4. Format success response
    let output = json!({
        "success": true,
        "filter": {
            "id": result.data.id.to_string(),
            "name": result.data.name,
            "filterType": result.data.filter_type,
            "description": result.data.description,
            "team": result.data.team,
            "version": result.data.version,
            "updatedAt": result.data.updated_at.to_rfc3339(),
        },
        "message": result.message.unwrap_or_else(|| format!(
            "Filter '{}' updated successfully. xDS configuration has been refreshed.",
            result.data.name
        )),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        filter_name = %result.data.name,
        filter_id = %result.data.id,
        "Successfully updated filter via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_delete_filter tool using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_delete_filter")]
pub async fn execute_delete_filter(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse filter name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        filter_name = %name,
        "Deleting filter via MCP"
    );

    // 2. Use internal API layer
    let ops = FilterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    let result = ops.delete(name, &auth).await?;

    // 3. Format success response
    let output = json!({
        "success": true,
        "message": result.message.unwrap_or_else(|| format!(
            "Filter '{}' deleted successfully. xDS configuration has been refreshed.",
            name
        )),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        filter_name = %name,
        "Successfully deleted filter via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

// =============================================================================
// FILTER ATTACHMENT TOOLS
// =============================================================================

/// Returns the MCP tool definition for attaching a filter to a resource.
pub fn cp_attach_filter_tool() -> Tool {
    Tool::new(
        "cp_attach_filter",
        r#"Attach a filter to a resource (listener or route configuration).

RESOURCE ORDER: Filters must be created BEFORE they can be attached.

ATTACHMENT HIERARCHY:
  Listener Level    - Filter applies to ALL traffic through the listener
  RouteConfig Level - Filter applies to ALL routes in the configuration
  VirtualHost Level - Filter applies to ALL routes in the virtual host
  Route Level       - Filter applies to SINGLE route only

ATTACHMENT BEHAVIOR:
- Filters are executed in order (lower numbers first)
- If no order is specified, filter is added at the end
- Same filter can be attached to multiple resources
- Attachments take effect immediately via xDS push

SUPPORTED TARGETS:
1. Listener: Attach to a named listener (affects all traffic)
   - Provide: filter, listener

2. RouteConfig: Attach to a route configuration (affects all routes in config)
   - Provide: filter, route_config
   - Optional: settings (per-scope configuration override)

COMMON USE CASES:
- Attach JWT auth filter to listener for API-wide authentication
- Attach CORS filter to specific route config for API endpoints
- Attach rate limiting to high-traffic routes

EXAMPLE (attach to listener):
{
  "filter": "api-jwt-auth",
  "listener": "main-listener",
  "order": 10
}

EXAMPLE (attach to route config with settings override):
{
  "filter": "rate-limit",
  "route_config": "api-routes",
  "order": 20,
  "settings": {"max_requests": 1000}
}

Authorization: Requires filters:write or cp:write scope.
"#,
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Name or ID of the filter to attach"
                },
                "listener": {
                    "type": "string",
                    "description": "Name of the listener to attach to (for listener-level attachment)"
                },
                "route_config": {
                    "type": "string",
                    "description": "Name of the route configuration to attach to (for route-config-level attachment)"
                },
                "order": {
                    "type": "integer",
                    "description": "Execution order (lower numbers execute first, default: append at end)",
                    "minimum": 0
                },
                "settings": {
                    "type": "object",
                    "description": "Per-scope configuration override (only for route_config attachments)"
                }
            },
            "required": ["filter"]
        }),
    )
}

/// Returns the MCP tool definition for detaching a filter from a resource.
pub fn cp_detach_filter_tool() -> Tool {
    Tool::new(
        "cp_detach_filter",
        r#"Detach a filter from a resource (listener or route configuration).

PURPOSE: Remove a filter attachment from a resource. The filter itself is not deleted,
only the association is removed.

IMPORTANT: Detaching takes effect immediately via xDS push.

SUPPORTED TARGETS:
1. Listener: Detach from a named listener
   - Provide: filter, listener

2. RouteConfig: Detach from a route configuration
   - Provide: filter, route_config

WORKFLOW:
1. Use cp_get_filter to see current attachments
2. Identify the resource to detach from
3. Call this tool with filter and target resource
4. Changes propagate immediately to Envoy

NOTE: Must specify exactly one target (listener OR route_config).

EXAMPLE:
{
  "filter": "api-jwt-auth",
  "listener": "main-listener"
}

Authorization: Requires filters:write or cp:write scope.
"#,
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Name or ID of the filter to detach"
                },
                "listener": {
                    "type": "string",
                    "description": "Name of the listener to detach from"
                },
                "route_config": {
                    "type": "string",
                    "description": "Name of the route configuration to detach from"
                }
            },
            "required": ["filter"]
        }),
    )
}

/// Returns the MCP tool definition for listing filter attachments.
pub fn cp_list_filter_attachments_tool() -> Tool {
    Tool::new(
        "cp_list_filter_attachments",
        r#"List all attachments for a specific filter.

PURPOSE: See where a filter is currently attached (listeners and route configs).
This is useful before modifying or deleting a filter.

RETURNS:
- filter: Full filter details (id, name, type, configuration)
- listener_attachments: Array of listener attachments with order
- route_config_attachments: Array of route config attachments with settings

ATTACHMENT INFO:
- resource_id: Internal ID of the attached resource
- resource_name: Name of the attached resource (listener/route_config)
- order: Execution order in the filter chain
- settings: Per-scope configuration override (route_config only)

WHEN TO USE:
- Before deleting a filter (check if it's attached anywhere)
- To audit filter usage across resources
- To understand filter execution order
- Before modifying filter configuration

EXAMPLE:
{
  "filter": "api-jwt-auth"
}

Authorization: Requires filters:read or cp:read scope.
"#,
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Name or ID of the filter to list attachments for"
                }
            },
            "required": ["filter"]
        }),
    )
}

/// Execute the cp_attach_filter tool.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_attach_filter")]
pub async fn execute_attach_filter(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required filter parameter
    let filter = args
        .get("filter")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: filter".to_string()))?;

    let listener = args.get("listener").and_then(|v| v.as_str());
    let route_config = args.get("route_config").and_then(|v| v.as_str());
    let order = args.get("order").and_then(|v| v.as_i64());
    let settings = args.get("settings").cloned();

    // 2. Validate exactly one target is specified
    if listener.is_none() && route_config.is_none() {
        return Err(McpError::InvalidParams(
            "Must specify either 'listener' or 'route_config' as attachment target".to_string(),
        ));
    }
    if listener.is_some() && route_config.is_some() {
        return Err(McpError::InvalidParams(
            "Cannot specify both 'listener' and 'route_config'. Choose one target.".to_string(),
        ));
    }

    let ops = FilterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    // 3. Execute appropriate attachment
    let (target_type, target_name) = if let Some(listener_name) = listener {
        tracing::debug!(
            team = %team,
            filter = %filter,
            listener = %listener_name,
            order = ?order,
            "Attaching filter to listener via MCP"
        );

        ops.attach_to_listener(filter, listener_name, order, &auth).await?;
        ("listener", listener_name)
    } else if let Some(route_config_name) = route_config {
        tracing::debug!(
            team = %team,
            filter = %filter,
            route_config = %route_config_name,
            order = ?order,
            "Attaching filter to route config via MCP"
        );

        ops.attach_to_route_config(filter, route_config_name, order, settings, &auth).await?;
        ("route_config", route_config_name)
    } else {
        unreachable!()
    };

    // 4. Format success response
    let output = json!({
        "success": true,
        "attachment": {
            "filter": filter,
            "target_type": target_type,
            "target_name": target_name,
            "order": order
        },
        "message": format!(
            "Filter '{}' attached to {} '{}' successfully. xDS configuration has been refreshed.",
            filter, target_type, target_name
        ),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        filter = %filter,
        target_type = %target_type,
        target_name = %target_name,
        "Successfully attached filter via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_detach_filter tool.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_detach_filter")]
pub async fn execute_detach_filter(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required filter parameter
    let filter = args
        .get("filter")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: filter".to_string()))?;

    let listener = args.get("listener").and_then(|v| v.as_str());
    let route_config = args.get("route_config").and_then(|v| v.as_str());

    // 2. Validate exactly one target is specified
    if listener.is_none() && route_config.is_none() {
        return Err(McpError::InvalidParams(
            "Must specify either 'listener' or 'route_config' as detachment target".to_string(),
        ));
    }
    if listener.is_some() && route_config.is_some() {
        return Err(McpError::InvalidParams(
            "Cannot specify both 'listener' and 'route_config'. Choose one target.".to_string(),
        ));
    }

    let ops = FilterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    // 3. Execute appropriate detachment
    let (target_type, target_name) = if let Some(listener_name) = listener {
        tracing::debug!(
            team = %team,
            filter = %filter,
            listener = %listener_name,
            "Detaching filter from listener via MCP"
        );

        ops.detach_from_listener(filter, listener_name, &auth).await?;
        ("listener", listener_name)
    } else if let Some(route_config_name) = route_config {
        tracing::debug!(
            team = %team,
            filter = %filter,
            route_config = %route_config_name,
            "Detaching filter from route config via MCP"
        );

        ops.detach_from_route_config(filter, route_config_name, &auth).await?;
        ("route_config", route_config_name)
    } else {
        unreachable!()
    };

    // 4. Format success response
    let output = json!({
        "success": true,
        "detachment": {
            "filter": filter,
            "target_type": target_type,
            "target_name": target_name
        },
        "message": format!(
            "Filter '{}' detached from {} '{}' successfully. xDS configuration has been refreshed.",
            filter, target_type, target_name
        ),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        filter = %filter,
        target_type = %target_type,
        target_name = %target_name,
        "Successfully detached filter via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_list_filter_attachments tool.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_filter_attachments")]
pub async fn execute_list_filter_attachments(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required filter parameter
    let filter = args
        .get("filter")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: filter".to_string()))?;

    tracing::debug!(
        team = %team,
        filter = %filter,
        "Listing filter attachments via MCP"
    );

    // 2. Use internal API layer
    let ops = FilterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    let filter_with_installations = ops.get_with_installations(filter, &auth).await?;
    let filter_data = &filter_with_installations.filter;

    // Parse configuration JSON
    let config: Value = serde_json::from_str(&filter_data.configuration).unwrap_or_else(|e| {
        tracing::warn!(filter_id = %filter_data.id, error = %e, "Failed to parse filter configuration");
        json!({"_parse_error": format!("Failed to parse configuration: {}", e)})
    });

    // 3. Format response
    let output = json!({
        "filter": {
            "id": filter_data.id.to_string(),
            "name": filter_data.name,
            "filter_type": filter_data.filter_type,
            "description": filter_data.description,
            "configuration": config,
            "version": filter_data.version,
            "team": filter_data.team
        },
        "listener_attachments": filter_with_installations.listener_installations.iter().map(|i| json!({
            "resource_id": i.resource_id,
            "resource_name": i.resource_name,
            "order": i.order
        })).collect::<Vec<_>>(),
        "route_config_attachments": filter_with_installations.route_config_installations.iter().map(|i| json!({
            "resource_id": i.resource_id,
            "resource_name": i.resource_name
        })).collect::<Vec<_>>(),
        "total_attachments": filter_with_installations.listener_installations.len() + filter_with_installations.route_config_installations.len()
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        filter = %filter,
        listener_count = filter_with_installations.listener_installations.len(),
        route_config_count = filter_with_installations.route_config_installations.len(),
        "Successfully listed filter attachments via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_filters_tool_definition() {
        let tool = cp_list_filters_tool();
        assert_eq!(tool.name, "cp_list_filters");
        assert!(tool.description.as_ref().unwrap().contains("filters"));
    }

    #[test]
    fn test_cp_get_filter_tool_definition() {
        let tool = cp_get_filter_tool();
        assert_eq!(tool.name, "cp_get_filter");
        assert!(tool.description.as_ref().unwrap().contains("detailed information"));

        // Check required field
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_cp_create_filter_tool_definition() {
        let tool = cp_create_filter_tool();
        assert_eq!(tool.name, "cp_create_filter");
        assert!(tool.description.as_ref().unwrap().contains("Create"));

        // Check required fields in schema
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("filterType")));
        assert!(required.contains(&json!("configuration")));
    }

    #[test]
    fn test_cp_update_filter_tool_definition() {
        let tool = cp_update_filter_tool();
        assert_eq!(tool.name, "cp_update_filter");
        assert!(tool.description.as_ref().unwrap().contains("Update"));
    }

    #[test]
    fn test_cp_delete_filter_tool_definition() {
        let tool = cp_delete_filter_tool();
        assert_eq!(tool.name, "cp_delete_filter");
        assert!(tool.description.as_ref().unwrap().contains("Delete"));
    }

    #[test]
    fn test_cp_attach_filter_tool_definition() {
        let tool = cp_attach_filter_tool();
        assert_eq!(tool.name, "cp_attach_filter");
        assert!(tool.description.as_ref().unwrap().contains("Attach"));
        assert!(tool.description.as_ref().unwrap().contains("listener"));
        assert!(tool.description.as_ref().unwrap().contains("route_config"));

        // Check required field
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("filter")));
    }

    #[test]
    fn test_cp_detach_filter_tool_definition() {
        let tool = cp_detach_filter_tool();
        assert_eq!(tool.name, "cp_detach_filter");
        assert!(tool.description.as_ref().unwrap().contains("Detach"));

        // Check required field
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("filter")));
    }

    #[test]
    fn test_cp_list_filter_attachments_tool_definition() {
        let tool = cp_list_filter_attachments_tool();
        assert_eq!(tool.name, "cp_list_filter_attachments");
        assert!(tool.description.as_ref().unwrap().contains("attachments"));

        // Check required field
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("filter")));
    }
}
