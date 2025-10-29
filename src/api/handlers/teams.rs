//! Team-scoped endpoints for bootstrap configuration and team management

use axum::{
    extract::{Path, Query, State},
    http::{header, Response},
    Extension,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::require_resource_access,
    auth::models::AuthContext,
};

/// Query parameters for bootstrap endpoint
#[derive(Debug, Clone, Deserialize, Serialize, IntoParams, ToSchema)]
pub struct BootstrapQuery {
    #[serde(default)]
    #[param(required = false)]
    pub format: Option<String>, // yaml|json (default yaml)
    #[serde(default)]
    #[param(required = false)]
    pub include_default: Option<bool>, // default false
}

/// Get Envoy bootstrap configuration for a team
///
/// This endpoint generates an Envoy bootstrap configuration that enables team-scoped
/// resource discovery via xDS. When Envoy starts with this bootstrap, it will:
/// 1. Connect to the xDS server with team metadata
/// 2. Discover all resources (listeners, routes, clusters) for the team
/// 3. Apply team-wide defaults (global filters, headers, etc.)
///
/// The bootstrap includes:
/// - Admin interface configuration
/// - Node metadata with team information for server-side filtering
/// - Dynamic resource configuration (ADS) pointing to xDS server
/// - Static xDS cluster definition
///
/// # Team Isolation
///
/// The xDS server filters all resources by team based on the node metadata,
/// ensuring Envoy only receives resources belonging to the specified team.
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/bootstrap",
    params(
        ("team" = String, Path, description = "Team name", example = "payments"),
        BootstrapQuery
    ),
    responses(
        (status = 200, description = "Envoy bootstrap configuration in YAML or JSON format. The configuration includes admin interface, node metadata, dynamic resource discovery (ADS) configuration, and xDS cluster definition. All resources (listeners, routes, clusters) are discovered dynamically via xDS based on team filtering.", content_type = "application/yaml"),
        (status = 403, description = "Forbidden - user does not have access to the specified team"),
        (status = 500, description = "Internal server error during bootstrap generation")
    ),
    tag = "teams"
)]
pub async fn get_team_bootstrap_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    Query(q): Query<BootstrapQuery>,
) -> Result<Response<axum::body::Body>, ApiError> {
    // Authorization: Check if user has permission to access bootstrap
    // Users need either:
    // 1. admin:all scope (bypass all checks)
    // 2. api-definitions:read scope (global access)
    // 3. team:{team}:api-definitions:read scope (team-specific access)
    // Note: We don't pass the team to require_resource_access because:
    // - Global scopes (api-definitions:read) should allow access to any team
    // - Team-scoped tokens will be filtered server-side by xDS based on node metadata
    require_resource_access(&context, "api-definitions", "read", None)?;

    let format = q.format.as_deref().unwrap_or("yaml").to_lowercase();
    let include_default = q.include_default.unwrap_or(false);

    // Build ADS bootstrap with node metadata for team-based filtering
    let xds_addr = state.xds_state.config.bind_address.clone();
    let xds_port = state.xds_state.config.port;
    let node_id = format!("team={}/dp-{}", team, uuid::Uuid::new_v4());
    let node_cluster = format!("{}-cluster", team);

    // Build node metadata with team information
    // The xDS server will use this to filter resources
    let metadata = serde_json::json!({
        "team": team,
        "include_default": include_default,
    });

    // Generate Envoy bootstrap configuration
    // This is minimal - it only tells Envoy where to find the xDS server
    // All actual resources (listeners, routes, clusters) are discovered dynamically
    let bootstrap = serde_json::json!({
        "admin": {
            "access_log_path": "/tmp/envoy_admin.log",
            "address": {
                "socket_address": {
                    "address": "127.0.0.1",
                    "port_value": 9901
                }
            }
        },
        "node": {
            "id": node_id,
            "cluster": node_cluster,
            "metadata": metadata
        },
        "dynamic_resources": {
            "lds_config": { "ads": {} },
            "cds_config": { "ads": {} },
            "ads_config": {
                "api_type": "GRPC",
                "transport_api_version": "V3",
                "grpc_services": [
                    {
                        "envoy_grpc": {
                            "cluster_name": "xds_cluster"
                        }
                    }
                ]
            }
        },
        "static_resources": {
            "clusters": [
                {
                    "name": "xds_cluster",
                    "type": "LOGICAL_DNS",
                    "dns_lookup_family": "V4_ONLY",
                    "connect_timeout": "1s",
                    "http2_protocol_options": {},
                    "load_assignment": {
                        "cluster_name": "xds_cluster",
                        "endpoints": [
                            {
                                "lb_endpoints": [
                                    {
                                        "endpoint": {
                                            "address": {
                                                "socket_address": {
                                                    "address": xds_addr,
                                                    "port_value": xds_port
                                                }
                                            }
                                        }
                                    }
                                ]
                            }
                        ]
                    }
                }
            ]
        }
    });

    // Return bootstrap in requested format (YAML or JSON)
    let response = if format == "json" {
        let body = serde_json::to_vec(&bootstrap)
            .map_err(|e| ApiError::service_unavailable(e.to_string()))?;
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .map_err(|e| {
                ApiError::service_unavailable(format!("Failed to build response: {}", e))
            })?
    } else {
        let yaml = serde_yaml::to_string(&bootstrap)
            .map_err(|e| ApiError::service_unavailable(e.to_string()))?;
        Response::builder()
            .header(header::CONTENT_TYPE, "application/yaml")
            .body(axum::body::Body::from(yaml))
            .map_err(|e| {
                ApiError::service_unavailable(format!("Failed to build response: {}", e))
            })?
    };

    Ok(response)
}
