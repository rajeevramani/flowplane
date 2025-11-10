use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::authorization::require_resource_access;
use crate::auth::{
    models::{AuthContext, PersonalAccessToken},
    session::{SessionService, SESSION_COOKIE_NAME},
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

fn session_service_for_state(state: &ApiState) -> Result<SessionService, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::service_unavailable("Token repository unavailable"))?;
    let pool = cluster_repo.pool().clone();
    let token_repo = Arc::new(crate::storage::repository::SqlxTokenRepository::new(pool.clone()));
    let audit_repository = Arc::new(AuditLogRepository::new(pool));
    Ok(SessionService::new(token_repo, audit_repository))
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
            created_by: Some(created_by.token_id.to_string()),
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
    // Authorization: require tokens:write scope
    require_resource_access(&context, "tokens", "write", None)?;

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
    Extension(context): Extension<AuthContext>,
    Query(params): Query<ListTokensQuery>,
) -> Result<Json<Vec<PersonalAccessToken>>, ApiError> {
    // Authorization: require tokens:read scope
    require_resource_access(&context, "tokens", "read", None)?;

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
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<PersonalAccessToken>, ApiError> {
    // Authorization: require tokens:read scope
    require_resource_access(&context, "tokens", "read", None)?;

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
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateTokenBody>,
) -> Result<Json<PersonalAccessToken>, ApiError> {
    // Authorization: require tokens:write scope
    require_resource_access(&context, "tokens", "write", None)?;

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
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<PersonalAccessToken>, ApiError> {
    // Authorization: require tokens:write scope (revoke is a write operation)
    require_resource_access(&context, "tokens", "write", None)?;

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
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<TokenSecretResponse>, ApiError> {
    // Authorization: require tokens:write scope for rotation
    require_resource_access(&context, "tokens", "write", None)?;

    let service = token_service_for_state(&state)?;
    let secret = service.rotate_token(&id).await.map_err(convert_error)?;
    Ok(Json(secret))
}

// Session Management Endpoints

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionBody {
    #[validate(length(min = 10, message = "Setup token must be at least 10 characters"))]
    pub setup_token: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionResponseBody {
    pub session_id: String,
    pub csrf_token: String,
    pub expires_at: DateTime<Utc>,
    pub teams: Vec<String>,
    pub scopes: Vec<String>,
}

/// Response wrapper that includes both JSON body and Set-Cookie header
pub struct SessionCreatedResponse {
    body: CreateSessionResponseBody,
    cookie: Cookie<'static>,
    csrf_token: String,
}

impl IntoResponse for SessionCreatedResponse {
    fn into_response(self) -> Response {
        let mut response = (StatusCode::CREATED, Json(self.body)).into_response();

        // Set the session cookie
        if let Ok(cookie_value) = self.cookie.to_string().parse() {
            response.headers_mut().insert(header::SET_COOKIE, cookie_value);
        }

        // Set the CSRF token header
        if let Ok(csrf_value) = self.csrf_token.parse() {
            response
                .headers_mut()
                .insert(header::HeaderName::from_static("x-csrf-token"), csrf_value);
        }

        response
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/session/create",
    request_body = CreateSessionBody,
    responses(
        (status = 201, description = "Session created successfully", body = CreateSessionResponseBody,
         headers(
             ("Set-Cookie" = String, description = "Session cookie (fp_session)"),
             ("X-CSRF-Token" = String, description = "CSRF token for state-changing requests")
         )
        ),
        (status = 400, description = "Invalid setup token format"),
        (status = 401, description = "Setup token invalid, expired, or exhausted"),
        (status = 503, description = "Session service unavailable")
    ),
    tag = "auth"
)]
pub async fn create_session_handler(
    State(state): State<ApiState>,
    Json(payload): Json<CreateSessionBody>,
) -> Result<SessionCreatedResponse, ApiError> {
    // Validate request
    payload.validate().map_err(|err| convert_error(Error::from(err)))?;

    // Create session service
    let service = session_service_for_state(&state)?;

    // Exchange setup token for session
    let session_response = service
        .create_session_from_setup_token(&payload.setup_token)
        .await
        .map_err(convert_error)?;

    // Build secure session cookie
    let cookie = Cookie::build((SESSION_COOKIE_NAME, session_response.session_token.clone()))
        .path("/")
        .http_only(true)
        .secure(true) // TODO: Make this configurable for development
        .same_site(SameSite::Strict)
        .expires(
            time::OffsetDateTime::from_unix_timestamp(session_response.expires_at.timestamp()).ok(),
        )
        .into();

    let response_body = CreateSessionResponseBody {
        session_id: session_response.session_id,
        csrf_token: session_response.csrf_token.clone(),
        expires_at: session_response.expires_at,
        teams: session_response.teams,
        scopes: session_response.scopes,
    };

    Ok(SessionCreatedResponse {
        body: response_body,
        cookie,
        csrf_token: session_response.csrf_token,
    })
}
