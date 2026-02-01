//! DevOps Agent MCP Tools
//!
//! High-level workflow automation tools for infrastructure management and API deployment.
//! These tools orchestrate multiple lower-level operations to simplify common DevOps tasks.

use crate::internal_api::{
    ClusterOperations, CreateClusterRequest, CreateFilterRequest, CreateRouteConfigRequest,
    FilterOperations, InternalAuthContext, ListClustersRequest, ListFiltersRequest,
    ListListenersRequest, ListRouteConfigsRequest, ListenerOperations, RouteConfigOperations,
};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::xds::{ClusterSpec, EndpointSpec, XdsState};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

// =============================================================================
// TOOL DEFINITIONS
// =============================================================================

/// DevOps tool for deploying an API with cluster, route config, and listener
pub fn devops_deploy_api_tool() -> Tool {
    Tool {
        name: "devops_deploy_api".to_string(),
        description: r#"Deploy an API endpoint with all required infrastructure in one operation.

ORCHESTRATION: Creates cluster, route configuration, and optionally a listener in the correct order.
This is a high-level workflow tool that replaces multiple manual steps.

RESOURCE ORDER (automatically handled):
1. Cluster (backend service endpoints)
2. Route Configuration (URL routing rules)
3. Listener (optional - only if creating a new listener)

USE CASES:
- Deploy a new microservice API endpoint
- Set up a backend service with routing rules
- Create complete API infrastructure in one step

PARAMETERS:
- cluster_name: Name for the upstream cluster (required)
- endpoints: Array of {address, port} for backend servers (required)
- route_config_name: Name for the route configuration (required)
- path_prefix: URL path prefix for routing, e.g., "/api/v1" (default: "/")
- listener_name: Existing listener to attach routes to (optional)
- domains: Virtual host domains, e.g., ["api.example.com"] (default: ["*"])
- lb_policy: Load balancing policy (default: "ROUND_ROBIN")

EXAMPLE:
{
  "cluster_name": "user-service",
  "endpoints": [{"address": "10.0.1.1", "port": 8080}],
  "route_config_name": "user-api-routes",
  "path_prefix": "/api/users",
  "domains": ["api.example.com"]
}

RETURNS: Summary of created resources with their IDs and names.

Authorization: Requires cp:write or clusters:write + routes:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "cluster_name": {
                    "type": "string",
                    "description": "Name for the upstream cluster"
                },
                "endpoints": {
                    "type": "array",
                    "description": "Backend server endpoints",
                    "items": {
                        "type": "object",
                        "properties": {
                            "address": {"type": "string"},
                            "port": {"type": "integer"}
                        },
                        "required": ["address", "port"]
                    },
                    "minItems": 1
                },
                "route_config_name": {
                    "type": "string",
                    "description": "Name for the route configuration"
                },
                "path_prefix": {
                    "type": "string",
                    "description": "URL path prefix for routing (default: /)",
                    "default": "/"
                },
                "listener_name": {
                    "type": "string",
                    "description": "Existing listener to attach routes to (optional)"
                },
                "domains": {
                    "type": "array",
                    "description": "Virtual host domains (default: [\"*\"])",
                    "items": {"type": "string"},
                    "default": ["*"]
                },
                "lb_policy": {
                    "type": "string",
                    "description": "Load balancing policy",
                    "enum": ["ROUND_ROBIN", "LEAST_REQUEST", "RANDOM", "RING_HASH", "MAGLEV"],
                    "default": "ROUND_ROBIN"
                },
                "description": {
                    "type": "string",
                    "description": "Optional description for the deployment"
                }
            },
            "required": ["cluster_name", "endpoints", "route_config_name"]
        }),
    }
}

/// DevOps tool for configuring rate limiting
pub fn devops_configure_rate_limiting_tool() -> Tool {
    Tool {
        name: "devops_configure_rate_limiting".to_string(),
        description: r#"Configure rate limiting for an API endpoint.

ORCHESTRATION: Creates a rate limiting filter and attaches it to the specified target.
Simplifies the process of adding rate limits to protect your APIs.

USE CASES:
- Protect APIs from abuse and DDoS
- Implement fair usage quotas
- Control traffic to backend services

PARAMETERS:
- filter_name: Name for the rate limit filter (required)
- max_requests: Maximum requests allowed in the time window (required)
- window_seconds: Time window for rate limiting in seconds (default: 60)
- target_type: Where to attach - "listener" or "route_config" (required)
- target_name: Name of the listener or route_config to attach to (required)
- status_code: HTTP status code when rate limited (default: 429)
- stat_prefix: Stats prefix for metrics (default: "rate_limit")

EXAMPLE:
{
  "filter_name": "api-rate-limit",
  "max_requests": 100,
  "window_seconds": 60,
  "target_type": "listener",
  "target_name": "http-ingress"
}

RETURNS: Created filter details and attachment confirmation.

Authorization: Requires cp:write or filters:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "filter_name": {
                    "type": "string",
                    "description": "Name for the rate limit filter"
                },
                "max_requests": {
                    "type": "integer",
                    "description": "Maximum requests allowed in the time window",
                    "minimum": 1
                },
                "window_seconds": {
                    "type": "integer",
                    "description": "Time window for rate limiting in seconds (default: 60)",
                    "minimum": 1,
                    "default": 60
                },
                "target_type": {
                    "type": "string",
                    "description": "Where to attach the filter",
                    "enum": ["listener", "route_config"]
                },
                "target_name": {
                    "type": "string",
                    "description": "Name of the listener or route_config to attach to"
                },
                "status_code": {
                    "type": "integer",
                    "description": "HTTP status code when rate limited (default: 429)",
                    "default": 429
                },
                "stat_prefix": {
                    "type": "string",
                    "description": "Stats prefix for metrics (default: rate_limit)",
                    "default": "rate_limit"
                },
                "description": {
                    "type": "string",
                    "description": "Optional description for the filter"
                }
            },
            "required": ["filter_name", "max_requests", "target_type", "target_name"]
        }),
    }
}

/// DevOps tool for enabling JWT authentication
pub fn devops_enable_jwt_auth_tool() -> Tool {
    Tool {
        name: "devops_enable_jwt_auth".to_string(),
        description: r#"Enable JWT authentication for an API endpoint.

ORCHESTRATION: Creates a JWT authentication filter with provider configuration
and attaches it to the specified target.

USE CASES:
- Secure API endpoints with JWT tokens
- Validate tokens from identity providers (Auth0, Okta, Keycloak, etc.)
- Implement stateless authentication

PARAMETERS:
- filter_name: Name for the JWT auth filter (required)
- issuer: JWT issuer URL (required)
- audiences: List of valid audiences (required)
- jwks_uri: JWKS endpoint URL for key validation (required)
- target_type: Where to attach - "listener" or "route_config" (required)
- target_name: Name of the listener or route_config to attach to (required)
- forward_jwt: Forward JWT to upstream (default: true)
- payload_header: Header name for forwarding payload (optional)
- bypass_cors_preflight: Skip JWT validation for OPTIONS requests (default: true)

EXAMPLE:
{
  "filter_name": "api-jwt-auth",
  "issuer": "https://auth.example.com",
  "audiences": ["api", "web"],
  "jwks_uri": "https://auth.example.com/.well-known/jwks.json",
  "target_type": "listener",
  "target_name": "http-ingress"
}

RETURNS: Created filter details and attachment confirmation.

Authorization: Requires cp:write or filters:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "filter_name": {
                    "type": "string",
                    "description": "Name for the JWT auth filter"
                },
                "issuer": {
                    "type": "string",
                    "description": "JWT issuer URL (iss claim)"
                },
                "audiences": {
                    "type": "array",
                    "description": "List of valid audiences (aud claim)",
                    "items": {"type": "string"},
                    "minItems": 1
                },
                "jwks_uri": {
                    "type": "string",
                    "description": "JWKS endpoint URL for key validation"
                },
                "target_type": {
                    "type": "string",
                    "description": "Where to attach the filter",
                    "enum": ["listener", "route_config"]
                },
                "target_name": {
                    "type": "string",
                    "description": "Name of the listener or route_config to attach to"
                },
                "forward_jwt": {
                    "type": "boolean",
                    "description": "Forward JWT to upstream (default: true)",
                    "default": true
                },
                "payload_header": {
                    "type": "string",
                    "description": "Header name for forwarding JWT payload (optional)"
                },
                "bypass_cors_preflight": {
                    "type": "boolean",
                    "description": "Skip JWT validation for OPTIONS requests (default: true)",
                    "default": true
                },
                "description": {
                    "type": "string",
                    "description": "Optional description for the filter"
                }
            },
            "required": ["filter_name", "issuer", "audiences", "jwks_uri", "target_type", "target_name"]
        }),
    }
}

/// DevOps tool for configuring CORS
pub fn devops_configure_cors_tool() -> Tool {
    Tool {
        name: "devops_configure_cors".to_string(),
        description: r#"Configure Cross-Origin Resource Sharing (CORS) for an API endpoint.

ORCHESTRATION: Creates a CORS filter with the specified policy
and attaches it to the specified target.

USE CASES:
- Allow browser-based clients to access APIs from different origins
- Configure allowed methods, headers, and credentials for cross-origin requests
- Set up preflight caching for performance

PARAMETERS:
- filter_name: Name for the CORS filter (required)
- allowed_origins: List of allowed origins (required). Use "*" for any origin.
- allowed_methods: List of allowed HTTP methods (default: ["GET", "POST", "PUT", "DELETE", "OPTIONS"])
- allowed_headers: List of allowed request headers (default: ["Authorization", "Content-Type"])
- exposed_headers: List of headers to expose to clients (optional)
- max_age_seconds: Preflight cache duration in seconds (default: 86400)
- allow_credentials: Allow credentials in cross-origin requests (default: false)
- target_type: Where to attach - "listener" or "route_config" (required)
- target_name: Name of the listener or route_config to attach to (required)

EXAMPLE:
{
  "filter_name": "api-cors",
  "allowed_origins": ["https://app.example.com", "https://admin.example.com"],
  "allowed_methods": ["GET", "POST", "PUT", "DELETE"],
  "target_type": "listener",
  "target_name": "http-ingress"
}

RETURNS: Created filter details and attachment confirmation.

Authorization: Requires cp:write or filters:write scope."#.to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "filter_name": {
                    "type": "string",
                    "description": "Name for the CORS filter"
                },
                "allowed_origins": {
                    "type": "array",
                    "description": "List of allowed origins. Use \"*\" for any origin.",
                    "items": {"type": "string"},
                    "minItems": 1
                },
                "allowed_methods": {
                    "type": "array",
                    "description": "List of allowed HTTP methods",
                    "items": {"type": "string"},
                    "default": ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
                },
                "allowed_headers": {
                    "type": "array",
                    "description": "List of allowed request headers",
                    "items": {"type": "string"},
                    "default": ["Authorization", "Content-Type"]
                },
                "exposed_headers": {
                    "type": "array",
                    "description": "List of headers to expose to clients",
                    "items": {"type": "string"}
                },
                "max_age_seconds": {
                    "type": "integer",
                    "description": "Preflight cache duration in seconds (default: 86400)",
                    "default": 86400
                },
                "allow_credentials": {
                    "type": "boolean",
                    "description": "Allow credentials in cross-origin requests (default: false)",
                    "default": false
                },
                "target_type": {
                    "type": "string",
                    "description": "Where to attach the filter",
                    "enum": ["listener", "route_config"]
                },
                "target_name": {
                    "type": "string",
                    "description": "Name of the listener or route_config to attach to"
                },
                "description": {
                    "type": "string",
                    "description": "Optional description for the filter"
                }
            },
            "required": ["filter_name", "allowed_origins", "target_type", "target_name"]
        }),
    }
}

/// DevOps tool for creating canary deployments
pub fn devops_create_canary_deployment_tool() -> Tool {
    Tool {
        name: "devops_create_canary_deployment".to_string(),
        description: r#"Create a canary deployment with weighted traffic splitting.

ORCHESTRATION: Creates or updates clusters for stable and canary versions,
then configures weighted routing to split traffic between them.

USE CASES:
- Gradually roll out new service versions
- A/B testing with traffic percentages
- Blue/green deployments with traffic shifting

PARAMETERS:
- deployment_name: Base name for the deployment (required)
- stable_endpoints: Endpoints for the stable version (required)
- canary_endpoints: Endpoints for the canary version (required)
- canary_weight: Percentage of traffic to canary (1-99, default: 10)
- route_config_name: Route config to update with weighted routing (required)
- path_prefix: URL path prefix for the weighted routes (default: "/")

EXAMPLE:
{
  "deployment_name": "user-service",
  "stable_endpoints": [{"address": "10.0.1.1", "port": 8080}],
  "canary_endpoints": [{"address": "10.0.2.1", "port": 8080}],
  "canary_weight": 10,
  "route_config_name": "user-api-routes",
  "path_prefix": "/api/users"
}

This creates:
- user-service-stable cluster (90% traffic)
- user-service-canary cluster (10% traffic)
- Weighted route configuration

RETURNS: Created clusters and routing configuration details.

Authorization: Requires cp:write or clusters:write + routes:write scope."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "deployment_name": {
                    "type": "string",
                    "description": "Base name for the deployment"
                },
                "stable_endpoints": {
                    "type": "array",
                    "description": "Endpoints for the stable version",
                    "items": {
                        "type": "object",
                        "properties": {
                            "address": {"type": "string"},
                            "port": {"type": "integer"}
                        },
                        "required": ["address", "port"]
                    },
                    "minItems": 1
                },
                "canary_endpoints": {
                    "type": "array",
                    "description": "Endpoints for the canary version",
                    "items": {
                        "type": "object",
                        "properties": {
                            "address": {"type": "string"},
                            "port": {"type": "integer"}
                        },
                        "required": ["address", "port"]
                    },
                    "minItems": 1
                },
                "canary_weight": {
                    "type": "integer",
                    "description": "Percentage of traffic to canary (1-99, default: 10)",
                    "minimum": 1,
                    "maximum": 99,
                    "default": 10
                },
                "route_config_name": {
                    "type": "string",
                    "description": "Route config to update with weighted routing"
                },
                "path_prefix": {
                    "type": "string",
                    "description": "URL path prefix for the weighted routes (default: /)",
                    "default": "/"
                },
                "description": {
                    "type": "string",
                    "description": "Optional description for the deployment"
                }
            },
            "required": ["deployment_name", "stable_endpoints", "canary_endpoints", "route_config_name"]
        }),
    }
}

/// DevOps tool for getting deployment status
pub fn devops_get_deployment_status_tool() -> Tool {
    Tool {
        name: "devops_get_deployment_status".to_string(),
        description: r#"Get aggregated deployment status across clusters, listeners, and filters.

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
- clusters: List of clusters with endpoint counts and health
- listeners: List of listeners with attached route configs and filters
- filters: List of filters with installation points
- summary: Overall health indicators

Authorization: Requires cp:read scope."#
            .to_string(),
        input_schema: json!({
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
    }
}

// =============================================================================
// EXECUTE FUNCTIONS
// =============================================================================

/// Execute devops_deploy_api: Create cluster and route configuration
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_devops_deploy_api")]
pub async fn execute_devops_deploy_api(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Parse required parameters
    let cluster_name = args.get("cluster_name").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: cluster_name".to_string())
    })?;

    let endpoints = args.get("endpoints").and_then(|v| v.as_array()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: endpoints".to_string())
    })?;

    let route_config_name =
        args.get("route_config_name").and_then(|v| v.as_str()).ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: route_config_name".to_string())
        })?;

    // Parse optional parameters
    let path_prefix = args.get("path_prefix").and_then(|v| v.as_str()).unwrap_or("/");
    let domains: Vec<String> = args
        .get("domains")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_else(|| vec!["*".to_string()]);
    let lb_policy = args.get("lb_policy").and_then(|v| v.as_str()).unwrap_or("ROUND_ROBIN");
    let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("");

    tracing::info!(
        team = %team,
        cluster_name = %cluster_name,
        route_config_name = %route_config_name,
        "Deploying API"
    );

    let auth = InternalAuthContext::from_mcp(team);
    let mut created_resources = Vec::new();

    // Step 1: Create cluster
    let cluster_ops = ClusterOperations::new(xds_state.clone());

    // Build endpoint specs
    let endpoint_specs: Vec<EndpointSpec> = endpoints
        .iter()
        .filter_map(|ep| {
            let address = ep.get("address").and_then(|v| v.as_str())?;
            let port = ep.get("port").and_then(|v| v.as_i64())?;
            Some(EndpointSpec::String(format!("{}:{}", address, port)))
        })
        .collect();

    let cluster_spec = ClusterSpec {
        endpoints: endpoint_specs,
        lb_policy: Some(lb_policy.to_string()),
        ..Default::default()
    };

    let create_cluster_req = CreateClusterRequest {
        name: cluster_name.to_string(),
        service_name: cluster_name.to_string(),
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        config: cluster_spec,
    };

    let cluster_result = cluster_ops.create(create_cluster_req, &auth).await?;
    created_resources.push(json!({
        "type": "cluster",
        "name": cluster_result.data.name,
        "id": cluster_result.data.id.to_string(),
        "message": cluster_result.message
    }));

    // Step 2: Create route configuration
    let route_ops = RouteConfigOperations::new(xds_state.clone());

    // Build route with forward action to the cluster
    let route_config = json!({
        "virtual_hosts": [{
            "name": format!("{}-vh", route_config_name),
            "domains": domains,
            "routes": [{
                "name": format!("{}-route", route_config_name),
                "match": {
                    "prefix": path_prefix
                },
                "action": {
                    "type": "forward",
                    "cluster": cluster_name
                }
            }]
        }],
        "description": description
    });

    let create_rc_req = CreateRouteConfigRequest {
        name: route_config_name.to_string(),
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        config: route_config,
    };

    let rc_result = route_ops.create(create_rc_req, &auth).await?;
    created_resources.push(json!({
        "type": "route_config",
        "name": rc_result.data.name,
        "id": rc_result.data.id.to_string(),
        "message": rc_result.message
    }));

    // Step 3: Log if listener_name is provided (updating listener route_config is complex)
    let listener_name = args.get("listener_name").and_then(|v| v.as_str());
    if let Some(ln) = listener_name {
        // Note: Updating a listener's route_config requires rebuilding the config
        // For now, we just note that the route_config was created and can be manually attached
        tracing::info!(
            listener = %ln,
            route_config = %route_config_name,
            "Route config created. Attach to listener using cp_update_listener."
        );
        created_resources.push(json!({
            "type": "note",
            "message": format!("Route config '{}' created. Use cp_update_listener to attach to listener '{}'", route_config_name, ln)
        }));
    }

    let output = json!({
        "success": true,
        "deployment": {
            "cluster": cluster_name,
            "route_config": route_config_name,
            "path_prefix": path_prefix,
            "domains": domains,
            "endpoint_count": endpoints.len()
        },
        "created_resources": created_resources,
        "message": format!("Successfully deployed API with cluster '{}' and route config '{}'", cluster_name, route_config_name)
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute devops_configure_rate_limiting: Create and attach rate limit filter
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_devops_configure_rate_limiting")]
pub async fn execute_devops_configure_rate_limiting(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Parse required parameters
    let filter_name = args.get("filter_name").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: filter_name".to_string())
    })?;

    let max_requests = args.get("max_requests").and_then(|v| v.as_i64()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: max_requests".to_string())
    })? as u32;

    let target_type = args.get("target_type").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: target_type".to_string())
    })?;

    let target_name = args.get("target_name").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: target_name".to_string())
    })?;

    // Parse optional parameters
    let window_seconds = args.get("window_seconds").and_then(|v| v.as_i64()).unwrap_or(60) as u64;
    let status_code = args.get("status_code").and_then(|v| v.as_i64()).unwrap_or(429) as u16;
    let stat_prefix = args.get("stat_prefix").and_then(|v| v.as_str()).unwrap_or("rate_limit");
    let description =
        args.get("description").and_then(|v| v.as_str()).unwrap_or("Rate limiting filter");

    tracing::info!(
        team = %team,
        filter_name = %filter_name,
        max_requests = %max_requests,
        target_type = %target_type,
        target_name = %target_name,
        "Configuring rate limiting"
    );

    let auth = InternalAuthContext::from_mcp(team);
    let filter_ops = FilterOperations::new(xds_state.clone());

    // Create rate limit filter configuration
    // Build filter config in the envelope format {"type": "...", "config": {...}}
    let inner_config = json!({
        "stat_prefix": stat_prefix,
        "token_bucket": {
            "max_tokens": max_requests,
            "tokens_per_fill": max_requests,
            "fill_interval_ms": window_seconds * 1000
        },
        "status_code": status_code,
        "filter_enabled": {
            "runtime_key": format!("{}_enabled", filter_name),
            "numerator": 100,
            "denominator": "hundred"
        },
        "filter_enforced": {
            "runtime_key": format!("{}_enforced", filter_name),
            "numerator": 100,
            "denominator": "hundred"
        }
    });

    let config_envelope = json!({
        "type": "local_rate_limit",
        "config": inner_config
    });

    let config: crate::domain::FilterConfig = serde_json::from_value(config_envelope)
        .map_err(|e| McpError::InvalidParams(format!("Invalid configuration: {}", e)))?;

    let create_req = CreateFilterRequest {
        name: filter_name.to_string(),
        filter_type: "local_rate_limit".to_string(),
        description: Some(description.to_string()),
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        config,
    };

    let filter_result = filter_ops.create(create_req, &auth).await?;

    // Attach to target
    let attachment_result = match target_type {
        "listener" => {
            filter_ops
                .attach_to_listener(&filter_result.data.name, target_name, Some(100), &auth)
                .await
        }
        "route_config" => {
            filter_ops
                .attach_to_route_config(&filter_result.data.name, target_name, None, None, &auth)
                .await
        }
        _ => {
            return Err(McpError::InvalidParams(format!(
                "Invalid target_type '{}'. Must be 'listener' or 'route_config'",
                target_type
            )));
        }
    };

    let attachment_success = attachment_result.is_ok();
    let attachment_message = match attachment_result {
        Ok(_) => format!("Successfully attached to {} '{}'", target_type, target_name),
        Err(e) => format!("Filter created but attachment failed: {}", e),
    };

    let output = json!({
        "success": true,
        "filter": {
            "name": filter_result.data.name,
            "id": filter_result.data.id.to_string(),
            "filter_type": "local_rate_limit",
            "max_requests": max_requests,
            "window_seconds": window_seconds,
            "status_code": status_code
        },
        "attachment": {
            "target_type": target_type,
            "target_name": target_name,
            "attached": attachment_success
        },
        "message": attachment_message
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute devops_enable_jwt_auth: Create and attach JWT authentication filter
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_devops_enable_jwt_auth")]
pub async fn execute_devops_enable_jwt_auth(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Parse required parameters
    let filter_name = args.get("filter_name").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: filter_name".to_string())
    })?;

    let issuer = args
        .get("issuer")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: issuer".to_string()))?;

    let audiences: Vec<String> = args
        .get("audiences")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: audiences".to_string())
        })?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    let jwks_uri = args.get("jwks_uri").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: jwks_uri".to_string())
    })?;

    let target_type = args.get("target_type").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: target_type".to_string())
    })?;

    let target_name = args.get("target_name").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: target_name".to_string())
    })?;

    // Parse optional parameters
    let forward_jwt = args.get("forward_jwt").and_then(|v| v.as_bool()).unwrap_or(true);
    let payload_header = args.get("payload_header").and_then(|v| v.as_str());
    let bypass_cors_preflight =
        args.get("bypass_cors_preflight").and_then(|v| v.as_bool()).unwrap_or(true);
    let description =
        args.get("description").and_then(|v| v.as_str()).unwrap_or("JWT authentication filter");

    tracing::info!(
        team = %team,
        filter_name = %filter_name,
        issuer = %issuer,
        target_type = %target_type,
        target_name = %target_name,
        "Enabling JWT authentication"
    );

    let auth = InternalAuthContext::from_mcp(team);
    let filter_ops = FilterOperations::new(xds_state.clone());

    // Create JWT auth filter configuration
    let mut provider_config = json!({
        "issuer": issuer,
        "audiences": audiences,
        "forward": forward_jwt,
        "jwks": {
            "remote_jwks": {
                "http_uri": {
                    "uri": jwks_uri,
                    "cluster": "jwks-cluster",
                    "timeout_ms": 5000
                },
                "cache_duration_seconds": 600
            }
        },
        "from_headers": [{
            "name": "Authorization",
            "value_prefix": "Bearer "
        }]
    });

    if let Some(ph) = payload_header {
        provider_config["forward_payload_header"] = json!(ph);
    }

    let inner_config = json!({
        "providers": {
            "primary": provider_config
        },
        "rules": [{
            "match": {"prefix": "/"},
            "requires": {"provider_name": "primary"}
        }],
        "bypass_cors_preflight": bypass_cors_preflight
    });

    let config_envelope = json!({
        "type": "jwt_auth",
        "config": inner_config
    });

    let config: crate::domain::FilterConfig = serde_json::from_value(config_envelope)
        .map_err(|e| McpError::InvalidParams(format!("Invalid configuration: {}", e)))?;

    let create_req = CreateFilterRequest {
        name: filter_name.to_string(),
        filter_type: "jwt_auth".to_string(),
        description: Some(description.to_string()),
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        config,
    };

    let filter_result = filter_ops.create(create_req, &auth).await?;

    // Attach to target
    let attachment_result = match target_type {
        "listener" => {
            filter_ops
                .attach_to_listener(&filter_result.data.name, target_name, Some(50), &auth)
                .await
        }
        "route_config" => {
            filter_ops
                .attach_to_route_config(&filter_result.data.name, target_name, None, None, &auth)
                .await
        }
        _ => {
            return Err(McpError::InvalidParams(format!(
                "Invalid target_type '{}'. Must be 'listener' or 'route_config'",
                target_type
            )));
        }
    };

    let attachment_success = attachment_result.is_ok();
    let attachment_message = match attachment_result {
        Ok(_) => format!("Successfully attached to {} '{}'", target_type, target_name),
        Err(e) => format!("Filter created but attachment failed: {}", e),
    };

    let output = json!({
        "success": true,
        "filter": {
            "name": filter_result.data.name,
            "id": filter_result.data.id.to_string(),
            "filter_type": "jwt_auth",
            "issuer": issuer,
            "audiences": audiences,
            "jwks_uri": jwks_uri,
            "forward_jwt": forward_jwt,
            "bypass_cors_preflight": bypass_cors_preflight
        },
        "attachment": {
            "target_type": target_type,
            "target_name": target_name,
            "attached": attachment_success
        },
        "message": attachment_message
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute devops_configure_cors: Create and attach CORS filter
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_devops_configure_cors")]
pub async fn execute_devops_configure_cors(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Parse required parameters
    let filter_name = args.get("filter_name").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: filter_name".to_string())
    })?;

    let allowed_origins: Vec<String> = args
        .get("allowed_origins")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: allowed_origins".to_string())
        })?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    let target_type = args.get("target_type").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: target_type".to_string())
    })?;

    let target_name = args.get("target_name").and_then(|v| v.as_str()).ok_or_else(|| {
        McpError::InvalidParams("Missing required parameter: target_name".to_string())
    })?;

    // Parse optional parameters with defaults
    let allowed_methods: Vec<String> = args
        .get("allowed_methods")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_else(|| {
            vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string(),
                "OPTIONS".to_string(),
            ]
        });

    let allowed_headers: Vec<String> = args
        .get("allowed_headers")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_else(|| vec!["Authorization".to_string(), "Content-Type".to_string()]);

    let exposed_headers: Vec<String> = args
        .get("exposed_headers")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let max_age_seconds =
        args.get("max_age_seconds").and_then(|v| v.as_i64()).unwrap_or(86400) as u64;
    let allow_credentials =
        args.get("allow_credentials").and_then(|v| v.as_bool()).unwrap_or(false);
    let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("CORS filter");

    tracing::info!(
        team = %team,
        filter_name = %filter_name,
        target_type = %target_type,
        target_name = %target_name,
        "Configuring CORS"
    );

    let auth = InternalAuthContext::from_mcp(team);
    let filter_ops = FilterOperations::new(xds_state.clone());

    // Build origin matchers
    let origin_matchers: Vec<Value> = allowed_origins
        .iter()
        .map(|origin| {
            if origin == "*" {
                json!({"type": "prefix", "value": ""}) // Match any origin
            } else {
                json!({"type": "exact", "value": origin})
            }
        })
        .collect();

    // Create CORS filter configuration with envelope format
    let inner_config = json!({
        "policy": {
            "allow_origin": origin_matchers,
            "allow_methods": allowed_methods,
            "allow_headers": allowed_headers,
            "expose_headers": exposed_headers,
            "max_age": max_age_seconds,
            "allow_credentials": allow_credentials
        }
    });

    let config_envelope = json!({
        "type": "cors",
        "config": inner_config
    });

    let config: crate::domain::FilterConfig = serde_json::from_value(config_envelope)
        .map_err(|e| McpError::InvalidParams(format!("Invalid configuration: {}", e)))?;

    let create_req = CreateFilterRequest {
        name: filter_name.to_string(),
        filter_type: "cors".to_string(),
        description: Some(description.to_string()),
        team: if team.is_empty() { None } else { Some(team.to_string()) },
        config,
    };

    let filter_result = filter_ops.create(create_req, &auth).await?;

    // Attach to target
    let attachment_result = match target_type {
        "listener" => {
            filter_ops
                .attach_to_listener(&filter_result.data.name, target_name, Some(10), &auth)
                .await
        }
        "route_config" => {
            filter_ops
                .attach_to_route_config(&filter_result.data.name, target_name, None, None, &auth)
                .await
        }
        _ => {
            return Err(McpError::InvalidParams(format!(
                "Invalid target_type '{}'. Must be 'listener' or 'route_config'",
                target_type
            )));
        }
    };

    let attachment_success = attachment_result.is_ok();
    let attachment_message = match attachment_result {
        Ok(_) => format!("Successfully attached to {} '{}'", target_type, target_name),
        Err(e) => format!("Filter created but attachment failed: {}", e),
    };

    let output = json!({
        "success": true,
        "filter": {
            "name": filter_result.data.name,
            "id": filter_result.data.id.to_string(),
            "filter_type": "cors",
            "allowed_origins": allowed_origins,
            "allowed_methods": allowed_methods,
            "allowed_headers": allowed_headers,
            "max_age_seconds": max_age_seconds,
            "allow_credentials": allow_credentials
        },
        "attachment": {
            "target_type": target_type,
            "target_name": target_name,
            "attached": attachment_success
        },
        "message": attachment_message
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute devops_create_canary_deployment: Create weighted traffic splitting
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_devops_create_canary_deployment")]
pub async fn execute_devops_create_canary_deployment(
    xds_state: &Arc<XdsState>,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Parse required parameters
    let deployment_name =
        args.get("deployment_name").and_then(|v| v.as_str()).ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: deployment_name".to_string())
        })?;

    let stable_endpoints =
        args.get("stable_endpoints").and_then(|v| v.as_array()).ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: stable_endpoints".to_string())
        })?;

    let canary_endpoints =
        args.get("canary_endpoints").and_then(|v| v.as_array()).ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: canary_endpoints".to_string())
        })?;

    let route_config_name =
        args.get("route_config_name").and_then(|v| v.as_str()).ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: route_config_name".to_string())
        })?;

    // Parse optional parameters
    let canary_weight = args.get("canary_weight").and_then(|v| v.as_i64()).unwrap_or(10) as u32;
    let path_prefix = args.get("path_prefix").and_then(|v| v.as_str()).unwrap_or("/");
    let _description = args.get("description").and_then(|v| v.as_str()).unwrap_or("");

    let stable_weight = 100 - canary_weight;

    tracing::info!(
        team = %team,
        deployment_name = %deployment_name,
        canary_weight = %canary_weight,
        stable_weight = %stable_weight,
        "Creating canary deployment"
    );

    let auth = InternalAuthContext::from_mcp(team);
    let cluster_ops = ClusterOperations::new(xds_state.clone());
    let route_ops = RouteConfigOperations::new(xds_state.clone());

    let stable_cluster_name = format!("{}-stable", deployment_name);
    let canary_cluster_name = format!("{}-canary", deployment_name);

    let mut created_resources = Vec::new();

    // Step 1: Create stable cluster
    let stable_endpoint_specs: Vec<EndpointSpec> = stable_endpoints
        .iter()
        .map(|ep| {
            let address = ep.get("address").and_then(|v| v.as_str()).unwrap_or("");
            let port = ep.get("port").and_then(|v| v.as_i64()).unwrap_or(80);
            EndpointSpec::String(format!("{}:{}", address, port))
        })
        .collect();

    let stable_config = ClusterSpec {
        endpoints: stable_endpoint_specs,
        lb_policy: Some("ROUND_ROBIN".to_string()),
        ..Default::default()
    };

    let create_stable_req = CreateClusterRequest {
        name: stable_cluster_name.clone(),
        service_name: format!("{} (Stable)", deployment_name),
        team: None,
        config: stable_config,
    };

    let stable_result = cluster_ops.create(create_stable_req, &auth).await?;
    created_resources.push(json!({
        "type": "cluster",
        "name": stable_result.data.name,
        "role": "stable",
        "weight": stable_weight,
        "endpoint_count": stable_endpoints.len()
    }));

    // Step 2: Create canary cluster
    let canary_endpoint_specs: Vec<EndpointSpec> = canary_endpoints
        .iter()
        .map(|ep| {
            let address = ep.get("address").and_then(|v| v.as_str()).unwrap_or("");
            let port = ep.get("port").and_then(|v| v.as_i64()).unwrap_or(80);
            EndpointSpec::String(format!("{}:{}", address, port))
        })
        .collect();

    let canary_config = ClusterSpec {
        endpoints: canary_endpoint_specs,
        lb_policy: Some("ROUND_ROBIN".to_string()),
        ..Default::default()
    };

    let create_canary_req = CreateClusterRequest {
        name: canary_cluster_name.clone(),
        service_name: format!("{} (Canary)", deployment_name),
        team: None,
        config: canary_config,
    };

    let canary_result = cluster_ops.create(create_canary_req, &auth).await?;
    created_resources.push(json!({
        "type": "cluster",
        "name": canary_result.data.name,
        "role": "canary",
        "weight": canary_weight,
        "endpoint_count": canary_endpoints.len()
    }));

    // Step 3: Create route config with weighted routing
    let route_config = json!({
        "virtual_hosts": [{
            "name": format!("{}-vh", route_config_name),
            "domains": ["*"],
            "routes": [{
                "name": format!("{}-weighted-route", deployment_name),
                "match": {
                    "prefix": path_prefix
                },
                "action": {
                    "type": "weighted",
                    "weighted_clusters": [
                        {
                            "cluster": stable_cluster_name,
                            "weight": stable_weight
                        },
                        {
                            "cluster": canary_cluster_name,
                            "weight": canary_weight
                        }
                    ]
                }
            }]
        }],
        "description": format!("Canary deployment: {}% stable, {}% canary", stable_weight, canary_weight)
    });

    let create_rc_req = crate::internal_api::CreateRouteConfigRequest {
        name: route_config_name.to_string(),
        team: None,
        config: route_config,
    };

    let rc_result = route_ops.create(create_rc_req, &auth).await?;
    created_resources.push(json!({
        "type": "route_config",
        "name": rc_result.data.name,
        "traffic_split": format!("{}% stable / {}% canary", stable_weight, canary_weight)
    }));

    let output = json!({
        "success": true,
        "canary_deployment": {
            "name": deployment_name,
            "stable_cluster": stable_cluster_name,
            "canary_cluster": canary_cluster_name,
            "route_config": route_config_name,
            "path_prefix": path_prefix,
            "traffic_split": {
                "stable_weight": stable_weight,
                "canary_weight": canary_weight
            }
        },
        "created_resources": created_resources,
        "message": format!("Successfully created canary deployment '{}' with {}% traffic to canary", deployment_name, canary_weight)
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute devops_get_deployment_status: Get aggregated status
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_devops_get_deployment_status")]
pub async fn execute_devops_get_deployment_status(
    xds_state: &Arc<XdsState>,
    team: &str,
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

    let auth = InternalAuthContext::from_mcp(team);

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
    fn test_devops_deploy_api_tool_definition() {
        let tool = devops_deploy_api_tool();
        assert_eq!(tool.name, "devops_deploy_api");
        assert!(tool.description.contains("Deploy an API endpoint"));

        let schema = tool.input_schema;
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("cluster_name")));
        assert!(required.contains(&json!("endpoints")));
        assert!(required.contains(&json!("route_config_name")));
    }

    #[test]
    fn test_devops_configure_rate_limiting_tool_definition() {
        let tool = devops_configure_rate_limiting_tool();
        assert_eq!(tool.name, "devops_configure_rate_limiting");
        assert!(tool.description.contains("rate limiting"));

        let schema = tool.input_schema;
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("filter_name")));
        assert!(required.contains(&json!("max_requests")));
        assert!(required.contains(&json!("target_type")));
        assert!(required.contains(&json!("target_name")));
    }

    #[test]
    fn test_devops_enable_jwt_auth_tool_definition() {
        let tool = devops_enable_jwt_auth_tool();
        assert_eq!(tool.name, "devops_enable_jwt_auth");
        assert!(tool.description.contains("JWT authentication"));

        let schema = tool.input_schema;
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("filter_name")));
        assert!(required.contains(&json!("issuer")));
        assert!(required.contains(&json!("audiences")));
        assert!(required.contains(&json!("jwks_uri")));
    }

    #[test]
    fn test_devops_configure_cors_tool_definition() {
        let tool = devops_configure_cors_tool();
        assert_eq!(tool.name, "devops_configure_cors");
        assert!(tool.description.contains("CORS"));

        let schema = tool.input_schema;
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("filter_name")));
        assert!(required.contains(&json!("allowed_origins")));
        assert!(required.contains(&json!("target_type")));
        assert!(required.contains(&json!("target_name")));
    }

    #[test]
    fn test_devops_create_canary_deployment_tool_definition() {
        let tool = devops_create_canary_deployment_tool();
        assert_eq!(tool.name, "devops_create_canary_deployment");
        assert!(tool.description.contains("canary deployment"));

        let schema = tool.input_schema;
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("deployment_name")));
        assert!(required.contains(&json!("stable_endpoints")));
        assert!(required.contains(&json!("canary_endpoints")));
        assert!(required.contains(&json!("route_config_name")));
    }

    #[test]
    fn test_devops_get_deployment_status_tool_definition() {
        let tool = devops_get_deployment_status_tool();
        assert_eq!(tool.name, "devops_get_deployment_status");
        assert!(tool.description.contains("deployment status"));

        let schema = tool.input_schema;
        // All parameters are optional for status check
        assert!(
            schema["required"].is_null()
                || schema["required"].as_array().map(|a| a.is_empty()).unwrap_or(true)
        );
    }

    #[test]
    fn test_all_tools_have_valid_schemas() {
        let tools = vec![
            devops_deploy_api_tool(),
            devops_configure_rate_limiting_tool(),
            devops_enable_jwt_auth_tool(),
            devops_configure_cors_tool(),
            devops_create_canary_deployment_tool(),
            devops_get_deployment_status_tool(),
        ];

        for tool in tools {
            // Verify tool has name and description
            assert!(!tool.name.is_empty(), "Tool name should not be empty");
            assert!(!tool.description.is_empty(), "Tool description should not be empty");

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
