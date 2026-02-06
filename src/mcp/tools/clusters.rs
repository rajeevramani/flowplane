//! MCP Tools for Cluster Control Plane Operations
//!
//! Provides tools for querying, creating, updating, and deleting cluster configurations
//! via the MCP protocol.
//!
//! The tools use the internal API layer (`ClusterOperations`) for unified
//! validation and team-based access control.

use crate::internal_api::{
    ClusterOperations, CreateClusterRequest as InternalCreateRequest, InternalAuthContext,
    ListClustersRequest, UpdateClusterRequest as InternalUpdateRequest,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::mcp::response_builders::{
    build_create_response, build_delete_response, build_query_response, build_update_response,
    ResourceRef,
};
use crate::storage::repositories::ClusterEndpointRepository;
use crate::xds::{ClusterSpec, EndpointSpec, XdsState};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Returns the MCP tool definition for listing clusters.
///
/// This tool supports pagination via `limit` and `offset` parameters.
pub fn cp_list_clusters_tool() -> Tool {
    Tool::new("cp_list_clusters", r#"List all upstream clusters in the Flowplane control plane.

RESOURCE ORDER: Clusters are foundational resources (order 1 of 4). Create clusters BEFORE route configurations.

PURPOSE: Use this tool to discover existing backend services before creating routes. Route configurations
reference clusters by name in their forwarding actions.

RETURNS: Array of cluster summaries with:
- name: Unique cluster identifier (used in route actions)
- service_name: Human-readable service description
- version: Configuration version for optimistic locking
- team: Owning team for multi-tenancy
- created_at/updated_at: Timestamps

WORKFLOW CONTEXT:
1. Call cp_list_clusters to see available backends
2. Use cluster names when creating route configs with "action": {"type": "forward", "cluster": "<cluster-name>"}
3. Create new clusters with cp_create_cluster if needed

RELATED TOOLS: cp_get_cluster (details), cp_create_cluster (create), cp_list_route_configs (routes using clusters)"#.to_string(), json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of clusters to return (default: 50, max: 1000)",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 50
                },
                "offset": {
                    "type": "integer",
                    "description": "Number of clusters to skip for pagination (default: 0)",
                    "minimum": 0,
                    "default": 0
                }
            }
        }))
}

/// Returns the MCP tool definition for getting a cluster by name.
///
/// Requires a `name` parameter to identify the cluster.
pub fn cp_get_cluster_tool() -> Tool {
    Tool::new("cp_get_cluster",
        r#"Get detailed information about a specific cluster by name.

PURPOSE: Retrieve complete cluster configuration to understand backend setup before modifying or
referencing it in route configurations.

RETURNS: Full cluster details including:
- id: Internal identifier
- name: Unique cluster name (use this in route actions)
- service_name: Human-readable service name
- configuration: Complete config with endpoints, load balancing, health checks, circuit breakers
- version: For optimistic locking during updates
- team: Owning team

CONFIGURATION DETAILS:
- endpoints: Array of {address, port} for backend servers
- lbPolicy: Load balancing strategy (ROUND_ROBIN, LEAST_REQUEST, etc.)
- healthCheck: Health check settings if configured
- circuitBreakers: Circuit breaker thresholds if configured
- useTls: Whether TLS is enabled for upstream connections

WHEN TO USE:
- Before updating a cluster (to see current config)
- To verify endpoint configuration
- To check health check settings
- Before referencing cluster in a new route config

RELATED TOOLS: cp_list_clusters (discovery), cp_update_cluster (modify), cp_create_route_config (reference)"#.to_string(),
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the cluster to retrieve"
                }
            },
            "required": ["name"]
        }))
}

/// Returns the MCP tool definition for getting cluster health status.
///
/// Aggregates endpoint health for a specific cluster.
pub fn cp_get_cluster_health_tool() -> Tool {
    Tool::new(
        "cp_get_cluster_health",
        r#"Get aggregated endpoint health status for a cluster.

PURPOSE: Check backend health before deployments or troubleshooting.

RETURNS: Health summary with:
- total_endpoints: Total endpoint count
- healthy: Count of healthy endpoints
- unhealthy: Count of unhealthy endpoints
- degraded: Count of degraded endpoints
- unknown: Count of endpoints with unknown status
- endpoints: Array of individual endpoint health (address, port, status)

TOKEN BUDGET: <80 tokens response

Authorization: Requires clusters:read or cp:read scope."#
            .to_string(),
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the cluster to check health for"
                }
            },
            "required": ["name"]
        }),
    )
}

/// Execute the cp_list_clusters tool.
///
/// Lists clusters with pagination, returning pretty-printed JSON output.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_clusters")]
pub async fn execute_list_clusters(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(50));
    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(0));

    tracing::debug!(team = %team, limit = ?limit, offset = ?offset, "Listing clusters for team");

    // Use internal API layer
    let ops = ClusterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    let list_req = ListClustersRequest {
        limit,
        offset,
        include_defaults: true, // MCP includes default resources
    };

    let result = ops.list(list_req, &auth).await?;

    // Build output with cluster summaries
    let cluster_summaries: Vec<Value> = result
        .clusters
        .iter()
        .map(|cluster| {
            let mut summary = json!({
                "name": cluster.name,
                "service_name": cluster.service_name,
                "version": cluster.version,
                "source": cluster.source,
                "team": cluster.team,
                "created_at": cluster.created_at.to_rfc3339(),
                "updated_at": cluster.updated_at.to_rfc3339(),
            });

            // Parse configuration to extract description/tags if present
            if let Ok(config) = serde_json::from_str::<Value>(&cluster.configuration) {
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
        "clusters": cluster_summaries,
        "count": result.count,
        "limit": limit,
        "offset": offset,
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, cluster_count = result.count, "Successfully listed clusters");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_get_cluster tool.
///
/// Retrieves a specific cluster by name, returning detailed configuration.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_cluster")]
pub async fn execute_get_cluster(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(team = %team, cluster_name = %name, "Getting cluster by name");

    // Use internal API layer
    let ops = ClusterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    let cluster = ops.get(name, &auth).await?;

    // Parse configuration JSON for pretty output
    let configuration: Value =
        serde_json::from_str(&cluster.configuration).map_err(McpError::SerializationError)?;

    let output = json!({
        "id": cluster.id.to_string(),
        "name": cluster.name,
        "service_name": cluster.service_name,
        "configuration": configuration,
        "version": cluster.version,
        "source": cluster.source,
        "team": cluster.team,
        "import_id": cluster.import_id,
        "created_at": cluster.created_at.to_rfc3339(),
        "updated_at": cluster.updated_at.to_rfc3339(),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, cluster_name = %name, "Successfully retrieved cluster");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Returns the MCP tool definition for creating a cluster.
///
/// This tool creates a new upstream cluster with the specified configuration.
pub fn cp_create_cluster_tool() -> Tool {
    Tool::new(
        "cp_create_cluster",
        r#"Create a new upstream cluster (backend service) in the Flowplane control plane.

RESOURCE ORDER: Clusters are foundational resources (order 1 of 4).
CREATE CLUSTERS FIRST, before route configurations that reference them.

DEPENDENCY GRAPH:
  [Clusters] ─────► [Route Configs] ─────► [Listeners]
  [Filters]  ───────────┘

A cluster represents a logical group of endpoints (backend services) with load balancing,
health checking, and resilience policies. After creation, xDS configuration is automatically
refreshed and pushed to connected Envoy proxies.

NEXT STEPS AFTER CREATING CLUSTER:
1. Create a route configuration that forwards traffic to this cluster
2. Create a listener to expose the route configuration

Required Parameters:
- name: Unique cluster identifier (alphanumeric with hyphens, 1-63 characters)
- serviceName: Descriptive name for the service
- endpoints: Array of endpoint objects. Each endpoint has:
  - address: IP address or hostname (e.g., "10.0.1.10" or "api.example.com")
  - port: Port number (1-65535)

Optional Parameters:
- connectTimeoutSeconds: Connection timeout in seconds (default: 5, max: 300)
- lbPolicy: Load balancing policy:
  - "ROUND_ROBIN" (default): Distribute requests evenly
  - "LEAST_REQUEST": Send to endpoint with fewest active requests
  - "RANDOM": Random endpoint selection
  - "RING_HASH": Consistent hashing
  - "MAGLEV": Maglev consistent hashing
- useTls: Enable TLS for upstream connections (default: false)
- healthCheck: Health check configuration object:
  - type: "http" or "tcp"
  - path: HTTP path for health checks (required for HTTP)
  - intervalSeconds: Check interval (default: 10)
  - timeoutSeconds: Check timeout (default: 5)
  - healthyThreshold: Successes before healthy (default: 2)
  - unhealthyThreshold: Failures before unhealthy (default: 3)
- circuitBreakers: Circuit breaker configuration:
  - maxConnections: Maximum concurrent connections (default: 1024)
  - maxPendingRequests: Maximum pending requests (default: 1024)
  - maxRequests: Maximum concurrent requests (default: 1024)
  - maxRetries: Maximum retries (default: 3)

Example:
{
  "name": "api-backend",
  "serviceName": "api.example.com",
  "endpoints": [
    {"address": "10.0.1.10", "port": 8080},
    {"address": "10.0.1.11", "port": 8080}
  ],
  "lbPolicy": "ROUND_ROBIN",
  "connectTimeoutSeconds": 10,
  "healthCheck": {
    "type": "http",
    "path": "/health"
  }
}

Authorization: Requires cp:write scope.
"#
        .to_string(),
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique cluster name (alphanumeric with hyphens)",
                    "pattern": "^[a-z0-9][a-z0-9-]*[a-z0-9]$|^[a-z0-9]$",
                    "minLength": 1,
                    "maxLength": 63
                },
                "serviceName": {
                    "type": "string",
                    "description": "Descriptive service name for this cluster",
                    "minLength": 1
                },
                "endpoints": {
                    "type": "array",
                    "description": "List of backend endpoints",
                    "minItems": 1,
                    "maxItems": 100,
                    "items": {
                        "type": "object",
                        "properties": {
                            "address": {
                                "type": "string",
                                "description": "IP address or hostname"
                            },
                            "port": {
                                "type": "integer",
                                "description": "Port number",
                                "minimum": 1,
                                "maximum": 65535
                            }
                        },
                        "required": ["address", "port"]
                    }
                },
                "connectTimeoutSeconds": {
                    "type": "integer",
                    "description": "Connection timeout in seconds",
                    "minimum": 1,
                    "maximum": 300,
                    "default": 5
                },
                "lbPolicy": {
                    "type": "string",
                    "description": "Load balancing policy",
                    "enum": ["ROUND_ROBIN", "LEAST_REQUEST", "RANDOM", "RING_HASH", "MAGLEV"],
                    "default": "ROUND_ROBIN"
                },
                "useTls": {
                    "type": "boolean",
                    "description": "Enable TLS for upstream connections",
                    "default": false
                },
                "healthCheck": {
                    "type": "object",
                    "description": "Health check configuration",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["http", "tcp"],
                            "description": "Health check protocol"
                        },
                        "path": {
                            "type": "string",
                            "description": "HTTP path for health checks (required for HTTP)"
                        },
                        "intervalSeconds": {
                            "type": "integer",
                            "default": 10,
                            "minimum": 1
                        },
                        "timeoutSeconds": {
                            "type": "integer",
                            "default": 5,
                            "minimum": 1
                        },
                        "healthyThreshold": {
                            "type": "integer",
                            "default": 2,
                            "minimum": 1
                        },
                        "unhealthyThreshold": {
                            "type": "integer",
                            "default": 3,
                            "minimum": 1
                        }
                    },
                    "required": ["type"]
                },
                "circuitBreakers": {
                    "type": "object",
                    "description": "Circuit breaker configuration",
                    "properties": {
                        "maxConnections": {
                            "type": "integer",
                            "default": 1024
                        },
                        "maxPendingRequests": {
                            "type": "integer",
                            "default": 1024
                        },
                        "maxRequests": {
                            "type": "integer",
                            "default": 1024
                        },
                        "maxRetries": {
                            "type": "integer",
                            "default": 3
                        }
                    }
                }
            },
            "required": ["name", "serviceName", "endpoints"]
        }),
    )
}

/// Returns the MCP tool definition for updating a cluster.
pub fn cp_update_cluster_tool() -> Tool {
    Tool::new(
        "cp_update_cluster".to_string(),
        r#"Update an existing cluster in the Flowplane control plane.

PURPOSE: Modify cluster configuration (endpoints, load balancing, health checks).
Changes are automatically pushed to Envoy proxies via xDS.

SAFE TO UPDATE: Cluster updates do not affect route configurations that reference this cluster.
Routes reference clusters by name, which cannot be changed.

COMMON USE CASES:
- Add/remove backend endpoints for scaling
- Change load balancing policy
- Update health check configuration
- Enable/disable TLS
- Adjust circuit breaker thresholds

Required Parameters:
- name: Name of the cluster to update (cannot be changed)

Optional Parameters (provide at least one):
- serviceName: New service description
- endpoints: New list of endpoints [{address, port}] - REPLACES all existing
- connectTimeoutSeconds: New connection timeout (1-300)
- lbPolicy: ROUND_ROBIN | LEAST_REQUEST | RANDOM | RING_HASH | MAGLEV
- useTls: Enable/disable TLS for upstream
- healthCheck: {type: "http"|"tcp", path, intervalSeconds, ...}
- circuitBreakers: {maxConnections, maxPendingRequests, maxRequests, maxRetries}

TIP: Use cp_get_cluster first to see current configuration before updating.

Authorization: Requires cp:write scope.
"#
        .to_string(),
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the cluster to update",
                    "minLength": 1
                },
                "serviceName": {
                    "type": "string",
                    "description": "New service name"
                },
                "endpoints": {
                    "type": "array",
                    "description": "New list of backend endpoints",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "address": {"type": "string"},
                            "port": {"type": "integer", "minimum": 1, "maximum": 65535}
                        },
                        "required": ["address", "port"]
                    }
                },
                "connectTimeoutSeconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 300
                },
                "lbPolicy": {
                    "type": "string",
                    "enum": ["ROUND_ROBIN", "LEAST_REQUEST", "RANDOM", "RING_HASH", "MAGLEV"]
                },
                "useTls": {
                    "type": "boolean"
                },
                "healthCheck": {
                    "type": "object",
                    "properties": {
                        "type": {"type": "string", "enum": ["http", "tcp"]},
                        "path": {"type": "string"},
                        "intervalSeconds": {"type": "integer"},
                        "timeoutSeconds": {"type": "integer"},
                        "healthyThreshold": {"type": "integer"},
                        "unhealthyThreshold": {"type": "integer"}
                    }
                },
                "circuitBreakers": {
                    "type": "object",
                    "properties": {
                        "maxConnections": {"type": "integer"},
                        "maxPendingRequests": {"type": "integer"},
                        "maxRequests": {"type": "integer"},
                        "maxRetries": {"type": "integer"}
                    }
                }
            },
            "required": ["name"]
        }),
    )
}

/// Returns the MCP tool definition for deleting a cluster.
pub fn cp_delete_cluster_tool() -> Tool {
    Tool::new(
        "cp_delete_cluster",
        r#"Delete a cluster from the Flowplane control plane.

DELETION ORDER: Delete in REVERSE order of creation.
Delete route configs referencing this cluster FIRST, then delete the cluster.

ORDER: [Listeners] ─► [Route Configs] ─► [Clusters/Filters]

PREREQUISITES FOR DELETION:
- No route configurations may reference this cluster
- If routes reference this cluster, delete or update them first

WILL FAIL IF:
- Cluster is referenced by any route's forwarding action
- Cluster name is "default-gateway-cluster" (system cluster)

WORKFLOW:
1. Use cp_list_routes to find routes using this cluster
2. Update or delete those routes first
3. Then delete the cluster

Required Parameters:
- name: Name of the cluster to delete

Authorization: Requires cp:write scope.
"#
        .to_string(),
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the cluster to delete",
                    "minLength": 1
                }
            },
            "required": ["name"]
        }),
    )
}

/// Execute the cp_create_cluster tool.
///
/// Creates a new cluster with the specified configuration.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_create_cluster")]
pub async fn execute_create_cluster(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse required fields
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    let service_name = args.get("serviceName").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: serviceName".to_string())
    })?;

    let endpoints_json = args.get("endpoints").ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: endpoints".to_string())
    })?;

    tracing::debug!(
        team = %team,
        cluster_name = %name,
        service_name = %service_name,
        "Creating cluster via MCP"
    );

    // 2. Parse endpoints
    let endpoints = parse_endpoints(endpoints_json)
        .map_err(|e| McpError::InvalidParams(format!("Invalid endpoints: {}", e)))?;

    if endpoints.is_empty() {
        return Err(McpError::InvalidParams("At least one endpoint is required".to_string()));
    }

    // 3. Build ClusterSpec from args
    let mut cluster_spec = ClusterSpec {
        endpoints,
        connect_timeout_seconds: args.get("connectTimeoutSeconds").and_then(|v| v.as_u64()),
        lb_policy: args.get("lbPolicy").and_then(|v| v.as_str()).map(|s| s.to_string()),
        use_tls: args.get("useTls").and_then(|v| v.as_bool()),
        ..Default::default()
    };

    // 4. Parse health check if provided
    if let Some(hc_json) = args.get("healthCheck") {
        if let Some(hc) = parse_health_check(hc_json) {
            cluster_spec.health_checks = vec![hc];
        }
    }

    // 5. Parse circuit breakers if provided
    if let Some(cb_json) = args.get("circuitBreakers") {
        cluster_spec.circuit_breakers = parse_circuit_breakers(cb_json);
    }

    // 6. Create request and use internal API layer
    let internal_req = InternalCreateRequest {
        name: name.to_string(),
        service_name: service_name.to_string(),
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        config: cluster_spec,
    };

    let ops = ClusterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    let result = ops.create(internal_req, &auth).await?;

    // 7. Format success response (minimal token-efficient format)
    let output = build_create_response("cluster", &result.data.name, result.data.id.as_ref());

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        cluster_name = %result.data.name,
        cluster_id = %result.data.id,
        "Successfully created cluster via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_update_cluster tool.
///
/// Updates an existing cluster with the specified configuration.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_update_cluster")]
pub async fn execute_update_cluster(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse cluster name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(team = %team, cluster_name = %name, "Updating cluster via MCP");

    // 2. Get existing cluster to merge configuration
    let ops = ClusterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    let existing = ops.get(name, &auth).await?;

    // 3. Parse existing configuration
    let mut cluster_spec: ClusterSpec = serde_json::from_str(&existing.configuration)
        .map_err(|e| McpError::InternalError(format!("Failed to parse cluster config: {}", e)))?;

    // 4. Apply updates
    if let Some(endpoints_json) = args.get("endpoints") {
        cluster_spec.endpoints = parse_endpoints(endpoints_json)
            .map_err(|e| McpError::InvalidParams(format!("Invalid endpoints: {}", e)))?;
    }

    if let Some(timeout) = args.get("connectTimeoutSeconds").and_then(|v| v.as_u64()) {
        cluster_spec.connect_timeout_seconds = Some(timeout);
    }

    if let Some(lb_policy) = args.get("lbPolicy").and_then(|v| v.as_str()) {
        cluster_spec.lb_policy = Some(lb_policy.to_string());
    }

    if let Some(use_tls) = args.get("useTls").and_then(|v| v.as_bool()) {
        cluster_spec.use_tls = Some(use_tls);
    }

    if let Some(hc_json) = args.get("healthCheck") {
        if let Some(hc) = parse_health_check(hc_json) {
            cluster_spec.health_checks = vec![hc];
        }
    }

    if let Some(cb_json) = args.get("circuitBreakers") {
        cluster_spec.circuit_breakers = parse_circuit_breakers(cb_json);
    }

    // 5. Get service name (use existing if not provided)
    let service_name = args.get("serviceName").and_then(|v| v.as_str()).map(|s| s.to_string());

    // 6. Create request and use internal API layer
    let internal_req = InternalUpdateRequest { service_name, config: cluster_spec };

    let result = ops.update(name, internal_req, &auth).await?;

    // 7. Format success response (minimal token-efficient format)
    let output = build_update_response("cluster", &result.data.name, result.data.id.as_ref());

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        cluster_name = %result.data.name,
        cluster_id = %result.data.id,
        "Successfully updated cluster via MCP"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_delete_cluster tool.
///
/// Deletes a cluster from the control plane.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_delete_cluster")]
pub async fn execute_delete_cluster(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // 1. Parse cluster name
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(team = %team, cluster_name = %name, "Deleting cluster via MCP");

    // 2. Use internal API layer
    let ops = ClusterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    ops.delete(name, &auth).await?;

    // 3. Format success response (minimal token-efficient format)
    let output = build_delete_response();

    let text = serde_json::to_string(&output).map_err(McpError::SerializationError)?;

    tracing::info!(team = %team, cluster_name = %name, "Successfully deleted cluster via MCP");

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_get_cluster_health tool.
///
/// Returns aggregated endpoint health status for a cluster.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_cluster_health")]
pub async fn execute_get_cluster_health(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(team = %team, cluster_name = %name, "Getting cluster health");

    // Verify cluster exists and get cluster ID
    let ops = ClusterOperations::new(xds_state.clone());
    let auth = InternalAuthContext::from_mcp(team);
    let cluster = ops.get(name, &auth).await?;
    let cluster_id = cluster.id.clone();

    // Get endpoint health from database
    let pool = xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Database not available".to_string()))?
        .pool();

    let endpoint_repo = ClusterEndpointRepository::new(pool.clone());
    let endpoints = endpoint_repo
        .list_by_cluster(&cluster_id)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to query endpoints: {}", e)))?;

    // Aggregate health status counts
    let mut healthy_count = 0;
    let mut unhealthy_count = 0;
    let mut degraded_count = 0;
    let mut unknown_count = 0;

    let endpoint_details: Vec<Value> = endpoints
        .iter()
        .map(|ep| {
            match ep.health_status {
                crate::domain::EndpointHealthStatus::Healthy => healthy_count += 1,
                crate::domain::EndpointHealthStatus::Unhealthy => unhealthy_count += 1,
                crate::domain::EndpointHealthStatus::Degraded => degraded_count += 1,
                crate::domain::EndpointHealthStatus::Unknown => unknown_count += 1,
            }
            json!({
                "address": ep.address,
                "port": ep.port,
                "status": ep.health_status.as_str()
            })
        })
        .collect();

    // Build response
    let data = json!({
        "total_endpoints": endpoints.len(),
        "healthy": healthy_count,
        "unhealthy": unhealthy_count,
        "degraded": degraded_count,
        "unknown": unknown_count,
        "endpoints": endpoint_details
    });

    let response = build_query_response(
        true,
        Some(ResourceRef::cluster(&cluster.name, cluster.id.to_string())),
        Some(data),
    );

    let text = serde_json::to_string(&response).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        cluster_name = %name,
        total_endpoints = endpoints.len(),
        healthy = healthy_count,
        "Successfully retrieved cluster health"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

// === Helper Functions for MCP-Specific Parsing ===

/// Helper function to parse endpoints from JSON
fn parse_endpoints(endpoints_json: &Value) -> Result<Vec<EndpointSpec>, String> {
    let endpoints_array =
        endpoints_json.as_array().ok_or_else(|| "endpoints must be an array".to_string())?;

    let mut endpoints = Vec::new();
    for (idx, ep) in endpoints_array.iter().enumerate() {
        let address = ep
            .get("address")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("endpoints[{}]: missing 'address'", idx))?;

        let port = ep
            .get("port")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("endpoints[{}]: missing 'port'", idx))?;

        if port == 0 || port > 65535 {
            return Err(format!("endpoints[{}]: port must be between 1 and 65535", idx));
        }

        // Use Address format to be consistent with REST API
        endpoints.push(EndpointSpec::Address { host: address.to_string(), port: port as u16 });
    }

    Ok(endpoints)
}

/// Helper function to parse health check from JSON
fn parse_health_check(hc_json: &Value) -> Option<crate::xds::HealthCheckSpec> {
    let hc_type = hc_json.get("type")?.as_str()?;

    match hc_type {
        "http" => {
            let path = hc_json.get("path").and_then(|v| v.as_str()).unwrap_or("/health");
            Some(crate::xds::HealthCheckSpec::Http {
                path: path.to_string(),
                host: hc_json.get("host").and_then(|v| v.as_str()).map(|s| s.to_string()),
                method: None,
                interval_seconds: hc_json.get("intervalSeconds").and_then(|v| v.as_u64()),
                timeout_seconds: hc_json.get("timeoutSeconds").and_then(|v| v.as_u64()),
                healthy_threshold: hc_json
                    .get("healthyThreshold")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32),
                unhealthy_threshold: hc_json
                    .get("unhealthyThreshold")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32),
                expected_statuses: None,
            })
        }
        "tcp" => Some(crate::xds::HealthCheckSpec::Tcp {
            interval_seconds: hc_json.get("intervalSeconds").and_then(|v| v.as_u64()),
            timeout_seconds: hc_json.get("timeoutSeconds").and_then(|v| v.as_u64()),
            healthy_threshold: hc_json
                .get("healthyThreshold")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
            unhealthy_threshold: hc_json
                .get("unhealthyThreshold")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
        }),
        _ => None,
    }
}

/// Helper function to parse circuit breakers from JSON
fn parse_circuit_breakers(cb_json: &Value) -> Option<crate::xds::CircuitBreakersSpec> {
    let thresholds = crate::xds::CircuitBreakerThresholdsSpec {
        max_connections: cb_json.get("maxConnections").and_then(|v| v.as_u64()).map(|v| v as u32),
        max_pending_requests: cb_json
            .get("maxPendingRequests")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        max_requests: cb_json.get("maxRequests").and_then(|v| v.as_u64()).map(|v| v as u32),
        max_retries: cb_json.get("maxRetries").and_then(|v| v.as_u64()).map(|v| v as u32),
    };

    Some(crate::xds::CircuitBreakersSpec { default: Some(thresholds), high: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimpleXdsConfig;
    use crate::storage::test_helpers::TestDatabase;

    async fn setup_test_xds() -> (TestDatabase, Arc<XdsState>) {
        let test_db = TestDatabase::new("mcp_tools_clusters").await;
        let pool = test_db.pool.clone();
        let xds_state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool));
        (test_db, xds_state)
    }

    /// Create a test team in the database
    async fn create_test_team(xds_state: &Arc<XdsState>, team_name: &str) {
        let pool = xds_state.cluster_repository.as_ref().unwrap().pool();
        let team_id = format!("team-{}", uuid::Uuid::new_v4());
        sqlx::query("INSERT INTO teams (id, name, display_name, status) VALUES ($1, $2, $3, $4) ON CONFLICT (name) DO NOTHING")
            .bind(&team_id)
            .bind(team_name)
            .bind(format!("Test {}", team_name))
            .bind("active")
            .execute(pool)
            .await
            .expect("Failed to create test team");
    }

    #[tokio::test]
    async fn test_cp_list_clusters_tool_definition() {
        let tool = cp_list_clusters_tool();
        assert_eq!(tool.name, "cp_list_clusters");
        assert!(tool.description.as_ref().unwrap().contains("List all upstream clusters"));
        assert!(tool.description.as_ref().unwrap().contains("RESOURCE ORDER")); // AI-agent friendly description
        assert!(tool.input_schema.get("properties").is_some());
    }

    #[tokio::test]
    async fn test_cp_get_cluster_tool_definition() {
        let tool = cp_get_cluster_tool();
        assert_eq!(tool.name, "cp_get_cluster");
        assert!(tool.description.as_ref().unwrap().contains("Get detailed information"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[test]
    fn test_cp_get_cluster_health_tool_definition() {
        let tool = cp_get_cluster_health_tool();
        assert_eq!(tool.name, "cp_get_cluster_health");
        assert!(tool.description.as_ref().unwrap().contains("endpoint health"));
        assert!(tool.input_schema.get("required").is_some());

        // Verify required parameters
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
    }

    #[tokio::test]
    async fn test_execute_list_clusters_returns_seed_data() {
        let (_db, xds_state) = setup_test_xds().await;
        let args = json!({});

        let result = execute_list_clusters(&xds_state, "test-team", args).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert_eq!(tool_result.content.len(), 1);

        if let ContentBlock::Text { text } = &tool_result.content[0] {
            let output: Value = serde_json::from_str(text).unwrap();
            // Seed data creates global clusters (test-cluster, cluster-a, cluster-b)
            assert!(output["count"].as_u64().unwrap() >= 3);
        } else {
            panic!("Expected text content block");
        }
    }

    #[tokio::test]
    async fn test_execute_create_and_get_cluster() {
        let (_db, xds_state) = setup_test_xds().await;

        // Create the team first
        create_test_team(&xds_state, "test-team").await;

        // Create a cluster (use unique name to avoid seed data conflicts)
        let create_args = json!({
            "name": "mcp-created-cluster",
            "serviceName": "test-service",
            "endpoints": [{"address": "10.0.0.1", "port": 8080}]
        });

        let result = execute_create_cluster(&xds_state, "test-team", create_args).await;
        assert!(result.is_ok());

        // Get the cluster
        let get_args = json!({"name": "mcp-created-cluster"});
        let result = execute_get_cluster(&xds_state, "test-team", get_args).await;
        assert!(result.is_ok());

        if let ContentBlock::Text { text } = &result.unwrap().content[0] {
            let output: Value = serde_json::from_str(text).unwrap();
            assert_eq!(output["name"], "mcp-created-cluster");
            assert_eq!(output["service_name"], "test-service");
        }
    }

    #[tokio::test]
    async fn test_execute_get_cluster_not_found() {
        let (_db, xds_state) = setup_test_xds().await;
        let args = json!({"name": "non-existent-cluster"});

        let result = execute_get_cluster(&xds_state, "test-team", args).await;
        assert!(result.is_err());

        if let Err(McpError::ResourceNotFound(msg)) = result {
            assert!(msg.contains("not found"));
        } else {
            panic!("Expected ResourceNotFound error");
        }
    }

    #[tokio::test]
    async fn test_execute_get_cluster_missing_name() {
        let (_db, xds_state) = setup_test_xds().await;
        let args = json!({});

        let result = execute_get_cluster(&xds_state, "test-team", args).await;
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("Missing required parameter: name"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }

    #[tokio::test]
    async fn test_execute_update_cluster() {
        let (_db, xds_state) = setup_test_xds().await;
        create_test_team(&xds_state, "test-team").await;

        // Create a cluster first
        let create_args = json!({
            "name": "update-test",
            "serviceName": "original",
            "endpoints": [{"address": "10.0.0.1", "port": 8080}]
        });
        execute_create_cluster(&xds_state, "test-team", create_args).await.expect("create cluster");

        // Update it
        let update_args = json!({
            "name": "update-test",
            "serviceName": "updated"
        });
        let result = execute_update_cluster(&xds_state, "test-team", update_args).await;
        assert!(result.is_ok());

        // Verify the update
        let get_args = json!({"name": "update-test"});
        let result = execute_get_cluster(&xds_state, "test-team", get_args).await;
        if let ContentBlock::Text { text } = &result.unwrap().content[0] {
            let output: Value = serde_json::from_str(text).unwrap();
            assert_eq!(output["service_name"], "updated");
        }
    }

    #[tokio::test]
    async fn test_execute_delete_cluster() {
        let (_db, xds_state) = setup_test_xds().await;
        create_test_team(&xds_state, "test-team").await;

        // Create a cluster first
        let create_args = json!({
            "name": "delete-test",
            "serviceName": "service",
            "endpoints": [{"address": "10.0.0.1", "port": 8080}]
        });
        execute_create_cluster(&xds_state, "test-team", create_args).await.expect("create cluster");

        // Delete it
        let delete_args = json!({"name": "delete-test"});
        let result = execute_delete_cluster(&xds_state, "test-team", delete_args).await;
        assert!(result.is_ok());

        // Verify it's gone
        let get_args = json!({"name": "delete-test"});
        let result = execute_get_cluster(&xds_state, "test-team", get_args).await;
        assert!(result.is_err());
    }
}
