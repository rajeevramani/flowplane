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

/// Base OpenAPI document; paths and schemas are contributed by the `routes!`
/// registrations below — the router and the document cannot drift (spec/10 §9).
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "Flowplane",
        description = "Envoy control plane with a learning loop. Errors are always \
                       {code, message, hint?, details?, request_id}.",
    ),
    modifiers(&SecurityAddon)
)]
struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearerAuth",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
        openapi.security = Some(vec![utoipa::openapi::security::SecurityRequirement::new::<
            _,
            _,
            &str,
        >("bearerAuth", [])]);
    }
}

/// Build the OpenAPI router + document for the secured /api/v1 surface.
fn secured_api() -> (Router<AppState>, utoipa::openapi::OpenApi) {
    use crate::identity_api;
    use crate::resources::{clusters, listeners, route_configs};
    use utoipa_axum::router::OpenApiRouter;
    use utoipa_axum::routes;

    OpenApiRouter::with_openapi(<ApiDoc as utoipa::OpenApi>::openapi())
        .routes(routes!(whoami))
        .routes(routes!(identity_api::list_teams, identity_api::create_team))
        .routes(routes!(identity_api::delete_team))
        .routes(routes!(
            identity_api::list_members,
            identity_api::add_member
        ))
        .routes(routes!(identity_api::remove_member))
        .routes(routes!(identity_api::list_grants, identity_api::add_grant))
        .routes(routes!(identity_api::remove_grant))
        .routes(routes!(
            crate::orgs_api::list_orgs,
            crate::orgs_api::create_org
        ))
        .routes(routes!(
            crate::orgs_api::get_org,
            crate::orgs_api::delete_org
        ))
        .routes(routes!(
            crate::orgs_api::list_members,
            crate::orgs_api::add_member
        ))
        .routes(routes!(crate::orgs_api::remove_member))
        .routes(routes!(clusters::list, clusters::create))
        .routes(routes!(clusters::get, clusters::update, clusters::delete))
        .routes(routes!(listeners::list, listeners::create))
        .routes(routes!(
            listeners::get,
            listeners::update,
            listeners::delete
        ))
        .routes(routes!(route_configs::list, route_configs::create))
        .routes(routes!(
            route_configs::get,
            route_configs::update,
            route_configs::delete
        ))
        .routes(routes!(
            crate::dataplanes_api::list_dataplanes,
            crate::dataplanes_api::create_dataplane
        ))
        .routes(routes!(crate::dataplanes_api::get_dataplane))
        .routes(routes!(crate::dataplanes_api::get_envoy_config))
        .routes(routes!(
            crate::dataplanes_api::list_proxy_certificates,
            crate::dataplanes_api::register_proxy_certificate
        ))
        .routes(routes!(crate::dataplanes_api::revoke_proxy_certificate))
        .routes(routes!(
            crate::secrets_api::list_secrets,
            crate::secrets_api::create_secret
        ))
        .routes(routes!(crate::secrets_api::get_secret))
        .routes(routes!(crate::secrets_api::rotate_secret))
        .routes(routes!(crate::xds_api::list_nacks))
        .split_for_parts()
}

/// The generated OpenAPI document for this build (public for the contract-diff tooling).
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    secured_api().1
}

pub fn build_router(state: AppState) -> Router {
    let (api_router, openapi) = secured_api();
    let secured = api_router
        // Throttle inside auth so the PrincipalCtx is available for tenant keying.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::throttle::tenant_write_throttle,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::authenticate,
        ));

    let openapi = std::sync::Arc::new(openapi);
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_endpoint))
        .route(
            "/api-docs/openapi.json",
            get(move || {
                let doc = openapi.clone();
                async move { Json(doc.as_ref().clone()) }
            }),
        )
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

#[derive(Serialize, utoipa::ToSchema)]
struct WhoAmI {
    user_id: String,
    platform_admin: bool,
    memberships: Vec<WhoAmIMembership>,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_role: Option<&'static str>,
    grant_count: usize,
}

#[derive(Serialize, utoipa::ToSchema)]
struct WhoAmIMembership {
    org_id: String,
    role: &'static str,
}

/// Identity echo: the authenticated principal as the authorization engine sees it.
#[utoipa::path(get, path = "/api/v1/auth/whoami", tag = "Auth",
    responses(
        (status = 200, body = WhoAmI),
        (status = 401, body = crate::error::ErrorBody),
    ))]
async fn whoami(Extension(ctx): Extension<fp_core::PrincipalCtx>) -> Json<WhoAmI> {
    match ctx {
        fp_core::PrincipalCtx::User {
            user_id,
            platform_admin,
            memberships,
            org,
            grants,
        } => Json(WhoAmI {
            user_id: user_id.to_string(),
            platform_admin,
            memberships: memberships
                .into_iter()
                .map(|(org_id, role)| WhoAmIMembership {
                    org_id: org_id.to_string(),
                    role: role.as_str(),
                })
                .collect(),
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
            memberships: vec![WhoAmIMembership {
                org_id: org_id.to_string(),
                role: "agent",
            }],
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
