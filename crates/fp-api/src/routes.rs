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
        // Throttle inside auth so the PrincipalCtx is available for tenant keying.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::throttle::tenant_write_throttle,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::authenticate,
        ));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_endpoint))
        .route("/api/v1/bootstrap/status", get(bootstrap_status))
        .route(
            "/api/v1/bootstrap/initialize",
            axum::routing::post(bootstrap_initialize),
        )
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

#[derive(Serialize)]
struct BootstrapStatus {
    initialized: bool,
}

/// Public: lets operators and the CLI see whether first-run setup is pending.
async fn bootstrap_status(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<BootstrapStatus>, ApiError> {
    let initialized = fp_storage::repos::bootstrap::is_initialized(&state.pool)
        .await
        .map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(BootstrapStatus { initialized }))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BootstrapInitialize {
    org_name: String,
    #[serde(default)]
    org_display_name: String,
    admin_subject: String,
    #[serde(default)]
    admin_email: String,
}

#[derive(Serialize)]
struct BootstrapResult {
    org_id: String,
    admin_user_id: String,
}

/// Public endpoint guarded by the one-shot bootstrap token (Authorization: Bearer fpboot_…).
async fn bootstrap_initialize(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    headers: axum::http::HeaderMap,
    Json(body): Json<BootstrapInitialize>,
) -> Result<Json<BootstrapResult>, ApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            ApiError::new(
                fp_domain::DomainError::new(
                    fp_domain::ErrorCode::Unauthorized,
                    "missing bootstrap token",
                )
                .with_hint("pass the boot-logged token as: Authorization: Bearer fpboot_…"),
                rid,
            )
        })?;
    let (org_id, admin_user_id) = fp_storage::repos::bootstrap::initialize(
        &state.pool,
        token,
        &body.org_name,
        &body.org_display_name,
        &body.admin_subject,
        &body.admin_email,
        rid,
    )
    .await
    .map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(BootstrapResult {
        org_id: org_id.to_string(),
        admin_user_id: admin_user_id.to_string(),
    }))
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
