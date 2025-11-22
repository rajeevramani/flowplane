//! Team-scoped endpoints for bootstrap configuration and team management

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, Response, StatusCode},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::{
        authorization::{has_admin_bypass, require_resource_access},
        models::AuthContext,
        team::{CreateTeamRequest, Team, UpdateTeamRequest},
    },
    domain::TeamId,
    errors::Error,
    storage::repositories::{
        SqlxTeamMembershipRepository, SqlxTeamRepository, TeamMembershipRepository, TeamRepository,
    },
};

/// Query parameters for bootstrap endpoint
#[derive(Debug, Clone, Deserialize, Serialize, IntoParams, ToSchema)]
pub struct BootstrapQuery {
    #[serde(default)]
    #[param(required = false)]
    pub format: Option<String>, // yaml|json (default yaml)
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
    tag = "teams"
)]
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
        // Admin users see all teams from the teams table
        let team_repo = SqlxTeamRepository::new(pool);
        let all_teams = team_repo
            .list_teams(1000, 0) // Large limit to get all teams
            .await
            .map_err(|err| ApiError::from(Error::from(err)))?;
        all_teams.into_iter().map(|t| t.name).collect()
    } else {
        // Non-admin users see only their teams from memberships
        let membership_repo = SqlxTeamMembershipRepository::new(pool);
        if let Some(user_id) = &context.user_id {
            let memberships = membership_repo
                .list_user_memberships(user_id)
                .await
                .map_err(|err| ApiError::from(Error::from(err)))?;
            memberships.into_iter().map(|m| m.team).collect()
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
    tag = "admin"
)]
pub async fn admin_create_team(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(mut payload): Json<CreateTeamRequest>,
) -> Result<(StatusCode, Json<Team>), ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Validate request
    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Set owner to current user if not specified
    if payload.owner_user_id.is_none() {
        payload.owner_user_id = context.user_id.clone();
    }

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
    tag = "admin"
)]
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
    tag = "admin"
)]
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
    tag = "admin"
)]
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
    tag = "admin"
)]
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
