//! Bootstrap initialization for Zitadel-backed deployments.
//!
//! Creates the default organization and team in the Flowplane database.
//! Users and authentication are managed entirely by Zitadel — bootstrap
//! only sets up the database resources that Flowplane needs to operate.

use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;

use crate::api::error::{ApiError, JsonBody};
use crate::api::routes::ApiState;
use crate::auth::organization::{CreateOrganizationRequest, OrgRole, Organization};
use crate::auth::team::{CreateTeamRequest, Team};
use crate::auth::zitadel_admin::ZitadelAdminClient;
use crate::errors::FlowplaneError;
use crate::storage::repositories::{
    OrgMembershipRepository, OrganizationRepository, SqlxOrgMembershipRepository,
    SqlxOrganizationRepository, SqlxTeamRepository, SqlxUserRepository, TeamRepository,
    UserRepository,
};
use crate::storage::DbPool;

/// Reserved organization name for platform governance. Cannot be requested by users.
const PLATFORM_ORG_NAME: &str = "platform";
const PLATFORM_ORG_DISPLAY_NAME: &str = "Platform";
const PLATFORM_ADMIN_TEAM_NAME: &str = "platform-admin";

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

/// Idempotently ensures an org + team pair exist, creating them if needed.
///
/// Returns the existing or newly created (org, team). Safe to call multiple times.
async fn ensure_org_and_team(
    org_repo: &dyn OrganizationRepository,
    team_repo: &dyn TeamRepository,
    org_name: &str,
    org_display_name: &str,
    team_name: &str,
) -> Result<(Organization, Team), ApiError> {
    // Get or create the organization
    let org = match org_repo.get_organization_by_name(org_name).await.map_err(ApiError::from)? {
        Some(existing) => {
            tracing::debug!(org_name = %org_name, "Bootstrap: org already exists, skipping creation");
            existing
        }
        None => {
            let req = CreateOrganizationRequest {
                name: org_name.to_string(),
                display_name: org_display_name.to_string(),
                description: None,
                owner_user_id: None,
                settings: None,
            };
            let created = org_repo.create_organization(req).await.map_err(|e| {
                tracing::error!(error = ?e, org_name = %org_name, "Bootstrap: failed to create organization");
                ApiError::from(e)
            })?;
            tracing::info!(org_id = %created.id, org_name = %org_name, "Bootstrap: organization created");
            created
        }
    };

    // Get or create the team under that org
    let team = match team_repo
        .get_team_by_org_and_name(&org.id, team_name)
        .await
        .map_err(ApiError::from)?
    {
        Some(existing) => {
            tracing::debug!(team_name = %team_name, org_name = %org_name, "Bootstrap: team already exists, skipping creation");
            existing
        }
        None => {
            let req = CreateTeamRequest {
                name: team_name.to_string(),
                display_name: team_name.to_string(),
                description: None,
                org_id: org.id.clone(),
                owner_user_id: None,
                settings: None,
            };
            let created = team_repo.create_team(req).await.map_err(|e| {
                tracing::error!(error = ?e, team_name = %team_name, org_name = %org_name, "Bootstrap: failed to create team");
                ApiError::from(e)
            })?;
            tracing::info!(team_id = %created.id, team_name = %team_name, org_name = %org_name, "Bootstrap: team created");
            created
        }
    };

    Ok((org, team))
}

/// Ensure platform org + team exist in the database.
///
/// Called synchronously at startup before the HTTP server begins accepting requests.
/// Creates the "platform" org and "platform-admin" team if they don't already exist.
///
/// Returns `true` if a platform owner already exists (skip superadmin seeding),
/// `false` if seeding is needed.
pub async fn ensure_platform_resources(pool: &DbPool) -> Result<bool, FlowplaneError> {
    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let team_repo = SqlxTeamRepository::new(pool.clone());

    ensure_org_and_team(
        &org_repo,
        &team_repo,
        PLATFORM_ORG_NAME,
        PLATFORM_ORG_DISPLAY_NAME,
        PLATFORM_ADMIN_TEAM_NAME,
    )
    .await
    .map_err(|e| FlowplaneError::internal(format!("Failed to ensure platform resources: {e:?}")))?;

    let has_owner = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM organization_memberships om \
         JOIN organizations o ON om.org_id = o.id \
         WHERE o.name = $1 AND om.role = $2",
    )
    .bind(PLATFORM_ORG_NAME)
    .bind("owner")
    .fetch_one(pool)
    .await
    .map_err(|e| FlowplaneError::Database {
        source: e,
        context: "Failed to check platform owner".to_string(),
    })?;

    Ok(has_owner > 0)
}

/// Seed the superadmin user from environment variables.
///
/// Runs as a background task. Polls Zitadel with exponential backoff until it is
/// ready, then finds or creates the user specified by `FLOWPLANE_SUPERADMIN_EMAIL`,
/// upserts a local user row, and creates an Owner membership in the platform org.
///
/// All operations are idempotent — safe to call even if the user already exists.
pub async fn seed_superadmin(pool: DbPool, admin_client: ZitadelAdminClient) {
    let email = match std::env::var("FLOWPLANE_SUPERADMIN_EMAIL") {
        Ok(e) if !e.is_empty() => e,
        _ => {
            tracing::info!("FLOWPLANE_SUPERADMIN_EMAIL not set, skipping superadmin seeding");
            return;
        }
    };
    let initial_password = std::env::var("FLOWPLANE_SUPERADMIN_INITIAL_PASSWORD").ok();

    // Poll Zitadel with exponential backoff (1s → 60s cap)
    let mut delay = Duration::from_secs(1);
    let max_delay = Duration::from_secs(60);
    loop {
        match admin_client.check_readiness().await {
            Ok(true) => break,
            Ok(false) | Err(_) => {
                tracing::info!(delay_secs = delay.as_secs(), "Zitadel not ready, retrying...");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_delay);
            }
        }
    }

    // Find or create the Zitadel user
    let zitadel_sub = match admin_client.search_user_by_email(&email).await {
        Ok(Some(id)) => {
            tracing::info!(%email, "Superadmin already exists in Zitadel");
            // Ensure password is set (user may have been created without one)
            if let Some(ref pw) = initial_password {
                if let Err(e) = admin_client.set_user_password(&id, pw).await {
                    tracing::warn!(%email, error = ?e, "Could not set superadmin password (may already be set)");
                }
            }
            id
        }
        Ok(None) => {
            match admin_client
                .create_human_user(&email, "Platform", "Admin", initial_password.as_deref())
                .await
            {
                Ok(id) => {
                    tracing::info!(%email, "Created superadmin in Zitadel");
                    id
                }
                Err(e) => {
                    tracing::error!(%email, error = ?e, "Failed to create superadmin in Zitadel");
                    return;
                }
            }
        }
        Err(e) => {
            tracing::error!(%email, error = ?e, "Failed to search for superadmin in Zitadel");
            return;
        }
    };

    // Upsert local user row (JIT provisioning)
    let user_repo = SqlxUserRepository::new(pool.clone());
    let user = match user_repo.upsert_from_jwt(&zitadel_sub, &email, "Platform Admin").await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = ?e, "Failed to upsert local superadmin user");
            return;
        }
    };

    // Fetch the platform org (created by ensure_platform_resources)
    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let platform_org = match org_repo.get_organization_by_name(PLATFORM_ORG_NAME).await {
        Ok(Some(org)) => org,
        Ok(None) => {
            tracing::error!("Platform org not found — ensure_platform_resources must run first");
            return;
        }
        Err(e) => {
            tracing::error!(error = ?e, "Failed to fetch platform org");
            return;
        }
    };

    // Create Owner membership (idempotent)
    let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
    match membership_repo.get_membership(&user.id, &platform_org.id).await {
        Ok(Some(_)) => {
            tracing::info!(%email, "Superadmin already has platform owner membership");
        }
        Ok(None) => {
            match membership_repo
                .create_membership(&user.id, &platform_org.id, OrgRole::Owner)
                .await
            {
                Ok(_) => tracing::info!(%email, "Superadmin platform owner membership created"),
                Err(e) => {
                    tracing::error!(error = ?e, "Failed to create superadmin org membership");
                    return;
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "Failed to check superadmin membership");
            return;
        }
    }

    tracing::info!(%email, "Superadmin seeding complete");
}

/// Bootstrap initialization — creates platform governance org/team and the default org/team.
///
/// Always creates the "platform" org with "platform-admin" team (idempotent).
/// Then creates the requested org and team for regular users.
///
/// This endpoint is idempotent for platform org/team creation. Calling it again after
/// a non-platform organization already exists returns a conflict error.
/// No authentication is required (available before the system is bootstrapped).
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
    JsonBody(payload): JsonBody<BootstrapInitializeRequest>,
) -> Result<(StatusCode, Json<BootstrapInitializeResponse>), ApiError> {
    // Require Zitadel project ID to be configured
    let project_id = std::env::var("FLOWPLANE_ZITADEL_PROJECT_ID").unwrap_or_default();
    if project_id.is_empty() {
        return Err(ApiError::ServiceUnavailable(
            "FLOWPLANE_ZITADEL_PROJECT_ID must be set before bootstrapping".to_string(),
        ));
    }

    // Guard: "platform" is a reserved org name
    if payload.org_name == PLATFORM_ORG_NAME {
        return Err(ApiError::BadRequest("'platform' is a reserved organization name".to_string()));
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
    let team_repo: Arc<dyn TeamRepository> = Arc::new(SqlxTeamRepository::new(pool.clone()));

    // Always ensure the platform governance org + team exist (idempotent)
    ensure_org_and_team(
        org_repo.as_ref(),
        team_repo.as_ref(),
        PLATFORM_ORG_NAME,
        PLATFORM_ORG_DISPLAY_NAME,
        PLATFORM_ADMIN_TEAM_NAME,
    )
    .await?;

    // Check if any non-platform org already exists (system already bootstrapped)
    let existing = org_repo.list_organizations(100, 0).await.map_err(ApiError::from)?;
    let real_orgs: Vec<_> = existing.iter().filter(|o| o.name != PLATFORM_ORG_NAME).collect();
    if !real_orgs.is_empty() {
        return Err(ApiError::Conflict(
            "System is already bootstrapped. An organization already exists.".to_string(),
        ));
    }

    // Create the requested org + team
    let (org, team) = ensure_org_and_team(
        org_repo.as_ref(),
        team_repo.as_ref(),
        &payload.org_name,
        &payload.display_name,
        &payload.team_name,
    )
    .await?;

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
            message: format!(
                "Organization '{}' and team '{}' created. Platform governance org '{}' with team '{}' initialized.",
                org.name, team.name, PLATFORM_ORG_NAME, PLATFORM_ADMIN_TEAM_NAME
            ),
            org_id: org.id.to_string(),
            team_id: team.id.to_string(),
            next_steps: vec![
                "Add users via POST /api/v1/admin/organizations/{id}/members".to_string(),
                "Permissions are managed in the Flowplane database, not Zitadel role grants"
                    .to_string(),
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
    let real_orgs: Vec<_> = orgs.iter().filter(|o| o.name != PLATFORM_ORG_NAME).collect();

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

#[cfg(test)]
mod tests {
    #[cfg(feature = "postgres_tests")]
    mod postgres {
        use super::super::*;
        use crate::storage::test_helpers::TestDatabase;

        #[tokio::test]
        async fn test_ensure_org_and_team_creates_new() {
            let db = TestDatabase::new("bootstrap-creates-new").await;
            let pool = db.pool.clone();

            let org_repo = SqlxOrganizationRepository::new(pool.clone());
            let team_repo = SqlxTeamRepository::new(pool.clone());

            let (org, team) = ensure_org_and_team(
                &org_repo,
                &team_repo,
                "bootstrap-creates-new-org",
                "Bootstrap Creates New Org",
                "new-team",
            )
            .await
            .expect("ensure_org_and_team should succeed");

            assert_eq!(org.name, "bootstrap-creates-new-org");
            assert_eq!(team.name, "new-team");
            assert_eq!(team.org_id, org.id);
        }

        #[tokio::test]
        async fn test_ensure_org_and_team_is_idempotent() {
            let db = TestDatabase::new("bootstrap-idempotent").await;
            let pool = db.pool.clone();

            let org_repo = SqlxOrganizationRepository::new(pool.clone());
            let team_repo = SqlxTeamRepository::new(pool.clone());

            // First call — creates
            let (org1, team1) = ensure_org_and_team(
                &org_repo,
                &team_repo,
                "idempotent-bootstrap-org",
                "Idempotent Bootstrap Org",
                "idempotent-team",
            )
            .await
            .expect("first call should succeed");

            // Second call — returns existing without error
            let (org2, team2) = ensure_org_and_team(
                &org_repo,
                &team_repo,
                "idempotent-bootstrap-org",
                "Idempotent Bootstrap Org",
                "idempotent-team",
            )
            .await
            .expect("second call should also succeed");

            assert_eq!(org1.id, org2.id, "org ID should be stable across calls");
            assert_eq!(team1.id, team2.id, "team ID should be stable across calls");
        }

        #[tokio::test]
        async fn test_ensure_platform_resources_creates_org_and_returns_false() {
            let db = TestDatabase::new("ensure-platform-resources-new").await;
            let pool = db.pool.clone();

            // Fresh DB — no owner should exist yet
            let has_owner = ensure_platform_resources(&pool)
                .await
                .expect("ensure_platform_resources should succeed");

            assert!(!has_owner, "fresh DB should have no platform owner");

            // Platform org and team should now exist
            let org_repo = SqlxOrganizationRepository::new(pool.clone());
            let platform_org =
                org_repo.get_organization_by_name(PLATFORM_ORG_NAME).await.expect("get org");
            assert!(platform_org.is_some(), "platform org should exist");

            let team_repo = SqlxTeamRepository::new(pool.clone());
            let platform_team = team_repo
                .get_team_by_org_and_name(&platform_org.unwrap().id, PLATFORM_ADMIN_TEAM_NAME)
                .await
                .expect("get team");
            assert!(platform_team.is_some(), "platform-admin team should exist");
        }

        #[tokio::test]
        async fn test_ensure_platform_resources_is_idempotent() {
            let db = TestDatabase::new("ensure-platform-resources-idempotent").await;
            let pool = db.pool.clone();

            let result1 =
                ensure_platform_resources(&pool).await.expect("first call should succeed");
            let result2 =
                ensure_platform_resources(&pool).await.expect("second call should succeed");

            assert_eq!(result1, result2, "idempotent: both calls should return the same result");
        }

        #[tokio::test]
        async fn test_ensure_platform_resources_returns_true_when_owner_exists() {
            use crate::auth::organization::OrgRole;
            use crate::storage::repositories::{
                OrgMembershipRepository, SqlxOrgMembershipRepository, SqlxUserRepository,
                UserRepository,
            };

            let db = TestDatabase::new("ensure-platform-resources-with-owner").await;
            let pool = db.pool.clone();

            // Setup platform resources
            ensure_platform_resources(&pool).await.expect("setup should succeed");

            // Create a user and add as owner
            let user_repo = SqlxUserRepository::new(pool.clone());
            let user = user_repo
                .upsert_from_jwt("zitadel-sub-test", "admin@platform.test", "Admin")
                .await
                .expect("upsert user");

            let org_repo = SqlxOrganizationRepository::new(pool.clone());
            let platform_org = org_repo
                .get_organization_by_name(PLATFORM_ORG_NAME)
                .await
                .expect("get org")
                .expect("platform org must exist");

            let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
            membership_repo
                .create_membership(&user.id, &platform_org.id, OrgRole::Owner)
                .await
                .expect("create owner membership");

            // Now ensure_platform_resources should detect the owner
            let has_owner =
                ensure_platform_resources(&pool).await.expect("call with owner should succeed");
            assert!(has_owner, "should return true when an owner exists");
        }

        #[tokio::test]
        async fn test_ensure_platform_org_separate_from_requested_org() {
            let db = TestDatabase::new("bootstrap-platform-separate").await;
            let pool = db.pool.clone();

            let org_repo = SqlxOrganizationRepository::new(pool.clone());
            let team_repo = SqlxTeamRepository::new(pool.clone());

            // Create platform org + team (idempotent)
            let (platform_org, platform_team) = ensure_org_and_team(
                &org_repo,
                &team_repo,
                PLATFORM_ORG_NAME,
                PLATFORM_ORG_DISPLAY_NAME,
                PLATFORM_ADMIN_TEAM_NAME,
            )
            .await
            .expect("platform org creation should succeed");

            // Create a separate requested org + team
            let (user_org, user_team) = ensure_org_and_team(
                &org_repo,
                &team_repo,
                "acme-corp-bootstrap",
                "Acme Corp",
                "engineering",
            )
            .await
            .expect("user org creation should succeed");

            assert_eq!(platform_org.name, PLATFORM_ORG_NAME);
            assert_eq!(platform_team.name, PLATFORM_ADMIN_TEAM_NAME);
            assert_eq!(user_org.name, "acme-corp-bootstrap");
            assert_eq!(user_team.name, "engineering");
            assert_ne!(platform_org.id, user_org.id, "platform and user orgs must be distinct");
        }
    }
}
