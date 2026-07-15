//! The CP-facing HTTP admin server: receives the full policy-set push (S5) and serves health.
//!
//! `POST /api/v1/admin/rls/policies` replaces the enforced set. The push is a full snapshot, so
//! this is idempotent and self-healing under the CP's 60 s reconcile (design pillar 4).
//!
//! Auth (fpv2-9sf S2): when a credential is configured, **every** route except the open health
//! allowlist requires `Authorization: Bearer <token>` — enforcement is a router-wide
//! middleware, deliberately default-deny, so a future route cannot ship unauthenticated by
//! forgetting a per-handler check. The token comparison is constant-time.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use subtle::ConstantTimeEq;

use crate::config::AdminCredential;
use crate::policy::{PolicyCache, PolicyPush};

/// The single shared route inventory: every admin route is declared here with its auth class.
/// `router()` is built FROM this list and the parity test asserts against it, so a route added
/// anywhere else is structurally impossible, and flipping a protected route open is a visible
/// one-line diff in this table.
pub const ROUTES: &[(&str, AuthClass)] = &[
    ("/api/v1/admin/rls/policies", AuthClass::Protected),
    ("/healthz", AuthClass::Open),
    ("/readyz", AuthClass::Open),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthClass {
    /// Requires `Authorization: Bearer <token>` when a credential is configured.
    Protected,
    /// Health probes: always open.
    Open,
}

#[derive(Clone)]
pub struct AdminState {
    pub policies: Arc<PolicyCache>,
    /// `None` => open dev admin (config layer gates this behind the loopback escape hatch).
    pub credential: Option<Arc<AdminCredential>>,
}

pub fn router(state: AdminState) -> Router {
    let mut router = Router::new();
    for (path, _) in ROUTES {
        router = match *path {
            "/api/v1/admin/rls/policies" => router.route(path, post(push_policies)),
            "/healthz" => router.route(path, get(healthz)),
            "/readyz" => router.route(path, get(readyz)),
            other => unreachable!("route {other} declared in ROUTES without a handler"),
        };
    }
    router
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ))
        .with_state(state)
}

/// Default-deny bearer middleware: with a configured credential, ONLY paths declared
/// `AuthClass::Open` in [`ROUTES`] bypass auth — anything else (including unknown paths)
/// needs the exact token. 401 on absent, malformed, non-UTF-8, or mismatched credentials;
/// the comparison is constant-time to avoid an equality-timing oracle.
async fn require_bearer(
    State(state): State<AdminState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(credential) = &state.credential else {
        return Ok(next.run(request).await);
    };
    let open = ROUTES
        .iter()
        .any(|(path, class)| *class == AuthClass::Open && *path == request.uri().path());
    if open {
        return Ok(next.run(request).await);
    }
    let presented = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match presented {
        Some(token) if bool::from(token.as_bytes().ct_eq(credential.secret())) => {
            Ok(next.run(request).await)
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn push_policies(
    State(state): State<AdminState>,
    Json(push): Json<PolicyPush>,
) -> StatusCode {
    let count = push.policies.len();
    state.policies.replace(push);
    tracing::info!(policies = count, "applied CP rate-limit policy push");
    StatusCode::NO_CONTENT
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// Ready as soon as the process is up: an empty/never-synced policy set is a valid state — it
/// simply matches nothing, so nothing is limited (design pillar 4, staleness).
async fn readyz() -> StatusCode {
    StatusCode::OK
}
