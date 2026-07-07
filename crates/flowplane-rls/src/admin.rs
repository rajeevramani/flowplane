//! The CP-facing HTTP admin server: receives the full policy-set push (S5) and serves health.
//!
//! `POST /api/v1/admin/rls/policies` replaces the enforced set. The push is a full snapshot, so
//! this is idempotent and self-healing under the CP's 60 s reconcile (design pillar 4).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::config::AdminCredential;
use crate::policy::{PolicyCache, PolicyPush};

#[derive(Clone)]
pub struct AdminState {
    pub policies: Arc<PolicyCache>,
    pub credential: Option<AdminCredential>,
}

pub fn router(state: AdminState) -> Router {
    Router::new()
        .route("/api/v1/admin/rls/policies", post(push_policies))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state)
}

async fn push_policies(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(push): Json<PolicyPush>,
) -> StatusCode {
    if let Err(status) = authorize_admin_request(&headers, state.credential.as_ref()) {
        return status;
    }
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

pub fn authorize_admin_request(
    headers: &HeaderMap,
    credential: Option<&AdminCredential>,
) -> Result<(), StatusCode> {
    let Some(credential) = credential else {
        return Ok(());
    };
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Ok(raw) = value.to_str() else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Some(token) = raw.strip_prefix("Bearer ") else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if token.is_empty() || !constant_time_eq(token.as_bytes(), credential.token().as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn credential() -> AdminCredential {
        AdminCredential::new("expected-token".to_string()).unwrap()
    }

    fn headers(value: Option<&'static str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert(
                axum::http::header::AUTHORIZATION,
                HeaderValue::from_static(value),
            );
        }
        headers
    }

    #[test]
    fn admin_authorizer_accepts_configured_bearer_credential() {
        assert_eq!(
            authorize_admin_request(&headers(Some("Bearer expected-token")), Some(&credential())),
            Ok(())
        );
    }

    #[test]
    fn admin_authorizer_rejects_missing_malformed_and_mismatched_credentials() {
        for header in [
            None,
            Some("expected-token"),
            Some("Basic expected-token"),
            Some("Bearer wrong"),
        ] {
            assert_eq!(
                authorize_admin_request(&headers(header), Some(&credential())),
                Err(StatusCode::UNAUTHORIZED),
                "header {header:?} must be rejected"
            );
        }
    }

    #[test]
    fn admin_authorizer_allows_uncredentialed_local_escape_hatch_state() {
        assert_eq!(authorize_admin_request(&headers(None), None), Ok(()));
    }
}
