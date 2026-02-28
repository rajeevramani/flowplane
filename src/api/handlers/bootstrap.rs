//! Bootstrap initialization for Zitadel-backed deployments.
//!
//! Creates the default organization and team in the Flowplane database.
//! Users and authentication are managed entirely by Zitadel — bootstrap
//! only sets up the database resources that Flowplane needs to operate.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::organization::CreateOrganizationRequest;
use crate::auth::team::CreateTeamRequest;
use crate::storage::repositories::{
    OrganizationRepository, SqlxOrganizationRepository, SqlxTeamRepository, TeamRepository,
};

/// Request body for bootstrap initialization.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeRequest {
    /// Organization name (lowercase, alphanumeric + hyphens).
    pub org_name: String,
    /// Human-readable display name for the organization.
    pub display_name: String,
    /// Initial team name to create within the org.
    #[serde(default = "default_team_name")]
    pub team_name: String,
}

fn default_team_name() -> String {
    "default".to_string()
}

/// Response from bootstrap initialization.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInitializeResponse {
    pub message: String,
    pub org_id: String,
    pub team_id: String,
    pub next_steps: Vec<String>,
}

/// Response from bootstrap status check.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapStatusResponse {
    /// Whether the system needs initialization.
    pub needs_initialization: bool,
    /// Message describing current state.
    pub message: String,
}

/// Bootstrap initialization — creates default org and team.
///
/// This endpoint is idempotent: calling it again after the org
/// already exists returns a conflict error. No authentication is
/// required (it is only available before the system is bootstrapped).
#[utoipa::path(
    post,
    path = "/api/v1/bootstrap/initialize",
    request_body = BootstrapInitializeRequest,
    responses(
        (status = 201, description = "Bootstrap successful", body = BootstrapInitializeResponse),
        (status = 409, description = "Already bootstrapped"),
        (status = 503, description = "Zitadel not configured"),
    ),
    tag = "System"
)]
#[instrument(skip(state))]
pub async fn bootstrap_initialize_handler(
    State(state): State<ApiState>,
    Json(payload): Json<BootstrapInitializeRequest>,
) -> Result<(StatusCode, Json<BootstrapInitializeResponse>), ApiError> {
    // Require Zitadel project ID to be configured
    let project_id = std::env::var("FLOWPLANE_ZITADEL_PROJECT_ID").unwrap_or_default();
    if project_id.is_empty() {
        return Err(ApiError::ServiceUnavailable(
            "FLOWPLANE_ZITADEL_PROJECT_ID must be set before bootstrapping".to_string(),
        ));
    }

    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;
    let pool = cluster_repo.pool().clone();

    let org_repo: Arc<dyn OrganizationRepository> =
        Arc::new(SqlxOrganizationRepository::new(pool.clone()));

    // Check if any org already exists (beyond the default "platform" seed)
    let existing = org_repo.list_organizations(100, 0).await.map_err(ApiError::from)?;
    let real_orgs: Vec<_> = existing.iter().filter(|o| o.name != "platform").collect();
    if !real_orgs.is_empty() {
        return Err(ApiError::Conflict(
            "System is already bootstrapped. An organization already exists.".to_string(),
        ));
    }

    // Create organization
    let org_request = CreateOrganizationRequest {
        name: payload.org_name.clone(),
        display_name: payload.display_name.clone(),
        description: None,
        owner_user_id: None,
        settings: None,
    };

    let org = org_repo.create_organization(org_request).await.map_err(|e| {
        tracing::error!(error = ?e, "Bootstrap: failed to create organization");
        ApiError::from(e)
    })?;

    // Create default team within the org
    let team_repo: Arc<dyn TeamRepository> = Arc::new(SqlxTeamRepository::new(pool.clone()));

    let team_request = CreateTeamRequest {
        name: payload.team_name.clone(),
        display_name: payload.team_name.clone(),
        description: None,
        org_id: org.id.clone(),
        owner_user_id: None,
        settings: None,
    };

    let team = team_repo.create_team(team_request).await.map_err(|e| {
        tracing::error!(error = ?e, "Bootstrap: failed to create team");
        ApiError::from(e)
    })?;

    tracing::info!(
        org_id = %org.id,
        org_name = %org.name,
        team_id = %team.id,
        team_name = %team.name,
        "Bootstrap: organization and team created"
    );

    Ok((
        StatusCode::CREATED,
        Json(BootstrapInitializeResponse {
            message: format!("Organization '{}' and team '{}' created.", org.name, team.name),
            org_id: org.id.to_string(),
            team_id: team.id.to_string(),
            next_steps: vec![
                "Assign role grants in Zitadel for your users".to_string(),
                format!(
                    "Role format: {}:<resource>:<action> (e.g., {}:clusters:read)",
                    team.name, team.name
                ),
            ],
        }),
    ))
}

/// Bootstrap status endpoint.
///
/// Checks whether the system needs initialization by verifying that
/// the Zitadel project ID is configured and at least one organization
/// exists (beyond the default platform org).
#[utoipa::path(
    get,
    path = "/api/v1/bootstrap/status",
    responses(
        (status = 200, description = "Bootstrap status", body = BootstrapStatusResponse),
    ),
    tag = "System"
)]
#[instrument(skip(state))]
pub async fn bootstrap_status_handler(
    State(state): State<ApiState>,
) -> Result<Json<BootstrapStatusResponse>, ApiError> {
    let project_id = std::env::var("FLOWPLANE_ZITADEL_PROJECT_ID").unwrap_or_default();

    if project_id.is_empty() {
        return Ok(Json(BootstrapStatusResponse {
            needs_initialization: true,
            message: "FLOWPLANE_ZITADEL_PROJECT_ID is not set. Configure Zitadel first."
                .to_string(),
        }));
    }

    // Check if any real org exists
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::Internal("Database not available".to_string()))?;
    let pool = cluster_repo.pool().clone();

    let org_repo: Arc<dyn OrganizationRepository> = Arc::new(SqlxOrganizationRepository::new(pool));

    let orgs = org_repo.list_organizations(100, 0).await.map_err(ApiError::from)?;
    let real_orgs: Vec<_> = orgs.iter().filter(|o| o.name != "platform").collect();

    if real_orgs.is_empty() {
        return Ok(Json(BootstrapStatusResponse {
            needs_initialization: true,
            message: "Zitadel configured but no organization created yet. Call POST /api/v1/bootstrap/initialize.".to_string(),
        }));
    }

    Ok(Json(BootstrapStatusResponse {
        needs_initialization: false,
        message: format!("System bootstrapped. {} organization(s) configured.", real_orgs.len()),
    }))
}
