//! Team-scoped endpoints for bootstrap configuration and team management

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, Response, StatusCode},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::{
    api::{
        error::ApiError, handlers::team_access::require_resource_access_resolved, routes::ApiState,
    },
    auth::{
        authorization::{extract_org_scopes, has_admin_bypass},
        models::AuthContext,
        team::{CreateTeamRequest, Team, UpdateTeamRequest},
    },
    domain::{OrgId, TeamId, UserId},
    errors::Error,
    storage::repositories::{
        SqlxTeamMembershipRepository, SqlxTeamRepository, TeamMembershipRepository, TeamRepository,
    },
};

/// Query parameters for bootstrap endpoint
#[derive(Debug, Clone, Deserialize, Serialize, IntoParams, ToSchema)]
pub struct BootstrapQuery {
    /// Output format: yaml or json (default: yaml)
    #[serde(default)]
    #[param(required = false)]
    pub format: Option<String>,

    /// Enable mTLS configuration in bootstrap. When true, adds transport_socket
    /// with TLS settings to the xds_cluster. Defaults to true if control plane
    /// has mTLS configured.
    #[serde(default)]
    #[param(required = false)]
    pub mtls: Option<bool>,

    /// Path to client certificate file (default: /etc/envoy/certs/client.pem)
    #[serde(default)]
    #[param(required = false)]
    pub cert_path: Option<String>,

    /// Path to client private key file (default: /etc/envoy/certs/client-key.pem)
    #[serde(default)]
    #[param(required = false)]
    pub key_path: Option<String>,

    /// Path to CA certificate file (default: /etc/envoy/certs/ca.pem)
    #[serde(default)]
    #[param(required = false)]
    pub ca_path: Option<String>,
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
/// - mTLS transport socket (when enabled)
///
/// # mTLS Configuration
///
/// When mTLS is enabled (via `mtls=true` query param or when the control plane has
/// mTLS configured), the xds_cluster will include a transport_socket with TLS settings.
/// Default certificate paths are:
/// - Client cert: /etc/envoy/certs/client.pem
/// - Client key: /etc/envoy/certs/client-key.pem
/// - CA cert: /etc/envoy/certs/ca.pem
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
        (status = 200, description = "Envoy bootstrap configuration in YAML or JSON format. The configuration includes admin interface, node metadata, dynamic resource discovery (ADS) configuration, and xDS cluster definition. When mTLS is enabled, includes transport_socket for client TLS.", content_type = "application/yaml"),
        (status = 403, description = "Forbidden - user does not have access to the specified team"),
        (status = 500, description = "Internal server error during bootstrap generation")
    ),
    tag = "Administration"
)]
#[instrument(skip(state, q), fields(team = %team, user_id = ?context.user_id, format = ?q.format, mtls = ?q.mtls))]
pub async fn get_team_bootstrap_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    Query(q): Query<BootstrapQuery>,
) -> Result<Response<axum::body::Body>, ApiError> {
    // Authorization: Check if user has permission to access bootstrap
    // Users need either:
    // 1. admin:all scope (bypass all checks)
    // 2. generate-envoy-config:read scope (global access to any team)
    // 3. team:{team}:generate-envoy-config:read scope (team-specific access)
    // 4. team:{team}:*:* scope (team wildcard - access to all resources for the team)
    require_resource_access_resolved(
        &state,
        &context,
        "generate-envoy-config",
        "read",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    let format = q.format.as_deref().unwrap_or("yaml").to_lowercase();

    // Determine if mTLS should be enabled
    // Priority: query param > control plane config
    let control_plane_mtls_enabled = crate::xds::services::is_xds_mtls_enabled();
    let mtls_enabled = q.mtls.unwrap_or(control_plane_mtls_enabled);

    // Get certificate paths (use defaults if not specified)
    let cert_path = q.cert_path.as_deref().unwrap_or(DEFAULT_CERT_PATH);
    let key_path = q.key_path.as_deref().unwrap_or(DEFAULT_KEY_PATH);
    let ca_path = q.ca_path.as_deref().unwrap_or(DEFAULT_CA_PATH);

    // Build ADS bootstrap with node metadata for team-based filtering
    let xds_addr = state.xds_state.config.bind_address.clone();
    let xds_port = state.xds_state.config.port;
    let node_id = format!("team={}/dp-{}", team, uuid::Uuid::new_v4());
    let node_cluster = format!("{}-cluster", team);

    // Build node metadata with team information
    // The xDS server will use this to filter resources
    // Note: Default resources (team IS NULL) are always included
    let metadata = serde_json::json!({
        "team": team,
    });

    // Get Envoy admin config from configuration (base values)
    let envoy_admin = &state.xds_state.config.envoy_admin;

    // Try to get team-specific admin port from database
    let team_repo = team_repository_for_state(&state)?;
    let team_data = team_repo.get_team_by_name(&team).await.map_err(convert_error)?;

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
            "mTLS enabled in bootstrap config"
        );
    }

    // Generate Envoy bootstrap configuration
    // This is minimal - it only tells Envoy where to find the xDS server
    // All actual resources (listeners, routes, clusters) are discovered dynamically
    let bootstrap = serde_json::json!({
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

/// Response for list teams endpoint
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ListTeamsResponse {
    pub teams: Vec<String>,
}

/// List teams accessible to the current user
///
/// Returns:
/// - All teams (from teams table) if user is admin
/// - Only user's teams (from memberships) if user is not admin
#[utoipa::path(
    get,
    path = "/api/v1/teams",
    responses(
        (status = 200, description = "List of teams", body = ListTeamsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    tag = "Administration"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id))]
pub async fn list_teams_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
) -> Result<Json<ListTeamsResponse>, ApiError> {
    // Get database pool
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    let pool = cluster_repo.pool().clone();

    let teams = if has_admin_bypass(&context) {
        // Platform admin: sees all teams (cross-org visibility is intentional)
        let team_repo = SqlxTeamRepository::new(pool);
        let all_teams =
            team_repo.list_teams(1000, 0).await.map_err(|err| ApiError::from(Error::from(err)))?;
        all_teams.into_iter().map(|t| t.name).collect()
    } else if !extract_org_scopes(&context).is_empty() && context.org_id.is_some() {
        // Org admin/member with org context: sees all teams in their org
        let team_repo = SqlxTeamRepository::new(pool);
        let org_id = context.org_id.as_ref().unwrap();
        let org_teams = team_repo
            .list_teams_by_org(org_id)
            .await
            .map_err(|err| ApiError::from(Error::from(err)))?;
        org_teams.into_iter().map(|t| t.name).collect()
    } else {
        // Non-admin users see only their teams from memberships
        let membership_repo = SqlxTeamMembershipRepository::new(pool.clone());
        if let Some(user_id) = &context.user_id {
            let memberships = membership_repo
                .list_user_memberships(user_id)
                .await
                .map_err(|err| ApiError::from(Error::from(err)))?;
            // After FK migration, m.team stores UUIDs. Resolve back to names
            // so the frontend gets human-readable names that match scope patterns.
            let team_ids: Vec<String> = memberships.into_iter().map(|m| m.team).collect();
            if team_ids.is_empty() {
                Vec::new()
            } else {
                let team_repo = SqlxTeamRepository::new(pool);
                team_repo
                    .resolve_team_names(context.org_id.as_ref(), &team_ids)
                    .await
                    .map_err(|err| ApiError::from(Error::from(err)))?
            }
        } else {
            // If no user_id (shouldn't happen for authenticated users), return empty
            Vec::new()
        }
    };

    Ok(Json(ListTeamsResponse { teams }))
}

// ===== Admin-Only Team Management Endpoints =====

/// Helper to create TeamRepository from ApiState.
fn team_repository_for_state(state: &ApiState) -> Result<Arc<dyn TeamRepository>, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Team repository unavailable"))?;
    let pool = cluster_repo.pool().clone();

    Ok(Arc::new(SqlxTeamRepository::new(pool)))
}

/// Check if the current context has admin privileges.
fn require_admin(context: &AuthContext) -> Result<(), ApiError> {
    if !has_admin_bypass(context) {
        return Err(ApiError::forbidden("Admin privileges required"));
    }
    Ok(())
}

/// Query parameters for admin list_teams endpoint.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct AdminListTeamsQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Response for admin list_teams endpoint.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminListTeamsResponse {
    pub teams: Vec<Team>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Create a new team (admin only).
///
/// Creates a new team with the specified details. The team name is immutable
/// after creation and must be unique across all teams.
/// API-level request for creating a team. `org_id` is optional and resolved from auth context.
#[derive(Debug, Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct ApiCreateTeamBody {
    #[validate(length(min = 1, max = 255), regex(path = "crate::utils::TEAM_NAME_REGEX"))]
    name: String,
    #[validate(length(min = 1, max = 255))]
    display_name: String,
    #[validate(length(max = 1000))]
    description: Option<String>,
    owner_user_id: Option<UserId>,
    #[serde(default)]
    org_id: Option<OrgId>,
    settings: Option<serde_json::Value>,
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/teams",
    request_body = CreateTeamRequest,
    responses(
        (status = 201, description = "Team created successfully", body = Team),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Admin privileges required"),
        (status = 409, description = "Team with name already exists")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Administration"
)]
#[instrument(skip(state, body), fields(team_name = %body.name, user_id = ?context.user_id))]
pub async fn admin_create_team(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(body): Json<ApiCreateTeamBody>,
) -> Result<(StatusCode, Json<Team>), ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Validate request
    body.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Resolve org_id: explicit body > auth context > error
    let org_id = body
        .org_id
        .or_else(|| context.org_id.clone())
        .ok_or_else(|| ApiError::BadRequest("org_id is required".to_string()))?;

    let payload = CreateTeamRequest {
        name: body.name.clone(),
        display_name: body.display_name,
        description: body.description,
        owner_user_id: body.owner_user_id.or_else(|| context.user_id.clone()),
        org_id,
        settings: body.settings,
    };

    // Check if name is available
    let repo = team_repository_for_state(&state)?;
    let is_available = repo.is_name_available(&payload.name).await.map_err(convert_error)?;

    if !is_available {
        return Err(ApiError::Conflict(format!(
            "Team with name '{}' already exists",
            payload.name
        )));
    }

    // Create team
    let team = repo.create_team(payload).await.map_err(convert_error)?;

    Ok((StatusCode::CREATED, Json(team)))
}

/// Get a team by ID (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/admin/teams/{id}",
    params(
        ("id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Team found", body = Team),
        (status = 403, description = "Admin privileges required"),
        (status = 404, description = "Team not found")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Administration"
)]
#[instrument(skip(state), fields(team_id = %id, user_id = ?context.user_id))]
pub async fn admin_get_team(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<Team>, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Parse team ID
    let team_id = TeamId::from_string(id);

    // Get team
    let repo = team_repository_for_state(&state)?;
    let team = repo
        .get_team_by_id(&team_id)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound("Team not found".to_string()))?;

    Ok(Json(team))
}

/// List all teams with pagination (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/admin/teams",
    params(AdminListTeamsQuery),
    responses(
        (status = 200, description = "Teams listed successfully", body = AdminListTeamsResponse),
        (status = 403, description = "Admin privileges required")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Administration"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, limit = %query.limit, offset = %query.offset))]
pub async fn admin_list_teams(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<AdminListTeamsQuery>,
) -> Result<Json<AdminListTeamsResponse>, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // List teams
    let repo = team_repository_for_state(&state)?;
    let teams = repo.list_teams(query.limit, query.offset).await.map_err(convert_error)?;
    let total = repo.count_teams().await.map_err(convert_error)?;

    Ok(Json(AdminListTeamsResponse { teams, total, limit: query.limit, offset: query.offset }))
}

/// Update a team (admin only).
///
/// Updates team details. Note that the team name is immutable and cannot be changed.
#[utoipa::path(
    put,
    path = "/api/v1/admin/teams/{id}",
    params(
        ("id" = String, Path, description = "Team ID")
    ),
    request_body = UpdateTeamRequest,
    responses(
        (status = 200, description = "Team updated successfully", body = Team),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Admin privileges required"),
        (status = 404, description = "Team not found")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Administration"
)]
#[instrument(skip(state, payload), fields(team_id = %id, user_id = ?context.user_id))]
pub async fn admin_update_team(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateTeamRequest>,
) -> Result<Json<Team>, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Validate request
    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Parse team ID
    let team_id = TeamId::from_string(id);

    // Update team
    let repo = team_repository_for_state(&state)?;
    let team = repo.update_team(&team_id, payload).await.map_err(convert_error)?;

    Ok(Json(team))
}

/// Delete a team (admin only).
///
/// Deletes a team. This operation will fail if there are resources (listeners, routes,
/// clusters, etc.) referencing this team due to foreign key constraints.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/teams/{id}",
    params(
        ("id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 204, description = "Team deleted successfully"),
        (status = 403, description = "Admin privileges required"),
        (status = 404, description = "Team not found"),
        (status = 409, description = "Team has resources - cannot delete")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Administration"
)]
#[instrument(skip(state), fields(team_id = %id, user_id = ?context.user_id))]
pub async fn admin_delete_team(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Parse team ID
    let team_id = TeamId::from_string(id);

    // Delete team
    let repo = team_repository_for_state(&state)?;
    repo.delete_team(&team_id).await.map_err(convert_error)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Convert domain errors to API errors.
fn convert_error(error: Error) -> ApiError {
    ApiError::from(error)
}

/// Response for mTLS status endpoint
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MtlsStatusResponse {
    /// Whether mTLS is fully enabled (PKI configured + xDS TLS configured)
    pub enabled: bool,

    /// Whether the xDS server has TLS enabled (server certificate configured)
    pub xds_server_tls: bool,

    /// Whether client certificate authentication is required
    pub client_auth_required: bool,

    /// SPIFFE trust domain for certificate identity URIs
    pub trust_domain: String,

    /// Whether Vault PKI mount is configured for certificate generation
    pub pki_mount_configured: bool,

    /// Message describing the current mTLS status
    pub message: String,
}

/// Get mTLS configuration status
///
/// Returns the current mTLS configuration status for the control plane.
/// This endpoint helps operators and developers understand whether mTLS
/// is enabled and properly configured.
///
/// # Response Fields
///
/// - `enabled`: True if both PKI and xDS TLS are configured
/// - `xds_server_tls`: True if xDS server has TLS certificate configured
/// - `client_auth_required`: True if client certificates are required
/// - `trust_domain`: The SPIFFE trust domain being used
/// - `pki_mount_configured`: True if Vault PKI is configured for cert generation
#[utoipa::path(
    get,
    path = "/api/v1/mtls/status",
    responses(
        (status = 200, description = "mTLS configuration status", body = MtlsStatusResponse),
    ),
    tag = "System"
)]
#[instrument]
pub async fn get_mtls_status_handler() -> Json<MtlsStatusResponse> {
    // Check if xDS server TLS is enabled
    let xds_server_tls =
        std::env::var("FLOWPLANE_XDS_TLS_CERT_PATH").ok().filter(|v| !v.is_empty()).is_some();

    // Check if client auth is required (enabled by default when TLS is enabled)
    let client_auth_required = crate::xds::services::is_xds_mtls_enabled();

    // Check if Vault PKI is configured
    let pki_mount_configured =
        std::env::var("FLOWPLANE_VAULT_PKI_MOUNT_PATH").ok().filter(|v| !v.is_empty()).is_some();

    // Get trust domain
    let trust_domain = std::env::var("FLOWPLANE_SPIFFE_TRUST_DOMAIN")
        .unwrap_or_else(|_| "flowplane.local".to_string());

    // mTLS is fully enabled when both PKI and xDS TLS are configured
    let enabled = pki_mount_configured && client_auth_required;

    let message = if enabled {
        "mTLS is fully enabled. Proxies must present valid client certificates.".to_string()
    } else if xds_server_tls && !client_auth_required {
        "TLS is enabled but client authentication is disabled. Proxies are not authenticated."
            .to_string()
    } else if pki_mount_configured && !xds_server_tls {
        "Vault PKI is configured but xDS server TLS is not enabled. Configure FLOWPLANE_XDS_TLS_* environment variables.".to_string()
    } else {
        "mTLS is disabled. Configure FLOWPLANE_VAULT_PKI_MOUNT_PATH and FLOWPLANE_XDS_TLS_* to enable.".to_string()
    };

    Json(MtlsStatusResponse {
        enabled,
        xds_server_tls,
        client_auth_required,
        trust_domain,
        pki_mount_configured,
        message,
    })
}
