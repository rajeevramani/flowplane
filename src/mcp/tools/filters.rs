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
    Tool {
        name: "cp_list_filters".to_string(),
        description: r#"List all filters available in the Flowplane control plane.

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

RELATED TOOLS: cp_get_filter (details), cp_create_filter (create new)"#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "filter_type": {
                    "type": "string",
                    "description": "Filter by filter type (e.g., jwt_auth, oauth2, cors, rate_limit)",
                    "enum": ["jwt_auth", "oauth2", "local_rate_limit", "cors", "header_mutation", "ext_authz", "rbac", "custom_response", "compressor", "mcp"]
                }
            }
        }),
    }
}

/// Tool definition for getting a specific filter
pub fn cp_get_filter_tool() -> Tool {
    Tool {
        name: "cp_get_filter".to_string(),
        description: r#"Get detailed information about a specific filter by name.

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

RELATED TOOLS: cp_list_filters (discovery), cp_update_filter (modify), cp_delete_filter (remove)"#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the filter to retrieve"
                }
            },
            "required": ["name"]
        }),
    }
}

/// Execute list filters operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_filters")]
pub async fn execute_list_filters(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let filter_type = args["filter_type"].as_str().map(|s| s.to_string());

    // Use internal API layer
    let ops = FilterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);

    let req = ListFiltersRequest { filter_type, include_defaults: true, ..Default::default() };

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
    Tool {
        name: "cp_create_filter".to_string(),
        description: r#"Create a new filter in the Flowplane control plane.

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
"#
        .to_string(),
        input_schema: json!({
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
    }
}

/// Returns the MCP tool definition for updating a filter.
pub fn cp_update_filter_tool() -> Tool {
    Tool {
        name: "cp_update_filter".to_string(),
        description: r#"Update an existing filter's configuration.

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
"#.to_string(),
        input_schema: json!({
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
    }
}

/// Returns the MCP tool definition for deleting a filter.
pub fn cp_delete_filter_tool() -> Tool {
    Tool {
        name: "cp_delete_filter".to_string(),
        description: r#"Delete a filter from the Flowplane control plane.

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
"#
        .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the filter to delete"
                }
            },
            "required": ["name"]
        }),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_filters_tool_definition() {
        let tool = cp_list_filters_tool();
        assert_eq!(tool.name, "cp_list_filters");
        assert!(tool.description.contains("filters"));
    }

    #[test]
    fn test_cp_get_filter_tool_definition() {
        let tool = cp_get_filter_tool();
        assert_eq!(tool.name, "cp_get_filter");
        assert!(tool.description.contains("detailed information"));

        // Check required field
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_cp_create_filter_tool_definition() {
        let tool = cp_create_filter_tool();
        assert_eq!(tool.name, "cp_create_filter");
        assert!(tool.description.contains("Create"));

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
        assert!(tool.description.contains("Update"));
    }

    #[test]
    fn test_cp_delete_filter_tool_definition() {
        let tool = cp_delete_filter_tool();
        assert_eq!(tool.name, "cp_delete_filter");
        assert!(tool.description.contains("Delete"));
    }
}
