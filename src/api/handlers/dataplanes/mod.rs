//! Dataplane API handlers
//!
//! CRUD operations for dataplanes (Envoy instances with gateway_host).
//! Certificates are managed via /proxy-certificates endpoint (shared per dataplane name).
//! See ADR-008 for certificate model decision.

pub mod types;

pub use types::{
    CreateDataplaneBody, DataplaneResponse, EnvoyConfigQuery, TeamDataplanePath,
    UpdateDataplaneBody,
};

use super::pagination::{PaginatedResponse, PaginationQuery};

use axum::{
    extract::{Path, Query, State},
    http::{header, Response, StatusCode},
    Extension, Json,
};
use tracing::instrument;
use validator::Validate;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::{authorization::require_resource_access, models::AuthContext},
    storage::repositories::{
        CreateDataplaneRequest, DataplaneRepository, TeamRepository, UpdateDataplaneRequest,
    },
};

use super::team_access::{
    get_effective_team_ids, require_resource_access_resolved, team_repo_from_state,
    verify_team_access,
};

/// Create a new dataplane
#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/dataplanes",
    request_body = CreateDataplaneBody,
    responses(
        (status = 201, description = "Dataplane created successfully", body = DataplaneResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 409, description = "Dataplane with name already exists")
    ),
    params(
        ("team" = String, Path, description = "Team name")
    ),
    tag = "Dataplanes"
)]
#[instrument(skip(state, payload), fields(team = %team, name = %payload.name, user_id = ?context.user_id))]
pub async fn create_dataplane_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    Json(payload): Json<CreateDataplaneBody>,
) -> Result<(StatusCode, Json<DataplaneResponse>), ApiError> {
    // Validate request
    payload.validate().map_err(ApiError::from)?;

    // Validate team matches
    if payload.team != team {
        return Err(ApiError::BadRequest("Team in body must match team in path".to_string()));
    }

    // Authorization
    require_resource_access_resolved(
        &state,
        &context,
        "dataplanes",
        "write",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database storage
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &team,
        context.org_id.as_ref(),
    )
    .await?;

    // Get repository
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let repo = DataplaneRepository::new(cluster_repo.pool().clone());

    // Check if name is available
    if repo.exists_by_name(&team_id, &payload.name).await.map_err(ApiError::from)? {
        return Err(ApiError::Conflict(format!(
            "Dataplane with name '{}' already exists for team '{}'",
            payload.name, team
        )));
    }

    // Create dataplane
    let request = CreateDataplaneRequest {
        team: team_id,
        name: payload.name,
        gateway_host: payload.gateway_host,
        description: payload.description,
    };

    let dataplane = repo.create(request).await.map_err(ApiError::from)?;

    Ok((StatusCode::CREATED, Json(DataplaneResponse::from(dataplane))))
}

/// List dataplanes for a team
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/dataplanes",
    responses(
        (status = 200, description = "List of dataplanes", body = PaginatedResponse<DataplaneResponse>),
        (status = 403, description = "Forbidden - insufficient permissions")
    ),
    params(
        ("team" = String, Path, description = "Team name"),
        PaginationQuery
    ),
    tag = "Dataplanes"
)]
#[instrument(skip(state, query), fields(team = %team, user_id = ?context.user_id, limit = %query.limit))]
pub async fn list_dataplanes_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<PaginatedResponse<DataplaneResponse>>, ApiError> {
    // Authorization
    require_resource_access_resolved(
        &state,
        &context,
        "dataplanes",
        "read",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    let (limit, offset) = query.clamp(1000);

    // Resolve team name to UUID for database storage
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &team,
        context.org_id.as_ref(),
    )
    .await?;

    // Get repository
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let repo = DataplaneRepository::new(cluster_repo.pool().clone());

    let dataplanes = repo
        .list_by_team(&team_id, Some(limit as i32), Some(offset as i32))
        .await
        .map_err(ApiError::from)?;

    let total = dataplanes.len() as i64;
    let items: Vec<DataplaneResponse> =
        dataplanes.into_iter().map(DataplaneResponse::from).collect();

    Ok(Json(PaginatedResponse::new(items, total, limit, offset)))
}

/// List all dataplanes (admin or multi-team access)
#[utoipa::path(
    get,
    path = "/api/v1/dataplanes",
    responses(
        (status = 200, description = "List of all dataplanes", body = PaginatedResponse<DataplaneResponse>),
        (status = 403, description = "Forbidden - insufficient permissions")
    ),
    params(PaginationQuery),
    tag = "Dataplanes"
)]
#[instrument(skip(state, query), fields(user_id = ?context.user_id, limit = %query.limit))]
pub async fn list_all_dataplanes_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<PaginatedResponse<DataplaneResponse>>, ApiError> {
    // Authorization: require dataplanes:read scope
    require_resource_access(&context, "dataplanes", "read", None)?;

    let (limit, offset) = query.clamp(1000);

    // Get effective teams for the user
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let pool = cluster_repo.pool().clone();

    let team_repo = team_repo_from_state(&state)?;
    let teams = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;

    let repo = DataplaneRepository::new(pool);

    let dataplanes = repo
        .list_by_teams(&teams, Some(limit as i32), Some(offset as i32))
        .await
        .map_err(ApiError::from)?;

    let total = dataplanes.len() as i64;
    let items: Vec<DataplaneResponse> =
        dataplanes.into_iter().map(DataplaneResponse::from).collect();

    Ok(Json(PaginatedResponse::new(items, total, limit, offset)))
}

/// Get a specific dataplane by name
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/dataplanes/{name}",
    responses(
        (status = 200, description = "Dataplane found", body = DataplaneResponse),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Dataplane not found")
    ),
    params(
        ("team" = String, Path, description = "Team name"),
        ("name" = String, Path, description = "Dataplane name")
    ),
    tag = "Dataplanes"
)]
#[instrument(skip(state), fields(team = %path.0, name = %path.1, user_id = ?context.user_id))]
pub async fn get_dataplane_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<(String, String)>,
) -> Result<Json<DataplaneResponse>, ApiError> {
    let (team, name) = path;

    // Authorization
    require_resource_access_resolved(
        &state,
        &context,
        "dataplanes",
        "read",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database storage
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &team,
        context.org_id.as_ref(),
    )
    .await?;

    // Get repository
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let repo = DataplaneRepository::new(cluster_repo.pool().clone());

    let dataplane =
        repo.get_by_name(&team_id, &name).await.map_err(ApiError::from)?.ok_or_else(|| {
            ApiError::NotFound(format!("Dataplane '{}' not found for team '{}'", name, team))
        })?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let dataplane = verify_team_access(dataplane, &team_scopes).await?;

    Ok(Json(DataplaneResponse::from(dataplane)))
}

/// Update an existing dataplane
#[utoipa::path(
    put,
    path = "/api/v1/teams/{team}/dataplanes/{name}",
    request_body = UpdateDataplaneBody,
    responses(
        (status = 200, description = "Dataplane updated successfully", body = DataplaneResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Dataplane not found")
    ),
    params(
        ("team" = String, Path, description = "Team name"),
        ("name" = String, Path, description = "Dataplane name")
    ),
    tag = "Dataplanes"
)]
#[instrument(skip(state, payload), fields(team = %path.0, name = %path.1, user_id = ?context.user_id))]
pub async fn update_dataplane_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<(String, String)>,
    Json(payload): Json<UpdateDataplaneBody>,
) -> Result<Json<DataplaneResponse>, ApiError> {
    let (team, name) = path;

    // Validate request
    payload.validate().map_err(ApiError::from)?;

    // Authorization
    require_resource_access_resolved(
        &state,
        &context,
        "dataplanes",
        "write",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database storage
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &team,
        context.org_id.as_ref(),
    )
    .await?;

    // Get repository
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let repo = DataplaneRepository::new(cluster_repo.pool().clone());

    // Get existing dataplane
    let dataplane =
        repo.get_by_name(&team_id, &name).await.map_err(ApiError::from)?.ok_or_else(|| {
            ApiError::NotFound(format!("Dataplane '{}' not found for team '{}'", name, team))
        })?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let dataplane = verify_team_access(dataplane, &team_scopes).await?;

    // Update dataplane
    let request = UpdateDataplaneRequest {
        gateway_host: Some(payload.gateway_host),
        description: Some(payload.description),
    };

    let updated = repo.update(&dataplane.id, request).await.map_err(ApiError::from)?;

    Ok(Json(DataplaneResponse::from(updated)))
}

/// Delete a dataplane
#[utoipa::path(
    delete,
    path = "/api/v1/teams/{team}/dataplanes/{name}",
    responses(
        (status = 204, description = "Dataplane deleted successfully"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Dataplane not found")
    ),
    params(
        ("team" = String, Path, description = "Team name"),
        ("name" = String, Path, description = "Dataplane name")
    ),
    tag = "Dataplanes"
)]
#[instrument(skip(state), fields(team = %path.0, name = %path.1, user_id = ?context.user_id))]
pub async fn delete_dataplane_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (team, name) = path;

    // Authorization
    require_resource_access_resolved(
        &state,
        &context,
        "dataplanes",
        "write",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database storage
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &team,
        context.org_id.as_ref(),
    )
    .await?;

    // Get repository
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let repo = DataplaneRepository::new(cluster_repo.pool().clone());

    // Get existing dataplane
    let dataplane =
        repo.get_by_name(&team_id, &name).await.map_err(ApiError::from)?.ok_or_else(|| {
            ApiError::NotFound(format!("Dataplane '{}' not found for team '{}'", name, team))
        })?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let dataplane = verify_team_access(dataplane, &team_scopes).await?;

    // Delete dataplane
    repo.delete(&dataplane.id).await.map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Default certificate paths for mTLS
const DEFAULT_CERT_PATH: &str = "/etc/envoy/certs/client.pem";
const DEFAULT_KEY_PATH: &str = "/etc/envoy/certs/client-key.pem";
const DEFAULT_CA_PATH: &str = "/etc/envoy/certs/ca.pem";

/// Build transport_socket configuration for mTLS
fn build_mtls_transport_socket(
    cert_path: &str,
    key_path: &str,
    ca_path: &str,
) -> serde_json::Value {
    serde_json::json!({
        "name": "envoy.transport_sockets.tls",
        "typed_config": {
            "@type": "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext",
            "common_tls_context": {
                "tls_certificates": [
                    {
                        "certificate_chain": {
                            "filename": cert_path
                        },
                        "private_key": {
                            "filename": key_path
                        }
                    }
                ],
                "validation_context": {
                    "trusted_ca": {
                        "filename": ca_path
                    }
                }
            }
        }
    })
}

/// Generate Envoy configuration for a specific dataplane
///
/// This endpoint generates an Envoy configuration that enables team-scoped
/// resource discovery via xDS with dataplane-specific node ID. When Envoy starts with
/// this config, it will:
/// 1. Connect to the xDS server with team and dataplane metadata
/// 2. Discover all resources (listeners, routes, clusters) for the team
/// 3. Include gateway_host in node metadata for MCP tool execution
///
/// The config includes:
/// - Admin interface configuration
/// - Node metadata with team and dataplane information
/// - Dynamic resource configuration (ADS) pointing to xDS server
/// - Static xDS cluster definition
/// - mTLS transport socket (when enabled)
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/dataplanes/{name}/envoy-config",
    params(
        ("team" = String, Path, description = "Team name"),
        ("name" = String, Path, description = "Dataplane name"),
        EnvoyConfigQuery
    ),
    responses(
        (status = 200, description = "Envoy configuration in YAML or JSON format"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Dataplane not found"),
        (status = 500, description = "Internal server error during envoy config generation")
    ),
    tag = "Dataplanes"
)]
#[instrument(skip(state, query), fields(team = %path.0, name = %path.1, user_id = ?context.user_id, format = ?query.format))]
pub async fn generate_envoy_config_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<(String, String)>,
    Query(query): Query<EnvoyConfigQuery>,
) -> Result<Response<axum::body::Body>, ApiError> {
    let (team, name) = path;

    // Authorization
    require_resource_access_resolved(
        &state,
        &context,
        "generate-envoy-config",
        "read",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database storage
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &team,
        context.org_id.as_ref(),
    )
    .await?;

    // Get repository
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let repo = DataplaneRepository::new(cluster_repo.pool().clone());

    // Get dataplane
    let dataplane =
        repo.get_by_name(&team_id, &name).await.map_err(ApiError::from)?.ok_or_else(|| {
            ApiError::NotFound(format!("Dataplane '{}' not found for team '{}'", name, team))
        })?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let dataplane = verify_team_access(dataplane, &team_scopes).await?;

    let format = query.format.as_deref().unwrap_or("yaml").to_lowercase();

    // Determine if mTLS should be enabled
    let control_plane_mtls_enabled = crate::xds::services::is_xds_mtls_enabled();
    let mtls_enabled = query.mtls.unwrap_or(control_plane_mtls_enabled);

    // Get certificate paths (use defaults if not specified)
    let cert_path = query.cert_path.as_deref().unwrap_or(DEFAULT_CERT_PATH);
    let key_path = query.key_path.as_deref().unwrap_or(DEFAULT_KEY_PATH);
    let ca_path = query.ca_path.as_deref().unwrap_or(DEFAULT_CA_PATH);

    // Build ADS config with node metadata for team-based filtering
    // Priority: query param > env var > config bind_address
    let xds_addr = query
        .xds_host
        .clone()
        .or_else(|| std::env::var("FLOWPLANE_XDS_ADVERTISE_ADDRESS").ok())
        .unwrap_or_else(|| state.xds_state.config.bind_address.clone());
    let xds_port = query.xds_port.unwrap_or(state.xds_state.config.port);

    tracing::debug!(
        xds_addr = %xds_addr,
        xds_port = %xds_port,
        from_query = %query.xds_host.is_some(),
        from_env = %std::env::var("FLOWPLANE_XDS_ADVERTISE_ADDRESS").is_ok(),
        "Using xDS address for envoy config"
    );

    // Use dataplane ID in node.id for explicit dataplane identification
    let node_id = format!("team={}/dp-{}", team, dataplane.id);
    let node_cluster = format!("{}-cluster", team);

    // Build node metadata with team and gateway_host information
    let metadata = serde_json::json!({
        "team": team,
        "dataplane_id": dataplane.id.to_string(),
        "dataplane_name": dataplane.name,
        "gateway_host": dataplane.gateway_host,
    });

    // Get Envoy admin config from configuration
    let envoy_admin = &state.xds_state.config.envoy_admin;

    // Try to get team-specific admin port from database
    let team_repo = team_repo_from_state(&state)?;
    let team_data = team_repo.get_team_by_name(&team).await.map_err(ApiError::from)?;

    // Use team-specific port if available, otherwise fall back to global config
    let admin_port =
        team_data.as_ref().and_then(|t| t.envoy_admin_port).unwrap_or(envoy_admin.port);

    // Build xds_cluster configuration
    let mut xds_cluster = serde_json::json!({
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
    });

    // Add transport_socket for mTLS if enabled
    if mtls_enabled {
        let transport_socket = build_mtls_transport_socket(cert_path, key_path, ca_path);
        let cluster_obj = xds_cluster.as_object_mut().ok_or_else(|| {
            tracing::error!("Invalid xDS cluster structure: expected JSON object");
            ApiError::Internal("Failed to configure mTLS: invalid cluster structure".to_string())
        })?;
        cluster_obj.insert("transport_socket".to_string(), transport_socket);

        tracing::debug!(
            cert_path = %cert_path,
            key_path = %key_path,
            ca_path = %ca_path,
            "mTLS enabled in dataplane envoy config"
        );
    }

    // Generate Envoy configuration
    let envoy_config = serde_json::json!({
        "admin": {
            "access_log_path": envoy_admin.access_log_path,
            "address": {
                "socket_address": {
                    "address": envoy_admin.bind_address,
                    "port_value": admin_port
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
            "clusters": [xds_cluster]
        }
    });

    // Return envoy config in requested format
    let response = if format == "json" {
        let body = serde_json::to_vec(&envoy_config)
            .map_err(|e| ApiError::service_unavailable(e.to_string()))?;
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .map_err(|e| {
                ApiError::service_unavailable(format!("Failed to build response: {}", e))
            })?
    } else {
        let yaml = serde_yaml::to_string(&envoy_config)
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
