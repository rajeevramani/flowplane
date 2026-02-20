//! Axum middleware for authentication and authorization.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ConnectInfo, Extension, State},
    http::{header::AUTHORIZATION, header::USER_AGENT, Method, Request},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;

use crate::api::error::ApiError;
use crate::auth::auth_service::AuthService;
use crate::auth::authorization::{
    action_from_request, extract_org_scopes, require_resource_access, resource_from_path,
};
use crate::auth::models::{AuthContext, AuthError};
use crate::auth::session::{SessionService, CSRF_HEADER_NAME, SESSION_COOKIE_NAME};
use crate::storage::repositories::{OrganizationRepository, SqlxOrganizationRepository};
use crate::storage::DbPool;
use tracing::{field, info_span, warn};

pub type AuthServiceState = Arc<AuthService>;
pub type SessionServiceState = Arc<SessionService>;
pub type ScopeState = Arc<Vec<String>>;

/// Helper to extract session token from cookies
fn extract_session_from_cookie(jar: &CookieJar) -> Option<String> {
    jar.get(SESSION_COOKIE_NAME).map(|cookie| cookie.value().to_string())
}

/// Helper to extract CSRF token from request headers
fn extract_csrf_from_header(request: &Request<Body>) -> Option<String> {
    request
        .headers()
        .get(CSRF_HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .map(|s| s.to_string())
}

/// Check if HTTP method requires CSRF validation
fn is_state_changing_method(method: &Method) -> bool {
    matches!(method, &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE)
}

/// Extract client IP from the request, preferring X-Forwarded-For header
fn extract_client_ip(request: &Request<Body>) -> Option<String> {
    // Try X-Forwarded-For header first (for proxied requests)
    if let Some(forwarded) = request.headers().get("x-forwarded-for") {
        if let Ok(value) = forwarded.to_str() {
            // X-Forwarded-For can contain multiple IPs; the first is the original client
            return value.split(',').next().map(|s| s.trim().to_string());
        }
    }

    // Fall back to ConnectInfo if available
    request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ci| ci.0.ip().to_string())
}

/// Extract User-Agent header from the request
fn extract_user_agent(request: &Request<Body>) -> Option<String> {
    request.headers().get(USER_AGENT).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
}

/// Middleware entry point that authenticates requests using the configured [`AuthService`] and [`SessionService`].
/// Supports both Bearer token and cookie-based authentication with CSRF validation.
pub async fn authenticate(
    State((auth_service, session_service, pool)): State<(
        AuthServiceState,
        SessionServiceState,
        DbPool,
    )>,
    jar: CookieJar,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    if request.method() == Method::OPTIONS {
        return Ok(next.run(request).await);
    }

    let method = request.method();
    let path = request.uri().path();
    let correlation_id = uuid::Uuid::new_v4();
    let span = info_span!(
        "auth_middleware.authenticate",
        http.method = %method,
        http.path = %path,
        auth.token_id = field::Empty,
        auth.org_id = field::Empty,
        auth.org_name = field::Empty,
        correlation_id = %correlation_id
    );
    let _guard = span.enter();

    // First, try Bearer token from Authorization header
    let header =
        request.headers().get(AUTHORIZATION).and_then(|value| value.to_str().ok()).unwrap_or("");

    let auth_context_result = if !header.is_empty() && header.starts_with("Bearer ") {
        // Check if it's a session token or PAT
        let token = header.strip_prefix("Bearer ").unwrap_or("");

        if token.starts_with("fp_session_") {
            // Session token via Bearer header
            // Validate CSRF for state-changing methods
            if is_state_changing_method(method) {
                let csrf_token = extract_csrf_from_header(&request);
                if csrf_token.is_none() {
                    warn!(%correlation_id, "CSRF token missing for state-changing request with session token");
                    return Err(ApiError::forbidden("CSRF token required for this operation"));
                }

                // Validate session and CSRF
                let session_info = session_service
                    .validate_session(token)
                    .await
                    .map_err(|e| ApiError::unauthorized(e.to_string()))?;

                // Validate CSRF token
                if let Some(csrf) = csrf_token {
                    session_service
                        .validate_csrf_token(&session_info.token.id, &csrf)
                        .await
                        .map_err(|_| ApiError::forbidden("Invalid CSRF token"))?;
                }

                // Use with_user if the token has user information, otherwise use new
                Ok(
                    if let (Some(user_id), Some(user_email)) =
                        (&session_info.token.user_id, &session_info.token.user_email)
                    {
                        AuthContext::with_user(
                            session_info.token.id,
                            session_info.token.name,
                            user_id.clone(),
                            user_email.clone(),
                            session_info.token.scopes,
                        )
                    } else {
                        AuthContext::new(
                            session_info.token.id,
                            session_info.token.name,
                            session_info.token.scopes,
                        )
                    },
                )
            } else {
                // GET requests don't need CSRF validation
                let session_info = session_service
                    .validate_session(token)
                    .await
                    .map_err(|e| ApiError::unauthorized(e.to_string()))?;

                // Use with_user if the token has user information, otherwise use new
                Ok(
                    if let (Some(user_id), Some(user_email)) =
                        (&session_info.token.user_id, &session_info.token.user_email)
                    {
                        AuthContext::with_user(
                            session_info.token.id,
                            session_info.token.name,
                            user_id.clone(),
                            user_email.clone(),
                            session_info.token.scopes,
                        )
                    } else {
                        AuthContext::new(
                            session_info.token.id,
                            session_info.token.name,
                            session_info.token.scopes,
                        )
                    },
                )
            }
        } else {
            // Regular PAT authentication - extract client context first
            let client_ip = extract_client_ip(&request);
            let user_agent = extract_user_agent(&request);
            auth_service.authenticate(header, client_ip, user_agent).await
        }
    } else if let Some(session_token) = extract_session_from_cookie(&jar) {
        // Authenticate using session cookie
        // Validate CSRF for state-changing methods
        if is_state_changing_method(method) {
            let csrf_token = extract_csrf_from_header(&request);
            if csrf_token.is_none() {
                warn!(%correlation_id, "CSRF token missing for state-changing request");
                return Err(ApiError::forbidden("CSRF token required for this operation"));
            }

            // Validate session and CSRF
            let session_info = session_service
                .validate_session(&session_token)
                .await
                .map_err(|e| ApiError::unauthorized(e.to_string()))?;

            // Validate CSRF token
            if let Some(csrf) = csrf_token {
                session_service
                    .validate_csrf_token(&session_info.token.id, &csrf)
                    .await
                    .map_err(|_| ApiError::forbidden("Invalid CSRF token"))?;
            }

            // Use with_user if the token has user information, otherwise use new
            Ok(
                if let (Some(user_id), Some(user_email)) =
                    (&session_info.token.user_id, &session_info.token.user_email)
                {
                    AuthContext::with_user(
                        session_info.token.id,
                        session_info.token.name,
                        user_id.clone(),
                        user_email.clone(),
                        session_info.token.scopes,
                    )
                } else {
                    AuthContext::new(
                        session_info.token.id,
                        session_info.token.name,
                        session_info.token.scopes,
                    )
                },
            )
        } else {
            // GET requests don't need CSRF validation
            let session_info = session_service
                .validate_session(&session_token)
                .await
                .map_err(|e| ApiError::unauthorized(e.to_string()))?;

            // Use with_user if the token has user information, otherwise use new
            Ok(
                if let (Some(user_id), Some(user_email)) =
                    (&session_info.token.user_id, &session_info.token.user_email)
                {
                    AuthContext::with_user(
                        session_info.token.id,
                        session_info.token.name,
                        user_id.clone(),
                        user_email.clone(),
                        session_info.token.scopes,
                    )
                } else {
                    AuthContext::new(
                        session_info.token.id,
                        session_info.token.name,
                        session_info.token.scopes,
                    )
                },
            )
        }
    } else {
        // No authentication credentials provided
        Err(AuthError::MissingBearer)
    };

    match auth_context_result {
        Ok(context) => {
            // Extract request context (client IP and user agent)
            let client_ip = extract_client_ip(&request);
            let user_agent = extract_user_agent(&request);

            // Add request context to auth context
            let mut context = context.with_request_context(client_ip, user_agent);

            // Populate org context from scopes (Phase 3.6)
            // SECURITY: Fail-closed — if a token has org scopes but the org cannot be
            // resolved, reject the request entirely. A token referencing a non-existent
            // org is invalid and should not be honored.
            let org_scopes = extract_org_scopes(&context);
            if let Some((org_name, _role)) = org_scopes.first() {
                let org_repo = SqlxOrganizationRepository::new(pool);
                match org_repo.get_organization_by_name(org_name).await {
                    Ok(Some(org)) => {
                        context = context.with_org(org.id, org.name);
                    }
                    Ok(None) => {
                        warn!(org_name = %org_name, "org scope references non-existent org, rejecting request");
                        return Ok(ApiError::Unauthorized(format!(
                            "Token references non-existent organization '{}'",
                            org_name
                        ))
                        .into_response());
                    }
                    Err(e) => {
                        warn!(org_name = %org_name, error = %e, "org lookup failed during authentication");
                        return Ok(ApiError::ServiceUnavailable(
                            "Organization validation temporarily unavailable".to_string(),
                        )
                        .into_response());
                    }
                }
            }

            let current_span = tracing::Span::current();
            current_span.record("auth.token_id", field::display(&context.token_id));
            if let Some(ref org_id) = context.org_id {
                current_span.record("auth.org_id", field::display(org_id));
            }
            if let Some(ref org_name) = context.org_name {
                current_span.record("auth.org_name", org_name.as_str());
            }
            request.extensions_mut().insert(context);
            Ok(next.run(request).await)
        }
        Err(err) => {
            warn!(%correlation_id, error = %err, "authentication failed");
            Err(map_auth_error(err))
        }
    }
}

/// Middleware entry point that verifies the caller has the required scopes.
pub async fn ensure_scopes(
    State(required_scopes): State<ScopeState>,
    Extension(context): Extension<AuthContext>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let required_summary =
        required_scopes.iter().map(|scope| scope.as_str()).collect::<Vec<_>>().join(" ");
    let granted_summary =
        context.scopes().map(|scope| scope.as_str()).collect::<Vec<_>>().join(" ");
    let correlation_id = uuid::Uuid::new_v4();
    let method = request.method();
    let path = request.uri().path();
    let span = info_span!(
        "auth_middleware.ensure_scopes",
        http.method = %method,
        http.path = %path,
        auth.token_id = %context.token_id,
        auth.org_id = context.org_id.as_ref().map(|id| id.to_string()).unwrap_or_default().as_str(),
        auth.org_name = context.org_name.as_deref().unwrap_or(""),
        required_scopes = %required_summary,
        correlation_id = %correlation_id
    );
    let _guard = span.enter();

    if required_scopes.iter().all(|scope| context.has_scope(scope)) {
        return Ok(next.run(request).await);
    }

    warn!(
        %correlation_id,
        required = %required_summary,
        granted = %granted_summary,
        "scope check failed"
    );
    Err(ApiError::forbidden("forbidden: missing required scope"))
}

/// Middleware that dynamically derives required scopes from the HTTP method and path.
///
/// This middleware automatically determines the resource and action from the request,
/// then checks if the authenticated user has the required permissions.
///
/// # How it works
///
/// 1. Extracts the resource from the path (e.g., `/api/v1/route-configs` → "routes")
/// 2. Derives the action from the HTTP method (e.g., GET → "read", POST → "write")
/// 3. Checks permissions using `require_resource_access`
///
/// # Examples
///
/// - GET /api/v1/route-configs → requires "routes:read"
/// - POST /api/v1/clusters → requires "clusters:write"
/// - DELETE /api/v1/listeners/foo → requires "listeners:delete"
pub async fn ensure_dynamic_scopes(
    Extension(context): Extension<AuthContext>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let method = request.method();
    let path = request.uri().path();
    let correlation_id = uuid::Uuid::new_v4();

    // Extract resource from path
    let resource = match resource_from_path(path) {
        Some(r) => r,
        None => {
            // Path doesn't match expected pattern, allow request to continue
            // (e.g., /health, /docs, etc.)
            return Ok(next.run(request).await);
        }
    };

    // Derive action from HTTP method and path (semantic action detection)
    let action = action_from_request(method.as_str(), path);

    let span = info_span!(
        "auth_middleware.ensure_dynamic_scopes",
        http.method = %method,
        http.path = %path,
        auth.token_id = %context.token_id,
        auth.org_id = context.org_id.as_ref().map(|id| id.to_string()).unwrap_or_default().as_str(),
        auth.org_name = context.org_name.as_deref().unwrap_or(""),
        resource = %resource,
        action = %action,
        correlation_id = %correlation_id
    );
    let _guard = span.enter();

    // Check if user has required permission
    // Note: team-scoped access will be checked at the handler level
    // where we have access to the actual team from the resource
    if let Err(err) = require_resource_access(&context, resource, action, None) {
        warn!(
            %correlation_id,
            resource = %resource,
            action = %action,
            token_id = %context.token_id,
            "dynamic scope check failed"
        );
        return Err(map_auth_error(err));
    }

    Ok(next.run(request).await)
}

fn map_auth_error(err: AuthError) -> ApiError {
    match err {
        AuthError::MissingBearer
        | AuthError::MalformedBearer
        | AuthError::TokenNotFound
        | AuthError::InactiveToken
        | AuthError::ExpiredToken => ApiError::unauthorized(err.to_string()),
        AuthError::Forbidden => ApiError::forbidden(err.to_string()),
        AuthError::Persistence(inner) => {
            ApiError::service_unavailable(format!("auth service unavailable: {}", inner))
        }
    }
}
