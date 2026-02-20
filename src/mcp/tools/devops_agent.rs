//! DevOps Agent MCP Tools
//!
//! Status aggregation tool for deployment health monitoring.

use crate::domain::OrgId;
use crate::internal_api::{
    ClusterOperations, FilterOperations, InternalAuthContext, ListClustersRequest,
    ListFiltersRequest, ListListenersRequest, ListRouteConfigsRequest, ListenerOperations,
    RouteConfigOperations,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

// =============================================================================
// TOOL DEFINITIONS
// =============================================================================

/// DevOps tool for getting deployment status
pub fn devops_get_deployment_status_tool() -> Tool {
    Tool::new(
        "devops_get_deployment_status",
        r#"Get aggregated deployment status across clusters, listeners, and filters.

PURPOSE: Provides a comprehensive view of deployment health by aggregating
status from multiple resources.

USE CASES:
- Health check before/after deployments
- Troubleshoot deployment issues
- Verify all components are properly configured
- Dashboard/monitoring integration

PARAMETERS:
- cluster_names: List of cluster names to check (optional - checks all if empty)
- listener_names: List of listener names to check (optional - checks all if empty)
- filter_names: List of filter names to check (optional - checks all if empty)
- include_details: Include full configuration details (default: false)

EXAMPLE:
{
  "cluster_names": ["user-service", "order-service"],
  "listener_names": ["http-ingress"],
  "include_details": true
}

RETURNS: Aggregated status with:
- summary: Overall health indicators with counts
- clusters: List of cluster statuses
- listeners: List of listener statuses
- filters: List of filter statuses

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "cluster_names": {
                    "type": "array",
                    "description": "List of cluster names to check (optional - checks all if empty)",
                    "items": {"type": "string"}
                },
                "listener_names": {
                    "type": "array",
                    "description": "List of listener names to check (optional - checks all if empty)",
                    "items": {"type": "string"}
                },
                "filter_names": {
                    "type": "array",
                    "description": "List of filter names to check (optional - checks all if empty)",
                    "items": {"type": "string"}
                },
                "include_details": {
                    "type": "boolean",
                    "description": "Include full configuration details (default: false)",
                    "default": false
                }
            }
        }),
    )
}

// =============================================================================
// EXECUTE FUNCTIONS
// =============================================================================

/// Execute devops_get_deployment_status: Get aggregated status
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_devops_get_deployment_status")]
pub async fn execute_devops_get_deployment_status(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let cluster_names: Vec<String> = args
        .get("cluster_names")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let listener_names: Vec<String> = args
        .get("listener_names")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let filter_names: Vec<String> = args
        .get("filter_names")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let include_details = args.get("include_details").and_then(|v| v.as_bool()).unwrap_or(false);

    tracing::info!(
        team = %team,
        cluster_count = cluster_names.len(),
        listener_count = listener_names.len(),
        filter_count = filter_names.len(),
        "Getting deployment status"
    );

    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    // Get cluster status
    let cluster_ops = ClusterOperations::new(xds_state.clone());
    let cluster_list_req =
        ListClustersRequest { limit: Some(100), offset: Some(0), include_defaults: true };
    let clusters_result = cluster_ops.list(cluster_list_req, &auth).await?;

    let cluster_statuses: Vec<Value> = clusters_result
        .clusters
        .iter()
        .filter(|c| cluster_names.is_empty() || cluster_names.contains(&c.name))
        .map(|c| {
            let config: Value = serde_json::from_str(&c.configuration).unwrap_or_default();
            let endpoint_count =
                config.get("endpoints").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);

            let mut status = json!({
                "name": c.name,
                "endpoint_count": endpoint_count,
                "healthy": endpoint_count > 0,
                "version": c.version,
                "updated_at": c.updated_at.to_rfc3339()
            });

            if include_details {
                status["configuration"] = config;
            }

            status
        })
        .collect();

    // Get listener status
    let listener_ops = ListenerOperations::new(xds_state.clone());
    let listener_list_req =
        ListListenersRequest { limit: Some(100), offset: Some(0), include_defaults: true };
    let listeners_result = listener_ops.list(listener_list_req, &auth).await?;

    let listener_statuses: Vec<Value> = listeners_result
        .listeners
        .iter()
        .filter(|l| listener_names.is_empty() || listener_names.contains(&l.name))
        .map(|l| {
            let mut status = json!({
                "name": l.name,
                "address": l.address,
                "port": l.port,
                "protocol": l.protocol,
                "healthy": true,
                "version": l.version,
                "updated_at": l.updated_at.to_rfc3339()
            });

            if include_details {
                // Parse configuration to extract additional details
                if let Ok(config) = serde_json::from_str::<Value>(&l.configuration) {
                    status["configuration"] = config;
                }
            }

            status
        })
        .collect();

    // Get filter status
    let filter_ops = FilterOperations::new(xds_state.clone());
    let filter_list_req = ListFiltersRequest { include_defaults: true, ..Default::default() };
    let filters_result = filter_ops.list(filter_list_req, &auth).await?;

    let filter_statuses: Vec<Value> = filters_result
        .filters
        .iter()
        .filter(|f| filter_names.is_empty() || filter_names.contains(&f.name))
        .map(|f| {
            let config: Value = serde_json::from_str(&f.configuration).unwrap_or_default();

            let mut status = json!({
                "name": f.name,
                "filter_type": f.filter_type,
                "healthy": true,
                "version": f.version,
                "updated_at": f.updated_at.to_rfc3339()
            });

            if include_details {
                status["configuration"] = config;
            }

            status
        })
        .collect();

    // Get route config count
    let route_ops = RouteConfigOperations::new(xds_state.clone());
    let route_list_req =
        ListRouteConfigsRequest { limit: Some(100), offset: Some(0), include_defaults: true };
    let routes_result = route_ops.list(route_list_req, &auth).await?;

    // Build summary
    let total_healthy =
        cluster_statuses.iter().filter(|c| c["healthy"].as_bool().unwrap_or(false)).count()
            + listener_statuses.iter().filter(|l| l["healthy"].as_bool().unwrap_or(false)).count()
            + filter_statuses.iter().filter(|f| f["healthy"].as_bool().unwrap_or(false)).count();

    let total_resources = cluster_statuses.len() + listener_statuses.len() + filter_statuses.len();

    let overall_health = if total_resources == 0 {
        "unknown"
    } else if total_healthy == total_resources {
        "healthy"
    } else if total_healthy > 0 {
        "degraded"
    } else {
        "unhealthy"
    };

    let output = json!({
        "success": true,
        "summary": {
            "overall_health": overall_health,
            "clusters_count": cluster_statuses.len(),
            "listeners_count": listener_statuses.len(),
            "filters_count": filter_statuses.len(),
            "route_configs_count": routes_result.count,
            "healthy_resources": total_healthy,
            "total_resources": total_resources
        },
        "clusters": cluster_statuses,
        "listeners": listener_statuses,
        "filters": filter_statuses,
        "message": format!("Deployment status: {} ({}/{} resources healthy)", overall_health, total_healthy, total_resources)
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_devops_get_deployment_status_tool_definition() {
        let tool = devops_get_deployment_status_tool();
        assert_eq!(tool.name, "devops_get_deployment_status");
        assert!(tool.description.as_ref().unwrap().contains("deployment status"));

        let schema = tool.input_schema;
        // All parameters are optional for status check
        assert!(
            schema["required"].is_null()
                || schema["required"].as_array().map(|a| a.is_empty()).unwrap_or(true)
        );
    }

    #[test]
    fn test_all_tools_have_valid_schemas() {
        let tools = vec![devops_get_deployment_status_tool()];

        for tool in tools {
            // Verify tool has name and description
            assert!(!tool.name.is_empty(), "Tool name should not be empty");
            assert!(
                tool.description.as_ref().is_some_and(|d| !d.is_empty()),
                "Tool description should not be empty"
            );

            // Verify schema is valid JSON object
            assert!(tool.input_schema.is_object(), "Tool schema should be an object");
            assert_eq!(tool.input_schema["type"], "object", "Tool schema type should be 'object'");
            assert!(
                tool.input_schema["properties"].is_object(),
                "Tool schema should have properties"
            );
        }
    }
}
