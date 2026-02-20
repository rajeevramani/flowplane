//! Virtual Host MCP Tools
//!
//! Control Plane tools for managing virtual hosts within route configurations.

use crate::domain::OrgId;
use crate::internal_api::{
    InternalAuthContext, ListVirtualHostsRequest, UpdateVirtualHostRequest, VirtualHostOperations,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::mcp::response_builders::{
    build_delete_response, build_rich_create_response, build_update_response,
};
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Tool definition for listing virtual hosts
pub fn cp_list_virtual_hosts_tool() -> Tool {
    Tool::new(
        "cp_list_virtual_hosts",
        r#"List all virtual hosts within route configurations.

RESOURCE ORDER: Virtual hosts are within Route Configurations (order 2.5 of 4).
Virtual hosts are created within route configs, which depend on clusters.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
                         │
                    contains virtual hosts
                         │
                    contains routes

PURPOSE: Discover existing virtual hosts to:
- Understand domain-based routing configuration
- Find virtual hosts to attach routes to
- Plan new virtual host additions
- Check virtual host ordering (rule_order determines matching priority)

HIERARCHY:
  Route Config → Virtual Hosts → Routes
  - Route configs contain multiple virtual hosts
  - Virtual hosts match domains (e.g., "api.example.com", "*.example.com", "*")
  - Virtual hosts contain routes that match paths

RETURNS: Array of virtual host objects with:
- id: Internal virtual host identifier
- name: Virtual host name (unique within route config)
- route_config_id: Parent route configuration ID
- domains: Array of domain patterns this virtual host matches
- rule_order: Priority order (lower numbers match first)
- created_at/updated_at: Timestamps

RELATED TOOLS: cp_get_virtual_host (details), cp_create_virtual_host (create), cp_list_routes (routes within virtual hosts)"#,
        json!({
            "type": "object",
            "properties": {
                "route_config": {
                    "type": "string",
                    "description": "Filter by route configuration name to see virtual hosts in a specific config"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of virtual hosts to return (default: 100, max: 1000)",
                    "minimum": 1,
                    "maximum": 1000
                },
                "offset": {
                    "type": "integer",
                    "description": "Number of virtual hosts to skip (for pagination)",
                    "minimum": 0
                }
            }
        }),
    )
}

/// Tool definition for getting a virtual host by name
pub fn cp_get_virtual_host_tool() -> Tool {
    Tool::new(
        "cp_get_virtual_host",
        r#"Get detailed information about a specific virtual host.

PURPOSE: Retrieve complete virtual host configuration to understand domain matching
before modifying or adding routes to it.

RETURNS: Full virtual host details including:
- id: Internal identifier
- name: Virtual host name (use when creating routes)
- route_config_id: Parent route configuration ID
- domains: Array of domain patterns (e.g., ["api.example.com", "*.example.com"])
- rule_order: Priority order for matching (lower numbers match first)
- created_at/updated_at: Timestamps

DOMAIN MATCHING:
- Exact match: "api.example.com" matches only that domain
- Wildcard prefix: "*.example.com" matches any subdomain
- Catch-all: "*" matches all domains

WHEN TO USE:
- Before adding routes to a virtual host
- To check domain configuration
- To verify virtual host exists before referencing it
- Before updating a virtual host

RELATED TOOLS: cp_list_virtual_hosts (discovery), cp_update_virtual_host (modify), cp_list_routes (routes within)"#,
        json!({
            "type": "object",
            "properties": {
                "route_config": {
                    "type": "string",
                    "description": "The route configuration name containing the virtual host"
                },
                "name": {
                    "type": "string",
                    "description": "The virtual host name to retrieve"
                }
            },
            "required": ["route_config", "name"]
        }),
    )
}

/// Tool definition for creating a virtual host
pub fn cp_create_virtual_host_tool() -> Tool {
    Tool::new(
        "cp_create_virtual_host",
        r#"Create a new virtual host within a route configuration.

RESOURCE ORDER: Virtual hosts are within Route Configurations (order 2.5 of 4).
PREREQUISITE: The route config must exist first.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
                         │
                    you are here (virtual host)

CREATION WORKFLOW:
1. First, ensure route config exists (cp_list_route_configs or cp_create_route_config)
2. Create this virtual host within the route config
3. Then, add routes to the virtual host (these routes will reference clusters)

STRUCTURE:
Virtual hosts provide domain-level routing within a route configuration.
Multiple virtual hosts in a route config allow different domain patterns to have
different route rules.

DOMAIN PATTERNS:
- Exact: "api.example.com" - matches only this domain
- Wildcard: "*.example.com" - matches any subdomain (api.example.com, web.example.com)
- Catch-all: "*" - matches all domains (use when domain doesn't matter)

RULE ORDER:
Virtual hosts are matched by rule_order (lower numbers match first).
If multiple virtual hosts match a request's domain, the one with the lowest
rule_order is selected.

EXAMPLE:
{
  "route_config": "api-routes",
  "name": "api-v1",
  "domains": ["api.example.com", "api-v1.example.com"],
  "rule_order": 10
}

NEXT STEPS:
After creating a virtual host, create routes within it using cp_create_route
(when that tool becomes available) or update the route config to include routes.

Authorization: Requires cp:write scope."#,
        json!({
            "type": "object",
            "properties": {
                "route_config": {
                    "type": "string",
                    "description": "Name of the route configuration to add this virtual host to"
                },
                "name": {
                    "type": "string",
                    "description": "Unique name for the virtual host (within the route config)"
                },
                "domains": {
                    "type": "array",
                    "description": "Domain patterns to match (e.g., ['api.example.com'], ['*'])",
                    "items": {"type": "string"},
                    "minItems": 1
                },
                "rule_order": {
                    "type": "integer",
                    "description": "Priority order for matching (default: 0, lower numbers match first)",
                    "minimum": 0
                }
            },
            "required": ["route_config", "name", "domains"]
        }),
    )
}

/// Tool definition for updating a virtual host
pub fn cp_update_virtual_host_tool() -> Tool {
    Tool::new(
        "cp_update_virtual_host",
        r#"Update an existing virtual host.

PURPOSE: Modify virtual host configuration (domains, rule_order).
Changes take effect immediately for all routes within the virtual host.

SAFE TO UPDATE: Virtual host updates do not affect routes within it.
Routes reference the virtual host by ID, which doesn't change.

COMMON USE CASES:
- Add or remove domain patterns
- Change matching priority (rule_order)
- Consolidate multiple domains into one virtual host
- Split domains into separate virtual hosts

IMPORTANT: Virtual host name cannot be changed.
To rename, create a new virtual host and migrate routes.

Required Parameters:
- route_config: Name of the route configuration
- name: Name of the virtual host to update

Optional Parameters (provide at least one):
- domains: New array of domain patterns (REPLACES all existing)
- rule_order: New priority order for matching

TIP: Use cp_get_virtual_host first to see current configuration before updating.

Authorization: Requires cp:write scope."#,
        json!({
            "type": "object",
            "properties": {
                "route_config": {
                    "type": "string",
                    "description": "Name of the route configuration containing the virtual host"
                },
                "name": {
                    "type": "string",
                    "description": "Name of the virtual host to update"
                },
                "domains": {
                    "type": "array",
                    "description": "New array of domain patterns (replaces existing)",
                    "items": {"type": "string"},
                    "minItems": 1
                },
                "rule_order": {
                    "type": "integer",
                    "description": "New priority order for matching",
                    "minimum": 0
                }
            },
            "required": ["route_config", "name"]
        }),
    )
}

/// Tool definition for deleting a virtual host
pub fn cp_delete_virtual_host_tool() -> Tool {
    Tool::new(
        "cp_delete_virtual_host",
        r#"Delete a virtual host from a route configuration.

DELETION ORDER: Delete in REVERSE order of creation.
Delete routes within the virtual host FIRST, then delete the virtual host.

ORDER: [Listeners] ─► [Route Configs] ─► [Virtual Hosts] ─► [Routes]

PREREQUISITES FOR DELETION:
- All routes within this virtual host must be deleted first
- If routes exist, they will be deleted automatically (CASCADE DELETE)

WARNING: Deleting a virtual host deletes ALL routes within it.
This is a destructive operation that cannot be undone.

WORKFLOW:
1. Use cp_list_routes to see routes in this virtual host
2. Consider if you want to preserve any routes (move them to another virtual host)
3. Delete the virtual host (routes will be deleted automatically)

Required Parameters:
- route_config: Name of the route configuration
- name: Name of the virtual host to delete

Authorization: Requires cp:write scope."#,
        json!({
            "type": "object",
            "properties": {
                "route_config": {
                    "type": "string",
                    "description": "Name of the route configuration containing the virtual host"
                },
                "name": {
                    "type": "string",
                    "description": "Name of the virtual host to delete"
                }
            },
            "required": ["route_config", "name"]
        }),
    )
}

/// Execute list virtual hosts operation
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_virtual_hosts")]
pub async fn execute_list_virtual_hosts(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let route_config = args.get("route_config").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32);
    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32);

    tracing::debug!(
        team = %team,
        route_config = ?route_config,
        limit = ?limit,
        offset = ?offset,
        "Listing virtual hosts"
    );

    // Use internal API layer
    let ops = VirtualHostOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let list_req =
        ListVirtualHostsRequest { route_config: route_config.map(String::from), limit, offset };

    let result = ops.list(list_req, &auth).await?;

    // Build output
    let virtual_hosts_json: Vec<Value> = result
        .virtual_hosts
        .iter()
        .map(|vh| {
            json!({
                "id": vh.id,
                "route_config_id": vh.route_config_id,
                "name": vh.name,
                "domains": vh.domains,
                "rule_order": vh.rule_order,
                "created_at": vh.created_at.to_rfc3339(),
                "updated_at": vh.updated_at.to_rfc3339()
            })
        })
        .collect();

    let output = json!({
        "virtual_hosts": virtual_hosts_json,
        "count": result.count,
        "limit": limit,
        "offset": offset
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        virtual_host_count = result.count,
        "Successfully listed virtual hosts"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute get virtual host operation
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_virtual_host")]
pub async fn execute_get_virtual_host(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let route_config = args.get("route_config").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: route_config".to_string())
    })?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        route_config = %route_config,
        name = %name,
        "Getting virtual host by name"
    );

    // Use internal API layer
    let ops = VirtualHostOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let virtual_host = ops.get(route_config, name, &auth).await?;

    let output = json!({
        "id": virtual_host.id,
        "route_config_id": virtual_host.route_config_id,
        "name": virtual_host.name,
        "domains": virtual_host.domains,
        "rule_order": virtual_host.rule_order,
        "created_at": virtual_host.created_at.to_rfc3339(),
        "updated_at": virtual_host.updated_at.to_rfc3339()
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config = %route_config,
        name = %name,
        "Successfully retrieved virtual host"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute create virtual host operation
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_create_virtual_host")]
pub async fn execute_create_virtual_host(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let route_config = args.get("route_config").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: route_config".to_string())
    })?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    let domains_json = args.get("domains").ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: domains".to_string())
    })?;

    // Parse domains array
    let domains = domains_json.as_array().ok_or_else(|| {
        McpError::InvalidParams("domains must be an array of strings".to_string())
    })?;

    let domains: Vec<String> = domains
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| McpError::InvalidParams("Each domain must be a string".to_string()))
                .map(String::from)
        })
        .collect::<Result<Vec<String>, McpError>>()?;

    if domains.is_empty() {
        return Err(McpError::InvalidParams("At least one domain is required".to_string()));
    }

    let rule_order = args.get("rule_order").and_then(|v| v.as_i64()).map(|v| v as i32);

    tracing::debug!(
        team = %team,
        route_config = %route_config,
        name = %name,
        domains = ?domains,
        "Creating virtual host via MCP"
    );

    // 2. Use internal API layer
    let ops = VirtualHostOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = crate::internal_api::CreateVirtualHostRequest {
        route_config: route_config.to_string(),
        name: name.to_string(),
        domains,
        rule_order,
    };

    let result = ops.create(req, &auth).await?;

    // 3. Format rich response with domain/parent context and next-step guidance
    let domain_list: Vec<&str> = domains_json
        .as_array()
        .map_or(vec![], |arr| arr.iter().filter_map(|v| v.as_str()).collect());
    let output = build_rich_create_response(
        "virtual_host",
        &result.data.name,
        result.data.id.as_ref(),
        Some(json!({"domains": domain_list, "route_config": route_config})),
        None,
        Some("Add routes with cp_create_route to define path matching rules"),
    );

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config = %route_config,
        virtual_host_name = %result.data.name,
        virtual_host_id = %result.data.id,
        "Successfully created virtual host via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute update virtual host operation
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_update_virtual_host")]
pub async fn execute_update_virtual_host(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let route_config = args.get("route_config").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: route_config".to_string())
    })?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    // 2. Parse optional fields
    let domains = if let Some(domains_json) = args.get("domains") {
        let domains_array = domains_json.as_array().ok_or_else(|| {
            McpError::InvalidParams("domains must be an array of strings".to_string())
        })?;

        let parsed_domains: Vec<String> = domains_array
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| {
                        McpError::InvalidParams("Each domain must be a string".to_string())
                    })
                    .map(String::from)
            })
            .collect::<Result<Vec<String>, McpError>>()?;

        if parsed_domains.is_empty() {
            return Err(McpError::InvalidParams(
                "At least one domain is required when updating domains".to_string(),
            ));
        }

        Some(parsed_domains)
    } else {
        None
    };

    let rule_order = args.get("rule_order").and_then(|v| v.as_i64()).map(|v| v as i32);

    // Ensure at least one field is being updated
    if domains.is_none() && rule_order.is_none() {
        return Err(McpError::InvalidParams(
            "At least one of 'domains' or 'rule_order' must be provided".to_string(),
        ));
    }

    tracing::debug!(
        team = %team,
        route_config = %route_config,
        name = %name,
        "Updating virtual host via MCP"
    );

    // 3. Use internal API layer
    let ops = VirtualHostOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = UpdateVirtualHostRequest { domains, rule_order };

    let result = ops.update(route_config, name, req, &auth).await?;

    // 4. Format success response (minimal token-efficient format)
    let output = build_update_response("virtual_host", &result.data.name, result.data.id.as_ref());

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config = %route_config,
        virtual_host_name = %result.data.name,
        virtual_host_id = %result.data.id,
        "Successfully updated virtual host via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute delete virtual host operation
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_delete_virtual_host")]
pub async fn execute_delete_virtual_host(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let route_config = args.get("route_config").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: route_config".to_string())
    })?;

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        route_config = %route_config,
        name = %name,
        "Deleting virtual host via MCP"
    );

    // 2. Use internal API layer
    let ops = VirtualHostOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    ops.delete(route_config, name, &auth).await?;

    // 3. Format success response (minimal token-efficient format)
    let output = build_delete_response();

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        route_config = %route_config,
        name = %name,
        "Successfully deleted virtual host via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_virtual_hosts_tool_definition() {
        let tool = cp_list_virtual_hosts_tool();
        assert_eq!(tool.name, "cp_list_virtual_hosts");
        assert!(tool.description.as_ref().unwrap().contains("virtual hosts"));
        assert!(tool.description.as_ref().unwrap().contains("RESOURCE ORDER"));
    }

    #[test]
    fn test_cp_get_virtual_host_tool_definition() {
        let tool = cp_get_virtual_host_tool();
        assert_eq!(tool.name, "cp_get_virtual_host");
        assert!(tool.description.as_ref().unwrap().contains("Get detailed information"));

        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("route_config")));
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_cp_create_virtual_host_tool_definition() {
        let tool = cp_create_virtual_host_tool();
        assert_eq!(tool.name, "cp_create_virtual_host");
        assert!(tool.description.as_ref().unwrap().contains("Create"));

        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("route_config")));
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("domains")));
    }

    #[test]
    fn test_cp_update_virtual_host_tool_definition() {
        let tool = cp_update_virtual_host_tool();
        assert_eq!(tool.name, "cp_update_virtual_host");
        assert!(tool.description.as_ref().unwrap().contains("Update"));

        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("route_config")));
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_cp_delete_virtual_host_tool_definition() {
        let tool = cp_delete_virtual_host_tool();
        assert_eq!(tool.name, "cp_delete_virtual_host");
        assert!(tool.description.as_ref().unwrap().contains("Delete"));

        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("route_config")));
        assert!(required.contains(&json!("name")));
    }
}
