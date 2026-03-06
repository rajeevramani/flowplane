//! Axum middleware for authentication and authorization.

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::{ConnectInfo, Extension, State},
    http::{header::AUTHORIZATION, header::USER_AGENT, Method, Request},
    middleware::Next,
    response::Response,
};

use crate::api::error::ApiError;
use crate::auth::authorization::{
    action_from_request, require_resource_access, resource_from_path,
};
use crate::auth::cache::{CachedPermissionSnapshot, CachedPermissions};
use crate::auth::models::{AgentContext, AuthContext, AuthError};
use crate::auth::permissions::load_permissions;
use crate::auth::zitadel::{fetch_user_email, validate_jwt_extract_sub, ZitadelAuthState};
use crate::domain::TokenId;
use crate::storage::repositories::user::SqlxUserRepository;
use crate::storage::repositories::UserRepository;
use tracing::{field, info_span, warn};

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

/// Middleware entry point that authenticates requests using Zitadel JWT validation.
///
/// All API requests must include a valid Zitadel JWT in the `Authorization: Bearer <token>`
/// header. The JWT is validated against the Zitadel JWKS endpoint, and role claims are
/// mapped into Flowplane's `AuthContext` scopes.
pub async fn authenticate(
    State(state): State<ZitadelAuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    // Pass through OPTIONS (CORS preflight)
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

    // Extract Bearer token
    let header =
        request.headers().get(AUTHORIZATION).and_then(|value| value.to_str().ok()).unwrap_or("");

    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("bearer token required"))?;

    // Validate JWT — extract sub, email, name only (no role parsing)
    let jwt_claims =
        validate_jwt_extract_sub(token, &state.config, &state.jwks_cache).await.map_err(|e| {
            warn!(%correlation_id, error = ?e, "JWT authentication failed");
            e
        })?;

    // Resolve user_id, permissions, and org context from cache or DB
    let snapshot: CachedPermissionSnapshot =
        if let Some(cached) = state.permission_cache.get(&jwt_claims.sub).await {
            cached
        } else {
            // JIT provision / update user from JWT claims
            let user_repo = SqlxUserRepository::new(state.pool.clone());
            let (mut email, mut name) =
                (jwt_claims.email.unwrap_or_default(), jwt_claims.name.unwrap_or_default());

            // Zitadel access tokens often omit email — fall back to userinfo endpoint
            if email.is_empty() {
                if let Some((ui_email, ui_name)) =
                    fetch_user_email(token, &state.config, state.jwks_cache.issuer_host()).await
                {
                    email = ui_email;
                    if let Some(n) = ui_name {
                        if name.is_empty() {
                            name = n;
                        }
                    }
                }
            }

            let user =
                user_repo.upsert_from_jwt(&jwt_claims.sub, &email, &name).await.map_err(|e| {
                    warn!(%correlation_id, error = ?e, "Failed to upsert user from JWT");
                    ApiError::Internal(format!("user provisioning failed: {e}"))
                })?;

            // Single load_permissions call for ALL user types (human + machine).
            // Returns org_scopes + unified grants (resource, gateway-tool, route).
            let permissions = load_permissions(&state.pool, &user.id).await.map_err(|e| {
                warn!(%correlation_id, error = ?e, "Failed to load permissions");
                ApiError::Internal(format!("permission loading failed: {e}"))
            })?;

            // Parse agent_context for machine users
            let agent_ctx = if user.user_type == "machine" {
                AgentContext::from_db(user.agent_context.as_deref())
            } else {
                None
            };

            let snap = CachedPermissionSnapshot {
                user_id: user.id.clone(),
                email: Some(user.email.clone()),
                org_scopes: permissions.org_scopes.clone(),
                grants: permissions.grants.clone(),
                org_id: permissions.org_id.clone(),
                org_name: permissions.org_name.clone(),
                org_role: permissions.org_role.clone(),
                agent_context: agent_ctx,
            };

            state
                .permission_cache
                .insert(
                    jwt_claims.sub.clone(),
                    CachedPermissions {
                        org_scopes: permissions.org_scopes,
                        grants: permissions.grants,
                        user_id: user.id,
                        email: Some(user.email),
                        org_id: permissions.org_id,
                        org_name: permissions.org_name,
                        org_role: permissions.org_role,
                        cached_at: std::time::Instant::now(),
                        agent_context: agent_ctx,
                    },
                )
                .await;

            snap
        };

    // Build AuthContext from DB-sourced data
    let token_id = TokenId::from_string(format!("zitadel:{}", jwt_claims.sub));
    let token_name = format!("zitadel/{}", jwt_claims.sub);
    let mut context = AuthContext::with_user(
        token_id,
        token_name,
        snapshot.user_id,
        snapshot.email.unwrap_or_default(),
        snapshot.org_scopes.into_iter().collect(),
    )
    .with_grants(snapshot.grants, snapshot.agent_context);

    // Populate org context if the user belongs to a non-platform org
    if let (Some(oid), Some(oname)) = (snapshot.org_id, snapshot.org_name) {
        context = context.with_org(oid, oname);
    }

    // Enrich with request context
    let client_ip = extract_client_ip(&request);
    let user_agent = extract_user_agent(&request);
    context = context.with_request_context(client_ip, user_agent);

    // Record auth context in tracing span
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

/// Middleware that dynamically derives required scopes from the HTTP method and path.
///
/// This middleware automatically determines the resource and action from the request,
/// then checks if the authenticated user has the required permissions.
///
/// # How it works
///
/// 1. Extracts the resource from the path (e.g., `/api/v1/route-configs` -> "routes")
/// 2. Derives the action from the HTTP method (e.g., GET -> "read", POST -> "create", PUT/PATCH -> "update", DELETE -> "delete")
/// 3. Checks permissions using `require_resource_access`
///
/// # Examples
///
/// - GET /api/v1/route-configs -> requires "routes:read"
/// - POST /api/v1/clusters -> requires "clusters:create"
/// - DELETE /api/v1/listeners/foo -> requires "listeners:delete"
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
