//! Admin-only user management API handlers.
//!
//! This module provides HTTP handlers for user lifecycle management and team membership
//! operations. All endpoints require admin authentication (`admin:all` scope).

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

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::authorization::has_admin_bypass;
use crate::auth::models::AuthContext;
use crate::auth::user::{
    CreateTeamMembershipRequest, CreateUserRequest, UpdateUser, UpdateUserRequest, UserResponse,
    UserTeamMembership, UserWithTeamsResponse,
};
use crate::auth::user_service::UserService;
use crate::domain::UserId;
use crate::errors::Error;
use crate::storage::repositories::user::{SqlxTeamMembershipRepository, SqlxUserRepository};
use crate::storage::repositories::AuditLogRepository;

/// Helper to create UserService from ApiState.
fn user_service_for_state(state: &ApiState) -> Result<UserService, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("User repository unavailable"))?;
    let pool = cluster_repo.pool().clone();

    let user_repo = Arc::new(SqlxUserRepository::new(pool.clone()));
    let membership_repo = Arc::new(SqlxTeamMembershipRepository::new(pool.clone()));
    let team_repo =
        Arc::new(crate::storage::repositories::team::SqlxTeamRepository::new(pool.clone()));
    let audit_repo = Arc::new(AuditLogRepository::new(pool));

    Ok(UserService::with_team_validation(user_repo, membership_repo, team_repo, audit_repo))
}

/// Check if the current context has admin privileges.
fn require_admin(context: &AuthContext) -> Result<(), ApiError> {
    if !has_admin_bypass(context) {
        return Err(ApiError::forbidden("Admin privileges required"));
    }
    Ok(())
}

/// Query parameters for list_users endpoint.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListUsersQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Response for list_users endpoint.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListUsersResponse {
    pub users: Vec<UserResponse>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Create a new user (admin only).
///
/// Creates a new user account with the specified details.
/// The password will be hashed before storage.
#[utoipa::path(
    post,
    path = "/api/v1/users",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "User created successfully", body = UserResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Admin privileges required"),
        (status = 409, description = "User with email already exists")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "users"
)]
#[instrument(skip(state, payload), fields(email = %payload.email, user_id = ?context.user_id))]
pub async fn create_user(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserResponse>), ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Validate request
    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Create user
    let service = user_service_for_state(&state)?;
    let user = service
        .create_user(
            payload.email,
            payload.password,
            payload.name,
            payload.is_admin,
            Some(context.token_id.to_string()),
            Some(&context),
        )
        .await
        .map_err(convert_error)?;

    Ok((StatusCode::CREATED, Json(user.into())))
}

/// Get a user by ID (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/users/{id}",
    params(
        ("id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 200, description = "User found", body = UserWithTeamsResponse),
        (status = 403, description = "Admin privileges required"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "users"
)]
#[instrument(skip(state), fields(target_user_id = %id, user_id = ?context.user_id))]
pub async fn get_user(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<UserWithTeamsResponse>, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Parse user ID
    let user_id = UserId::from_string(id);

    // Get user
    let service = user_service_for_state(&state)?;
    let user = service
        .get_user(&user_id)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound("User not found".to_string()))?;

    // Get team memberships
    let teams = service.list_user_teams(&user_id).await.map_err(convert_error)?;

    Ok(Json(UserWithTeamsResponse { user: user.into(), teams }))
}

/// List all users with pagination (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/users",
    params(ListUsersQuery),
    responses(
        (status = 200, description = "Users listed successfully", body = ListUsersResponse),
        (status = 403, description = "Admin privileges required")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "users"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, limit = %query.limit, offset = %query.offset))]
pub async fn list_users(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<ListUsersResponse>, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // List users
    let service = user_service_for_state(&state)?;
    let users = service.list_users(query.limit, query.offset).await.map_err(convert_error)?;
    let total = service.count_users().await.map_err(convert_error)?;

    Ok(Json(ListUsersResponse {
        users: users.into_iter().map(|u| u.into()).collect(),
        total,
        limit: query.limit,
        offset: query.offset,
    }))
}

/// Update a user (admin only).
#[utoipa::path(
    put,
    path = "/api/v1/users/{id}",
    params(
        ("id" = String, Path, description = "User ID")
    ),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "User updated successfully", body = UserResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Admin privileges required"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "users"
)]
#[instrument(skip(state, payload), fields(target_user_id = %id, user_id = ?context.user_id))]
pub async fn update_user(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Validate request
    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Parse user ID
    let user_id = UserId::from_string(id);

    // Convert to update payload
    let update = UpdateUser {
        email: payload.email,
        name: payload.name,
        status: payload.status,
        is_admin: payload.is_admin,
    };

    // Update user
    let service = user_service_for_state(&state)?;
    let user = service
        .update_user(&user_id, update, Some(context.token_id.to_string()), Some(&context))
        .await
        .map_err(convert_error)?;

    Ok(Json(user.into()))
}

/// Delete a user (admin only).
#[utoipa::path(
    delete,
    path = "/api/v1/users/{id}",
    params(
        ("id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 204, description = "User deleted successfully"),
        (status = 403, description = "Admin privileges required"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "users"
)]
#[instrument(skip(state), fields(target_user_id = %id, user_id = ?context.user_id))]
pub async fn delete_user(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Parse user ID
    let user_id = UserId::from_string(id);

    // Delete user
    let service = user_service_for_state(&state)?;
    service
        .delete_user(&user_id, Some(context.token_id.to_string()), Some(&context))
        .await
        .map_err(convert_error)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Add a user to a team (admin only).
#[utoipa::path(
    post,
    path = "/api/v1/users/{id}/teams",
    params(
        ("id" = String, Path, description = "User ID")
    ),
    request_body = CreateTeamMembershipRequest,
    responses(
        (status = 201, description = "Team membership created successfully", body = UserTeamMembership),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Admin privileges required"),
        (status = 404, description = "User not found"),
        (status = 409, description = "User already member of team")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "users"
)]
#[instrument(skip(state, payload), fields(target_user_id = %id, team = %payload.team, user_id = ?context.user_id))]
pub async fn add_team_membership(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<CreateTeamMembershipRequest>,
) -> Result<(StatusCode, Json<UserTeamMembership>), ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Validate request
    payload.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Parse user ID
    let user_id = UserId::from_string(id);

    // Verify user ID in path matches payload
    if user_id != payload.user_id {
        return Err(ApiError::BadRequest("User ID in path and body must match".to_string()));
    }

    // Add team membership
    let service = user_service_for_state(&state)?;
    let membership = service
        .add_team_membership(
            &user_id,
            payload.team,
            payload.scopes,
            Some(context.token_id.to_string()),
            Some(&context),
        )
        .await
        .map_err(convert_error)?;

    Ok((StatusCode::CREATED, Json(membership)))
}

/// Remove a user from a team (admin only).
#[utoipa::path(
    delete,
    path = "/api/v1/users/{id}/teams/{team}",
    params(
        ("id" = String, Path, description = "User ID"),
        ("team" = String, Path, description = "Team name")
    ),
    responses(
        (status = 204, description = "Team membership removed successfully"),
        (status = 403, description = "Admin privileges required"),
        (status = 404, description = "User not found or not member of team")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "users"
)]
#[instrument(skip(state), fields(target_user_id = %id, team = %team, user_id = ?context.user_id))]
pub async fn remove_team_membership(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((id, team)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Parse user ID
    let user_id = UserId::from_string(id);

    // Remove team membership
    let service = user_service_for_state(&state)?;
    service
        .remove_team_membership(&user_id, &team, Some(context.token_id.to_string()), Some(&context))
        .await
        .map_err(convert_error)?;

    Ok(StatusCode::NO_CONTENT)
}

/// List all teams for a user (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/users/{id}/teams",
    params(
        ("id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 200, description = "Team memberships listed successfully", body = Vec<UserTeamMembership>),
        (status = 403, description = "Admin privileges required"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = ["admin:all"])),
    tag = "users"
)]
#[instrument(skip(state), fields(target_user_id = %id, user_id = ?context.user_id))]
pub async fn list_user_teams(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<Vec<UserTeamMembership>>, ApiError> {
    // Check admin authorization
    require_admin(&context)?;

    // Parse user ID
    let user_id = UserId::from_string(id);

    // List user teams
    let service = user_service_for_state(&state)?;
    let teams = service.list_user_teams(&user_id).await.map_err(convert_error)?;

    Ok(Json(teams))
}

/// Convert domain errors to API errors.
fn convert_error(error: Error) -> ApiError {
    ApiError::from(error)
}
