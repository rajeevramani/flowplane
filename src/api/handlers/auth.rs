use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::{
    models::{AuthContext, PersonalAccessToken},
    token_service::{TokenSecretResponse, TokenService},
    validation::{CreateTokenRequest, UpdateTokenRequest},
};
use crate::errors::Error;
use crate::storage::repository::AuditLogRepository;

fn token_service_for_state(state: &ApiState) -> Result<TokenService, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Token repository unavailable"))?;
    let pool = cluster_repo.pool().clone();
    let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
    Ok(TokenService::with_sqlx(pool, audit_repository))
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTokenBody {
    #[validate(length(min = 3, max = 64))]
    pub name: String,
    pub description: Option<String>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<DateTime<Utc>>,
    #[validate(length(min = 1))]
    pub scopes: Vec<String>,
}

impl CreateTokenBody {
    fn into_request(self, created_by: &AuthContext) -> CreateTokenRequest {
        CreateTokenRequest {
            name: self.name,
            description: self.description,
            expires_at: self.expires_at,
            scopes: self.scopes,
            created_by: Some(created_by.token_id.clone()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTokenBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    #[schema(value_type = Option<String>, format = DateTime, nullable)]
    pub expires_at: Option<Option<DateTime<Utc>>>,
    pub scopes: Option<Vec<String>>,
}

impl UpdateTokenBody {
    fn into_request(self) -> UpdateTokenRequest {
        UpdateTokenRequest {
            name: self.name,
            description: self.description,
            status: self.status,
            expires_at: self.expires_at,
            scopes: self.scopes,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListTokensQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn convert_error(err: Error) -> ApiError {
    ApiError::from(err)
}

#[utoipa::path(
    post,
    path = "/api/v1/tokens",
    request_body = CreateTokenBody,
    responses(
        (status = 201, description = "Token created", body = TokenSecretResponse),
        (status = 400, description = "Validation error"),
        (status = 503, description = "Token repository unavailable")
    ),
    security(("bearerAuth" = [])),
    tag = "tokens"
)]
pub async fn create_token_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<CreateTokenBody>,
) -> Result<(StatusCode, Json<TokenSecretResponse>), ApiError> {
    payload.validate().map_err(|err| convert_error(Error::from(err)))?;

    let request = payload.into_request(&context);
    request.validate().map_err(|err| convert_error(Error::from(err)))?;

    let service = token_service_for_state(&state)?;
    let secret = service.create_token(request).await.map_err(convert_error)?;

    Ok((StatusCode::CREATED, Json(secret)))
}

#[utoipa::path(
    get,
    path = "/api/v1/tokens",
    params(ListTokensQuery),
    responses(
        (status = 200, description = "Tokens list", body = [PersonalAccessToken]),
        (status = 503, description = "Token repository unavailable")
    ),
    security(("bearerAuth" = [])),
    tag = "tokens"
)]
pub async fn list_tokens_handler(
    State(state): State<ApiState>,
    Query(params): Query<ListTokensQuery>,
) -> Result<Json<Vec<PersonalAccessToken>>, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 1000);
    let offset = params.offset.unwrap_or(0).max(0);

    let service = token_service_for_state(&state)?;
    let tokens = service.list_tokens(limit, offset).await.map_err(convert_error)?;

    Ok(Json(tokens))
}

#[utoipa::path(
    get,
    path = "/api/v1/tokens/{id}",
    params(("id" = String, Path, description = "Token identifier")),
    responses(
        (status = 200, description = "Token details", body = PersonalAccessToken),
        (status = 404, description = "Token not found"),
        (status = 503, description = "Token repository unavailable")
    ),
    security(("bearerAuth" = [])),
    tag = "tokens"
)]
pub async fn get_token_handler(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<PersonalAccessToken>, ApiError> {
    let service = token_service_for_state(&state)?;
    let token = service.get_token(&id).await.map_err(convert_error)?;
    Ok(Json(token))
}

#[utoipa::path(
    patch,
    path = "/api/v1/tokens/{id}",
    request_body = UpdateTokenBody,
    params(("id" = String, Path, description = "Token identifier")),
    responses(
        (status = 200, description = "Token updated", body = PersonalAccessToken),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Token not found"),
        (status = 503, description = "Token repository unavailable")
    ),
    security(("bearerAuth" = [])),
    tag = "tokens"
)]
pub async fn update_token_handler(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateTokenBody>,
) -> Result<Json<PersonalAccessToken>, ApiError> {
    let request = payload.into_request();
    request.validate().map_err(|err| convert_error(Error::from(err)))?;

    let service = token_service_for_state(&state)?;
    let token = service.update_token(&id, request).await.map_err(convert_error)?;

    Ok(Json(token))
}

#[utoipa::path(
    delete,
    path = "/api/v1/tokens/{id}",
    params(("id" = String, Path, description = "Token identifier")),
    responses(
        (status = 200, description = "Token revoked", body = PersonalAccessToken),
        (status = 404, description = "Token not found"),
        (status = 503, description = "Token repository unavailable")
    ),
    security(("bearerAuth" = [])),
    tag = "tokens"
)]
pub async fn revoke_token_handler(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<PersonalAccessToken>, ApiError> {
    let service = token_service_for_state(&state)?;
    let token = service.revoke_token(&id).await.map_err(convert_error)?;
    Ok(Json(token))
}

#[utoipa::path(
    post,
    path = "/api/v1/tokens/{id}/rotate",
    params(("id" = String, Path, description = "Token identifier")),
    responses(
        (status = 200, description = "Token rotated", body = TokenSecretResponse),
        (status = 404, description = "Token not found"),
        (status = 503, description = "Token repository unavailable")
    ),
    security(("bearerAuth" = [])),
    tag = "tokens"
)]
pub async fn rotate_token_handler(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<TokenSecretResponse>, ApiError> {
    let service = token_service_for_state(&state)?;
    let secret = service.rotate_token(&id).await.map_err(convert_error)?;
    Ok(Json(secret))
}
