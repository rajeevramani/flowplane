//! Router assembly: health, readiness, metrics, JSON 404 fallback.

use crate::error::ApiError;
use crate::middleware::request_id;
use crate::state::AppState;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
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
    use crate::ai_api;
    use crate::api_lifecycle_api;
    use crate::dataplanes_api;
    use crate::discovery_api;
    use crate::identity_api;
    use crate::learning_api;
    use crate::resources::{clusters, listeners, route_configs};
    use crate::route_generation_api;
    use crate::secrets_api;
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
            identity_api::list_agents,
            identity_api::create_agent
        ))
        .routes(routes!(identity_api::get_agent))
        .routes(routes!(identity_api::rotate_agent_token))
        .routes(routes!(identity_api::disable_agent))
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
            api_lifecycle_api::list_apis,
            api_lifecycle_api::create_api
        ))
        .routes(routes!(
            api_lifecycle_api::get_api,
            api_lifecycle_api::delete_api
        ))
        .routes(routes!(api_lifecycle_api::api_status))
        .routes(routes!(api_lifecycle_api::update_mcp_tool))
        .routes(routes!(api_lifecycle_api::reject_spec_version))
        .routes(routes!(api_lifecycle_api::publish_spec_version))
        .routes(routes!(
            learning_api::list_learning_sessions,
            learning_api::start_learning_session
        ))
        .routes(routes!(
            learning_api::get_learning_session,
            learning_api::cancel_learning_session
        ))
        .routes(routes!(learning_api::stop_learning_session))
        .routes(routes!(learning_api::create_learned_spec_version))
        .routes(routes!(
            discovery_api::list_discovery_sessions,
            discovery_api::start_discovery_session
        ))
        .routes(routes!(
            discovery_api::get_discovery_session,
            discovery_api::stop_discovery_session
        ))
        .routes(routes!(discovery_api::create_discovery_spec_versions))
        .routes(routes!(
            crate::expose_api::expose,
            crate::expose_api::unexpose
        ))
        .routes(routes!(route_generation_api::create_route_plan))
        .routes(routes!(route_generation_api::apply_route_plan))
        .routes(routes!(
            ai_api::list_ai_providers,
            ai_api::create_ai_provider
        ))
        .routes(routes!(
            ai_api::get_ai_provider,
            ai_api::update_ai_provider,
            ai_api::delete_ai_provider
        ))
        .routes(routes!(ai_api::list_ai_routes, ai_api::create_ai_route))
        .routes(routes!(
            ai_api::get_ai_route,
            ai_api::update_ai_route,
            ai_api::delete_ai_route
        ))
        .routes(routes!(ai_api::list_ai_budgets, ai_api::create_ai_budget))
        .routes(routes!(
            ai_api::get_ai_budget,
            ai_api::update_ai_budget,
            ai_api::delete_ai_budget
        ))
        .routes(routes!(ai_api::get_ai_usage))
        .routes(routes!(
            dataplanes_api::list_dataplanes,
            dataplanes_api::create_dataplane
        ))
        .routes(routes!(dataplanes_api::get_dataplane))
        .routes(routes!(dataplanes_api::record_dataplane_telemetry))
        .routes(routes!(dataplanes_api::get_envoy_config))
        .routes(routes!(dataplanes_api::stats_overview))
        .routes(routes!(
            dataplanes_api::list_proxy_certificates,
            dataplanes_api::register_proxy_certificate
        ))
        .routes(routes!(dataplanes_api::issue_proxy_certificate))
        .routes(routes!(dataplanes_api::revoke_proxy_certificate))
        .routes(routes!(
            secrets_api::list_secrets,
            secrets_api::create_secret
        ))
        .routes(routes!(secrets_api::get_secret))
        .routes(routes!(secrets_api::rotate_secret))
        .routes(routes!(crate::xds_api::list_nacks))
        .routes(routes!(crate::xds_api::status))
        .routes(routes!(crate::xds_api::trace))
        .split_for_parts()
}

/// The generated OpenAPI document for this build (public for the contract-diff tooling).
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    secured_api().1
}

pub fn build_router(state: AppState) -> Router {
    let (api_router, openapi) = secured_api();
    let secured = api_router
        .route("/api/v1/mcp", post(crate::mcp_api::post))
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
struct WhoAmIMembership {
    org_id: String,
    org_role: &'static str,
}

#[derive(Serialize, utoipa::ToSchema)]
struct WhoAmI {
    user_id: String,
    platform_admin: bool,
    /// The validated ACTIVE org for this request (from the `X-Flowplane-Org` selector or the
    /// caller's sole non-platform membership), if resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    org_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_role: Option<&'static str>,
    /// True when the caller must name an org (multi-org, no selector) for tenant-scoped ops.
    org_selector_required: bool,
    /// Selectable non-platform org memberships for the `X-Flowplane-Org` active-org header.
    memberships: Vec<WhoAmIMembership>,
    grant_count: usize,
}

/// Identity echo: the authenticated principal as the authorization engine sees it.
#[utoipa::path(get, path = "/api/v1/auth/whoami", tag = "Auth",
    responses(
        (status = 200, body = WhoAmI),
        (status = 401, body = crate::error::ErrorBody),
    ))]
async fn whoami(
    Extension(ctx): Extension<fp_core::PrincipalCtx>,
    memberships: Option<Extension<crate::auth::OrgMemberships>>,
) -> Json<WhoAmI> {
    let memberships = memberships
        .map(|Extension(memberships)| {
            memberships
                .0
                .into_iter()
                .map(|(org_id, org_role)| WhoAmIMembership {
                    org_id: org_id.to_string(),
                    org_role: org_role.as_str(),
                })
                .collect()
        })
        .unwrap_or_default();
    match ctx {
        fp_core::PrincipalCtx::User {
            user_id,
            platform_admin,
            org,
            org_selector_required,
            grants,
        } => Json(WhoAmI {
            user_id: user_id.to_string(),
            platform_admin,
            org_id: org.map(|(id, _)| id.to_string()),
            org_role: org.map(|(_, role)| role.as_str()),
            org_selector_required,
            memberships,
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
            org_selector_required: false,
            memberships,
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

/// Readiness: dependencies answer. Returns 503 with per-check detail when not ready.
async fn readyz(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Ready>, ApiError> {
    let db = fp_storage::ping(&state.pool).await;
    let mut checks = vec![ReadyCheck {
        name: "database",
        ok: db.is_ok(),
        detail: db.as_ref().err().map(|e| e.message.clone()),
    }];
    if let Some(xds) = &state.xds_readiness {
        let failed = xds.failed.load(std::sync::atomic::Ordering::SeqCst);
        checks.push(ReadyCheck {
            name: "xds_outbox_consumer",
            ok: !failed,
            detail: failed.then(|| "consumer task exited with error".to_string()),
        });
        let lag = fp_storage::outbox::consumer_lag(&state.pool, xds.consumer).await;
        checks.push(match lag {
            Ok(lag) => ReadyCheck {
                name: "xds_outbox_lag",
                ok: lag <= xds.max_lag,
                detail: (lag > xds.max_lag)
                    .then(|| format!("lag {lag} exceeds threshold {}", xds.max_lag)),
            },
            Err(e) => ReadyCheck {
                name: "xds_outbox_lag",
                ok: false,
                detail: Some(e.message),
            },
        });
    }

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
