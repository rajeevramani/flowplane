use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;
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
use crate::domain::UserId;
use crate::storage::repositories::{SqlxUserRepository, UserRepository};
use crate::storage::repository::AuditLogRepository;

/// Extract client IP from headers, preferring X-Forwarded-For
fn extract_client_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    // Try X-Forwarded-For header first (for proxied requests)
    if let Some(forwarded) = headers.get("x-forwarded-for") {
        if let Ok(value) = forwarded.to_str() {
            // X-Forwarded-For can contain multiple IPs; the first is the original client
            return value.split(',').next().map(|s| s.trim().to_string());
        }
    }
    // Note: We don't have access to ConnectInfo here, so we just return None
    // if X-Forwarded-For is not present
    None
}

/// Extract User-Agent header
fn extract_user_agent(headers: &axum::http::HeaderMap) -> Option<String> {
    headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
}

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
        // Use user_id if available (for user sessions), otherwise fall back to token_id (for setup tokens)
        let creator = if let Some(user_id) = &created_by.user_id {
            format!("user:{}", user_id)
        } else {
            created_by.token_id.to_string()
        };

        CreateTokenRequest {
            name: self.name,
            description: self.description,
            expires_at: self.expires_at,
            scopes: self.scopes,
            created_by: Some(creator),
            user_id: created_by.user_id.clone(),
            user_email: created_by.user_email.clone(),
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

use super::pagination::{PaginatedResponse, PaginationQuery};

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
    tag = "Authentication"
)]
#[instrument(skip(state, payload), fields(token_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_token_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<CreateTokenBody>,
) -> Result<(StatusCode, Json<TokenSecretResponse>), ApiError> {
    // Authorization: require tokens:write scope
    require_resource_access(&context, "tokens", "write", None)?;

    payload.validate().map_err(ApiError::from)?;

    let request = payload.into_request(&context);
    request.validate().map_err(ApiError::from)?;

    let service = token_service_for_state(&state)?;
    let secret = service.create_token(request, Some(&context)).await.map_err(ApiError::from)?;

    Ok((StatusCode::CREATED, Json(secret)))
}

#[utoipa::path(
    get,
    path = "/api/v1/tokens",
    params(PaginationQuery),
    responses(
        (status = 200, description = "Tokens list", body = PaginatedResponse<PersonalAccessToken>),
        (status = 503, description = "Token repository unavailable")
    ),
    security(("bearerAuth" = [])),
    tag = "Authentication"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, limit = %params.limit, offset = %params.offset))]
pub async fn list_tokens_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<PaginatedResponse<PersonalAccessToken>>, ApiError> {
    // Authorization: require tokens:read scope
    require_resource_access(&context, "tokens", "read", None)?;

    let (limit, offset) = params.clamp(1000);

    // Filter tokens by current user - only show tokens created by this user
    let created_by_filter = context.user_id.as_ref().map(|user_id| format!("user:{}", user_id));

    let service = token_service_for_state(&state)?;
    let tokens = service
        .list_tokens(limit, offset, created_by_filter.as_deref())
        .await
        .map_err(ApiError::from)?;

    let total = tokens.len() as i64;
    Ok(Json(PaginatedResponse::new(tokens, total, limit, offset)))
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
    tag = "Authentication"
)]
#[instrument(skip(state), fields(token_id = %id, user_id = ?context.user_id))]
pub async fn get_token_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<PersonalAccessToken>, ApiError> {
    // Authorization: require tokens:read scope
    require_resource_access(&context, "tokens", "read", None)?;

    let service = token_service_for_state(&state)?;
    let token = service.get_token(&id).await.map_err(ApiError::from)?;
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
    tag = "Authentication"
)]
#[instrument(skip(state, payload), fields(token_id = %id, user_id = ?context.user_id))]
pub async fn update_token_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateTokenBody>,
) -> Result<Json<PersonalAccessToken>, ApiError> {
    // Authorization: require tokens:write scope
    require_resource_access(&context, "tokens", "write", None)?;

    let request = payload.into_request();
    request.validate().map_err(ApiError::from)?;

    let service = token_service_for_state(&state)?;
    let token = service.update_token(&id, request, Some(&context)).await.map_err(ApiError::from)?;

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
    tag = "Authentication"
)]
#[instrument(skip(state), fields(token_id = %id, user_id = ?context.user_id))]
pub async fn revoke_token_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<PersonalAccessToken>, ApiError> {
    // Authorization: require tokens:write scope (revoke is a write operation)
    require_resource_access(&context, "tokens", "write", None)?;

    let service = token_service_for_state(&state)?;
    let token = service.revoke_token(&id, Some(&context)).await.map_err(ApiError::from)?;
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
    tag = "Authentication"
)]
#[instrument(skip(state), fields(token_id = %id, user_id = ?context.user_id))]
pub async fn rotate_token_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<TokenSecretResponse>, ApiError> {
    // Authorization: require tokens:write scope for rotation
    require_resource_access(&context, "tokens", "write", None)?;

    let service = token_service_for_state(&state)?;
    let secret = service.rotate_token(&id, Some(&context)).await.map_err(ApiError::from)?;
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
    path = "/api/v1/auth/sessions",
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
    tag = "Authentication"
)]
#[instrument(skip(state, payload))]
pub async fn create_session_handler(
    State(state): State<ApiState>,
    Json(payload): Json<CreateSessionBody>,
) -> Result<SessionCreatedResponse, ApiError> {
    // Validate request
    payload.validate().map_err(ApiError::from)?;

    // Create session service
    let service = session_service_for_state(&state)?;

    // Exchange setup token for session
    let session_response = service
        .create_session_from_setup_token(&payload.setup_token)
        .await
        .map_err(ApiError::from)?;

    // Build secure session cookie
    // Note: .secure(false) allows cookie to work over HTTP in development
    // In production, this should be set to true and use HTTPS
    let cookie = Cookie::build((SESSION_COOKIE_NAME, session_response.session_token.clone()))
        .path("/")
        .http_only(true)
        .secure(false) // Allow HTTP in development
        .same_site(SameSite::Lax) // Lax instead of Strict for cross-site navigation
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

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoResponse {
    pub session_id: String,
    pub user_id: String,
    pub name: String,
    pub email: String,
    pub is_admin: bool,
    pub teams: Vec<String>,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Organization ID (if user belongs to an org)
    pub org_id: Option<String>,
    /// Organization name (if user belongs to an org)
    pub org_name: Option<String>,
    /// Control plane version
    #[schema(example = "0.0.11")]
    pub version: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/sessions/me",
    responses(
        (status = 200, description = "Current session information", body = SessionInfoResponse),
        (status = 401, description = "Invalid or expired session token"),
        (status = 503, description = "Session service unavailable")
    ),
    tag = "Authentication"
)]
#[instrument(skip(state, jar, headers))]
pub async fn get_session_info_handler(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
) -> Result<Json<SessionInfoResponse>, ApiError> {
    // Try to extract session token from cookie first, then from Authorization header
    let session_token = jar
        .get(SESSION_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string())
        .or_else(|| {
            // Try Bearer token from Authorization header
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .map(|s| s.to_string())
        })
        .ok_or_else(|| {
            ApiError::unauthorized("Session token required (via cookie or Authorization header)")
        })?;

    // Create session service
    let service = session_service_for_state(&state)?;

    // Validate session
    let session_info = service.validate_session(&session_token).await.map_err(ApiError::from)?;

    // Get user information - need to find the user associated with this session
    let (user_id_str, name, email, is_admin, user_org_id) =
        match &session_info.token.created_by {
            Some(created_by) if created_by.starts_with("user:") => {
                // Session created from login - extract user ID
                let user_id_str = created_by.strip_prefix("user:").unwrap();

                let cluster_repo =
                    state.xds_state.cluster_repository.as_ref().cloned().ok_or_else(|| {
                        ApiError::service_unavailable("User repository unavailable")
                    })?;
                let pool = cluster_repo.pool().clone();
                let user_repo = SqlxUserRepository::new(pool);

                let user_id = UserId::from_string(user_id_str.to_string());
                let user = user_repo.get_user(&user_id).await.map_err(ApiError::from)?.ok_or_else(
                    || ApiError::Internal("User not found for session token".to_string()),
                )?;

                (
                    user_id_str.to_string(),
                    user.name.clone(),
                    user.email.clone(),
                    user.is_admin,
                    user.org_id.to_string(),
                )
            }
            Some(created_by) if created_by.starts_with("setup_token:") => {
                // For bootstrap sessions, we need to find the admin user
                let cluster_repo =
                    state.xds_state.cluster_repository.as_ref().cloned().ok_or_else(|| {
                        ApiError::service_unavailable("User repository unavailable")
                    })?;
                let pool = cluster_repo.pool().clone();
                let user_repo = SqlxUserRepository::new(pool);

                // Get all users and find the admin (during bootstrap, there's only one user)
                let users = user_repo.list_users(100, 0).await.map_err(ApiError::from)?;
                let admin_user = users
                    .iter()
                    .find(|u| u.is_admin)
                    .ok_or_else(|| ApiError::Internal("No admin user found".to_string()))?;

                (
                    admin_user.id.as_str().to_string(),
                    admin_user.name.clone(),
                    admin_user.email.clone(),
                    admin_user.is_admin,
                    admin_user.org_id.to_string(),
                )
            }
            Some(_) => {
                return Err(ApiError::Internal("Unknown session token creator format".to_string()))
            }
            None => {
                return Err(ApiError::Internal("Session token has no associated user".to_string()))
            }
        };

    // Extract org info: use user's org_id directly (scopes only contain org name, not ID)
    let (_, org_name) = crate::auth::session::extract_org_from_scopes(&session_info.token.scopes);
    let org_id = Some(user_org_id);

    let response = SessionInfoResponse {
        session_id: session_info.token.id.to_string(),
        user_id: user_id_str,
        name,
        email,
        is_admin,
        teams: session_info.teams,
        scopes: session_info.token.scopes,
        expires_at: session_info.token.expires_at,
        org_id,
        org_name,
        version: crate::VERSION.to_string(),
    };

    Ok(Json(response))
}

/// Response for logout endpoint with cookie clearing
pub struct LogoutResponse {
    cookie: Cookie<'static>,
}

impl IntoResponse for LogoutResponse {
    fn into_response(self) -> Response {
        let mut response = StatusCode::NO_CONTENT.into_response();

        // Set the session cookie with immediate expiration to clear it
        if let Ok(cookie_value) = self.cookie.to_string().parse() {
            response.headers_mut().insert(header::SET_COOKIE, cookie_value);
        }

        response
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/sessions/logout",
    responses(
        (status = 204, description = "Session logged out successfully",
         headers(
             ("Set-Cookie" = String, description = "Cookie clearing directive")
         )
        ),
        (status = 401, description = "Invalid or expired session token"),
        (status = 503, description = "Session service unavailable")
    ),
    tag = "Authentication"
)]
#[instrument(skip(state, jar, headers))]
pub async fn logout_handler(
    State(state): State<ApiState>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
) -> Result<LogoutResponse, ApiError> {
    // Try to extract session token from cookie first, then from Authorization header
    let session_token = jar
        .get(SESSION_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string())
        .or_else(|| {
            // Try Bearer token from Authorization header
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .filter(|s| s.starts_with("fp_session_"))
                .map(|s| s.to_string())
        })
        .ok_or_else(|| {
            ApiError::unauthorized("Session token required (via cookie or Authorization header)")
        })?;

    // Parse session token to get the ID (format: fp_session_{id}.{secret})
    let token_id = session_token
        .split('.')
        .next()
        .and_then(|prefix| prefix.strip_prefix("fp_session_"))
        .ok_or_else(|| ApiError::unauthorized("Invalid session token format"))?;

    // Create session service
    let session_service = session_service_for_state(&state)?;

    // Validate the session exists and is active before revoking
    session_service.validate_session(&session_token).await.map_err(ApiError::from)?;

    // Create token service and revoke the session token
    // Note: No AuthContext available for logout since we're terminating the session
    let token_service = token_service_for_state(&state)?;
    token_service.revoke_token(token_id, None).await.map_err(ApiError::from)?;

    // Build cookie clearing directive (same name, empty value, immediate expiration)
    let clear_cookie = Cookie::build((SESSION_COOKIE_NAME, ""))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .expires(time::OffsetDateTime::UNIX_EPOCH)
        .into();

    Ok(LogoutResponse { cookie: clear_cookie })
}

// Email/Password Login Endpoint

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginBody {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 1))]
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponseBody {
    pub session_id: String,
    pub csrf_token: String,
    pub expires_at: DateTime<Utc>,
    pub user_id: String,
    pub user_email: String,
    pub teams: Vec<String>,
    pub scopes: Vec<String>,
    /// Organization ID (if user belongs to an org)
    pub org_id: Option<String>,
    /// Organization name (if user belongs to an org)
    pub org_name: Option<String>,
}

/// Response wrapper that includes both JSON body and Set-Cookie header
pub struct LoginResponse {
    body: LoginResponseBody,
    cookie: Cookie<'static>,
    csrf_token: String,
}

impl IntoResponse for LoginResponse {
    fn into_response(self) -> Response {
        let mut response = (StatusCode::OK, Json(self.body)).into_response();

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
    path = "/api/v1/auth/login",
    request_body = LoginBody,
    responses(
        (status = 200, description = "Login successful", body = LoginResponseBody,
         headers(
             ("Set-Cookie" = String, description = "Session cookie (fp_session)"),
             ("X-CSRF-Token" = String, description = "CSRF token for state-changing requests")
         )
        ),
        (status = 400, description = "Invalid request format"),
        (status = 401, description = "Invalid credentials or account not active"),
        (status = 503, description = "Service unavailable")
    ),
    tag = "Authentication"
)]
#[instrument(skip(state, payload, headers), fields(email = %payload.email))]
pub async fn login_handler(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<LoginBody>,
) -> Result<LoginResponse, ApiError> {
    use crate::auth::login_service::LoginService;
    use crate::auth::LoginRequest;

    // Validate request
    payload.validate().map_err(ApiError::from)?;

    // Extract client context from headers for audit logging
    let client_ip = extract_client_ip(&headers);
    let user_agent = extract_user_agent(&headers);

    // Get database pool
    let pool = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?
        .pool()
        .clone();

    // Create login service
    let login_service = LoginService::with_sqlx(pool.clone());

    // Perform login
    let login_request = LoginRequest { email: payload.email, password: payload.password };
    let (user, scopes) = login_service
        .login(&login_request, client_ip.clone(), user_agent.clone())
        .await
        .map_err(ApiError::from)?;

    // Create session service
    let session_service = session_service_for_state(&state)?;

    // Create session from user authentication
    let session_response = session_service
        .create_session_from_user(&user.id, &user.email, scopes.clone(), client_ip, user_agent)
        .await
        .map_err(ApiError::from)?;

    // Extract teams from scopes
    let teams: Vec<String> = crate::auth::session::extract_teams_from_scopes(&scopes);

    // Build secure session cookie
    // Note: .secure(false) allows cookie to work over HTTP in development
    // In production, this should be set to true and use HTTPS
    let cookie = Cookie::build((SESSION_COOKIE_NAME, session_response.session_token.clone()))
        .path("/")
        .http_only(true)
        .secure(false) // Allow HTTP in development
        .same_site(SameSite::Lax) // Lax instead of Strict for cross-site navigation
        .expires(
            time::OffsetDateTime::from_unix_timestamp(session_response.expires_at.timestamp()).ok(),
        )
        .into();

    // Extract org info: use user's org_id directly (scopes only contain org name, not ID)
    let (_, org_name) = crate::auth::session::extract_org_from_scopes(&scopes);
    let org_id = Some(user.org_id.to_string());

    let response_body = LoginResponseBody {
        session_id: session_response.session_id,
        csrf_token: session_response.csrf_token.clone(),
        expires_at: session_response.expires_at,
        user_id: user.id.to_string(),
        user_email: user.email,
        teams,
        scopes,
        org_id,
        org_name,
    };

    Ok(LoginResponse { body: response_body, cookie, csrf_token: session_response.csrf_token })
}

// Password Change Endpoint

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordBody {
    #[validate(length(min = 1))]
    pub current_password: String,
    #[validate(length(min = 8, message = "New password must be at least 8 characters"))]
    pub new_password: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/change-password",
    request_body = ChangePasswordBody,
    responses(
        (status = 204, description = "Password changed successfully"),
        (status = 400, description = "Invalid request format or password validation failed"),
        (status = 401, description = "Current password is incorrect or user not authenticated"),
        (status = 503, description = "Service unavailable")
    ),
    security(("session" = [])),
    tag = "Authentication"
)]
#[instrument(skip(state, payload), fields(user_id = ?context.user_id))]
pub async fn change_password_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<ChangePasswordBody>,
) -> Result<StatusCode, ApiError> {
    use crate::auth::user_service::UserService;

    // Validate request
    payload.validate().map_err(ApiError::from)?;

    // Ensure user is authenticated via session (not PAT)
    let user_id_str = context
        .user_id
        .as_ref()
        .ok_or_else(|| ApiError::unauthorized("Password change requires session authentication"))?;

    let user_id = UserId::from_string(user_id_str.to_string());

    // Get database pool
    let pool = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?
        .pool()
        .clone();

    // Create user service
    let user_repo = Arc::new(SqlxUserRepository::new(pool.clone()));
    let membership_repo = Arc::new(
        crate::storage::repositories::user::SqlxTeamMembershipRepository::new(pool.clone()),
    );
    let audit_repo = Arc::new(AuditLogRepository::new(pool));
    let user_service = UserService::new(user_repo, membership_repo, audit_repo);

    // Change password with verification
    user_service
        .change_password_with_verification(
            &user_id,
            payload.current_password,
            payload.new_password,
            Some(&context),
        )
        .await
        .map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, Query, State};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Extension;
    use chrono::{Duration, Utc};

    use crate::api::test_utils::{
        admin_auth_context, create_test_state, minimal_auth_context, readonly_resource_auth_context,
    };

    // Use test_utils helpers for auth contexts:
    // - admin_auth_context() -> admin_auth_context()
    // - resource_auth_context("tokens") -> resource_auth_context("tokens")
    // - readonly_resource_auth_context("tokens") -> readonly_resource_auth_context("tokens")
    // - minimal_auth_context() -> minimal_auth_context()

    fn sample_create_token_body() -> CreateTokenBody {
        CreateTokenBody {
            name: "test-token".to_string(),
            description: Some("A test token".to_string()),
            expires_at: Some(Utc::now() + Duration::days(30)),
            scopes: vec!["clusters:read".to_string(), "routes:read".to_string()],
        }
    }

    // === Token Handler Tests ===

    #[tokio::test]
    async fn test_create_token_with_admin_auth_context() {
        let (_db, state) = create_test_state().await;
        let body = sample_create_token_body();

        let result =
            create_token_handler(State(state), Extension(admin_auth_context()), Json(body)).await;

        assert!(result.is_ok());
        let (status, Json(response)) = result.unwrap();
        assert_eq!(status, StatusCode::CREATED);
        // TokenSecretResponse has id (token id) and token (the secret)
        assert!(!response.id.is_empty());
        assert!(!response.token.is_empty());
        assert!(response.token.starts_with("fp_"));
    }

    #[tokio::test]
    async fn test_create_token_with_tokens_write_scope() {
        let (_db, state) = create_test_state().await;
        let body = sample_create_token_body();

        // Use admin context: global resource scopes like "tokens:write" and "clusters:read"
        // are restricted to platform admins. Admin can grant any scope.
        let context = AuthContext::new(
            crate::domain::TokenId::new(),
            "tokens-test-token".into(),
            vec![
                "admin:all".into(),
                "tokens:read".into(),
                "tokens:write".into(),
                "clusters:read".into(),
                "routes:read".into(),
            ],
        );

        let result = create_token_handler(State(state), Extension(context), Json(body)).await;

        assert!(result.is_ok());
        let (status, _) = result.unwrap();
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_create_token_fails_without_write_scope() {
        let (_db, state) = create_test_state().await;
        let body = sample_create_token_body();

        let result = create_token_handler(
            State(state),
            Extension(readonly_resource_auth_context("tokens")),
            Json(body),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_create_token_fails_with_no_permissions() {
        let (_db, state) = create_test_state().await;
        let body = sample_create_token_body();

        let result =
            create_token_handler(State(state), Extension(minimal_auth_context()), Json(body)).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_create_token_validates_name_length() {
        let (_db, state) = create_test_state().await;
        let mut body = sample_create_token_body();
        body.name = "ab".to_string(); // Too short (min is 3)

        let result =
            create_token_handler(State(state), Extension(admin_auth_context()), Json(body)).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_token_validates_empty_scopes() {
        let (_db, state) = create_test_state().await;
        let mut body = sample_create_token_body();
        body.scopes = vec![]; // Empty scopes (min is 1)

        let result =
            create_token_handler(State(state), Extension(admin_auth_context()), Json(body)).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_list_tokens_returns_created_tokens() {
        let (_db, state) = create_test_state().await;

        // Create a token first
        let body = sample_create_token_body();
        let _ =
            create_token_handler(State(state.clone()), Extension(admin_auth_context()), Json(body))
                .await
                .expect("create token");

        // List tokens
        let result = list_tokens_handler(
            State(state),
            Extension(admin_auth_context()),
            Query(PaginationQuery { limit: 50, offset: 0 }),
        )
        .await;

        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(!resp.items.is_empty());
    }

    #[tokio::test]
    async fn test_list_tokens_requires_read_scope() {
        let (_db, state) = create_test_state().await;

        let result = list_tokens_handler(
            State(state),
            Extension(minimal_auth_context()),
            Query(PaginationQuery { limit: 50, offset: 0 }),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_list_tokens_with_pagination() {
        let (_db, state) = create_test_state().await;

        // Create multiple tokens
        for i in 0..5 {
            let mut body = sample_create_token_body();
            body.name = format!("test-token-{}", i);
            let _ = create_token_handler(
                State(state.clone()),
                Extension(admin_auth_context()),
                Json(body),
            )
            .await
            .expect("create token");
        }

        // List with limit
        let result = list_tokens_handler(
            State(state),
            Extension(admin_auth_context()),
            Query(PaginationQuery { limit: 2, offset: 0 }),
        )
        .await;

        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.items.len(), 2);
    }

    #[tokio::test]
    async fn test_get_token_returns_token_details() {
        let (_db, state) = create_test_state().await;

        // Create a token
        let body = sample_create_token_body();
        let (_, Json(created)) =
            create_token_handler(State(state.clone()), Extension(admin_auth_context()), Json(body))
                .await
                .expect("create token");

        // Get the token using the id from the response
        let result = get_token_handler(
            State(state),
            Extension(admin_auth_context()),
            Path(created.id.clone()),
        )
        .await;

        assert!(result.is_ok());
        let Json(token) = result.unwrap();
        assert_eq!(token.name, "test-token");
        assert_eq!(token.id.to_string(), created.id);
    }

    #[tokio::test]
    async fn test_get_token_not_found() {
        let (_db, state) = create_test_state().await;

        let result = get_token_handler(
            State(state),
            Extension(admin_auth_context()),
            Path("non-existent-token-id".to_string()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_update_token_changes_name() {
        let (_db, state) = create_test_state().await;

        // Create a token
        let body = sample_create_token_body();
        let (_, Json(created)) =
            create_token_handler(State(state.clone()), Extension(admin_auth_context()), Json(body))
                .await
                .expect("create token");

        // Update the token
        let update_body = UpdateTokenBody {
            name: Some("updated-token-name".to_string()),
            description: None,
            status: None,
            expires_at: None,
            scopes: None,
        };

        let result = update_token_handler(
            State(state),
            Extension(admin_auth_context()),
            Path(created.id.clone()),
            Json(update_body),
        )
        .await;

        assert!(result.is_ok());
        let Json(token) = result.unwrap();
        assert_eq!(token.name, "updated-token-name");
    }

    #[tokio::test]
    async fn test_update_token_requires_write_scope() {
        let (_db, state) = create_test_state().await;

        // Create a token
        let body = sample_create_token_body();
        let (_, Json(created)) =
            create_token_handler(State(state.clone()), Extension(admin_auth_context()), Json(body))
                .await
                .expect("create token");

        // Try to update with readonly context
        let update_body = UpdateTokenBody::default();

        let result = update_token_handler(
            State(state),
            Extension(readonly_resource_auth_context("tokens")),
            Path(created.id.clone()),
            Json(update_body),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_revoke_token_changes_status() {
        let (_db, state) = create_test_state().await;

        // Create a token
        let body = sample_create_token_body();
        let (_, Json(created)) =
            create_token_handler(State(state.clone()), Extension(admin_auth_context()), Json(body))
                .await
                .expect("create token");

        // Revoke the token
        let result = revoke_token_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Path(created.id.clone()),
        )
        .await;

        assert!(result.is_ok());
        let Json(token) = result.unwrap();
        assert_eq!(token.status.as_str(), "revoked");

        // Verify we can still get the revoked token
        let get_result = get_token_handler(
            State(state),
            Extension(admin_auth_context()),
            Path(created.id.clone()),
        )
        .await;

        assert!(get_result.is_ok());
        let Json(fetched) = get_result.unwrap();
        assert_eq!(fetched.status.as_str(), "revoked");
    }

    #[tokio::test]
    async fn test_rotate_token_returns_new_secret() {
        let (_db, state) = create_test_state().await;

        // Create a token
        let body = sample_create_token_body();
        let (_, Json(created)) =
            create_token_handler(State(state.clone()), Extension(admin_auth_context()), Json(body))
                .await
                .expect("create token");

        let original_secret = created.token.clone();

        // Rotate the token
        let result = rotate_token_handler(
            State(state),
            Extension(admin_auth_context()),
            Path(created.id.clone()),
        )
        .await;

        assert!(result.is_ok());
        let Json(rotated) = result.unwrap();
        assert!(!rotated.token.is_empty());
        assert_ne!(rotated.token, original_secret);
        assert!(rotated.token.starts_with("fp_"));
    }

    #[tokio::test]
    async fn test_rotate_token_requires_write_scope() {
        let (_db, state) = create_test_state().await;

        // Create a token
        let body = sample_create_token_body();
        let (_, Json(created)) =
            create_token_handler(State(state.clone()), Extension(admin_auth_context()), Json(body))
                .await
                .expect("create token");

        // Try to rotate with readonly context
        let result = rotate_token_handler(
            State(state),
            Extension(readonly_resource_auth_context("tokens")),
            Path(created.id.clone()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // === Helper Function Tests ===

    #[test]
    fn test_extract_client_ip_from_x_forwarded_for() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "192.168.1.1, 10.0.0.1".parse().unwrap());

        let ip = extract_client_ip(&headers);
        assert_eq!(ip, Some("192.168.1.1".to_string()));
    }

    #[test]
    fn test_extract_client_ip_single_ip() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "192.168.1.1".parse().unwrap());

        let ip = extract_client_ip(&headers);
        assert_eq!(ip, Some("192.168.1.1".to_string()));
    }

    #[test]
    fn test_extract_client_ip_no_header() {
        let headers = axum::http::HeaderMap::new();

        let ip = extract_client_ip(&headers);
        assert_eq!(ip, None);
    }

    #[test]
    fn test_extract_user_agent() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(axum::http::header::USER_AGENT, "Mozilla/5.0 (Test)".parse().unwrap());

        let ua = extract_user_agent(&headers);
        assert_eq!(ua, Some("Mozilla/5.0 (Test)".to_string()));
    }

    #[test]
    fn test_extract_user_agent_no_header() {
        let headers = axum::http::HeaderMap::new();

        let ua = extract_user_agent(&headers);
        assert_eq!(ua, None);
    }

    // === CreateTokenBody Tests ===

    #[test]
    fn test_create_token_body_into_request() {
        let body = CreateTokenBody {
            name: "my-token".to_string(),
            description: Some("Test description".to_string()),
            expires_at: None,
            scopes: vec!["clusters:read".to_string()],
        };

        // Create a context with user_id
        let mut context = admin_auth_context();
        context.user_id = Some(UserId::from_string("user-123".to_string()));
        context.user_email = Some("test@example.com".to_string());

        let request = body.into_request(&context);

        assert_eq!(request.name, "my-token");
        assert_eq!(request.description, Some("Test description".to_string()));
        assert_eq!(request.scopes, vec!["clusters:read".to_string()]);
        assert_eq!(request.created_by, Some("user:user-123".to_string()));
        assert_eq!(request.user_id, Some(UserId::from_string("user-123".to_string())));
        assert_eq!(request.user_email, Some("test@example.com".to_string()));
    }

    #[test]
    fn test_update_token_body_into_request() {
        let body = UpdateTokenBody {
            name: Some("new-name".to_string()),
            description: Some("new-description".to_string()),
            status: Some("active".to_string()),
            expires_at: None,
            scopes: Some(vec!["clusters:write".to_string()]),
        };

        let request = body.into_request();

        assert_eq!(request.name, Some("new-name".to_string()));
        assert_eq!(request.description, Some("new-description".to_string()));
        assert_eq!(request.status, Some("active".to_string()));
        assert_eq!(request.scopes, Some(vec!["clusters:write".to_string()]));
    }
}
