//! AI gateway REST endpoints.

use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::resources::{optional_revision_from, resolve_team, revision_from, ListQuery, Page};
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use fp_core::services::ai as ai_svc;
use fp_core::PrincipalCtx;
use fp_domain::{
    AiBudget, AiBudgetSpec, AiProvider, AiProviderSpec, AiRoute, AiRouteSpec, AiTraceEvent,
    AiUsageSummary, RequestId,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
pub struct AiProviderView {
    pub id: uuid::Uuid,
    pub name: String,
    pub spec: AiProviderSpec,
    pub revision: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AiRouteView {
    pub id: uuid::Uuid,
    pub name: String,
    pub spec: AiRouteSpec,
    pub status: fp_domain::AiRouteStatus,
    pub materialized: fp_domain::AiRouteMaterializedResources,
    pub revision: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AiBudgetView {
    pub id: uuid::Uuid,
    pub name: String,
    pub spec: AiBudgetSpec,
    pub revision: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<AiProvider> for AiProviderView {
    fn from(value: AiProvider) -> Self {
        Self {
            id: value.id.as_uuid(),
            name: value.name,
            spec: value.spec,
            revision: value.version,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<AiRoute> for AiRouteView {
    fn from(value: AiRoute) -> Self {
        Self {
            id: value.id.as_uuid(),
            name: value.name,
            spec: value.spec,
            status: value.status,
            materialized: value.materialized,
            revision: value.version,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<AiBudget> for AiBudgetView {
    fn from(value: AiBudget) -> Self {
        Self {
            id: value.id.as_uuid(),
            name: value.name,
            spec: value.spec,
            revision: value.version,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAiProviderBody {
    pub name: String,
    pub spec: AiProviderSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAiProviderBody {
    pub spec: AiProviderSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAiRouteBody {
    pub name: String,
    pub spec: AiRouteSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAiRouteBody {
    pub spec: AiRouteSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAiBudgetBody {
    pub name: String,
    pub spec: AiBudgetSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAiBudgetBody {
    pub spec: AiBudgetSpec,
}

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AiUsageQuery {
    #[serde(default)]
    pub route_config_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub provider_id: Option<uuid::Uuid>,
    /// RFC 3339 lower bound (inclusive) of the half-open usage window `[since, until)`.
    /// Omitted = an explicit all-time read (exempt from the span cap).
    #[serde(default)]
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    /// RFC 3339 upper bound (exclusive). Omitted = resolved server-side to `now`.
    /// With `since` present the span `until - since` is capped at 92 days (400 beyond).
    #[serde(default)]
    pub until: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default = "default_usage_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_usage_limit() -> i64 {
    50
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/ai/providers",
    tag = "AI",
    params(("team" = String, Path, description = "Team name or UUID"), ListQuery),
    responses((status = 200, body = Page<AiProviderView>)))]
pub async fn list_ai_providers(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<AiProviderView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::list_providers(&state.pool, &ctx, team, query.limit, query.offset, rid).await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(AiProviderView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/ai/providers",
    tag = "AI",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = CreateAiProviderBody,
    responses((status = 201, body = AiProviderView)))]
pub async fn create_ai_provider(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<CreateAiProviderBody>,
) -> Result<(axum::http::StatusCode, Json<AiProviderView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::create_provider(
            &state.pool,
            &ctx,
            team,
            &body.name,
            body.spec,
            rid,
            state.egress_advisory.clone(),
        )
        .await
    };
    let created = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(AiProviderView::from(created)),
    ))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/ai/providers/{name}",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "AI provider name"),
    ),
    responses((status = 200, body = AiProviderView)))]
pub async fn get_ai_provider(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<AiProviderView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::get_provider(&state.pool, &ctx, team, &name, rid).await
    };
    run.await
        .map(|v| Json(AiProviderView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(patch, path = "/api/v1/teams/{team}/ai/providers/{name}",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "AI provider name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    request_body = UpdateAiProviderBody,
    responses((status = 200, body = AiProviderView)))]
pub async fn update_ai_provider(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<UpdateAiProviderBody>,
) -> Result<Json<AiProviderView>, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::update_provider(
            &state.pool,
            &ctx,
            team,
            &name,
            body.spec,
            revision,
            rid,
            state.egress_advisory.clone(),
        )
        .await
    };
    run.await
        .map(|v| Json(AiProviderView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/ai/providers/{name}",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "AI provider name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    responses((status = 204)))]
pub async fn delete_ai_provider(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<axum::http::StatusCode, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::delete_provider(&state.pool, &ctx, team, &name, revision, rid).await
    };
    run.await
        .map(|_| axum::http::StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/ai/routes",
    tag = "AI",
    params(("team" = String, Path, description = "Team name or UUID"), ListQuery),
    responses((status = 200, body = Page<AiRouteView>)))]
pub async fn list_ai_routes(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<AiRouteView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::list_routes(&state.pool, &ctx, team, query.limit, query.offset, rid).await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(AiRouteView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/ai/routes",
    tag = "AI",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = CreateAiRouteBody,
    responses((status = 201, body = AiRouteView)))]
pub async fn create_ai_route(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<CreateAiRouteBody>,
) -> Result<(axum::http::StatusCode, Json<AiRouteView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::create_route(&state.pool, &ctx, team, &body.name, body.spec, rid).await
    };
    let created = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(AiRouteView::from(created)),
    ))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/ai/routes/{name}",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "AI route name"),
    ),
    responses((status = 200, body = AiRouteView)))]
pub async fn get_ai_route(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<AiRouteView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::get_route(&state.pool, &ctx, team, &name, rid).await
    };
    run.await
        .map(|v| Json(AiRouteView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(patch, path = "/api/v1/teams/{team}/ai/routes/{name}",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "AI route name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    request_body = UpdateAiRouteBody,
    responses((status = 200, body = AiRouteView)))]
pub async fn update_ai_route(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<UpdateAiRouteBody>,
) -> Result<Json<AiRouteView>, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::update_route(&state.pool, &ctx, team, &name, body.spec, revision, rid).await
    };
    run.await
        .map(|v| Json(AiRouteView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/ai/routes/{name}",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "AI route name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    responses((status = 204)))]
pub async fn delete_ai_route(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<axum::http::StatusCode, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::delete_route(&state.pool, &ctx, team, &name, revision, rid).await
    };
    run.await
        .map(|_| axum::http::StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/ai/budgets",
    tag = "AI",
    params(("team" = String, Path, description = "Team name or UUID"), ListQuery),
    responses((status = 200, body = Page<AiBudgetView>)))]
pub async fn list_ai_budgets(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<AiBudgetView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::list_budgets(&state.pool, &ctx, team, query.limit, query.offset, rid).await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(AiBudgetView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/ai/budgets",
    tag = "AI",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = CreateAiBudgetBody,
    responses((status = 201, body = AiBudgetView)))]
pub async fn create_ai_budget(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<CreateAiBudgetBody>,
) -> Result<(axum::http::StatusCode, Json<AiBudgetView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::create_budget(&state.pool, &ctx, team, &body.name, body.spec, rid).await
    };
    let created = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(AiBudgetView::from(created)),
    ))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/ai/budgets/{name}",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "AI budget name"),
    ),
    responses((status = 200, body = AiBudgetView)))]
pub async fn get_ai_budget(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<AiBudgetView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::get_budget(&state.pool, &ctx, team, &name, rid).await
    };
    run.await
        .map(|v| Json(AiBudgetView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(patch, path = "/api/v1/teams/{team}/ai/budgets/{name}",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "AI budget name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    request_body = UpdateAiBudgetBody,
    responses((status = 200, body = AiBudgetView)))]
pub async fn update_ai_budget(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<UpdateAiBudgetBody>,
) -> Result<Json<AiBudgetView>, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::update_budget(&state.pool, &ctx, team, &name, body.spec, revision, rid).await
    };
    run.await
        .map(|v| Json(AiBudgetView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/ai/budgets/{name}",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "AI budget name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    responses((status = 204)))]
pub async fn delete_ai_budget(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<axum::http::StatusCode, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::delete_budget(&state.pool, &ctx, team, &name, revision, rid).await
    };
    run.await
        .map(|_| axum::http::StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AiTraceParams {
    /// The server-generated `x-request-id` returned on the AI data-plane response.
    #[serde(default)]
    pub request_id: Option<String>,
    /// W3C trace id from an inbound `traceparent` (non-unique, list-queryable).
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default = "default_trace_limit")]
    pub limit: i64,
}

fn default_trace_limit() -> i64 {
    50
}

/// One correlated trace row: the per-request hop timeline plus its top-level correlation ids.
#[derive(Debug, Serialize, ToSchema)]
pub struct AiTraceEventView {
    pub id: uuid::Uuid,
    pub request_id: String,
    pub trace_id: Option<String>,
    pub route_config_id: uuid::Uuid,
    pub listener_id: Option<uuid::Uuid>,
    pub provider_id: Option<uuid::Uuid>,
    pub model: Option<String>,
    pub status_code: Option<i32>,
    /// The first hop (in timeline order) whose outcome is a failure, if any.
    pub failure_hop: Option<String>,
    /// Timeline of `{hop, started_at, ended_at, outcome, origin, failed, detail}` entries.
    #[schema(value_type = Object)]
    pub hops: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl From<AiTraceEvent> for AiTraceEventView {
    fn from(value: AiTraceEvent) -> Self {
        Self {
            id: value.id,
            request_id: value.request_id,
            trace_id: value.trace_id,
            route_config_id: value.route_config_id.as_uuid(),
            listener_id: value.listener_id.map(|id| id.as_uuid()),
            provider_id: value.provider_id.map(|id| id.as_uuid()),
            model: value.model,
            status_code: value.status_code,
            failure_hop: value.failure_hop,
            hops: value.hops,
            created_at: value.created_at,
            expires_at: value.expires_at,
        }
    }
}

/// Present exactly when an id-filtered query matched no row: distinguishes "no trace row
/// found" (and names the request classes that are never traced) from an empty list.
#[derive(Debug, Serialize, ToSchema)]
pub struct AiTraceMiss {
    pub message: String,
    pub hint: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AiTraceResponse {
    pub traces: Vec<AiTraceEventView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub miss: Option<AiTraceMiss>,
}

/// The request classes that never produce a trace row (design Risk 5): the hint a miss
/// carries so an operator can tell "never traced" from "wrong id".
const NEVER_TRACED_HINT: &str =
    "no trace row exists for this id; requests that never complete HTTP processing at the AI \
     listener are never traced: TCP/TLS-level failures, client disconnect before request \
     headers, and pre-ExtProc declared-filter denials";

/// Clamp the caller's limit to the repo's supported window (1..=500), defaulting to 50.
fn trace_limit(limit: i64) -> i64 {
    limit.clamp(1, 500)
}

/// A miss (id filter given, nothing matched) is distinguishable from an empty unfiltered
/// list: only the miss carries the `miss` payload naming the never-traced classes.
fn shape_trace_response(events: Vec<AiTraceEvent>, id_filtered: bool) -> AiTraceResponse {
    let miss = (id_filtered && events.is_empty()).then(|| AiTraceMiss {
        message: "no trace row found".into(),
        hint: NEVER_TRACED_HINT.into(),
    });
    AiTraceResponse {
        traces: events.into_iter().map(AiTraceEventView::from).collect(),
        miss,
    }
}

/// Correlated hop timeline for AI data-plane requests, newest first.
#[utoipa::path(get, path = "/api/v1/teams/{team}/ai/trace",
    tag = "AI",
    params(("team" = String, Path, description = "Team name or UUID"), AiTraceParams),
    responses((status = 200, body = AiTraceResponse)))]
pub async fn get_ai_trace(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(params): Query<AiTraceParams>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<AiTraceResponse>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::trace_events(
            &state.pool,
            &ctx,
            team,
            fp_storage::repos::ai_trace::AiTraceQuery {
                request_id: params.request_id.as_deref(),
                trace_id: params.trace_id.as_deref(),
                limit: trace_limit(params.limit),
            },
            rid,
        )
        .await
    };
    let events = run.await.map_err(|e| ApiError::new(e, rid))?;
    let id_filtered = params.request_id.is_some() || params.trace_id.is_some();
    Ok(Json(shape_trace_response(events, id_filtered)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetAiRetentionBody {
    /// Days a new trace row lives before the expiry sweep removes it (1–365).
    pub trace_ttl_days: i32,
}

/// The retention policy in force for the team's AI trace rows. `is_default: true` means no
/// team policy row exists and the built-in 30-day default applies (`revision`/`updated_at`
/// absent); a stored policy carries its revision and update time.
#[derive(Debug, Serialize, ToSchema)]
pub struct AiRetentionView {
    pub trace_ttl_days: i32,
    pub is_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The effective policy: the team's row when present, the built-in default otherwise.
fn shape_retention_view(policy: Option<fp_domain::AiRetentionPolicy>) -> AiRetentionView {
    match policy {
        Some(policy) => AiRetentionView {
            trace_ttl_days: policy.trace_ttl_days,
            is_default: false,
            revision: Some(policy.version),
            updated_at: Some(policy.updated_at),
        },
        None => AiRetentionView {
            trace_ttl_days: fp_storage::repos::ai_trace::DEFAULT_AI_TRACE_TTL_DAYS,
            is_default: true,
            revision: None,
            updated_at: None,
        },
    }
}

/// Effective AI trace retention policy for the team.
#[utoipa::path(get, path = "/api/v1/teams/{team}/ai/retention",
    tag = "AI",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses((status = 200, body = AiRetentionView)))]
pub async fn get_ai_retention(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<AiRetentionView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::get_retention_policy(&state.pool, &ctx, team, rid).await
    };
    run.await
        .map(|policy| Json(shape_retention_view(policy)))
        .map_err(|e| ApiError::new(e, rid))
}

/// Set the team's AI trace retention policy (create-or-replace; one policy per team).
/// Existing rows keep the expiry they were stamped with; only new rows use the new TTL.
#[utoipa::path(put, path = "/api/v1/teams/{team}/ai/retention",
    tag = "AI",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("If-Match" = Option<i64>, Header, description = "Current retention revision; omit to create or replace, supply to guard against a concurrent change"),
    ),
    request_body = SetAiRetentionBody,
    responses((status = 200, body = AiRetentionView)))]
pub async fn put_ai_retention(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<SetAiRetentionBody>,
) -> Result<Json<AiRetentionView>, ApiError> {
    let run = async {
        let expected_version = optional_revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::set_retention_policy(
            &state.pool,
            &ctx,
            team,
            body.trace_ttl_days,
            expected_version,
            rid,
        )
        .await
    };
    run.await
        .map(|policy| Json(shape_retention_view(Some(policy))))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/ai/usage",
    tag = "AI",
    params(("team" = String, Path, description = "Team name or UUID"), AiUsageQuery),
    responses((status = 200, body = Page<AiUsageSummary>)))]
pub async fn get_ai_usage(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<AiUsageQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<AiUsageSummary>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::usage_summary(
            &state.pool,
            &ctx,
            team,
            fp_storage::repos::ai::AiUsageQuery {
                route_config_id: query.route_config_id.map(fp_domain::RouteConfigId::from),
                provider_id: query.provider_id.map(fp_domain::AiProviderId::from),
                since: query.since,
                until: query.until,
                limit: query.limit,
                offset: query.offset,
            },
            rid,
        )
        .await
    };
    run.await
        .map(|(items, total)| {
            Json(Page {
                items,
                total,
                limit: query.limit,
                offset: query.offset,
            })
        })
        .map_err(|e| ApiError::new(e, rid))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn trace_event(request_id: &str) -> AiTraceEvent {
        AiTraceEvent {
            id: uuid::Uuid::now_v7(),
            team_id: fp_domain::TeamId::from(uuid::Uuid::now_v7()),
            request_id: request_id.to_string(),
            trace_id: None,
            route_config_id: fp_domain::RouteConfigId::from(uuid::Uuid::now_v7()),
            listener_id: None,
            provider_id: None,
            model: Some("gpt-5".into()),
            status_code: Some(200),
            failure_hop: None,
            hops: serde_json::json!([
                {"hop": "route_match", "outcome": "matched", "origin": "listener", "failed": false}
            ]),
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn id_filtered_miss_carries_the_never_traced_hint() {
        let shaped = shape_trace_response(Vec::new(), true);
        assert!(shaped.traces.is_empty());
        let miss = shaped
            .miss
            .expect("miss payload on an id-filtered empty result");
        assert_eq!(miss.message, "no trace row found");
        // The hint must name every never-traced class from design Risk 5.
        for class in [
            "TCP/TLS-level failures",
            "client disconnect before request headers",
            "pre-ExtProc declared-filter denials",
        ] {
            assert!(
                miss.hint.contains(class),
                "hint must name the never-traced class {class:?}, got: {}",
                miss.hint
            );
        }
    }

    #[test]
    fn unfiltered_empty_list_is_a_plain_empty_success_without_miss() {
        let shaped = shape_trace_response(Vec::new(), false);
        assert!(shaped.traces.is_empty());
        assert!(shaped.miss.is_none());
        // Distinguishable on the wire: no `miss` key at all for the empty success.
        let json = serde_json::to_value(&shaped).unwrap();
        assert_eq!(json, serde_json::json!({ "traces": [] }));
    }

    #[test]
    fn hits_render_the_hop_timeline_and_never_a_miss() {
        let shaped = shape_trace_response(vec![trace_event("req-1")], true);
        assert!(shaped.miss.is_none());
        assert_eq!(shaped.traces.len(), 1);
        assert_eq!(shaped.traces[0].request_id, "req-1");
        assert_eq!(shaped.traces[0].hops[0]["hop"], "route_match");
    }

    #[test]
    fn trace_limit_clamps_to_the_repo_window() {
        assert_eq!(default_trace_limit(), 50);
        assert_eq!(trace_limit(default_trace_limit()), 50);
        assert_eq!(trace_limit(0), 1);
        assert_eq!(trace_limit(-5), 1);
        assert_eq!(trace_limit(25), 25);
        assert_eq!(trace_limit(10_000), 500);
    }

    #[test]
    fn retention_view_distinguishes_stored_policy_from_builtin_default() {
        // No policy row: the 30-day default, flagged as such, with no revision/updated_at
        // keys on the wire at all.
        let default_view = shape_retention_view(None);
        assert_eq!(default_view.trace_ttl_days, 30);
        assert!(default_view.is_default);
        let json = serde_json::to_value(&default_view).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "trace_ttl_days": 30, "is_default": true })
        );

        // Stored policy: its TTL, revision, and update time; is_default false.
        let updated_at = chrono::Utc::now();
        let stored = shape_retention_view(Some(fp_domain::AiRetentionPolicy {
            id: uuid::Uuid::now_v7(),
            team_id: fp_domain::TeamId::from(uuid::Uuid::now_v7()),
            trace_ttl_days: 7,
            version: 3,
            created_at: updated_at,
            updated_at,
        }));
        assert_eq!(stored.trace_ttl_days, 7);
        assert!(!stored.is_default);
        assert_eq!(stored.revision, Some(3));
        assert_eq!(stored.updated_at, Some(updated_at));
    }
}
