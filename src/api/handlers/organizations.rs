//! Admin organization management API handlers.
//!
//! This module provides HTTP handlers for organization lifecycle management and
//! organization membership operations. Organization creation and listing require
//! platform admin (`admin:all` scope). Get, update, and delete accept platform
//! admin or org admin. Membership endpoints accept platform admin or org admin.

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
        authorization::{has_admin_bypass, has_org_admin},
        models::AuthContext,
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

/// Helper to create UserRepository from ApiState.
fn user_repository_for_state(state: &ApiState) -> Result<Arc<dyn UserRepository>, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("User repository unavailable"))?;
    let pool = cluster_repo.pool().clone();
    Ok(Arc::new(crate::storage::repositories::SqlxUserRepository::new(pool)))
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

/// Update an organization (admin or org admin).
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
        (status = 403, description = "Admin or org admin privileges required"),
        (status = 404, description = "Organization not found")
    ),
    security(("bearer_auth" = ["admin:all"])),
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

    require_admin_or_org_admin(&context, &existing_org.name)?;

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

// ===== Organization Membership Endpoints (Platform Admin or Org Admin) =====

/// List members of an organization (admin or org admin).
#[utoipa::path(
    get,
    path = "/api/v1/admin/organizations/{id}/members",
    params(
        ("id" = String, Path, description = "Organization ID")
    ),
    responses(
        (status = 200, description = "Organization members listed successfully", body = ListOrgMembersResponse),
        (status = 403, description = "Admin or org admin privileges required"),
        (status = 404, description = "Organization not found")
    ),
    security(("bearer_auth" = ["admin:all"])),
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

/// Add a member to an organization (admin or org admin).
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
        (status = 403, description = "Admin or org admin privileges required"),
        (status = 404, description = "Organization or user not found"),
        (status = 409, description = "User is already a member")
    ),
    security(("bearer_auth" = ["admin:all"])),
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

    require_admin_or_org_admin(&context, &org.name)?;

    // SECURITY: Use a transaction with SELECT FOR UPDATE to prevent TOCTOU race
    // on user.org_id. Without this, two concurrent requests could both see org_id=None
    // and assign the user to different organizations simultaneously.
    let pool = pool_for_state(&state)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to begin transaction: {}", e)))?;

    // Lock the user row and get current org_id
    let user_row = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT id, org_id FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(payload.user_id.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to fetch user: {}", e)))?
    .ok_or_else(|| ApiError::NotFound("User not found".to_string()))?;

    let user_org_id = user_row.1;

    // Cross-org isolation: verify user belongs to the same org, or auto-assign if unset
    match user_org_id {
        Some(ref existing_org_id) if *existing_org_id != org.id.as_str() => {
            // No need to commit - rollback on drop is fine for read-only path
            tracing::warn!(
                attempted_org = %org.id,
                user_org = %existing_org_id,
                user_id = %payload.user_id,
                "cross-org member add violation: user belongs to different org"
            );
            return Err(ApiError::Forbidden(format!(
                "Cross-organization access denied: user belongs to org '{}', cannot be added to org '{}'",
                existing_org_id, org.name
            )));
        }
        None => {
            // User has no org_id set -- auto-assign within the same transaction
            tracing::info!(
                user_id = %payload.user_id,
                org_id = %org.id,
                org_name = %org.name,
                "auto-assigning user org_id during membership creation"
            );
            sqlx::query("UPDATE users SET org_id = $1, updated_at = $2 WHERE id = $3")
                .bind(org.id.as_str())
                .bind(chrono::Utc::now())
                .bind(payload.user_id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to update user org: {}", e)))?;
        }
        _ => {} // org_id matches, no action needed
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
        (String, String, String, String, chrono::DateTime<chrono::Utc>, String),
    >(
        "WITH inserted AS (
            INSERT INTO organization_memberships (id, user_id, org_id, role, created_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
        )
        SELECT i.id, i.user_id, i.org_id, i.role, i.created_at, o.name AS org_name
        FROM inserted i
        JOIN organizations o ON o.id = i.org_id",
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

    let role = crate::auth::organization::OrgRole::from_str(&row.3)
        .map_err(|e| ApiError::Internal(format!("Invalid role in DB: {}", e)))?;

    let membership = crate::auth::organization::OrganizationMembership {
        id: row.0,
        user_id: UserId::from_string(row.1),
        org_id: OrgId::from_string(row.2),
        role,
        org_name: row.5,
        created_at: row.4,
    };

    Ok((StatusCode::CREATED, Json(membership.into())))
}

/// Update a member's role in an organization (admin or org admin).
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
        (status = 403, description = "Admin or org admin privileges required"),
        (status = 404, description = "Organization or membership not found"),
        (status = 409, description = "Cannot downgrade the last owner")
    ),
    security(("bearer_auth" = ["admin:all"])),
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

    require_admin_or_org_admin(&context, &org.name)?;

    // Update role atomically (repository enforces last-owner constraint via transaction)
    let membership_repo = org_membership_repository_for_state(&state)?;
    let updated = membership_repo
        .update_membership_role(&target_user_id, &org_id, payload.role)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(updated.into()))
}

/// Remove a member from an organization (admin or org admin).
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
        (status = 403, description = "Admin or org admin privileges required"),
        (status = 404, description = "Organization or membership not found"),
        (status = 409, description = "Cannot remove the last owner")
    ),
    security(("bearer_auth" = ["admin:all"])),
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

    require_admin_or_org_admin(&context, &org.name)?;

    // Delete atomically (repository enforces last-owner constraint via transaction)
    let membership_repo = org_membership_repository_for_state(&state)?;
    membership_repo.delete_membership(&target_user_id, &org_id).await.map_err(ApiError::from)?;

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

    // Get user to retrieve org_id
    let user_repo = user_repository_for_state(&state)?;
    let user = user_repo
        .get_user(user_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("User not found".to_string()))?;

    let org_id = user.org_id;

    // Fetch organization
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    // Fetch user's membership to get their role
    let membership_repo = org_membership_repository_for_state(&state)?;
    let membership = membership_repo
        .get_membership(user_id, &org_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| {
            ApiError::NotFound("User is not a member of this organization".to_string())
        })?;

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
    // Verify caller is org admin
    crate::auth::authorization::require_org_admin(&context, &org_name)
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
        let scopes = crate::auth::invitation_service::scopes_for_role(member.role, &team.name);
        let membership = crate::auth::user::NewUserTeamMembership {
            id: format!("utm_{}", uuid::Uuid::new_v4()),
            user_id: member.user_id.clone(),
            team: team.id.to_string(),
            scopes,
        };
        team_membership_repo.create_membership(membership).await.map_err(ApiError::from)?;
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
    // Verify caller is org admin
    crate::auth::authorization::require_org_admin(&context, &path.org_name)
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
    // Verify caller is org admin
    crate::auth::authorization::require_org_admin(&context, &path.org_name)
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
}

impl From<crate::auth::user::UserTeamMembership> for TeamMemberResponse {
    fn from(m: crate::auth::user::UserTeamMembership) -> Self {
        Self {
            id: m.id,
            user_id: m.user_id.to_string(),
            team: m.team,
            scopes: m.scopes,
            created_at: m.created_at.to_rfc3339(),
        }
    }
}

/// Response for listing team members.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListTeamMembersResponse {
    pub members: Vec<TeamMemberResponse>,
}

/// Helper: resolve org + team from path, verify org admin.
async fn resolve_org_team(
    state: &ApiState,
    context: &AuthContext,
    org_name: &str,
    team_name: &str,
) -> Result<(crate::auth::organization::OrganizationResponse, crate::auth::team::Team), ApiError> {
    crate::auth::authorization::require_org_admin(context, org_name)
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
        crate::auth::invitation_service::scopes_for_role(org_membership.role, &team.name)
    } else {
        payload.scopes
    };

    let membership = crate::auth::user::NewUserTeamMembership {
        id: format!("utm_{}", uuid::Uuid::new_v4()),
        user_id: payload.user_id,
        team: team.id.to_string(),
        scopes,
    };

    let created = membership_repo.create_membership(membership).await.map_err(ApiError::from)?;

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

    let updated = membership_repo
        .update_membership_scopes(&membership.id, payload.scopes)
        .await
        .map_err(ApiError::from)?;

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

    Ok(StatusCode::NO_CONTENT)
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

    // Tests for org-scoped team management (require_org_admin)

    #[test]
    fn org_admin_can_manage_own_teams() {
        let ctx = org_admin_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin(&ctx, "acme-corp").is_ok(),
            "Org admin should manage teams in their org"
        );
    }

    #[test]
    fn org_admin_cannot_manage_other_org_teams() {
        let ctx = org_admin_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin(&ctx, "globex-corp").is_err(),
            "Org admin should NOT manage teams in other orgs"
        );
    }

    #[test]
    fn platform_admin_can_manage_any_org_teams() {
        // admin:all bypasses org admin check (has_org_admin bypasses for admin:all)
        let ctx = admin_context();
        assert!(
            crate::auth::authorization::require_org_admin(&ctx, "acme-corp").is_ok(),
            "Platform admin should manage teams in any org"
        );
    }

    #[test]
    fn org_member_cannot_manage_teams() {
        let ctx = org_member_context("acme-corp");
        assert!(
            crate::auth::authorization::require_org_admin(&ctx, "acme-corp").is_err(),
            "Org member should NOT manage teams"
        );
    }
}
