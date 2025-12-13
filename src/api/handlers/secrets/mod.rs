//! Secret management HTTP handlers for SDS (Secret Discovery Service)
//!
//! Provides CRUD operations for secrets that are delivered to Envoy via SDS.
//! IMPORTANT: API responses never include decrypted secret values.

pub mod types;

pub use types::{
    CreateSecretReferenceRequest, CreateSecretRequest, ListSecretsQuery, SecretResponse, TeamPath,
    TeamSecretPath, UpdateSecretRequest,
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use tracing::instrument;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::{extract_team_scopes, has_admin_bypass, require_resource_access},
    auth::models::AuthContext,
    domain::SecretSpec,
    errors::Error,
    storage::CreateSecretRequest as DbCreateSecretRequest,
    storage::UpdateSecretRequest as DbUpdateSecretRequest,
};

/// Verify that a secret belongs to one of the user's teams.
/// Returns error if not authorized (to avoid leaking existence).
fn verify_secret_access(secret_team: &str, team_scopes: &[String]) -> Result<(), ApiError> {
    // Admin or empty scopes can access everything
    if team_scopes.is_empty() {
        return Ok(());
    }

    if team_scopes.contains(&secret_team.to_string()) {
        Ok(())
    } else {
        // Record cross-team access attempt
        if let Some(from_team) = team_scopes.first() {
            tokio::spawn({
                let from = from_team.clone();
                let to = secret_team.to_string();
                async move {
                    crate::observability::metrics::record_cross_team_access_attempt(
                        &from, &to, "secrets",
                    )
                    .await;
                }
            });
        }
        Err(ApiError::NotFound("Secret not found".to_string()))
    }
}

// === Handler Implementations ===

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/secrets",
    request_body = CreateSecretRequest,
    responses(
        (status = 201, description = "Secret created", body = SecretResponse),
        (status = 400, description = "Validation error"),
        (status = 503, description = "Secret repository unavailable")
    ),
    tag = "secrets"
)]
#[instrument(skip(state, payload), fields(team = %team, secret_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_secret_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Json(payload): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<SecretResponse>), ApiError> {
    use validator::Validate;
    payload.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    // Verify user has write access to the specified team
    require_resource_access(&context, "secrets", "write", Some(&team))?;

    // Validate the configuration matches the secret type
    let spec: SecretSpec = serde_json::from_value(payload.configuration.clone()).map_err(|e| {
        ApiError::BadRequest(format!(
            "Invalid secret configuration for type {:?}: {}",
            payload.secret_type, e
        ))
    })?;

    // Verify the spec type matches the declared secret type
    if spec.secret_type() != payload.secret_type {
        return Err(ApiError::BadRequest(format!(
            "Configuration type mismatch: expected {:?}, got {:?}",
            payload.secret_type,
            spec.secret_type()
        )));
    }

    // Validate the secret spec
    spec.validate().map_err(|e| ApiError::BadRequest(format!("Invalid secret: {}", e)))?;

    // Get repository
    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    // Check for duplicate name
    if repo.get_by_name(&team, &payload.name).await.is_ok() {
        return Err(ApiError::Conflict(format!(
            "Secret with name '{}' already exists in team '{}'",
            payload.name, team
        )));
    }

    // Create the secret
    let request = DbCreateSecretRequest {
        name: payload.name,
        secret_type: payload.secret_type,
        description: payload.description,
        configuration: spec,
        team: team.clone(),
        expires_at: payload.expires_at,
    };

    let created = repo.create(request).await.map_err(ApiError::from)?;

    Ok((StatusCode::CREATED, Json(SecretResponse::from_data(&created))))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/secrets/reference",
    request_body = CreateSecretReferenceRequest,
    responses(
        (status = 201, description = "Secret reference created", body = SecretResponse),
        (status = 400, description = "Validation error"),
        (status = 503, description = "Secret repository unavailable or backend unavailable")
    ),
    tag = "secrets"
)]
#[instrument(skip(state, payload), fields(team = %team, secret_name = %payload.name, backend = %payload.backend, user_id = ?context.user_id))]
pub async fn create_secret_reference_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Json(payload): Json<CreateSecretReferenceRequest>,
) -> Result<(StatusCode, Json<SecretResponse>), ApiError> {
    use validator::Validate;
    payload.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    // Verify user has write access to the specified team
    require_resource_access(&context, "secrets", "write", Some(&team))?;

    // Validate backend type
    let valid_backends = ["vault", "aws_secrets_manager", "gcp_secret_manager"];
    if !valid_backends.contains(&payload.backend.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "Invalid backend type '{}'. Valid options: {:?}",
            payload.backend, valid_backends
        )));
    }

    // Check if external_secrets feature is enabled
    // (Optional - could be skipped for MVP)

    // Get repository
    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    // Check for duplicate name
    if repo.get_by_name(&team, &payload.name).await.is_ok() {
        return Err(ApiError::Conflict(format!(
            "Secret with name '{}' already exists in team '{}'",
            payload.name, team
        )));
    }

    // Create the secret reference
    let request = crate::storage::CreateSecretReferenceRequest {
        name: payload.name,
        secret_type: payload.secret_type,
        description: payload.description,
        backend: payload.backend,
        reference: payload.reference,
        reference_version: payload.reference_version,
        team: team.clone(),
        expires_at: payload.expires_at,
    };

    let created = repo.create_reference(request).await.map_err(ApiError::from)?;

    Ok((StatusCode::CREATED, Json(SecretResponse::from_data(&created))))
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/secrets",
    params(
        ("limit" = Option<i64>, Query, description = "Maximum number of secrets to return"),
        ("offset" = Option<i64>, Query, description = "Offset for pagination"),
        ("secret_type" = Option<String>, Query, description = "Filter by secret type"),
    ),
    responses(
        (status = 200, description = "List of secrets (metadata only)", body = [SecretResponse]),
        (status = 503, description = "Secret repository unavailable"),
    ),
    tag = "secrets"
)]
#[instrument(skip(state, params), fields(team = %team, user_id = ?context.user_id))]
pub async fn list_secrets_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Query(params): Query<ListSecretsQuery>,
) -> Result<Json<Vec<SecretResponse>>, ApiError> {
    // Authorization: require secrets:read scope
    require_resource_access(&context, "secrets", "read", Some(&team))?;

    // Verify team access
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };
    verify_secret_access(&team, &team_scopes)?;

    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    let secrets = repo
        .list_by_teams(&[team], params.limit.map(|l| l as i32), params.offset.map(|o| o as i32))
        .await
        .map_err(ApiError::from)?;

    // Filter by secret_type if specified
    let secrets = match params.secret_type {
        Some(ref st) => secrets.into_iter().filter(|s| &s.secret_type == st).collect(),
        None => secrets,
    };

    let responses: Vec<SecretResponse> = secrets.iter().map(SecretResponse::from_data).collect();

    Ok(Json(responses))
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/secrets/{secret_id}",
    params(
        ("team" = String, Path, description = "Team name"),
        ("secret_id" = String, Path, description = "Secret ID"),
    ),
    responses(
        (status = 200, description = "Secret metadata (no secret value)", body = SecretResponse),
        (status = 404, description = "Secret not found"),
        (status = 503, description = "Secret repository unavailable"),
    ),
    tag = "secrets"
)]
#[instrument(skip(state), fields(team = %path.team, secret_id = %path.secret_id, user_id = ?context.user_id))]
pub async fn get_secret_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<TeamSecretPath>,
) -> Result<Json<SecretResponse>, ApiError> {
    // Authorization: require secrets:read scope
    require_resource_access(&context, "secrets", "read", Some(&path.team))?;

    // Verify team access
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };
    verify_secret_access(&path.team, &team_scopes)?;

    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    let secret = repo.get_by_id(&path.secret_id).await.map_err(|_| {
        ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id))
    })?;

    // Verify the secret belongs to the specified team
    if secret.team != path.team {
        return Err(ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id)));
    }

    Ok(Json(SecretResponse::from_data(&secret)))
}

#[utoipa::path(
    put,
    path = "/api/v1/teams/{team}/secrets/{secret_id}",
    params(
        ("team" = String, Path, description = "Team name"),
        ("secret_id" = String, Path, description = "Secret ID"),
    ),
    request_body = UpdateSecretRequest,
    responses(
        (status = 200, description = "Secret updated", body = SecretResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Secret not found"),
        (status = 503, description = "Secret repository unavailable"),
    ),
    tag = "secrets"
)]
#[instrument(skip(state, payload), fields(team = %path.team, secret_id = %path.secret_id, user_id = ?context.user_id))]
pub async fn update_secret_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<TeamSecretPath>,
    Json(payload): Json<UpdateSecretRequest>,
) -> Result<Json<SecretResponse>, ApiError> {
    use validator::Validate;
    payload.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    // Authorization: require secrets:write scope
    require_resource_access(&context, "secrets", "write", Some(&path.team))?;

    // Verify team access
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };
    verify_secret_access(&path.team, &team_scopes)?;

    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    // Get existing secret to verify ownership
    let existing = repo.get_by_id(&path.secret_id).await.map_err(|_| {
        ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id))
    })?;

    // Verify the secret belongs to the specified team
    if existing.team != path.team {
        return Err(ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id)));
    }

    // Validate configuration if provided
    let configuration = if let Some(config) = payload.configuration {
        let spec: SecretSpec = serde_json::from_value(config).map_err(|e| {
            ApiError::BadRequest(format!(
                "Invalid secret configuration for type {:?}: {}",
                existing.secret_type, e
            ))
        })?;

        // Verify the spec type matches the existing secret type
        if spec.secret_type() != existing.secret_type {
            return Err(ApiError::BadRequest(format!(
                "Configuration type mismatch: expected {:?}, got {:?}",
                existing.secret_type,
                spec.secret_type()
            )));
        }

        spec.validate().map_err(|e| ApiError::BadRequest(format!("Invalid secret: {}", e)))?;

        Some(spec)
    } else {
        None
    };

    let request = DbUpdateSecretRequest {
        description: payload.description,
        configuration,
        expires_at: payload.expires_at.map(Some),
    };

    let updated = repo.update(&path.secret_id, request).await.map_err(ApiError::from)?;

    Ok(Json(SecretResponse::from_data(&updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/teams/{team}/secrets/{secret_id}",
    params(
        ("team" = String, Path, description = "Team name"),
        ("secret_id" = String, Path, description = "Secret ID"),
    ),
    responses(
        (status = 204, description = "Secret deleted"),
        (status = 404, description = "Secret not found"),
        (status = 503, description = "Secret repository unavailable"),
    ),
    tag = "secrets"
)]
#[instrument(skip(state), fields(team = %path.team, secret_id = %path.secret_id, user_id = ?context.user_id))]
pub async fn delete_secret_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<TeamSecretPath>,
) -> Result<StatusCode, ApiError> {
    // Authorization: require secrets:write scope
    require_resource_access(&context, "secrets", "write", Some(&path.team))?;

    // Verify team access
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };
    verify_secret_access(&path.team, &team_scopes)?;

    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    // Get existing secret to verify ownership
    let existing = repo.get_by_id(&path.secret_id).await.map_err(|_| {
        ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id))
    })?;

    // Verify the secret belongs to the specified team
    if existing.team != path.team {
        return Err(ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id)));
    }

    repo.delete(&path.secret_id).await.map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}
