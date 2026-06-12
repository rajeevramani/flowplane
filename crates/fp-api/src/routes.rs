//! Router assembly: health, readiness, metrics, JSON 404 fallback.

use crate::error::ApiError;
use crate::middleware::request_id;
use crate::state::AppState;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use fp_domain::{DomainError, RequestId};
use serde::Serialize;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_endpoint))
        .fallback(not_found)
        .layer(axum::middleware::from_fn(request_id))
        .with_state(state)
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

/// Liveness: the process is up and serving. No dependencies consulted.
async fn healthz(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        status: "ok",
        version: state.version,
    })
}

#[derive(Serialize)]
struct Ready {
    status: &'static str,
    checks: Vec<ReadyCheck>,
}

#[derive(Serialize)]
struct ReadyCheck {
    name: &'static str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// Readiness: dependencies answer. Returns 503 with per-check detail when not ready
/// (spec/10 §10; outbox-lag check joins in S3).
async fn readyz(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Ready>, ApiError> {
    let db = fp_storage::ping(&state.pool).await;
    let checks = vec![ReadyCheck {
        name: "database",
        ok: db.is_ok(),
        detail: db.as_ref().err().map(|e| e.message.clone()),
    }];

    if checks.iter().all(|c| c.ok) {
        Ok(Json(Ready {
            status: "ready",
            checks,
        }))
    } else {
        Err(ApiError::new(
            DomainError::unavailable("one or more readiness checks failed")
                .with_hint("GET /readyz returns per-check detail; see `checks`")
                .with_details(serde_json::json!({ "checks": checks })),
            rid,
        ))
    }
}

async fn metrics_endpoint(State(state): State<AppState>) -> String {
    state.prometheus.render()
}

/// Unknown paths return the standard envelope, not HTML or plain text.
async fn not_found(Extension(rid): Extension<RequestId>) -> impl IntoResponse {
    let err = ApiError::new(
        DomainError::new(fp_domain::ErrorCode::NotFound, "no such endpoint")
            .with_hint("see /api-docs/openapi.json for the API contract"),
        rid,
    );
    (StatusCode::NOT_FOUND, err.into_response())
}
