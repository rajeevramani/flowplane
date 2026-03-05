//! Admin organization management API handlers.
//!
//! This module provides HTTP handlers for organization lifecycle management and
//! organization membership operations. Organization creation, listing, and deletion
//! require platform admin (`admin:all` scope). Get and member listing accept platform
//! admin or org admin (platform admin needs governance visibility). Update, member
//! mutation, and team management require org admin only (no platform admin bypass)
//! to enforce the invariant that platform admin cannot modify org internals.

use std::str::FromStr;
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
        authorization::{
            check_resource_access, has_admin_bypass, has_org_admin, require_org_admin_only,
        },
        models::{AgentContext, AuthContext},
        organization::{
            CreateOrganizationRequest, OrgRole, OrganizationResponse, UpdateOrganizationRequest,
        },
        team::CreateTeamRequest,
    },
    domain::{OrgId, UserId},
    errors::Error,
    storage::{
        repositories::{
            OrgMembershipRepository, OrganizationRepository, SqlxOrgMembershipRepository,
            SqlxOrganizationRepository, SqlxTeamRepository, TeamMembershipRepository,
            TeamRepository, UserRepository,
        },
        DbPool,
    },
};

// ===== Helper Functions =====

/// Derive default team scopes from an org role.
///
/// Previously lived in `auth::invitation_service`; inlined here after
/// that module was deleted during the Zitadel migration.
fn scopes_for_org_role(role: OrgRole, team_name: &str) -> Vec<String> {
    match role {
        OrgRole::Admin | OrgRole::Owner => {
            vec![format!("team:{team_name}:*:*")]
        }
        OrgRole::Member => {
            vec![
                format!("team:{team_name}:routes:read"),
                format!("team:{team_name}:routes:write"),
                format!("team:{team_name}:clusters:read"),
                format!("team:{team_name}:clusters:write"),
                format!("team:{team_name}:listeners:read"),
                format!("team:{team_name}:listeners:write"),
                format!("team:{team_name}:filters:read"),
                format!("team:{team_name}:filters:write"),
                format!("team:{team_name}:stats:read"),
            ]
        }
        OrgRole::Viewer => {
            vec![
                format!("team:{team_name}:routes:read"),
                format!("team:{team_name}:clusters:read"),
                format!("team:{team_name}:listeners:read"),
            ]
        }
    }
}

/// Helper to create OrganizationRepository from ApiState.
fn org_repository_for_state(state: &ApiState) -> Result<Arc<dyn OrganizationRepository>, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Organization repository unavailable"))?;
    let pool = cluster_repo.pool().clone();
    Ok(Arc::new(SqlxOrganizationRepository::new(pool)))
}

/// Helper to create OrgMembershipRepository from ApiState.
fn org_membership_repository_for_state(
    state: &ApiState,
) -> Result<Arc<dyn OrgMembershipRepository>, ApiError> {
    let cluster_repo = state.xds_state.cluster_repository.as_ref().cloned().ok_or_else(|| {
        ApiError::service_unavailable("Organization membership repository unavailable")
    })?;
    let pool = cluster_repo.pool().clone();
    Ok(Arc::new(SqlxOrgMembershipRepository::new(pool)))
}

/// Helper to extract the database pool from ApiState.
fn pool_for_state(state: &ApiState) -> Result<DbPool, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Database pool unavailable"))?;
    Ok(cluster_repo.pool().clone())
}

/// Check if the current context has platform admin or org admin privileges.
fn require_admin_or_org_admin(context: &AuthContext, org_name: &str) -> Result<(), ApiError> {
    if has_admin_bypass(context) || has_org_admin(context, org_name) {
        return Ok(());
    }
    Err(ApiError::forbidden("Admin or organization admin privileges required"))
}

// ===== Request/Response Types =====

use super::pagination::{PaginatedResponse, PaginationQuery};

/// Request to add a member to an organization.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct AddOrgMemberRequest {
    pub user_id: UserId,
    pub role: OrgRole,
}

/// Request to update an organization member's role.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOrgMemberRoleRequest {
    pub role: OrgRole,
}

/// Response for listing organization members.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListOrgMembersResponse {
    pub members: Vec<crate::auth::organization::OrgMembershipResponse>,
}

/// Request to invite a member to an organization by email.
/// Creates the user in Zitadel if they don't exist, then provisions
/// local user row, org membership, and team memberships.
///
/// `initial_password` is optional — when provided, the user is created with
/// this password pre-set (useful for local dev without SMTP). In production,
/// omit this field so Zitadel sends the normal welcome/password-set email.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct InviteOrgMemberRequest {
    #[validate(email)]
    pub email: String,
    pub role: OrgRole,
    pub first_name: String,
    pub last_name: String,
    /// Optional initial password for local dev (bypasses Zitadel email flow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_password: Option<String>,
}

/// Response from inviting an org member.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InviteOrgMemberResponse {
    pub user_id: String,
    pub email: String,
    pub role: OrgRole,
    pub org_id: String,
    pub user_created: bool,
}

// ===== Organization CRUD Endpoints (Platform Admin Only) =====

/// Create a new organization (admin only).
#[utoipa::path(
    post,
    path = "/api/v1/admin/organizations",
    request_body = CreateOrganizationRequest,
    responses(
        (status = 201, description = "Organization created successfully", body = OrganizationResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Admin privileges required"),
        (status = 409, description = "Organization with name already exists")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_name = %payload.name, user_id = ?context.user_id))]
pub async fn admin_create_organization(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(mut payload): Json<CreateOrganizationRequest>,
) -> Result<(StatusCode, Json<OrganizationResponse>), ApiError> {
    require_admin(&context)?;

    payload.validate().map_err(ApiError::from)?;

    // Set owner to current user if not specified
    if payload.owner_user_id.is_none() {
        payload.owner_user_id = context.user_id.clone();
    }

    let repo = org_repository_for_state(&state)?;

    let org_name = payload.name.clone();
    let org = repo.create_organization(payload).await.map_err(|e| {
        // Catch unique constraint violation and return a user-friendly message
        if let Error::Database { ref source, .. } = e {
            if let Some(db_err) = source.as_database_error() {
                if db_err.code().is_some_and(|c| c.as_ref() == "23505") {
                    return ApiError::Conflict(format!(
                        "Organization with name '{}' already exists",
                        org_name
                    ));
                }
            }
        }
        ApiError::from(e)
    })?;

    // Auto-create default team for the new organization (follows bootstrap pattern)
    let pool = pool_for_state(&state)?;
    let team_repo = Arc::new(SqlxTeamRepository::new(pool));

    let default_team_name = format!("{}-default", org_name);
    let create_team_request = CreateTeamRequest {
        name: default_team_name.clone(),
        display_name: format!("{} Default Team", org.display_name),
        description: Some(format!(
            "Default team created automatically for organization '{}'",
            org_name
        )),
        owner_user_id: context.user_id.clone(),
        org_id: org.id.clone(),
        settings: None,
    };

    team_repo.create_team(create_team_request).await.map_err(|e| {
        tracing::error!(
            org_id = %org.id,
            team_name = %default_team_name,
            error = %e,
            "failed to create default team for new organization"
        );
        ApiError::from(e)
    })?;

    tracing::info!(
        org_id = %org.id,
        org_name = %org_name,
        default_team = %default_team_name,
        "created default team for new organization"
    );

    Ok((StatusCode::CREATED, Json(org.into())))
}

/// List all organizations with pagination (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/admin/organizations",
    params(PaginationQuery),
    responses(
        (status = 200, description = "Organizations listed successfully", body = PaginatedResponse<OrganizationResponse>),
        (status = 403, description = "Admin privileges required")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, limit = %query.limit, offset = %query.offset))]
pub async fn admin_list_organizations(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<PaginatedResponse<OrganizationResponse>>, ApiError> {
    require_admin(&context)?;

    let (limit, offset) = query.clamp(100);

    let repo = org_repository_for_state(&state)?;
    let organizations = repo.list_organizations(limit, offset).await.map_err(ApiError::from)?;
    let total = repo.count_organizations().await.map_err(ApiError::from)?;

    Ok(Json(PaginatedResponse::new(
        organizations.into_iter().map(|o| o.into()).collect(),
        total,
        limit,
        offset,
    )))
}

/// Get an organization by ID (admin or org admin).
#[utoipa::path(
    get,
    path = "/api/v1/admin/organizations/{id}",
    params(
        ("id" = String, Path, description = "Organization ID")
    ),
    responses(
        (status = 200, description = "Organization found", body = OrganizationResponse),
        (status = 403, description = "Admin or org admin privileges required"),
        (status = 404, description = "Organization not found")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_id = %id, user_id = ?context.user_id))]
pub async fn admin_get_organization(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<OrganizationResponse>, ApiError> {
    let org_id = OrgId::from_string(id);

    // Resolve org first to get name for auth check
    let repo = org_repository_for_state(&state)?;
    let org = repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_admin_or_org_admin(&context, &org.name)?;

    Ok(Json(org.into()))
}

/// Update an organization (org admin only).
#[utoipa::path(
    put,
    path = "/api/v1/admin/organizations/{id}",
    params(
        ("id" = String, Path, description = "Organization ID")
    ),
    request_body = UpdateOrganizationRequest,
    responses(
        (status = 200, description = "Organization updated successfully", body = OrganizationResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization not found")
    ),
    security(("bearer_auth" = ["org:admin"])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_id = %id, user_id = ?context.user_id))]
pub async fn admin_update_organization(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateOrganizationRequest>,
) -> Result<Json<OrganizationResponse>, ApiError> {
    payload.validate().map_err(ApiError::from)?;

    let org_id = OrgId::from_string(id);

    // Resolve org first to get name for auth check
    let repo = org_repository_for_state(&state)?;
    let existing_org = repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_org_admin_only(&context, &existing_org.name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    let org = repo.update_organization(&org_id, payload).await.map_err(ApiError::from)?;

    Ok(Json(org.into()))
}

/// Delete an organization (admin or org admin).
///
/// This operation will fail if there are teams or users referencing this
/// organization due to foreign key constraints.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/organizations/{id}",
    params(
        ("id" = String, Path, description = "Organization ID")
    ),
    responses(
        (status = 204, description = "Organization deleted successfully"),
        (status = 403, description = "Admin or org admin privileges required"),
        (status = 404, description = "Organization not found"),
        (status = 409, description = "Organization has resources - cannot delete")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_id = %id, user_id = ?context.user_id))]
pub async fn admin_delete_organization(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Platform admin only — org admins cannot delete their own org
    require_admin(&context)?;

    let org_id = OrgId::from_string(id);

    let repo = org_repository_for_state(&state)?;
    // Verify org exists (returns 404 if not)
    repo.get_organization_by_id(&org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    repo.delete_organization(&org_id).await.map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

// ===== Organization Membership Endpoints =====

/// List members of an organization (platform admin or org admin).
///
/// Platform admin needs member visibility for governance — e.g. verifying
/// that an org admin has been onboarded after invite.
#[utoipa::path(
    get,
    path = "/api/v1/admin/organizations/{id}/members",
    params(
        ("id" = String, Path, description = "Organization ID")
    ),
    responses(
        (status = 200, description = "Organization members listed successfully", body = ListOrgMembersResponse),
        (status = 403, description = "Admin or organization admin privileges required"),
        (status = 404, description = "Organization not found")
    ),
    security(("bearer_auth" = ["admin:all", "org:admin"])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_id = %id, user_id = ?context.user_id))]
pub async fn admin_list_org_members(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ListOrgMembersResponse>, ApiError> {
    let org_id = OrgId::from_string(id);

    // Resolve org to get its name for auth check
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_admin_or_org_admin(&context, &org.name)?;

    let membership_repo = org_membership_repository_for_state(&state)?;
    let members = membership_repo.list_org_members(&org_id).await.map_err(ApiError::from)?;

    Ok(Json(ListOrgMembersResponse { members: members.into_iter().map(|m| m.into()).collect() }))
}

/// Add a member to an organization (org admin only).
#[utoipa::path(
    post,
    path = "/api/v1/admin/organizations/{id}/members",
    params(
        ("id" = String, Path, description = "Organization ID")
    ),
    request_body = AddOrgMemberRequest,
    responses(
        (status = 201, description = "Member added successfully", body = crate::auth::organization::OrgMembershipResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization or user not found"),
        (status = 409, description = "User is already a member")
    ),
    security(("bearer_auth" = ["org:admin"])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_id = %id, target_user_id = %payload.user_id, user_id = ?context.user_id))]
pub async fn admin_add_org_member(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<AddOrgMemberRequest>,
) -> Result<(StatusCode, Json<crate::auth::organization::OrgMembershipResponse>), ApiError> {
    payload.validate().map_err(ApiError::from)?;

    let org_id = OrgId::from_string(id);

    // Resolve org
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_org_admin_only(&context, &org.name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    // SECURITY: Check for cross-org isolation via org_memberships.
    // A user who already belongs to a different org cannot be added.
    let pool = pool_for_state(&state)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to begin transaction: {}", e)))?;

    // Verify user exists
    let user_exists = sqlx::query_scalar::<_, String>("SELECT id FROM users WHERE id = $1")
        .bind(payload.user_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to fetch user: {}", e)))?;

    if user_exists.is_none() {
        return Err(ApiError::NotFound("User not found".to_string()));
    }

    // Check if user has existing memberships in a DIFFERENT org
    let other_org = sqlx::query_scalar::<_, String>(
        "SELECT org_id FROM organization_memberships WHERE user_id = $1 AND org_id != $2 LIMIT 1",
    )
    .bind(payload.user_id.as_str())
    .bind(org_id.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to check org memberships: {}", e)))?;

    if let Some(ref existing_org_id) = other_org {
        tracing::warn!(
            attempted_org = %org.id,
            user_org = %existing_org_id,
            user_id = %payload.user_id,
            "cross-org member add violation: user belongs to different org"
        );
        return Err(ApiError::Forbidden(format!(
            "Cross-organization access denied: user belongs to a different org, cannot be added to org '{}'",
            org.name
        )));
    }

    // Check if already a member (within same transaction)
    let existing_membership = sqlx::query_scalar::<_, String>(
        "SELECT id FROM organization_memberships WHERE user_id = $1 AND org_id = $2",
    )
    .bind(payload.user_id.as_str())
    .bind(org_id.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to check existing membership: {}", e)))?;

    if existing_membership.is_some() {
        return Err(ApiError::Conflict(
            "User is already a member of this organization".to_string(),
        ));
    }

    // Create membership within the same transaction
    let membership_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let role_str = payload.role.as_str();

    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            chrono::DateTime<chrono::Utc>,
            String,
            Option<String>,
            Option<String>,
        ),
    >(
        "WITH inserted AS (
            INSERT INTO organization_memberships (id, user_id, org_id, role, created_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
        )
        SELECT i.id, i.user_id, i.org_id, i.role, i.created_at, o.name AS org_name,
               u.name AS user_name, u.email AS user_email
        FROM inserted i
        JOIN organizations o ON o.id = i.org_id
        LEFT JOIN users u ON u.id = i.user_id",
    )
    .bind(&membership_id)
    .bind(payload.user_id.as_str())
    .bind(org_id.as_str())
    .bind(role_str)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to create membership: {}", e)))?;

    tx.commit()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to commit transaction: {}", e)))?;

    // Evict permission cache so next request picks up new permissions
    if let Some(ref cache) = state.permission_cache {
        cache.evict_by_user_id(&payload.user_id).await;
    }

    let role = crate::auth::organization::OrgRole::from_str(&row.3)
        .map_err(|e| ApiError::Internal(format!("Invalid role in DB: {}", e)))?;

    let membership = crate::auth::organization::OrganizationMembership {
        id: row.0,
        user_id: UserId::from_string(row.1),
        org_id: OrgId::from_string(row.2),
        role,
        org_name: row.5,
        created_at: row.4,
        user_name: row.6,
        user_email: row.7,
    };

    Ok((StatusCode::CREATED, Json(membership.into())))
}

/// Invite a member to an organization by email (admin or org admin).
///
/// Creates the user in Zitadel if they don't exist, provisions a local user row,
/// creates org membership and team memberships. Idempotent: re-inviting returns 200.
#[utoipa::path(
    post,
    path = "/api/v1/admin/organizations/{id}/invite",
    params(
        ("id" = String, Path, description = "Organization ID")
    ),
    request_body = InviteOrgMemberRequest,
    responses(
        (status = 201, description = "User invited successfully", body = InviteOrgMemberResponse),
        (status = 200, description = "User already a member (idempotent)", body = InviteOrgMemberResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Admin or org admin privileges required"),
        (status = 404, description = "Organization not found"),
        (status = 503, description = "Zitadel admin client not configured")
    ),
    security(("bearer_auth" = ["admin:all", "org:admin"])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_id = %id, email = %payload.email, user_id = ?context.user_id))]
pub async fn admin_invite_org_member(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<InviteOrgMemberRequest>,
) -> Result<(StatusCode, Json<InviteOrgMemberResponse>), ApiError> {
    payload.validate().map_err(ApiError::from)?;

    let org_id = OrgId::from_string(id);

    // Resolve org
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    // Platform admin CAN invite (e.g. first org admin), org admin can invite their own org
    require_admin_or_org_admin(&context, &org.name)?;

    // Zitadel admin client required for user lookup/creation
    let zitadel_client = state
        .zitadel_admin
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Zitadel admin client not configured"))?;

    // Step 1: Search Zitadel for existing user by email
    let (zitadel_sub, user_created) =
        match zitadel_client.search_user_by_email(&payload.email).await? {
            Some(sub) => {
                // If initial_password provided for existing user, set it via v2 API
                if let Some(ref pw) = payload.initial_password {
                    if let Err(e) = zitadel_client.set_user_password(&sub, pw).await {
                        tracing::warn!(
                            email = %payload.email,
                            error = ?e,
                            "Could not set password for existing user (may already be set)"
                        );
                    }
                }
                (sub, false)
            }
            None => {
                let sub = zitadel_client
                    .create_human_user(
                        &payload.email,
                        &payload.first_name,
                        &payload.last_name,
                        payload.initial_password.as_deref(),
                    )
                    .await?;
                (sub, true)
            }
        };

    // Step 2: JIT provision local user row
    let pool = pool_for_state(&state)?;
    let user_repo = crate::storage::repositories::SqlxUserRepository::new(pool.clone());
    let display_name = format!("{} {}", payload.first_name, payload.last_name);
    let local_user =
        user_repo
            .upsert_from_jwt(&zitadel_sub, &payload.email, &display_name)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to provision local user: {e}")))?;

    // Step 3: Transaction for org membership + team memberships
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to begin transaction: {e}")))?;

    // Cross-org isolation: user in a different org -> 403
    let other_org = sqlx::query_scalar::<_, String>(
        "SELECT org_id FROM organization_memberships WHERE user_id = $1 AND org_id != $2 LIMIT 1",
    )
    .bind(local_user.id.as_str())
    .bind(org_id.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to check org memberships: {e}")))?;

    if let Some(ref existing_org_id) = other_org {
        tracing::warn!(
            attempted_org = %org.id,
            user_org = %existing_org_id,
            user_id = %local_user.id,
            "cross-org invite violation: user belongs to different org"
        );
        return Err(ApiError::Forbidden(format!(
            "Cross-organization access denied: user belongs to a different org, \
             cannot be invited to org '{}'",
            org.name
        )));
    }

    // Check existing membership in this org
    let existing_role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM organization_memberships WHERE user_id = $1 AND org_id = $2",
    )
    .bind(local_user.id.as_str())
    .bind(org_id.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to check existing membership: {e}")))?;

    let status_code = match existing_role {
        Some(ref role_str) => {
            let existing = OrgRole::from_str(role_str)
                .map_err(|e| ApiError::Internal(format!("Invalid role in DB: {e}")))?;
            if existing == payload.role {
                // Idempotent: same role -> 200, no changes needed
                tx.commit()
                    .await
                    .map_err(|e| ApiError::Internal(format!("Failed to commit: {e}")))?;
                return Ok((
                    StatusCode::OK,
                    Json(InviteOrgMemberResponse {
                        user_id: local_user.id.to_string(),
                        email: payload.email,
                        role: payload.role,
                        org_id: org_id.to_string(),
                        user_created,
                    }),
                ));
            }
            // Different role -> update membership and re-create team memberships
            sqlx::query(
                "UPDATE organization_memberships SET role = $1 \
                 WHERE user_id = $2 AND org_id = $3",
            )
            .bind(payload.role.as_str())
            .bind(local_user.id.as_str())
            .bind(org_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to update membership role: {e}")))?;

            // Delete existing team memberships so we re-create with new scopes
            sqlx::query("DELETE FROM user_team_memberships WHERE user_id = $1")
                .bind(local_user.id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    ApiError::Internal(format!("Failed to clear team memberships: {e}"))
                })?;

            StatusCode::OK
        }
        None => {
            // New membership -> insert
            let membership_id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now();
            sqlx::query(
                "INSERT INTO organization_memberships (id, user_id, org_id, role, created_at) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(&membership_id)
            .bind(local_user.id.as_str())
            .bind(org_id.as_str())
            .bind(payload.role.as_str())
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to create membership: {e}")))?;

            StatusCode::CREATED
        }
    };

    // Create team memberships for all teams in the org
    let team_repo = SqlxTeamRepository::new(pool.clone());
    let teams = team_repo
        .list_teams_by_org(&org_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list org teams: {e}")))?;

    for team in &teams {
        let scopes = scopes_for_org_role(payload.role, &team.name);
        let scopes_json = serde_json::to_string(&scopes)
            .map_err(|e| ApiError::Internal(format!("Failed to serialize scopes: {e}")))?;
        let utm_id = format!("utm_{}", uuid::Uuid::new_v4());

        sqlx::query(
            "INSERT INTO user_team_memberships (id, user_id, team, scopes, created_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (user_id, team) DO NOTHING",
        )
        .bind(&utm_id)
        .bind(local_user.id.as_str())
        .bind(team.id.as_str())
        .bind(&scopes_json)
        .bind(chrono::Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create team membership: {e}")))?;
    }

    tx.commit()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to commit transaction: {e}")))?;

    // Evict permission cache
    if let Some(ref cache) = state.permission_cache {
        cache.evict_by_user_id(&local_user.id).await;
    }

    tracing::info!(
        user_id = %local_user.id,
        email = %payload.email,
        role = %payload.role,
        org_id = %org_id,
        user_created = user_created,
        teams_count = teams.len(),
        "invited member to organization"
    );

    Ok((
        status_code,
        Json(InviteOrgMemberResponse {
            user_id: local_user.id.to_string(),
            email: payload.email,
            role: payload.role,
            org_id: org_id.to_string(),
            user_created,
        }),
    ))
}

/// Update a member's role in an organization (org admin only).
///
/// Prevents downgrading the last owner of an organization.
#[utoipa::path(
    put,
    path = "/api/v1/admin/organizations/{id}/members/{user_id}",
    params(
        ("id" = String, Path, description = "Organization ID"),
        ("user_id" = String, Path, description = "User ID")
    ),
    request_body = UpdateOrgMemberRoleRequest,
    responses(
        (status = 200, description = "Member role updated successfully", body = crate::auth::organization::OrgMembershipResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization or membership not found"),
        (status = 409, description = "Cannot downgrade the last owner")
    ),
    security(("bearer_auth" = ["org:admin"])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_id = %id, target_user_id = %user_id, user_id = ?context.user_id))]
pub async fn admin_update_org_member_role(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((id, user_id)): Path<(String, String)>,
    Json(payload): Json<UpdateOrgMemberRoleRequest>,
) -> Result<Json<crate::auth::organization::OrgMembershipResponse>, ApiError> {
    payload.validate().map_err(ApiError::from)?;

    let org_id = OrgId::from_string(id);
    let target_user_id = UserId::from_string(user_id);

    // Resolve org
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_org_admin_only(&context, &org.name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    // Update role atomically (repository enforces last-owner constraint via transaction)
    let membership_repo = org_membership_repository_for_state(&state)?;
    let updated = membership_repo
        .update_membership_role(&target_user_id, &org_id, payload.role)
        .await
        .map_err(ApiError::from)?;

    // Evict permission cache so next request picks up new permissions
    if let Some(ref cache) = state.permission_cache {
        cache.evict_by_user_id(&target_user_id).await;
    }

    Ok(Json(updated.into()))
}

/// Remove a member from an organization (org admin only).
///
/// Prevents removing the last owner of an organization.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/organizations/{id}/members/{user_id}",
    params(
        ("id" = String, Path, description = "Organization ID"),
        ("user_id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 204, description = "Member removed successfully"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization or membership not found"),
        (status = 409, description = "Cannot remove the last owner")
    ),
    security(("bearer_auth" = ["org:admin"])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_id = %id, target_user_id = %user_id, user_id = ?context.user_id))]
pub async fn admin_remove_org_member(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((id, user_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let org_id = OrgId::from_string(id);
    let target_user_id = UserId::from_string(user_id);

    // Resolve org
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_org_admin_only(&context, &org.name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    // Delete atomically (repository enforces last-owner constraint via transaction)
    let membership_repo = org_membership_repository_for_state(&state)?;
    membership_repo.delete_membership(&target_user_id, &org_id).await.map_err(ApiError::from)?;

    // Evict permission cache so next request picks up new permissions
    if let Some(ref cache) = state.permission_cache {
        cache.evict_by_user_id(&target_user_id).await;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ===== Org-Scoped Endpoints (Authenticated Users) =====

/// Response for GET /api/v1/orgs/current - returns org + user's role
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CurrentOrgResponse {
    pub organization: OrganizationResponse,
    pub role: OrgRole,
}

/// Get the current authenticated user's organization.
#[utoipa::path(
    get,
    path = "/api/v1/orgs/current",
    responses(
        (status = 200, description = "Current organization retrieved successfully", body = CurrentOrgResponse),
        (status = 401, description = "Authentication required"),
        (status = 404, description = "User has no organization")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id))]
pub async fn get_current_org(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
) -> Result<Json<CurrentOrgResponse>, ApiError> {
    // Extract user_id from context
    let user_id = context
        .user_id
        .as_ref()
        .ok_or_else(|| ApiError::Unauthorized("User ID required".to_string()))?;

    // Look up org membership from org_memberships table (v3: source of truth)
    let membership_repo = org_membership_repository_for_state(&state)?;
    let memberships =
        membership_repo.list_user_memberships(user_id).await.map_err(ApiError::from)?;

    let membership = memberships
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::NotFound("User has no organization membership".to_string()))?;

    // Fetch organization
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_id(&membership.org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    Ok(Json(CurrentOrgResponse { organization: org.into(), role: membership.role }))
}

/// Response for listing teams within an organization.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListOrgTeamsResponse {
    pub teams: Vec<crate::auth::team::Team>,
}

/// List teams belonging to a specific organization.
#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_name}/teams",
    params(
        ("org_name" = String, Path, description = "Organization name")
    ),
    responses(
        (status = 200, description = "Teams listed successfully", body = ListOrgTeamsResponse),
        (status = 403, description = "Organization membership required"),
        (status = 404, description = "Organization not found")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_name = %org_name, user_id = ?context.user_id))]
pub async fn list_org_teams(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(org_name): Path<String>,
) -> Result<Json<ListOrgTeamsResponse>, ApiError> {
    // Verify caller has org membership (admin or member)
    if !crate::auth::authorization::has_org_membership(&context, &org_name) {
        return Err(ApiError::forbidden("Organization membership required to view teams"));
    }

    // Resolve org_name to organization
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;

    // List teams by org_id
    let team_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Team repository unavailable"))?;
    let pool = team_repo.pool().clone();
    let team_repo = Arc::new(crate::storage::repositories::SqlxTeamRepository::new(pool));

    let teams = team_repo.list_teams_by_org(&org.id).await.map_err(ApiError::from)?;

    Ok(Json(ListOrgTeamsResponse { teams }))
}

/// Create a team within an organization.
#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_name}/teams",
    params(
        ("org_name" = String, Path, description = "Organization name")
    ),
    request_body = crate::auth::team::CreateTeamRequest,
    responses(
        (status = 201, description = "Team created successfully", body = crate::auth::team::Team),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization not found"),
        (status = 409, description = "Team with name already exists")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_name = %org_name, team_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_org_team(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(org_name): Path<String>,
    Json(mut payload): Json<crate::auth::team::CreateTeamRequest>,
) -> Result<(StatusCode, Json<crate::auth::team::Team>), ApiError> {
    // Verify caller is org admin (no platform admin bypass)
    crate::auth::authorization::require_org_admin_only(&context, &org_name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    // Validate request
    payload.validate().map_err(ApiError::from)?;

    // Resolve org_name to organization
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;

    // Set org_id on the request
    payload.org_id = org.id.clone();

    // Create team via TeamRepository
    let team_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Team repository unavailable"))?;
    let pool = team_repo.pool().clone();
    let team_repo = Arc::new(crate::storage::repositories::SqlxTeamRepository::new(pool.clone()));

    let team = team_repo.create_team(payload).await.map_err(|e| {
        // Catch unique constraint violation
        if let crate::errors::FlowplaneError::Database { ref source, .. } = e {
            if let Some(db_err) = source.as_database_error() {
                if db_err.code().is_some_and(|c| c.as_ref() == "23505") {
                    return ApiError::Conflict("Team with this name already exists".to_string());
                }
            }
        }
        ApiError::from(e)
    })?;

    // Auto-create team memberships for all existing org members
    let org_membership_repo = org_membership_repository_for_state(&state)?;
    let team_membership_repo: Arc<dyn TeamMembershipRepository> =
        Arc::new(crate::storage::repositories::SqlxTeamMembershipRepository::new(pool));
    let org_members =
        org_membership_repo.list_org_members(&org.id).await.map_err(ApiError::from)?;
    for member in &org_members {
        let scopes = scopes_for_org_role(member.role, &team.name);
        let membership = crate::auth::user::NewUserTeamMembership {
            id: format!("utm_{}", uuid::Uuid::new_v4()),
            user_id: member.user_id.clone(),
            team: team.id.to_string(),
            scopes,
        };
        team_membership_repo.create_membership(membership).await.map_err(ApiError::from)?;
    }

    // Evict permission cache for all org members — new team means new team memberships
    if let Some(ref cache) = state.permission_cache {
        for member in &org_members {
            cache.evict_by_user_id(&member.user_id).await;
        }
    }

    Ok((StatusCode::CREATED, Json(team)))
}

// ===== Org-Scoped Team Update/Delete (Org Admin) =====

/// Path params for org-scoped team endpoints.
#[derive(Debug, Deserialize)]
pub struct OrgTeamPath {
    pub org_name: String,
    pub team_name: String,
}

/// Update a team within an organization (org admin only).
#[utoipa::path(
    put,
    path = "/api/v1/orgs/{org_name}/teams/{team_name}",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("team_name" = String, Path, description = "Team name"),
    ),
    request_body = crate::auth::team::UpdateTeamRequest,
    responses(
        (status = 200, description = "Team updated successfully", body = crate::auth::team::Team),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization or team not found")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_name = %path.org_name, team_name = %path.team_name, user_id = ?context.user_id))]
pub async fn update_org_team(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<OrgTeamPath>,
    Json(payload): Json<crate::auth::team::UpdateTeamRequest>,
) -> Result<Json<crate::auth::team::Team>, ApiError> {
    // Verify caller is org admin (no platform admin bypass)
    crate::auth::authorization::require_org_admin_only(&context, &path.org_name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    payload.validate().map_err(ApiError::from)?;

    // Resolve org
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&path.org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", path.org_name)))?;

    // Resolve team within org (prevents cross-org access)
    let pool = pool_for_state(&state)?;
    let team_repo = Arc::new(SqlxTeamRepository::new(pool));
    let team = team_repo
        .get_team_by_org_and_name(&org.id, &path.team_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Team '{}' not found", path.team_name)))?;

    // Update the team
    let updated = team_repo.update_team(&team.id, payload).await.map_err(ApiError::from)?;

    Ok(Json(updated))
}

/// Delete a team within an organization (org admin only).
///
/// This will fail if the team has resources (listeners, routes, clusters, etc.)
/// due to foreign key constraints.
#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{org_name}/teams/{team_name}",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("team_name" = String, Path, description = "Team name"),
    ),
    responses(
        (status = 204, description = "Team deleted successfully"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization or team not found"),
        (status = 409, description = "Team has resources - cannot delete")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_name = %path.org_name, team_name = %path.team_name, user_id = ?context.user_id))]
pub async fn delete_org_team(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<OrgTeamPath>,
) -> Result<StatusCode, ApiError> {
    // Verify caller is org admin (no platform admin bypass)
    crate::auth::authorization::require_org_admin_only(&context, &path.org_name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    // Resolve org
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&path.org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", path.org_name)))?;

    // Resolve team within org
    let pool = pool_for_state(&state)?;
    let team_repo = Arc::new(SqlxTeamRepository::new(pool));
    let team = team_repo
        .get_team_by_org_and_name(&org.id, &path.team_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Team '{}' not found", path.team_name)))?;

    team_repo.delete_team(&team.id).await.map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

// ===== Org-Scoped Team Membership Endpoints (Org Admin) =====

/// Path params for team member endpoints.
#[derive(Debug, Deserialize)]
pub struct OrgTeamMemberPath {
    pub org_name: String,
    pub team_name: String,
    pub user_id: String,
}

/// Request to add a member to a team.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct AddTeamMemberRequest {
    pub user_id: UserId,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Request to update a team member's scopes.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamMemberScopesRequest {
    pub scopes: Vec<String>,
}

/// Team member response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemberResponse {
    pub id: String,
    pub user_id: String,
    pub team: String,
    pub scopes: Vec<String>,
    pub created_at: String,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
}

impl From<crate::auth::user::UserTeamMembership> for TeamMemberResponse {
    fn from(m: crate::auth::user::UserTeamMembership) -> Self {
        // Strip "team:{name}:" prefix from scopes for the API response.
        // DB stores fully-qualified scopes like "team:engineering:clusters:read",
        // but the API should return base scopes like "clusters:read".
        let base_scopes: Vec<String> = m
            .scopes
            .into_iter()
            .map(|s| {
                if let Some(rest) = s.strip_prefix("team:") {
                    // Skip the team name segment: "team:{name}:{resource}:{action}" → "{resource}:{action}"
                    if let Some(pos) = rest.find(':') {
                        rest[pos + 1..].to_string()
                    } else {
                        s
                    }
                } else {
                    s
                }
            })
            .collect();
        Self {
            id: m.id,
            user_id: m.user_id.to_string(),
            team: m.team,
            scopes: base_scopes,
            created_at: m.created_at.to_rfc3339(),
            user_name: m.user_name,
            user_email: m.user_email,
        }
    }
}

/// Response for listing team members.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListTeamMembersResponse {
    pub members: Vec<TeamMemberResponse>,
}

/// Helper: resolve org + team from path, verify org admin (no platform admin bypass).
async fn resolve_org_team(
    state: &ApiState,
    context: &AuthContext,
    org_name: &str,
    team_name: &str,
) -> Result<(crate::auth::organization::OrganizationResponse, crate::auth::team::Team), ApiError> {
    crate::auth::authorization::require_org_admin_only(context, org_name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    let org_repo = org_repository_for_state(state)?;
    let org = org_repo
        .get_organization_by_name(org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;

    let pool = pool_for_state(state)?;
    let team_repo = Arc::new(SqlxTeamRepository::new(pool));
    let team = team_repo
        .get_team_by_org_and_name(&org.id, team_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Team '{}' not found", team_name)))?;

    Ok((org.into(), team))
}

/// List members of a team (org admin only).
#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_name}/teams/{team_name}/members",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("team_name" = String, Path, description = "Team name"),
    ),
    responses(
        (status = 200, description = "Team members listed", body = ListTeamMembersResponse),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization or team not found")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_name = %path.org_name, team_name = %path.team_name, user_id = ?context.user_id))]
pub async fn list_team_members(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<OrgTeamPath>,
) -> Result<Json<ListTeamMembersResponse>, ApiError> {
    let (_org, team) = resolve_org_team(&state, &context, &path.org_name, &path.team_name).await?;

    let pool = pool_for_state(&state)?;
    let membership_repo: Arc<dyn TeamMembershipRepository> =
        Arc::new(crate::storage::repositories::SqlxTeamMembershipRepository::new(pool));

    let members =
        membership_repo.list_team_members(team.id.as_ref()).await.map_err(ApiError::from)?;

    Ok(Json(ListTeamMembersResponse {
        members: members.into_iter().map(TeamMemberResponse::from).collect(),
    }))
}

/// Add a member to a team (org admin only).
///
/// The user must already be a member of the organization. If no scopes are
/// specified, default scopes are assigned based on the user's org role.
#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_name}/teams/{team_name}/members",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("team_name" = String, Path, description = "Team name"),
    ),
    request_body = AddTeamMemberRequest,
    responses(
        (status = 201, description = "Member added to team", body = TeamMemberResponse),
        (status = 400, description = "User is not a member of this organization"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization or team not found"),
        (status = 409, description = "User is already a member of this team")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_name = %path.org_name, team_name = %path.team_name, user_id = ?context.user_id))]
pub async fn add_team_member(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<OrgTeamPath>,
    Json(payload): Json<AddTeamMemberRequest>,
) -> Result<(StatusCode, Json<TeamMemberResponse>), ApiError> {
    let (org, team) = resolve_org_team(&state, &context, &path.org_name, &path.team_name).await?;

    // Verify user belongs to the org
    let org_membership_repo = org_membership_repository_for_state(&state)?;
    let org_membership = org_membership_repo
        .get_membership(&payload.user_id, &org.id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| {
            ApiError::BadRequest("User is not a member of this organization".to_string())
        })?;

    let pool = pool_for_state(&state)?;
    let membership_repo: Arc<dyn TeamMembershipRepository> =
        Arc::new(crate::storage::repositories::SqlxTeamMembershipRepository::new(pool));

    // Check if user is already a member of this team
    let existing = membership_repo
        .get_user_team_membership(&payload.user_id, team.id.as_ref())
        .await
        .map_err(ApiError::from)?;
    if existing.is_some() {
        return Err(ApiError::Conflict("User is already a member of this team".to_string()));
    }

    // Use provided scopes or derive defaults from org role
    let scopes = if payload.scopes.is_empty() {
        scopes_for_org_role(org_membership.role, &team.name)
    } else {
        // Qualify base scopes with team name (e.g. "clusters:read" → "team:engineering:clusters:read")
        payload
            .scopes
            .into_iter()
            .map(|s| if s.starts_with("team:") { s } else { format!("team:{}:{}", team.name, s) })
            .collect()
    };

    let membership = crate::auth::user::NewUserTeamMembership {
        id: format!("utm_{}", uuid::Uuid::new_v4()),
        user_id: payload.user_id,
        team: team.id.to_string(),
        scopes,
    };

    let created = membership_repo.create_membership(membership).await.map_err(ApiError::from)?;

    // Evict permission cache so next request picks up new permissions
    if let Some(ref cache) = state.permission_cache {
        cache.evict_by_user_id(&created.user_id).await;
    }

    Ok((StatusCode::CREATED, Json(TeamMemberResponse::from(created))))
}

/// Update a team member's scopes (org admin only).
#[utoipa::path(
    put,
    path = "/api/v1/orgs/{org_name}/teams/{team_name}/members/{user_id}",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("team_name" = String, Path, description = "Team name"),
        ("user_id" = String, Path, description = "User ID"),
    ),
    request_body = UpdateTeamMemberScopesRequest,
    responses(
        (status = 200, description = "Member scopes updated", body = TeamMemberResponse),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Membership not found")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_name = %path.org_name, team_name = %path.team_name, target_user_id = %path.user_id, user_id = ?context.user_id))]
pub async fn update_team_member_scopes(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<OrgTeamMemberPath>,
    Json(payload): Json<UpdateTeamMemberScopesRequest>,
) -> Result<Json<TeamMemberResponse>, ApiError> {
    let (_org, team) = resolve_org_team(&state, &context, &path.org_name, &path.team_name).await?;

    let target_user_id = UserId::from_str_unchecked(&path.user_id);

    let pool = pool_for_state(&state)?;
    let membership_repo: Arc<dyn TeamMembershipRepository> =
        Arc::new(crate::storage::repositories::SqlxTeamMembershipRepository::new(pool));

    // Find the membership
    let membership = membership_repo
        .get_user_team_membership(&target_user_id, team.id.as_ref())
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Team membership not found".to_string()))?;

    // Qualify base scopes with team name (e.g. "clusters:read" → "team:engineering:clusters:read")
    let qualified_scopes: Vec<String> = payload
        .scopes
        .into_iter()
        .map(|s| if s.starts_with("team:") { s } else { format!("team:{}:{}", team.name, s) })
        .collect();

    let updated = membership_repo
        .update_membership_scopes(&membership.id, qualified_scopes)
        .await
        .map_err(ApiError::from)?;

    // Evict permission cache so next request picks up new permissions
    if let Some(ref cache) = state.permission_cache {
        cache.evict_by_user_id(&target_user_id).await;
    }

    Ok(Json(TeamMemberResponse::from(updated)))
}

/// Remove a member from a team (org admin only).
#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{org_name}/teams/{team_name}/members/{user_id}",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("team_name" = String, Path, description = "Team name"),
        ("user_id" = String, Path, description = "User ID"),
    ),
    responses(
        (status = 204, description = "Member removed from team"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Membership not found")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_name = %path.org_name, team_name = %path.team_name, target_user_id = %path.user_id, user_id = ?context.user_id))]
pub async fn remove_team_member(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<OrgTeamMemberPath>,
) -> Result<StatusCode, ApiError> {
    let (_org, team) = resolve_org_team(&state, &context, &path.org_name, &path.team_name).await?;

    let target_user_id = UserId::from_str_unchecked(&path.user_id);

    let pool = pool_for_state(&state)?;
    let membership_repo: Arc<dyn TeamMembershipRepository> =
        Arc::new(crate::storage::repositories::SqlxTeamMembershipRepository::new(pool));

    // Verify membership exists before deleting
    membership_repo
        .get_user_team_membership(&target_user_id, team.id.as_ref())
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Team membership not found".to_string()))?;

    membership_repo
        .delete_user_team_membership(&target_user_id, team.id.as_ref())
        .await
        .map_err(ApiError::from)?;

    // Evict permission cache so next request picks up new permissions
    if let Some(ref cache) = state.permission_cache {
        cache.evict_by_user_id(&target_user_id).await;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ===== Agent Provisioning =====

/// Request to create an agent (machine user) in an organization.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub teams: Vec<String>,
}

/// Response from creating or re-provisioning an agent.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentResponse {
    pub agent_id: String,
    pub name: String,
    pub username: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    pub org_id: String,
    pub teams: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn validate_agent_name(name: &str) -> Result<(), ApiError> {
    if name.len() < 3 || name.len() > 63 {
        return Err(ApiError::BadRequest(
            "Agent name must be between 3 and 63 characters".to_string(),
        ));
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(ApiError::BadRequest(
            "Agent name must contain only lowercase letters, digits, and hyphens".to_string(),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(ApiError::BadRequest(
            "Agent name must start and end with a letter or digit".to_string(),
        ));
    }
    Ok(())
}

fn get_token_endpoint() -> String {
    std::env::var("FLOWPLANE_ZITADEL_ISSUER")
        .map(|issuer| format!("{}/oauth/v2/token", issuer.trim_end_matches('/')))
        .unwrap_or_else(|_| "/oauth/v2/token".to_string())
}

/// Core machine user provisioning.
///
/// Creates a Zitadel machine user, generates client credentials, and creates
/// local DB rows (user, org_membership, team_memberships). Shared by both the
/// agent provisioning endpoint (C.2) and the DCR proxy rework (C.4).
///
/// `teams` is a list of `(team_id, fully-qualified scopes)` pairs.
/// Returns `(local_user_id, client_id, client_secret)`.
#[allow(clippy::too_many_arguments)]
pub async fn provision_machine_user(
    admin_client: &crate::auth::zitadel_admin::ZitadelAdminClient,
    pool: &DbPool,
    cache: Option<&crate::auth::cache::PermissionCache>,
    org_id: &str,
    username: &str,
    display_name: &str,
    teams: &[(String, Vec<String>)],
    agent_context: AgentContext,
) -> Result<(String, String, String), ApiError> {
    let zitadel_sub = admin_client.create_machine_user(username, display_name).await?;
    let (client_id, client_secret) = admin_client.create_client_secret(&zitadel_sub).await?;
    let local_user_id =
        provision_machine_user_db(pool, &zitadel_sub, display_name, org_id, teams, agent_context)
            .await?;
    if let Some(cache) = cache {
        cache.evict(&zitadel_sub).await;
    }
    Ok((local_user_id, client_id, client_secret))
}

/// Create local DB rows for a machine user (users, org_membership, team_memberships).
/// Uses ON CONFLICT DO NOTHING to be idempotent.
async fn provision_machine_user_db(
    pool: &DbPool,
    zitadel_sub: &str,
    display_name: &str,
    org_id: &str,
    teams: &[(String, Vec<String>)],
    agent_context: AgentContext,
) -> Result<String, ApiError> {
    let user_id = UserId::new().to_string();
    let now = chrono::Utc::now();
    let placeholder_email = format!("{}@machine.local", zitadel_sub);

    sqlx::query(
        "INSERT INTO users \
         (id, email, password_hash, name, status, is_admin, zitadel_sub, user_type, agent_context, \
          created_at, updated_at) \
         VALUES ($1, $2, '', $3, 'active', false, $4, 'machine', $5, $6, $7) \
         ON CONFLICT (zitadel_sub) DO NOTHING",
    )
    .bind(&user_id)
    .bind(&placeholder_email)
    .bind(display_name)
    .bind(zitadel_sub)
    .bind(agent_context.as_str())
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to create local user row: {e}")))?;

    // Fetch actual user_id — handles the case where ON CONFLICT triggered
    let actual_user_id =
        sqlx::query_scalar::<_, String>("SELECT id FROM users WHERE zitadel_sub = $1")
            .bind(zitadel_sub)
            .fetch_one(pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to fetch user after insert: {e}")))?;

    let membership_id = format!("om_{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO organization_memberships (id, user_id, org_id, role, created_at) \
         VALUES ($1, $2, $3, 'member', $4) \
         ON CONFLICT (user_id, org_id) DO NOTHING",
    )
    .bind(&membership_id)
    .bind(&actual_user_id)
    .bind(org_id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to create org membership: {e}")))?;

    for (team_id, scopes) in teams {
        let utm_id = format!("utm_{}", uuid::Uuid::new_v4());
        let scopes_json = serde_json::to_string(scopes)
            .map_err(|e| ApiError::Internal(format!("Failed to serialize scopes: {e}")))?;
        sqlx::query(
            "INSERT INTO user_team_memberships (id, user_id, team, scopes, created_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (user_id, team) DO NOTHING",
        )
        .bind(&utm_id)
        .bind(&actual_user_id)
        .bind(team_id)
        .bind(&scopes_json)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create team membership: {e}")))?;
    }

    Ok(actual_user_id)
}

/// Ensure local DB rows exist for a machine user that already exists in Zitadel.
///
/// Called on the idempotent path when `search_user_by_username` finds an existing
/// user. Reconciles any missing DB rows (handles DB-wipe + re-provision scenarios).
async fn reconcile_machine_user_db(
    pool: &DbPool,
    zitadel_sub: &str,
    username: &str,
    org_id: &str,
    teams: &[(String, Vec<String>)],
    agent_context: AgentContext,
) -> Result<String, ApiError> {
    let now = chrono::Utc::now();

    let existing_id =
        sqlx::query_scalar::<_, String>("SELECT id FROM users WHERE zitadel_sub = $1")
            .bind(zitadel_sub)
            .fetch_optional(pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to check existing user: {e}")))?;

    let user_id = match existing_id {
        Some(id) => {
            // Existing user — update agent_context if NULL (migrating legacy agents)
            sqlx::query(
                "UPDATE users SET agent_context = $1 WHERE zitadel_sub = $2 AND agent_context IS NULL",
            )
            .bind(agent_context.as_str())
            .bind(zitadel_sub)
            .execute(pool)
            .await
            .map_err(|e| {
                ApiError::Internal(format!("Failed to update legacy agent context: {e}"))
            })?;
            id
        }
        None => {
            // DB was wiped — reconcile by re-creating the user row
            let new_id = UserId::new().to_string();
            let placeholder_email = format!("{}@machine.local", zitadel_sub);
            sqlx::query(
                "INSERT INTO users \
                 (id, email, password_hash, name, status, is_admin, zitadel_sub, user_type, agent_context, \
                  created_at, updated_at) \
                 VALUES ($1, $2, '', $3, 'active', false, $4, 'machine', $5, $6, $7) \
                 ON CONFLICT (zitadel_sub) DO NOTHING",
            )
            .bind(&new_id)
            .bind(&placeholder_email)
            .bind(username)
            .bind(zitadel_sub)
            .bind(agent_context.as_str())
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to reconcile user row: {e}")))?;
            sqlx::query_scalar::<_, String>("SELECT id FROM users WHERE zitadel_sub = $1")
                .bind(zitadel_sub)
                .fetch_one(pool)
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to fetch reconciled user: {e}")))?
        }
    };

    let membership_id = format!("om_{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO organization_memberships (id, user_id, org_id, role, created_at) \
         VALUES ($1, $2, $3, 'member', $4) \
         ON CONFLICT (user_id, org_id) DO NOTHING",
    )
    .bind(&membership_id)
    .bind(&user_id)
    .bind(org_id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to ensure org membership: {e}")))?;

    for (team_id, scopes) in teams {
        let utm_id = format!("utm_{}", uuid::Uuid::new_v4());
        let scopes_json = serde_json::to_string(scopes)
            .map_err(|e| ApiError::Internal(format!("Failed to serialize scopes: {e}")))?;
        sqlx::query(
            "INSERT INTO user_team_memberships (id, user_id, team, scopes, created_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (user_id, team) DO NOTHING",
        )
        .bind(&utm_id)
        .bind(&user_id)
        .bind(team_id)
        .bind(&scopes_json)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to ensure team membership: {e}")))?;
    }

    Ok(user_id)
}

/// Create an agent (machine user) for an organization.
///
/// Provisions a Zitadel machine user and returns client credentials for MCP
/// tool access. Org admin only — platform admin cannot provision agents.
/// Idempotent: re-provisioning the same agent name returns 200 without credentials.
#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_name}/agents",
    params(("org_name" = String, Path, description = "Organization name")),
    request_body = CreateAgentRequest,
    responses(
        (status = 201, description = "Agent created", body = CreateAgentResponse),
        (status = 200, description = "Agent already exists (idempotent)", body = CreateAgentResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization or team not found"),
        (status = 503, description = "Zitadel admin client not configured")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_name = %org_name, agent_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_org_agent(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(org_name): Path<String>,
    Json(payload): Json<CreateAgentRequest>,
) -> Result<(StatusCode, Json<CreateAgentResponse>), ApiError> {
    // Step 1: Check team-scoped agents:write permission
    let team_for_auth = payload
        .teams
        .first()
        .ok_or_else(|| ApiError::BadRequest("At least one team must be specified".to_string()))?;
    if !check_resource_access(&context, "agents", "write", Some(team_for_auth)) {
        return Err(ApiError::forbidden("agents:write permission required for the target team"));
    }

    // Step 2: Validate request
    validate_agent_name(&payload.name)?;
    if payload.teams.is_empty() {
        return Err(ApiError::BadRequest("At least one team must be specified".to_string()));
    }
    if let Some(ref desc) = payload.description {
        if desc.len() > 256 {
            return Err(ApiError::BadRequest(
                "Description must be 256 characters or fewer".to_string(),
            ));
        }
    }

    // Step 3: Resolve org by name
    let pool = pool_for_state(&state)?;
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;
    let org_id_str = org.id.to_string();

    // Step 4: Validate each team exists in org and build (team_id, scopes) pairs
    let team_repo = Arc::new(SqlxTeamRepository::new(pool.clone()));
    let mut team_entries: Vec<(String, Vec<String>)> = Vec::new();
    for team_name_str in &payload.teams {
        let team = team_repo
            .get_team_by_org_and_name(&org.id, team_name_str)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| {
                ApiError::NotFound(format!(
                    "Team '{}' not found in org '{}'",
                    team_name_str, org_name
                ))
            })?;
        // Agents get empty scopes at creation — access is managed via agent_grants.
        team_entries.push((team.id.to_string(), Vec::new()));
    }

    // Step 5: Check ZitadelAdminClient available
    let zitadel_client = state
        .zitadel_admin
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Zitadel admin client not configured"))?;

    // Step 6: Build Zitadel username
    let username = format!("{}--{}", org_name, payload.name);

    // Step 7: Check if user already exists in Zitadel
    if let Some(zitadel_sub) = zitadel_client.search_user_by_username(&username).await? {
        let user_id = reconcile_machine_user_db(
            &pool,
            &zitadel_sub,
            &username,
            &org_id_str,
            &team_entries,
            AgentContext::CpTool, // TODO(E.3): make configurable from request body
        )
        .await?;
        if let Some(ref cache) = state.permission_cache {
            cache.evict(&zitadel_sub).await;
        }
        tracing::info!(
            username = %username,
            user_id = %user_id,
            org_name = %org_name,
            "agent already exists — returning idempotent response"
        );
        return Ok((
            StatusCode::OK,
            Json(CreateAgentResponse {
                agent_id: user_id,
                name: payload.name,
                username,
                client_id: None,
                client_secret: None,
                token_endpoint: get_token_endpoint(),
                org_id: org_id_str,
                teams: payload.teams,
                created_at: chrono::Utc::now(),
                message: Some(
                    "Agent already exists. Credentials were returned at creation time only."
                        .to_string(),
                ),
            }),
        ));
    }

    // Steps 8–13: Create new machine user
    let (local_user_id, client_id, client_secret) = provision_machine_user(
        zitadel_client,
        &pool,
        state.permission_cache.as_deref(),
        &org_id_str,
        &username,
        &payload.name,
        &team_entries,
        AgentContext::CpTool, // TODO(E.3): make configurable from request body
    )
    .await?;

    tracing::info!(
        username = %username,
        user_id = %local_user_id,
        org_name = %org_name,
        teams = ?payload.teams,
        "provisioned new agent"
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateAgentResponse {
            agent_id: local_user_id,
            name: payload.name,
            username,
            client_id: Some(client_id),
            client_secret: Some(client_secret),
            token_endpoint: get_token_endpoint(),
            org_id: org_id_str,
            teams: payload.teams,
            created_at: chrono::Utc::now(),
            message: None,
        }),
    ))
}

/// Agent entry in list response (no credentials).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub agent_id: String,
    pub name: String,
    pub username: String,
    pub teams: Vec<String>,
    pub agent_context: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Response for listing agents in an org.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentInfo>,
}

/// Row type for the agents-with-memberships join query.
#[derive(sqlx::FromRow)]
struct AgentMembershipRow {
    id: String,
    name: String,
    agent_context: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    team: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_name}/agents",
    params(("org_name" = String, Path, description = "Organization name")),
    responses(
        (status = 200, description = "List of agents", body = ListAgentsResponse),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Organization not found")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_name = %org_name, user_id = ?context.user_id))]
pub async fn list_org_agents(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(org_name): Path<String>,
) -> Result<Json<ListAgentsResponse>, ApiError> {
    if !check_resource_access(&context, "agents", "read", None) {
        return Err(ApiError::forbidden("agents:read permission required"));
    }

    let pool = pool_for_state(&state)?;
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;

    // Single join query: machine users + their team memberships (avoids N+1)
    let rows = sqlx::query_as::<_, AgentMembershipRow>(
        r#"
        SELECT
            u.id, u.name, u.agent_context, u.created_at,
            t.name AS team
        FROM users u
        JOIN organization_memberships om ON u.id = om.user_id AND om.org_id = $1
        LEFT JOIN user_team_memberships utm ON u.id = utm.user_id
        LEFT JOIN teams t ON utm.team = t.id
        WHERE u.user_type = 'machine'
        ORDER BY u.created_at DESC, t.name
        "#,
    )
    .bind(org.id.to_string())
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to list agents: {e}")))?;

    // Group rows by user id preserving insertion order
    let mut seen: Vec<String> = Vec::new();
    let mut by_user: std::collections::HashMap<String, AgentInfo> =
        std::collections::HashMap::new();
    for row in rows {
        let entry = by_user.entry(row.id.clone()).or_insert_with(|| {
            let username = format!("{}--{}", org_name, row.name);
            seen.push(row.id.clone());
            AgentInfo {
                agent_id: row.id.clone(),
                name: row.name,
                username,
                teams: Vec::new(),
                agent_context: row.agent_context,
                created_at: row.created_at,
            }
        });
        if let Some(team) = row.team {
            entry.teams.push(team);
        }
    }

    let agents = seen.into_iter().filter_map(|id| by_user.remove(&id)).collect();
    Ok(Json(ListAgentsResponse { agents }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{org_name}/agents/{agent_name}",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("agent_name" = String, Path, description = "Agent name")
    ),
    responses(
        (status = 204, description = "Agent deleted"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Agent not found")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_name = %org_name, agent_name = %agent_name, user_id = ?context.user_id))]
pub async fn delete_org_agent(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((org_name, agent_name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    if !check_resource_access(&context, "agents", "write", None) {
        return Err(ApiError::forbidden("agents:write permission required"));
    }

    let pool = pool_for_state(&state)?;
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;

    let user_repo = crate::storage::repositories::SqlxUserRepository::new(pool.clone());
    let machine_users = user_repo
        .find_machine_users_by_org(org.id.as_ref())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to find agent: {e}")))?;

    let agent = machine_users.into_iter().find(|u| u.name == agent_name).ok_or_else(|| {
        ApiError::NotFound(format!("Agent '{}' not found in org '{}'", agent_name, org_name))
    })?;

    // Evict from permission cache before deletion (while user_id is still known)
    if let Some(ref cache) = state.permission_cache {
        if let Some(ref zitadel_sub) = agent.zitadel_sub {
            cache.evict(zitadel_sub).await;
        }
    }

    // Delete user row — cascades to org_memberships and user_team_memberships via FK
    user_repo
        .delete_user(&agent.id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete agent: {e}")))?;

    tracing::info!(
        agent_name = %agent_name,
        agent_id = %agent.id,
        org_name = %org_name,
        "deleted agent"
    );

    Ok(StatusCode::NO_CONTENT)
}

// ===== Agent Grant CRUD =====

/// Request body for creating an agent grant.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateGrantRequest {
    pub grant_type: String,
    pub resource_type: Option<String>,
    pub action: Option<String>,
    pub team: String,
    pub route_id: Option<String>,
    pub allowed_methods: Option<Vec<String>>,
    pub expires_at: Option<String>,
}

/// A single grant returned from the API.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrantResponse {
    pub id: String,
    pub grant_type: String,
    pub resource_type: Option<String>,
    pub action: Option<String>,
    pub team: String,
    pub route_id: Option<String>,
    pub allowed_methods: Option<Vec<String>>,
    pub created_by: String,
    pub created_at: String,
    pub expires_at: Option<String>,
}

/// Response for listing grants.
#[derive(Debug, Serialize, ToSchema)]
pub struct GrantListResponse {
    pub grants: Vec<GrantResponse>,
}

#[derive(sqlx::FromRow)]
struct GrantRow {
    id: String,
    grant_type: String,
    resource_type: Option<String>,
    action: Option<String>,
    team: String,
    route_id: Option<String>,
    allowed_methods: Option<Vec<String>>,
    created_by: String,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Create a grant for an agent (org-admin only).
#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_name}/agents/{agent_name}/grants",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("agent_name" = String, Path, description = "Agent name")
    ),
    request_body = CreateGrantRequest,
    responses(
        (status = 201, description = "Grant created", body = GrantResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Agent or org not found"),
        (status = 409, description = "Duplicate grant")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state, payload), fields(org_name = %org_name, agent_name = %agent_name, user_id = ?context.user_id))]
pub async fn create_agent_grant(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((org_name, agent_name)): Path<(String, String)>,
    Json(payload): Json<CreateGrantRequest>,
) -> Result<(StatusCode, Json<GrantResponse>), ApiError> {
    require_org_admin_only(&context, &org_name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    // Validate grant_type
    if !matches!(payload.grant_type.as_str(), "cp-tool" | "gateway-tool" | "route") {
        return Err(ApiError::BadRequest(format!(
            "invalid grant type '{}': must be cp-tool, gateway-tool, or route",
            payload.grant_type
        )));
    }

    let pool = pool_for_state(&state)?;
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;

    // Resolve agent by name within this org
    let user_repo = crate::storage::repositories::SqlxUserRepository::new(pool.clone());
    let machine_users = user_repo
        .find_machine_users_by_org(org.id.as_ref())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to find agent: {e}")))?;
    let agent = machine_users.into_iter().find(|u| u.name == agent_name).ok_or_else(|| {
        ApiError::NotFound(format!("Agent '{}' not found in org '{}'", agent_name, org_name))
    })?;

    // Validate cp-tool grant requirements
    if payload.grant_type == "cp-tool" {
        let resource_type = payload.resource_type.as_deref().ok_or_else(|| {
            ApiError::BadRequest("cp-tool grants require resourceType".to_string())
        })?;
        let action = payload
            .action
            .as_deref()
            .ok_or_else(|| ApiError::BadRequest("cp-tool grants require action".to_string()))?;

        // Agent context must be cp-tool
        let agent_ctx = AgentContext::from_db(agent.agent_context.as_deref());
        if !matches!(agent_ctx, Some(AgentContext::CpTool)) {
            return Err(ApiError::BadRequest(
                "agent context mismatch: agent is not a cp-tool agent".to_string(),
            ));
        }

        // Validate (resource_type, action) pair exists in TOOL_AUTHORIZATIONS
        if !crate::mcp::tool_registry::is_valid_cp_grant_pair(resource_type, action) {
            return Err(ApiError::BadRequest(format!(
                "unknown resource type '{}' or action '{}' for cp-tool grant",
                resource_type, action
            )));
        }
    }

    // Validate gateway-tool grant requirements
    if payload.grant_type == "gateway-tool" {
        let agent_ctx = AgentContext::from_db(agent.agent_context.as_deref());
        if !matches!(agent_ctx, Some(AgentContext::GatewayTool)) {
            return Err(ApiError::BadRequest(
                "agent context mismatch: gateway-tool grants require a gateway-tool agent"
                    .to_string(),
            ));
        }
        let route_id = payload.route_id.as_deref().ok_or_else(|| {
            ApiError::BadRequest("gateway-tool grants require routeId".to_string())
        })?;
        let route_repo = crate::storage::repositories::route::RouteRepository::new(pool.clone());
        let route = route_repo
            .get_by_id(&crate::domain::RouteId::from_string(route_id.to_string()))
            .await
            .map_err(|_| ApiError::NotFound(format!("Route '{}' not found", route_id)))?;
        if route.exposure != "external" {
            return Err(ApiError::BadRequest(
                "Cannot grant access to internal route. Set route exposure to 'external' first."
                    .to_string(),
            ));
        }
    }

    // Validate route grant requirements
    if payload.grant_type == "route" {
        let agent_ctx = AgentContext::from_db(agent.agent_context.as_deref());
        if !matches!(agent_ctx, Some(AgentContext::ApiConsumer)) {
            return Err(ApiError::BadRequest(
                "agent context mismatch: route grants require an api-consumer agent".to_string(),
            ));
        }
        let route_id = payload
            .route_id
            .as_deref()
            .ok_or_else(|| ApiError::BadRequest("route grants require routeId".to_string()))?;
        let route_repo = crate::storage::repositories::route::RouteRepository::new(pool.clone());
        let route = route_repo
            .get_by_id(&crate::domain::RouteId::from_string(route_id.to_string()))
            .await
            .map_err(|_| ApiError::NotFound(format!("Route '{}' not found", route_id)))?;
        if route.exposure != "external" {
            return Err(ApiError::BadRequest(
                "Cannot grant access to internal route. Set route exposure to 'external' first."
                    .to_string(),
            ));
        }
    }

    // Validate agent is a member of the specified team
    let team_member_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_team_memberships utm \
         JOIN teams t ON utm.team = t.id AND t.org_id = $1 \
         WHERE utm.user_id = $2 AND t.name = $3",
    )
    .bind(org.id.as_ref())
    .bind(&agent.id)
    .bind(&payload.team)
    .fetch_one(&pool)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to check team membership: {e}")))?;

    if team_member_count == 0 {
        return Err(ApiError::BadRequest(format!(
            "agent is not a member of team '{}'",
            payload.team
        )));
    }

    let creator_id =
        context.user_id.as_ref().ok_or_else(|| ApiError::forbidden("user context required"))?;

    // Insert the grant; unique index will reject duplicates
    let grant_id = uuid::Uuid::new_v4().to_string();
    let row = sqlx::query_as::<_, GrantRow>(
        "INSERT INTO agent_grants \
         (id, agent_id, org_id, team, grant_type, resource_type, action, route_id, allowed_methods, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         RETURNING id, grant_type, resource_type, action, team, route_id, allowed_methods, \
                   created_by, created_at, expires_at",
    )
    .bind(&grant_id)
    .bind(&agent.id)
    .bind(org.id.as_ref())
    .bind(&payload.team)
    .bind(&payload.grant_type)
    .bind(payload.resource_type.as_deref())
    .bind(payload.action.as_deref())
    .bind(payload.route_id.as_deref())
    .bind(payload.allowed_methods.as_deref())
    .bind(creator_id.as_str())
    .fetch_one(&pool)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(ref db_err) = e {
            if db_err.code().as_deref() == Some("23505") {
                return ApiError::Conflict("grant already exists".to_string());
            }
        }
        ApiError::Internal(format!("Failed to create grant: {e}"))
    })?;

    // Evict permission cache so agent picks up new grant on next request
    if let Some(ref cache) = state.permission_cache {
        if let Some(ref sub) = agent.zitadel_sub {
            cache.evict(sub).await;
        }
    }

    // For route grants, trigger xDS snapshot update so RBAC filter is refreshed
    if payload.grant_type == "route" {
        if let Err(e) = state.xds_state.refresh_listeners_from_repository().await {
            tracing::error!(error = %e, "Failed to refresh xDS after route grant creation");
        }
    }

    tracing::info!(
        agent_name = %agent_name,
        grant_type = %payload.grant_type,
        org_name = %org_name,
        "created agent grant"
    );

    Ok((StatusCode::CREATED, Json(grant_row_to_response(row))))
}

/// List all grants for an agent (org-admin only).
#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_name}/agents/{agent_name}/grants",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("agent_name" = String, Path, description = "Agent name")
    ),
    responses(
        (status = 200, description = "Grants listed", body = GrantListResponse),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Agent or org not found")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_name = %org_name, agent_name = %agent_name, user_id = ?context.user_id))]
pub async fn list_agent_grants(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((org_name, agent_name)): Path<(String, String)>,
) -> Result<Json<GrantListResponse>, ApiError> {
    require_org_admin_only(&context, &org_name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    let pool = pool_for_state(&state)?;
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;

    let user_repo = crate::storage::repositories::SqlxUserRepository::new(pool.clone());
    let machine_users = user_repo
        .find_machine_users_by_org(org.id.as_ref())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to find agent: {e}")))?;
    let agent = machine_users.into_iter().find(|u| u.name == agent_name).ok_or_else(|| {
        ApiError::NotFound(format!("Agent '{}' not found in org '{}'", agent_name, org_name))
    })?;

    let rows = sqlx::query_as::<_, GrantRow>(
        "SELECT id, grant_type, resource_type, action, team, route_id, allowed_methods, \
                created_by, created_at, expires_at \
         FROM agent_grants WHERE agent_id = $1 ORDER BY created_at",
    )
    .bind(&agent.id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to list grants: {e}")))?;

    let grants = rows.into_iter().map(grant_row_to_response).collect();
    Ok(Json(GrantListResponse { grants }))
}

/// Delete a grant for an agent (org-admin only).
#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{org_name}/agents/{agent_name}/grants/{grant_id}",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("agent_name" = String, Path, description = "Agent name"),
        ("grant_id" = String, Path, description = "Grant ID")
    ),
    responses(
        (status = 204, description = "Grant deleted"),
        (status = 403, description = "Organization admin privileges required"),
        (status = 404, description = "Agent, org, or grant not found")
    ),
    security(("bearer_auth" = [])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(org_name = %org_name, agent_name = %agent_name, grant_id = %grant_id, user_id = ?context.user_id))]
pub async fn delete_agent_grant(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((org_name, agent_name, grant_id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    require_org_admin_only(&context, &org_name)
        .map_err(|_| ApiError::forbidden("Organization admin privileges required"))?;

    let pool = pool_for_state(&state)?;
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_name(&org_name)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;

    let user_repo = crate::storage::repositories::SqlxUserRepository::new(pool.clone());
    let machine_users = user_repo
        .find_machine_users_by_org(org.id.as_ref())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to find agent: {e}")))?;
    let agent = machine_users.into_iter().find(|u| u.name == agent_name).ok_or_else(|| {
        ApiError::NotFound(format!("Agent '{}' not found in org '{}'", agent_name, org_name))
    })?;

    // Fetch grant type before deletion so we know whether to trigger xDS
    let grant_type_row: Option<(String,)> =
        sqlx::query_as("SELECT grant_type FROM agent_grants WHERE id = $1 AND agent_id = $2")
            .bind(&grant_id)
            .bind(&agent.id)
            .fetch_optional(&pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to fetch grant: {e}")))?;
    let grant_type = grant_type_row.map(|(t,)| t).ok_or_else(|| {
        ApiError::NotFound(format!("Grant '{}' not found for agent '{}'", grant_id, agent_name))
    })?;

    // Delete the grant
    sqlx::query("DELETE FROM agent_grants WHERE id = $1 AND agent_id = $2")
        .bind(&grant_id)
        .bind(&agent.id)
        .execute(&pool)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete grant: {e}")))?;

    // Evict permission cache so agent loses access on next request
    if let Some(ref cache) = state.permission_cache {
        if let Some(ref sub) = agent.zitadel_sub {
            cache.evict(sub).await;
        }
    }

    // For route grants, trigger xDS snapshot update so RBAC filter is refreshed
    if grant_type == "route" {
        if let Err(e) = state.xds_state.refresh_listeners_from_repository().await {
            tracing::error!(error = %e, "Failed to refresh xDS after route grant deletion");
        }
    }

    tracing::info!(
        agent_name = %agent_name,
        grant_id = %grant_id,
        org_name = %org_name,
        "deleted agent grant"
    );

    Ok(StatusCode::NO_CONTENT)
}

fn grant_row_to_response(row: GrantRow) -> GrantResponse {
    GrantResponse {
        id: row.id,
        grant_type: row.grant_type,
        resource_type: row.resource_type,
        action: row.action,
        team: row.team,
        route_id: row.route_id,
        allowed_methods: row.allowed_methods,
        created_by: row.created_by,
        created_at: row.created_at.to_rfc3339(),
        expires_at: row.expires_at.map(|t| t.to_rfc3339()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::AuthContext;
    use crate::domain::TokenId;

    fn admin_context() -> AuthContext {
        AuthContext::new(
            TokenId::from_str_unchecked("admin-token"),
            "admin".into(),
            vec!["admin:all".into()],
        )
    }

    fn org_admin_context(org_name: &str) -> AuthContext {
        AuthContext::new(
            TokenId::from_str_unchecked("org-admin-token"),
            "org-admin".into(),
            vec![format!("org:{}:admin", org_name)],
        )
    }

    fn regular_context() -> AuthContext {
        AuthContext::new(
            TokenId::from_str_unchecked("regular-token"),
            "regular".into(),
            vec!["routes:read".into()],
        )
    }

    #[test]
    fn require_admin_allows_platform_admin() {
        let ctx = admin_context();
        assert!(require_admin(&ctx).is_ok());
    }

    #[test]
    fn require_admin_rejects_non_admin() {
        let ctx = regular_context();
        assert!(require_admin(&ctx).is_err());
    }

    #[test]
    fn require_admin_rejects_org_admin() {
        let ctx = org_admin_context("acme");
        assert!(require_admin(&ctx).is_err());
    }

    #[test]
    fn require_admin_or_org_admin_allows_platform_admin() {
        let ctx = admin_context();
        assert!(require_admin_or_org_admin(&ctx, "acme").is_ok());
        assert!(require_admin_or_org_admin(&ctx, "any-org").is_ok());
    }

    #[test]
    fn require_admin_or_org_admin_allows_matching_org_admin() {
        let ctx = org_admin_context("acme");
        assert!(require_admin_or_org_admin(&ctx, "acme").is_ok());
    }

    #[test]
    fn require_admin_or_org_admin_rejects_wrong_org_admin() {
        let ctx = org_admin_context("acme");
        assert!(require_admin_or_org_admin(&ctx, "globex").is_err());
    }

    #[test]
    fn require_admin_or_org_admin_rejects_regular_user() {
        let ctx = regular_context();
        assert!(require_admin_or_org_admin(&ctx, "acme").is_err());
    }

    // Tests for org-scoped endpoints

    fn org_member_context(org_name: &str) -> AuthContext {
        let mut ctx = AuthContext::new(
            TokenId::from_str_unchecked("member-token"),
            "member".into(),
            vec![format!("org:{}:member", org_name)],
        );
        ctx.user_id = Some(UserId::from_str_unchecked("user-123"));
        ctx
    }

    #[test]
    fn test_has_org_membership_with_member_scope() {
        let ctx = org_member_context("acme");
        assert!(crate::auth::authorization::has_org_membership(&ctx, "acme"));
        assert!(!crate::auth::authorization::has_org_membership(&ctx, "globex"));
    }

    #[test]
    fn test_has_org_admin_with_admin_scope() {
        let ctx = org_admin_context("acme");
        assert!(crate::auth::authorization::has_org_admin(&ctx, "acme"));
        assert!(!crate::auth::authorization::has_org_admin(&ctx, "globex"));
    }

    #[test]
    fn test_require_org_admin_allows_admin() {
        // Note: has_org_admin/require_org_admin still allows platform admin bypass for governance
        // (invite endpoint). Use require_org_admin_only for member/team management.
        let ctx = org_admin_context("acme");
        assert!(crate::auth::authorization::require_org_admin(&ctx, "acme").is_ok());
    }

    #[test]
    fn test_require_org_admin_rejects_member() {
        let ctx = org_member_context("acme");
        assert!(crate::auth::authorization::require_org_admin(&ctx, "acme").is_err());
    }

    // Tests verifying org admin access to org CRUD endpoints
    // (admin_get_organization, admin_update_organization, admin_delete_organization
    //  now use require_admin_or_org_admin instead of require_admin)

    #[test]
    fn org_admin_can_access_own_org_crud() {
        let ctx = org_admin_context("acme-corp");
        // Org admin should pass the auth check for their own org
        assert!(require_admin_or_org_admin(&ctx, "acme-corp").is_ok());
    }

    #[test]
    fn org_admin_cannot_access_other_org_crud() {
        let ctx = org_admin_context("acme-corp");
        // Org admin must NOT pass for a different org
        assert!(require_admin_or_org_admin(&ctx, "globex-corp").is_err());
    }

    #[test]
    fn platform_admin_can_access_any_org_crud() {
        let ctx = admin_context();
        // Platform admin can access any org
        assert!(require_admin_or_org_admin(&ctx, "acme-corp").is_ok());
        assert!(require_admin_or_org_admin(&ctx, "globex-corp").is_ok());
    }

    #[test]
    fn org_member_cannot_access_org_crud() {
        let ctx = org_member_context("acme-corp");
        // Regular org member should be rejected
        assert!(require_admin_or_org_admin(&ctx, "acme-corp").is_err());
    }

    #[test]
    fn unauthenticated_user_cannot_access_org_crud() {
        let ctx = regular_context();
        // User with no org scopes should be rejected
        assert!(require_admin_or_org_admin(&ctx, "acme-corp").is_err());
    }

    // Tests for org delete restriction (platform admin only)

    #[test]
    fn org_delete_requires_platform_admin() {
        // admin_delete_organization uses require_admin (not require_admin_or_org_admin)
        let ctx = admin_context();
        assert!(require_admin(&ctx).is_ok(), "Platform admin should be able to delete orgs");
    }

    #[test]
    fn org_admin_cannot_delete_own_org() {
        // Org admin should NOT be able to delete their own org
        let ctx = org_admin_context("acme-corp");
        assert!(require_admin(&ctx).is_err(), "Org admin should NOT be able to delete orgs");
    }

    #[test]
    fn org_member_cannot_delete_org() {
        let ctx = org_member_context("acme-corp");
        assert!(require_admin(&ctx).is_err(), "Org member should NOT be able to delete orgs");
    }

    // Tests for org-scoped team management (require_org_admin_only — no platform admin bypass)

    #[test]
    fn org_admin_can_manage_own_teams() {
        let ctx = org_admin_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "acme-corp").is_ok(),
            "Org admin should manage teams in their org"
        );
    }

    #[test]
    fn org_admin_cannot_manage_other_org_teams() {
        let ctx = org_admin_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "globex-corp").is_err(),
            "Org admin should NOT manage teams in other orgs"
        );
    }

    #[test]
    fn platform_admin_cannot_manage_org_teams() {
        // Platform admin must NOT bypass org admin for team/member management
        let ctx = admin_context();
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "acme-corp").is_err(),
            "Platform admin should NOT manage teams in orgs"
        );
    }

    #[test]
    fn org_member_cannot_manage_teams() {
        let ctx = org_member_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "acme-corp").is_err(),
            "Org member should NOT manage teams"
        );
    }

    // Tests for org admin auth boundary (B.6 invariant: platform admin cannot manage org members)

    #[test]
    fn platform_admin_cannot_manage_org_members() {
        let ctx = admin_context();
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "acme-corp").is_err(),
            "Platform admin should NOT manage org members"
        );
    }

    #[test]
    fn org_admin_can_manage_own_org_members() {
        let ctx = org_admin_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "acme-corp").is_ok(),
            "Org admin should manage members in their org"
        );
    }

    #[test]
    fn org_admin_cannot_manage_other_org_members() {
        let ctx = org_admin_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "globex-corp").is_err(),
            "Org admin should NOT manage members in other orgs"
        );
    }

    #[test]
    fn has_org_admin_only_excludes_platform_admin() {
        let ctx = admin_context();
        assert!(
            !crate::auth::authorization::has_org_admin_only(&ctx, "acme-corp"),
            "has_org_admin_only must not grant access to platform admin"
        );
    }

    #[test]
    fn has_org_admin_only_allows_matching_org_admin() {
        let ctx = org_admin_context("acme-corp");
        assert!(
            crate::auth::authorization::has_org_admin_only(&ctx, "acme-corp"),
            "has_org_admin_only should allow matching org admin"
        );
        assert!(
            !crate::auth::authorization::has_org_admin_only(&ctx, "globex-corp"),
            "has_org_admin_only should reject non-matching org"
        );
    }

    // Tests for scopes_for_org_role

    #[test]
    fn scopes_for_org_role_admin_returns_wildcard() {
        let scopes = scopes_for_org_role(OrgRole::Admin, "engineering");
        assert_eq!(scopes, vec!["team:engineering:*:*"]);
    }

    #[test]
    fn scopes_for_org_role_owner_returns_wildcard() {
        let scopes = scopes_for_org_role(OrgRole::Owner, "engineering");
        assert_eq!(scopes, vec!["team:engineering:*:*"]);
    }

    #[test]
    fn scopes_for_org_role_member_returns_specific() {
        let scopes = scopes_for_org_role(OrgRole::Member, "eng");
        assert!(scopes.contains(&"team:eng:routes:read".to_string()));
        assert!(scopes.contains(&"team:eng:routes:write".to_string()));
        assert!(scopes.contains(&"team:eng:clusters:read".to_string()));
        assert!(scopes.contains(&"team:eng:clusters:write".to_string()));
        assert!(scopes.contains(&"team:eng:listeners:read".to_string()));
        assert!(scopes.contains(&"team:eng:listeners:write".to_string()));
        assert!(scopes.contains(&"team:eng:filters:read".to_string()));
        assert!(scopes.contains(&"team:eng:filters:write".to_string()));
        assert!(scopes.contains(&"team:eng:stats:read".to_string()));
        // Member should NOT have wildcard
        assert!(!scopes.contains(&"team:eng:*:*".to_string()));
    }

    #[test]
    fn scopes_for_org_role_viewer_returns_read_only() {
        let scopes = scopes_for_org_role(OrgRole::Viewer, "frontend");
        assert_eq!(scopes.len(), 3);
        assert!(scopes.contains(&"team:frontend:routes:read".to_string()));
        assert!(scopes.contains(&"team:frontend:clusters:read".to_string()));
        assert!(scopes.contains(&"team:frontend:listeners:read".to_string()));
        // Viewer should NOT have write scopes
        for scope in &scopes {
            assert!(!scope.contains("write"), "Viewer should not have write scopes");
        }
    }

    // Tests for InviteOrgMemberRequest validation

    #[test]
    fn invite_request_rejects_invalid_email() {
        let req = InviteOrgMemberRequest {
            email: "not-an-email".to_string(),
            role: OrgRole::Admin,
            first_name: "Test".to_string(),
            last_name: "User".to_string(),
            initial_password: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn invite_request_accepts_valid_email() {
        let req = InviteOrgMemberRequest {
            email: "valid@example.com".to_string(),
            role: OrgRole::Admin,
            first_name: "Test".to_string(),
            last_name: "User".to_string(),
            initial_password: None,
        };
        assert!(req.validate().is_ok());
    }

    // ===== validate_agent_name tests =====

    #[test]
    fn validate_agent_name_accepts_valid_name() {
        assert!(validate_agent_name("my-agent").is_ok());
        assert!(validate_agent_name("agent123").is_ok());
        assert!(validate_agent_name("abc").is_ok()); // minimum length
        assert!(validate_agent_name("a-b-c-1-2-3").is_ok());
    }

    #[test]
    fn validate_agent_name_rejects_too_short() {
        assert!(validate_agent_name("ab").is_err());
        assert!(validate_agent_name("a").is_err());
        assert!(validate_agent_name("").is_err());
    }

    #[test]
    fn validate_agent_name_rejects_too_long() {
        let long = "a".repeat(64);
        assert!(validate_agent_name(&long).is_err());
    }

    #[test]
    fn validate_agent_name_rejects_uppercase() {
        assert!(validate_agent_name("MyAgent").is_err());
        assert!(validate_agent_name("AGENT").is_err());
    }

    #[test]
    fn validate_agent_name_rejects_spaces() {
        assert!(validate_agent_name("my agent").is_err());
    }

    #[test]
    fn validate_agent_name_rejects_leading_hyphen() {
        assert!(validate_agent_name("-agent").is_err());
    }

    #[test]
    fn validate_agent_name_rejects_trailing_hyphen() {
        assert!(validate_agent_name("agent-").is_err());
    }

    #[test]
    fn validate_agent_name_accepts_max_length() {
        let name = "a".repeat(63);
        assert!(validate_agent_name(&name).is_ok());
    }

    // ===== Agent authorization tests (check_resource_access based) =====

    fn team_member_with_agents_write(team: &str) -> AuthContext {
        AuthContext::new(
            TokenId::from_str_unchecked("member-token"),
            "member".into(),
            vec![format!("team:{}:agents:write", team)],
        )
    }

    fn team_member_with_agents_read(team: &str) -> AuthContext {
        AuthContext::new(
            TokenId::from_str_unchecked("member-token"),
            "member".into(),
            vec![format!("team:{}:agents:read", team)],
        )
    }

    #[test]
    fn create_agent_team_member_with_agents_write_can_create() {
        let ctx = team_member_with_agents_write("engineering");
        assert!(
            check_resource_access(&ctx, "agents", "write", Some("engineering")),
            "Team member with agents:write should be able to create agents in their team"
        );
    }

    #[test]
    fn create_agent_team_member_without_agents_write_cannot_create() {
        let ctx = org_member_context("acme-corp");
        assert!(
            !check_resource_access(&ctx, "agents", "write", Some("engineering")),
            "Team member without agents:write must not create agents"
        );
    }

    #[test]
    fn create_agent_platform_admin_cannot_create() {
        // Platform admin (admin:all) does NOT get access — agents is not a governance resource
        let ctx = admin_context();
        assert!(
            !check_resource_access(&ctx, "agents", "write", Some("engineering")),
            "Platform admin must not be allowed to provision agents"
        );
    }

    #[test]
    fn list_agents_team_member_with_agents_read_can_list() {
        let ctx = team_member_with_agents_read("engineering");
        assert!(
            check_resource_access(&ctx, "agents", "read", None),
            "Team member with agents:read should be able to list agents"
        );
    }

    #[test]
    fn list_agents_without_agents_read_cannot_list() {
        // A user with team scopes for other resources but not agents:read cannot list agents.
        // (org_member_context has org-level scopes so it passes the broader org-member check —
        //  this test uses a pure team-scoped user with no agents scope.)
        let ctx = AuthContext::new(
            TokenId::from_str_unchecked("member-token"),
            "member".into(),
            vec!["team:engineering:clusters:read".into()],
        );
        assert!(
            !check_resource_access(&ctx, "agents", "read", None),
            "Team member without agents:read must not list agents"
        );
    }

    #[test]
    fn list_agents_platform_admin_cannot_list() {
        let ctx = admin_context();
        assert!(
            !check_resource_access(&ctx, "agents", "read", None),
            "Platform admin must not be allowed to list agents"
        );
    }

    #[test]
    fn delete_agent_team_member_with_agents_write_can_delete() {
        let ctx = team_member_with_agents_write("engineering");
        assert!(
            check_resource_access(&ctx, "agents", "write", None),
            "Team member with agents:write should be able to delete agents"
        );
    }

    #[test]
    fn delete_agent_platform_admin_cannot_delete() {
        let ctx = admin_context();
        assert!(
            !check_resource_access(&ctx, "agents", "write", None),
            "Platform admin must not be allowed to delete agents"
        );
    }

    // ===== Grant API authorization tests =====

    #[test]
    fn create_grant_requires_org_admin_allows_org_admin() {
        let ctx = org_admin_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "acme-corp").is_ok(),
            "Org admin should be able to create grants"
        );
    }

    #[test]
    fn create_grant_requires_org_admin_rejects_platform_admin() {
        let ctx = admin_context();
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "acme-corp").is_err(),
            "Platform admin must not create grants (no visibility inside orgs)"
        );
    }

    #[test]
    fn create_grant_requires_org_admin_rejects_team_member() {
        let ctx = team_member_with_agents_write("engineering");
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "acme-corp").is_err(),
            "Team member must not create grants even with agents:write"
        );
    }

    #[test]
    fn create_grant_requires_org_admin_rejects_wrong_org_admin() {
        let ctx = org_admin_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin_only(&ctx, "other-org").is_err(),
            "Org admin of different org must not create grants"
        );
    }

    #[test]
    fn grant_type_validation_accepts_valid_types() {
        // Verify the set of accepted grant_type values
        for valid in &["cp-tool", "gateway-tool", "route"] {
            assert!(
                matches!(*valid, "cp-tool" | "gateway-tool" | "route"),
                "Grant type '{}' should be valid",
                valid
            );
        }
    }

    #[test]
    fn grant_type_validation_rejects_invalid_types() {
        // These should NOT match any valid grant type
        for invalid in &["admin", "read", "execute", "", "cp_tool", "GATEWAY-TOOL"] {
            assert!(
                !matches!(*invalid, "cp-tool" | "gateway-tool" | "route"),
                "Grant type '{}' should be invalid",
                invalid
            );
        }
    }

    // ===== Cross-context isolation authorization checks =====

    #[test]
    fn cp_tool_agent_cannot_access_governance_resources() {
        let ctx = AuthContext::with_user(
            TokenId::from_str_unchecked("cp-agent-token"),
            "cp-agent".into(),
            crate::domain::UserId::from_str_unchecked("cp-1"),
            "cp@test.com".into(),
            vec![],
        )
        .with_agent_data(
            Some(crate::auth::models::AgentContext::CpTool),
            vec![crate::auth::models::CpGrant {
                resource_type: "clusters".to_string(),
                action: "read".to_string(),
                team: "engineering".to_string(),
            }],
        );

        // CP tool with clusters:read can access clusters
        assert!(
            check_resource_access(&ctx, "clusters", "read", Some("engineering")),
            "CP-tool agent with clusters:read grant should access clusters"
        );
        // But cannot access governance resources even with grants
        assert!(
            !check_resource_access(&ctx, "organizations", "read", None),
            "CP-tool agent must not access governance resources"
        );
    }

    #[test]
    fn gateway_tool_agent_cannot_access_any_cp_resource() {
        let ctx = AuthContext::with_user(
            TokenId::from_str_unchecked("gw-agent-token"),
            "gw-agent".into(),
            crate::domain::UserId::from_str_unchecked("gw-1"),
            "gw@test.com".into(),
            vec![],
        )
        .with_agent_data(Some(crate::auth::models::AgentContext::GatewayTool), vec![]);

        // Gateway-tool agent with no CP grants cannot access any CP resource
        assert!(!check_resource_access(&ctx, "clusters", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "routes", "write", Some("engineering")));
        assert!(!check_resource_access(&ctx, "listeners", "read", None));
        assert!(!check_resource_access(&ctx, "agents", "read", None));
    }

    #[test]
    fn api_consumer_agent_cannot_access_any_cp_resource() {
        let ctx = AuthContext::with_user(
            TokenId::from_str_unchecked("consumer-token"),
            "consumer".into(),
            crate::domain::UserId::from_str_unchecked("consumer-1"),
            "consumer@test.com".into(),
            vec![],
        )
        .with_agent_data(Some(crate::auth::models::AgentContext::ApiConsumer), vec![]);

        // API-consumer agent cannot call CP endpoints
        assert!(!check_resource_access(&ctx, "clusters", "read", Some("engineering")));
        assert!(!check_resource_access(&ctx, "routes", "read", None));
        assert!(!check_resource_access(&ctx, "agents", "write", None));
    }
}
