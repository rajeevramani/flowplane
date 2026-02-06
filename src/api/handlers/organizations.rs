//! Admin organization management API handlers.
//!
//! This module provides HTTP handlers for organization lifecycle management and
//! organization membership operations. Organization CRUD requires platform admin
//! (`admin:all` scope). Membership endpoints accept platform admin or org admin.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::{
        authorization::{has_admin_bypass, has_org_admin},
        models::AuthContext,
        organization::{
            CreateOrganizationRequest, OrgRole, OrganizationResponse, UpdateOrganizationRequest,
        },
    },
    domain::{OrgId, UserId},
    errors::Error,
    storage::repositories::{
        OrgMembershipRepository, OrganizationRepository, SqlxOrgMembershipRepository,
        SqlxOrganizationRepository, UserRepository,
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

/// Check if the current context has platform admin privileges.
fn require_admin(context: &AuthContext) -> Result<(), ApiError> {
    if !has_admin_bypass(context) {
        return Err(ApiError::forbidden("Admin privileges required"));
    }
    Ok(())
}

/// Check if the current context has platform admin or org admin privileges.
fn require_admin_or_org_admin(context: &AuthContext, org_name: &str) -> Result<(), ApiError> {
    if has_admin_bypass(context) || has_org_admin(context, org_name) {
        return Ok(());
    }
    Err(ApiError::forbidden("Admin or organization admin privileges required"))
}

/// Convert domain errors to API errors.
fn convert_error(error: Error) -> ApiError {
    ApiError::from(error)
}

// ===== Request/Response Types =====

/// Query parameters for listing organizations.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListOrganizationsQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Response for listing organizations.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListOrganizationsResponse {
    pub organizations: Vec<OrganizationResponse>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

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

    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

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
        convert_error(e)
    })?;

    Ok((StatusCode::CREATED, Json(org.into())))
}

/// List all organizations with pagination (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/admin/organizations",
    params(ListOrganizationsQuery),
    responses(
        (status = 200, description = "Organizations listed successfully", body = ListOrganizationsResponse),
        (status = 403, description = "Admin privileges required")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "Organizations"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, limit = %query.limit, offset = %query.offset))]
pub async fn admin_list_organizations(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<ListOrganizationsQuery>,
) -> Result<Json<ListOrganizationsResponse>, ApiError> {
    require_admin(&context)?;

    let repo = org_repository_for_state(&state)?;
    let organizations =
        repo.list_organizations(query.limit, query.offset).await.map_err(convert_error)?;
    let total = repo.count_organizations().await.map_err(convert_error)?;

    Ok(Json(ListOrganizationsResponse {
        organizations: organizations.into_iter().map(|o| o.into()).collect(),
        total,
        limit: query.limit,
        offset: query.offset,
    }))
}

/// Get an organization by ID (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/admin/organizations/{id}",
    params(
        ("id" = String, Path, description = "Organization ID")
    ),
    responses(
        (status = 200, description = "Organization found", body = OrganizationResponse),
        (status = 403, description = "Admin privileges required"),
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
    require_admin(&context)?;

    let org_id = OrgId::from_string(id);

    let repo = org_repository_for_state(&state)?;
    let org = repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    Ok(Json(org.into()))
}

/// Update an organization (admin only).
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
        (status = 403, description = "Admin privileges required"),
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
    require_admin(&context)?;

    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let org_id = OrgId::from_string(id);

    let repo = org_repository_for_state(&state)?;
    let org = repo.update_organization(&org_id, payload).await.map_err(convert_error)?;

    Ok(Json(org.into()))
}

/// Delete an organization (admin only).
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
        (status = 403, description = "Admin privileges required"),
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
    require_admin(&context)?;

    let org_id = OrgId::from_string(id);

    let repo = org_repository_for_state(&state)?;
    repo.delete_organization(&org_id).await.map_err(convert_error)?;

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
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_admin_or_org_admin(&context, &org.name)?;

    let membership_repo = org_membership_repository_for_state(&state)?;
    let members = membership_repo.list_org_members(&org_id).await.map_err(convert_error)?;

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
    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let org_id = OrgId::from_string(id);

    // Resolve org
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_admin_or_org_admin(&context, &org.name)?;

    // Verify user exists
    let user_repo = user_repository_for_state(&state)?;
    let user = user_repo
        .get_user(&payload.user_id)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound("User not found".to_string()))?;

    // Cross-org isolation: verify user belongs to the same org (if user has an org_id set)
    if let Some(ref user_org_id) = user.org_id {
        if *user_org_id != org.id {
            return Err(ApiError::BadRequest(
                "User belongs to a different organization".to_string(),
            ));
        }
    }

    // Check if already a member
    let membership_repo = org_membership_repository_for_state(&state)?;
    let existing =
        membership_repo.get_membership(&payload.user_id, &org_id).await.map_err(convert_error)?;
    if existing.is_some() {
        return Err(ApiError::Conflict(
            "User is already a member of this organization".to_string(),
        ));
    }

    let membership = membership_repo
        .create_membership(&payload.user_id, &org_id, payload.role)
        .await
        .map_err(convert_error)?;

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
    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let org_id = OrgId::from_string(id);
    let target_user_id = UserId::from_string(user_id);

    // Resolve org
    let org_repo = org_repository_for_state(&state)?;
    let org = org_repo
        .get_organization_by_id(&org_id)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_admin_or_org_admin(&context, &org.name)?;

    // Update role atomically (repository enforces last-owner constraint via transaction)
    let membership_repo = org_membership_repository_for_state(&state)?;
    let updated = membership_repo
        .update_membership_role(&target_user_id, &org_id, payload.role)
        .await
        .map_err(convert_error)?;

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
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound("Organization not found".to_string()))?;

    require_admin_or_org_admin(&context, &org.name)?;

    // Delete atomically (repository enforces last-owner constraint via transaction)
    let membership_repo = org_membership_repository_for_state(&state)?;
    membership_repo.delete_membership(&target_user_id, &org_id).await.map_err(convert_error)?;

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
}
