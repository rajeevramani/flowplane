//! Axum middleware for authentication and authorization.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Extension, State},
    http::{header::AUTHORIZATION, Method, Request},
    middleware::Next,
    response::Response,
};

use crate::api::error::ApiError;
use crate::auth::auth_service::AuthService;
use crate::auth::authorization::{action_from_http_method, require_resource_access, resource_from_path};
use crate::auth::models::{AuthContext, AuthError};
use tracing::{field, info_span, warn};

pub type AuthServiceState = Arc<AuthService>;
pub type ScopeState = Arc<Vec<String>>;

/// Middleware entry point that authenticates requests using the configured [`AuthService`].
pub async fn authenticate(
    State(auth_service): State<AuthServiceState>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    if request.method() == Method::OPTIONS {
        return Ok(next.run(request).await);
    }

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let correlation_id = uuid::Uuid::new_v4();
    let span = info_span!(
        "auth_middleware.authenticate",
        http.method = %method,
        http.path = %path,
        auth.token_id = field::Empty,
        correlation_id = %correlation_id
    );
    let _guard = span.enter();

    let header =
        request.headers().get(AUTHORIZATION).and_then(|value| value.to_str().ok()).unwrap_or("");

    match auth_service.authenticate(header).await {
        Ok(context) => {
            tracing::Span::current().record("auth.token_id", field::display(&context.token_id));
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
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let span = info_span!(
        "auth_middleware.ensure_scopes",
        http.method = %method,
        http.path = %path,
        auth.token_id = %context.token_id,
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
/// 1. Extracts the resource from the path (e.g., `/api/v1/routes` → "routes")
/// 2. Derives the action from the HTTP method (e.g., GET → "read", POST → "write")
/// 3. Checks permissions using `require_resource_access`
///
/// # Examples
///
/// - GET /api/v1/routes → requires "routes:read"
/// - POST /api/v1/clusters → requires "clusters:write"
/// - DELETE /api/v1/listeners/foo → requires "listeners:delete"
pub async fn ensure_dynamic_scopes(
    Extension(context): Extension<AuthContext>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let correlation_id = uuid::Uuid::new_v4();

    // Extract resource from path
    let resource = match resource_from_path(&path) {
        Some(r) => r,
        None => {
            // Path doesn't match expected pattern, allow request to continue
            // (e.g., /health, /docs, etc.)
            return Ok(next.run(request).await);
        }
    };

    // Derive action from HTTP method
    let action = action_from_http_method(method.as_str());

    let span = info_span!(
        "auth_middleware.ensure_dynamic_scopes",
        http.method = %method,
        http.path = %path,
        auth.token_id = %context.token_id,
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
