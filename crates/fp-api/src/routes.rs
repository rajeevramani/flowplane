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
    let secured = Router::new()
        .route("/api/v1/auth/whoami", get(whoami))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::authenticate,
        ));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_endpoint))
        .merge(secured)
        .fallback(not_found)
        .layer(axum::middleware::from_fn(request_id))
        .with_state(state)
}

#[derive(Serialize)]
struct WhoAmI {
    user_id: String,
    platform_admin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_role: Option<&'static str>,
    grant_count: usize,
}

/// Identity echo: the authenticated principal as the authorization engine sees it.
async fn whoami(Extension(ctx): Extension<fp_core::PrincipalCtx>) -> Json<WhoAmI> {
    match ctx {
        fp_core::PrincipalCtx::User {
            user_id,
            platform_admin,
            org,
            grants,
        } => Json(WhoAmI {
            user_id: user_id.to_string(),
            platform_admin,
            org_id: org.map(|(id, _)| id.to_string()),
            org_role: org.map(|(_, role)| role.as_str()),
            grant_count: grants.len(),
        }),
        fp_core::PrincipalCtx::Agent {
            agent_id,
            org_id,
            grants,
            ..
        } => Json(WhoAmI {
            user_id: agent_id.to_string(),
            platform_admin: false,
            org_id: Some(org_id.to_string()),
            org_role: None,
            grant_count: grants.len(),
        }),
    }
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
