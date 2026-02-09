//! Team-scoped endpoints for team management

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;
use validator::Validate;

use crate::{
    api::{error::ApiError, handlers::team_access::require_admin, routes::ApiState},
    auth::{
        authorization::has_admin_bypass,
        models::AuthContext,
        team::{CreateTeamRequest, Team, UpdateTeamRequest},
    },
    domain::{OrgId, TeamId, UserId},
    errors::Error,
    storage::repositories::{
        SqlxTeamMembershipRepository, SqlxTeamRepository, TeamMembershipRepository, TeamRepository,
    },
};

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

use super::pagination::{PaginatedResponse, PaginationQuery};

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
    body.validate().map_err(ApiError::from)?;

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
    let is_available = repo.is_name_available(&payload.name).await.map_err(ApiError::from)?;

    if !is_available {
        return Err(ApiError::Conflict(format!(
            "Team with name '{}' already exists",
            payload.name
        )));
    }

    // Create team
    let team = repo.create_team(payload).await.map_err(ApiError::from)?;

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
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Team not found".to_string()))?;

    Ok(Json(team))
}

/// List all teams with pagination (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/admin/teams",
    params(PaginationQuery),
    responses(
        (status = 200, description = "Teams listed successfully", body = PaginatedResponse<Team>),
        (status = 403, description = "Admin privileges required")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Administration"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, limit = %query.limit, offset = %query.offset))]
pub async fn admin_list_teams(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<PaginatedResponse<Team>>, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    let (limit, offset) = query.clamp(100);

    // List teams
    let repo = team_repository_for_state(&state)?;
    let teams = repo.list_teams(limit, offset).await.map_err(ApiError::from)?;
    let total = repo.count_teams().await.map_err(ApiError::from)?;

    Ok(Json(PaginatedResponse::new(teams, total, limit, offset)))
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
    payload.validate().map_err(ApiError::from)?;

    // Parse team ID
    let team_id = TeamId::from_string(id);

    // Update team
    let repo = team_repository_for_state(&state)?;
    let team = repo.update_team(&team_id, payload).await.map_err(ApiError::from)?;

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
    repo.delete_team(&team_id).await.map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
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
