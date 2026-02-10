//! Secret management HTTP handlers for SDS (Secret Discovery Service)
//!
//! Provides CRUD operations for secrets that are delivered to Envoy via SDS.
//! IMPORTANT: API responses never include decrypted secret values.

pub mod types;

use super::team_access::TeamPath;
pub use types::{
    CreateSecretReferenceRequest, CreateSecretRequest, ListSecretsQuery, SecretResponse,
    TeamSecretPath, UpdateSecretRequest,
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use tracing::instrument;

use crate::{
    api::{
        error::ApiError,
        handlers::{
            pagination::PaginatedResponse,
            team_access::{
                get_effective_team_ids, require_resource_access_resolved, team_repo_from_state,
                verify_team_access,
            },
        },
        routes::ApiState,
    },
    auth::models::AuthContext,
    domain::SecretSpec,
    storage::CreateSecretRequest as DbCreateSecretRequest,
    storage::UpdateSecretRequest as DbUpdateSecretRequest,
};

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
    tag = "Secrets"
)]
#[instrument(skip(state, payload), fields(team = %team, secret_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_secret_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Json(payload): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<SecretResponse>), ApiError> {
    use validator::Validate;
    payload.validate().map_err(ApiError::from)?;

    // Verify user has write access to the specified team
    require_resource_access_resolved(
        &state,
        &context,
        "secrets",
        "write",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database operations
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &team,
        context.org_id.as_ref(),
    )
    .await?;

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
    if repo.get_by_name(&team_id, &payload.name).await.is_ok() {
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
        team: team_id.clone(),
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
    tag = "Secrets"
)]
#[instrument(skip(state, payload), fields(team = %team, secret_name = %payload.name, backend = %payload.backend, user_id = ?context.user_id))]
pub async fn create_secret_reference_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Json(payload): Json<CreateSecretReferenceRequest>,
) -> Result<(StatusCode, Json<SecretResponse>), ApiError> {
    use validator::Validate;
    payload.validate().map_err(ApiError::from)?;

    // Verify user has write access to the specified team
    require_resource_access_resolved(
        &state,
        &context,
        "secrets",
        "write",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database operations
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &team,
        context.org_id.as_ref(),
    )
    .await?;

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
    if repo.get_by_name(&team_id, &payload.name).await.is_ok() {
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
        team: team_id.clone(),
        expires_at: payload.expires_at,
    };

    let created = repo.create_reference(request).await.map_err(ApiError::from)?;

    Ok((StatusCode::CREATED, Json(SecretResponse::from_data(&created))))
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/secrets",
    params(
        ("team" = String, Path, description = "Team name"),
        ListSecretsQuery
    ),
    responses(
        (status = 200, description = "List of secrets (metadata only)", body = PaginatedResponse<SecretResponse>),
        (status = 503, description = "Secret repository unavailable"),
    ),
    tag = "Secrets"
)]
#[instrument(skip(state, params), fields(team = %team, user_id = ?context.user_id))]
pub async fn list_secrets_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Query(params): Query<ListSecretsQuery>,
) -> Result<Json<PaginatedResponse<SecretResponse>>, ApiError> {
    // Authorization: require secrets:read scope
    require_resource_access_resolved(
        &state,
        &context,
        "secrets",
        "read",
        Some(&team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database operations
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &team,
        context.org_id.as_ref(),
    )
    .await?;

    // Verify team access - user must have access to the requested team
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    if !team_scopes.is_empty() && !team_scopes.contains(&team_id) {
        return Err(ApiError::NotFound("Team not found".to_string()));
    }

    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    let secrets = repo
        .list_by_teams(&[team_id], Some(params.limit as i32), Some(params.offset as i32))
        .await
        .map_err(ApiError::from)?;

    // Filter by secret_type if specified
    let secrets = match params.secret_type {
        Some(ref st) => secrets.into_iter().filter(|s| &s.secret_type == st).collect(),
        None => secrets,
    };

    let responses: Vec<SecretResponse> = secrets.iter().map(SecretResponse::from_data).collect();
    let total = responses.len() as i64;

    Ok(Json(PaginatedResponse::new(responses, total, params.limit, params.offset)))
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
    tag = "Secrets"
)]
#[instrument(skip(state), fields(team = %path.team, secret_id = %path.secret_id, user_id = ?context.user_id))]
pub async fn get_secret_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<TeamSecretPath>,
) -> Result<Json<SecretResponse>, ApiError> {
    // Authorization: require secrets:read scope
    require_resource_access_resolved(
        &state,
        &context,
        "secrets",
        "read",
        Some(&path.team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database operations
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &path.team,
        context.org_id.as_ref(),
    )
    .await?;

    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    let secret = repo.get_by_id(&path.secret_id).await.map_err(|_| {
        ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id))
    })?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let secret = verify_team_access(secret, &team_scopes).await?;

    // Verify the secret belongs to the specified team (URL consistency)
    if secret.team != team_id {
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
    tag = "Secrets"
)]
#[instrument(skip(state, payload), fields(team = %path.team, secret_id = %path.secret_id, user_id = ?context.user_id))]
pub async fn update_secret_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<TeamSecretPath>,
    Json(payload): Json<UpdateSecretRequest>,
) -> Result<Json<SecretResponse>, ApiError> {
    use validator::Validate;
    payload.validate().map_err(ApiError::from)?;

    // Authorization: require secrets:write scope
    require_resource_access_resolved(
        &state,
        &context,
        "secrets",
        "write",
        Some(&path.team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database operations
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &path.team,
        context.org_id.as_ref(),
    )
    .await?;

    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    // Get existing secret to verify ownership
    let existing = repo.get_by_id(&path.secret_id).await.map_err(|_| {
        ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id))
    })?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let existing = verify_team_access(existing, &team_scopes).await?;

    // Verify the secret belongs to the specified team (URL consistency)
    if existing.team != team_id {
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
    tag = "Secrets"
)]
#[instrument(skip(state), fields(team = %path.team, secret_id = %path.secret_id, user_id = ?context.user_id))]
pub async fn delete_secret_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<TeamSecretPath>,
) -> Result<StatusCode, ApiError> {
    // Authorization: require secrets:write scope
    require_resource_access_resolved(
        &state,
        &context,
        "secrets",
        "write",
        Some(&path.team),
        context.org_id.as_ref(),
    )
    .await?;

    // Resolve team name to UUID for database operations
    let team_id = crate::api::handlers::team_access::resolve_team_name(
        &state,
        &path.team,
        context.org_id.as_ref(),
    )
    .await?;

    let repo = state
        .xds_state
        .secret_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Secret repository unavailable"))?;

    // Get existing secret to verify ownership
    let existing = repo.get_by_id(&path.secret_id).await.map_err(|_| {
        ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id))
    })?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let existing = verify_team_access(existing, &team_scopes).await?;

    // Verify the secret belongs to the specified team (URL consistency)
    if existing.team != team_id {
        return Err(ApiError::NotFound(format!("Secret with ID '{}' not found", path.secret_id)));
    }

    repo.delete(&path.secret_id).await.map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}
