//! The CP-facing HTTP admin server: receives the full policy-set push (S5) and serves health.
//!
//! `POST /api/v1/admin/rls/policies` replaces the enforced set. The push is a full snapshot, so
//! this is idempotent and self-healing under the CP's 60 s reconcile (design pillar 4).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::policy::{PolicyCache, PolicyPush};

#[derive(Clone)]
pub struct AdminState {
    pub policies: Arc<PolicyCache>,
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
