//! Invitation API handlers for invite-only registration.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::Deserialize;
use tracing::instrument;
use utoipa::IntoParams;
use validator::Validate;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::api::util::{extract_client_ip, extract_user_agent};
use crate::auth::authorization::require_org_admin;
use crate::auth::invitation::{
    AcceptInvitationRequest, CreateInvitationRequest, CreateInvitationResponse, InviteTokenInfo,
    PaginatedInvitations,
};
use crate::auth::invitation_service::InvitationService;
use crate::auth::models::AuthContext;
use crate::auth::session::{SessionService, SESSION_COOKIE_NAME};
use crate::domain::InvitationId;
use crate::errors::Error;
use crate::storage::repositories::{OrganizationRepository, SqlxOrganizationRepository};
use crate::storage::repository::AuditLogRepository;

fn convert_error(err: Error) -> ApiError {
    ApiError::from(err)
}

fn get_db_pool(state: &ApiState) -> Result<crate::storage::DbPool, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database unavailable"))?;
    Ok(cluster_repo.pool().clone())
}

fn session_service_for_state(state: &ApiState) -> Result<SessionService, ApiError> {
    let pool = get_db_pool(state)?;
    let token_repo = Arc::new(crate::storage::repository::SqlxTokenRepository::new(pool.clone()));
    let audit_repository = Arc::new(AuditLogRepository::new(pool));
    Ok(SessionService::new(token_repo, audit_repository))
}

fn invitation_service_for_state(state: &ApiState) -> Result<InvitationService, ApiError> {
    let pool = get_db_pool(state)?;
    Ok(InvitationService::with_sqlx(
        pool,
        state.auth_config.invite_expiry_hours,
        state.auth_config.base_url.clone(),
    ))
}

async fn resolve_org_id(
    state: &ApiState,
    org_name: &str,
) -> Result<crate::domain::OrgId, ApiError> {
    let pool = get_db_pool(state)?;
    let org_repo = SqlxOrganizationRepository::new(pool);
    let org = org_repo
        .get_organization_by_name(org_name)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| ApiError::NotFound(format!("Organization '{}' not found", org_name)))?;
    Ok(org.id)
}

// --- Query params ---

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListInvitationsQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ValidateTokenQuery {
    pub token: String,
}

// --- Org path params ---

#[derive(Debug, Deserialize)]
pub struct OrgInvitationPath {
    pub org_name: String,
    pub id: String,
}

// --- Secured endpoints (org admin) ---

#[utoipa::path(
    post,
    path = "/api/v1/orgs/{org_name}/invitations",
    request_body = CreateInvitationRequest,
    params(
        ("org_name" = String, Path, description = "Organization name")
    ),
    responses(
        (status = 201, description = "Invitation created", body = CreateInvitationResponse),
        (status = 400, description = "Invalid request or duplicate pending invitation"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Organization not found"),
        (status = 409, description = "Duplicate pending invitation"),
        (status = 429, description = "Rate limit exceeded"),
        (status = 503, description = "Service unavailable")
    ),
    security(("session" = [])),
    tag = "Invitations"
)]
#[instrument(skip(state, payload, headers), fields(org_name = %org_name, email = %payload.email))]
pub async fn create_invitation_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(org_name): Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CreateInvitationRequest>,
) -> Result<(StatusCode, Json<CreateInvitationResponse>), ApiError> {
    // Rate limit: 20/hour per IP
    let client_ip =
        extract_client_ip(&headers, &state.auth_config).unwrap_or_else(|| "unknown".to_string());
    if let Err(retry_after) =
        state.auth_rate_limiters.invite_create.check_rate_limit(&client_ip).await
    {
        return Err(ApiError::rate_limited("Too many invitation requests", retry_after));
    }

    // Validate request
    payload.validate().map_err(|err| convert_error(Error::from(err)))?;

    // Auth: org admin or platform admin
    require_org_admin(&context, &org_name).map_err(|_| {
        ApiError::forbidden("Organization admin access required to create invitations")
    })?;

    // Resolve org_name -> org_id
    let org_id = resolve_org_id(&state, &org_name).await?;

    let user_agent = extract_user_agent(&headers);
    let service = invitation_service_for_state(&state)?;

    let response = service
        .create_invitation(
            &context,
            &org_id,
            &payload.email,
            payload.role,
            Some(client_ip),
            user_agent,
        )
        .await
        .map_err(convert_error)?;

    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/orgs/{org_name}/invitations",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ListInvitationsQuery,
    ),
    responses(
        (status = 200, description = "Invitation list", body = PaginatedInvitations),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Organization not found"),
        (status = 503, description = "Service unavailable")
    ),
    security(("session" = [])),
    tag = "Invitations"
)]
#[instrument(skip(state), fields(org_name = %org_name))]
pub async fn list_invitations_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(org_name): Path<String>,
    Query(query): Query<ListInvitationsQuery>,
) -> Result<Json<PaginatedInvitations>, ApiError> {
    // Auth: org admin or platform admin
    require_org_admin(&context, &org_name).map_err(|_| {
        ApiError::forbidden("Organization admin access required to list invitations")
    })?;

    let org_id = resolve_org_id(&state, &org_name).await?;
    let service = invitation_service_for_state(&state)?;

    let result = service
        .list_invitations(&org_id, query.limit, query.offset)
        .await
        .map_err(convert_error)?;

    Ok(Json(result))
}

#[utoipa::path(
    delete,
    path = "/api/v1/orgs/{org_name}/invitations/{id}",
    params(
        ("org_name" = String, Path, description = "Organization name"),
        ("id" = String, Path, description = "Invitation ID"),
    ),
    responses(
        (status = 204, description = "Invitation revoked"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Invitation not found"),
        (status = 503, description = "Service unavailable")
    ),
    security(("session" = [])),
    tag = "Invitations"
)]
#[instrument(skip(state, headers), fields(org_name = %path.org_name, invitation_id = %path.id))]
pub async fn revoke_invitation_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(path): Path<OrgInvitationPath>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, ApiError> {
    // Auth: org admin or platform admin
    require_org_admin(&context, &path.org_name).map_err(|_| {
        ApiError::forbidden("Organization admin access required to revoke invitations")
    })?;

    let org_id = resolve_org_id(&state, &path.org_name).await?;
    let invitation_id = InvitationId::from_string(path.id);

    let client_ip = extract_client_ip(&headers, &state.auth_config);
    let user_agent = extract_user_agent(&headers);
    let service = invitation_service_for_state(&state)?;

    service
        .revoke_invitation(&context, &invitation_id, &org_id, client_ip, user_agent)
        .await
        .map_err(convert_error)?;

    Ok(StatusCode::NO_CONTENT)
}

// --- Public endpoints (registration flow) ---

#[utoipa::path(
    get,
    path = "/api/v1/invitations/validate",
    params(ValidateTokenQuery),
    responses(
        (status = 200, description = "Token is valid", body = InviteTokenInfo),
        (status = 400, description = "Invalid or expired invitation"),
        (status = 429, description = "Rate limit exceeded"),
        (status = 503, description = "Service unavailable")
    ),
    tag = "Invitations"
)]
#[instrument(skip(state, query, headers))]
pub async fn validate_invitation_handler(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<ValidateTokenQuery>,
) -> Result<Json<InviteTokenInfo>, ApiError> {
    // Rate limit: 10/min per IP
    let client_ip =
        extract_client_ip(&headers, &state.auth_config).unwrap_or_else(|| "unknown".to_string());
    if let Err(retry_after) =
        state.auth_rate_limiters.invite_validate.check_rate_limit(&client_ip).await
    {
        return Err(ApiError::rate_limited("Too many validation requests", retry_after));
    }

    let service = invitation_service_for_state(&state)?;
    let info = service.validate_invite_token(&query.token).await.map_err(convert_error)?;

    Ok(Json(info))
}

/// Accept invitation response — wraps LoginResponseBody with session cookie + CSRF.
pub struct AcceptInvitationResponse {
    body: crate::api::handlers::auth::LoginResponseBody,
    cookie: Cookie<'static>,
    csrf_token: String,
}

impl IntoResponse for AcceptInvitationResponse {
    fn into_response(self) -> Response {
        let mut response = (StatusCode::CREATED, Json(self.body)).into_response();

        if let Ok(cookie_value) = self.cookie.to_string().parse() {
            response.headers_mut().insert(header::SET_COOKIE, cookie_value);
        }

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
    path = "/api/v1/invitations/accept",
    request_body = AcceptInvitationRequest,
    responses(
        (status = 201, description = "Registration successful", body = crate::api::handlers::auth::LoginResponseBody,
         headers(
             ("Set-Cookie" = String, description = "Session cookie (fp_session)"),
             ("X-CSRF-Token" = String, description = "CSRF token for state-changing requests")
         )
        ),
        (status = 400, description = "Invalid request or token"),
        (status = 401, description = "Invalid or expired invitation"),
        (status = 409, description = "Email already registered"),
        (status = 429, description = "Rate limit exceeded"),
        (status = 503, description = "Service unavailable")
    ),
    tag = "Invitations"
)]
#[instrument(skip(state, payload, headers))]
pub async fn accept_invitation_handler(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<AcceptInvitationRequest>,
) -> Result<AcceptInvitationResponse, ApiError> {
    // Rate limit: 5/min per IP
    let client_ip =
        extract_client_ip(&headers, &state.auth_config).unwrap_or_else(|| "unknown".to_string());
    if let Err(retry_after) =
        state.auth_rate_limiters.invite_accept.check_rate_limit(&client_ip).await
    {
        return Err(ApiError::rate_limited("Too many registration attempts", retry_after));
    }

    // Validate request fields (name, password)
    payload.validate().map_err(|err| convert_error(Error::from(err)))?;

    let user_agent = extract_user_agent(&headers);
    let service = invitation_service_for_state(&state)?;

    // Accept invitation — creates user + org membership
    // Note: CSRF protection not required here — the invite token (64 bytes / 512 bits entropy)
    // acts as the anti-CSRF measure. An attacker cannot forge a valid accept request without
    // knowing the token, which is only shared via the invite URL hash fragment.
    let (user, scopes) = service
        .accept_invitation(
            &payload.token,
            &payload.name,
            &payload.password,
            Some(client_ip),
            user_agent,
        )
        .await
        .map_err(convert_error)?;

    // Create session (same pattern as login_handler)
    let session_service = session_service_for_state(&state)?;
    let session_response = session_service
        .create_session_from_user(&user.id, &user.email, scopes.clone(), None, None)
        .await
        .map_err(convert_error)?;

    // Extract teams from scopes
    let teams: Vec<String> = crate::auth::session::extract_teams_from_scopes(&scopes);

    // Build session cookie — SameSite::Lax allows cross-site navigation from email clients
    let cookie = Cookie::build((SESSION_COOKIE_NAME, session_response.session_token.clone()))
        .path("/")
        .http_only(true)
        .secure(state.auth_config.cookie_secure)
        .same_site(SameSite::Lax)
        .expires(
            time::OffsetDateTime::from_unix_timestamp(session_response.expires_at.timestamp()).ok(),
        )
        .into();

    // Extract org info from scopes
    let (_, org_name) = crate::auth::session::extract_org_from_scopes(&scopes);
    let org_id = Some(user.org_id.to_string());

    let response_body = crate::api::handlers::auth::LoginResponseBody {
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

    Ok(AcceptInvitationResponse {
        body: response_body,
        cookie,
        csrf_token: session_response.csrf_token,
    })
}
