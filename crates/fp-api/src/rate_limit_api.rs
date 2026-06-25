//! Rate-limit REST surface (feature fpv2-4ht, slices S5 + S3). The force-repush admin
//! trigger (S5, spec/01:152) plus the team-scoped domain/policy/override CRUD (S3,
//! spec/01:146-151). Every handler is a thin delegate to `fp_core::services::rate_limit`
//! (slice S2) — no business logic lives here. Each carries its own `#[utoipa::path]` so the
//! router and the OpenAPI document split from the same registration and cannot drift.
//!
//! Resources are nested to match the storage chain: a policy lives under a domain, an override
//! lives under a policy. The override is a singleton sub-resource of a policy (one per team),
//! so it has no name segment. PATCH + `If-Match` revisions mirror the gateway-resource
//! convention in `resources.rs`; cross-team access surfaces as 404, not 403 (spec/02:327).

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use fp_core::services::rate_limit as svc;
use fp_core::PrincipalCtx;
use fp_domain::rate_limit::{
    RateLimitDomain, RateLimitPolicy, RateLimitPolicySpec, RateLimitTeamOverride,
    RateLimitTeamOverrideSpec,
};
use fp_domain::{DomainError, RequestId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::resources::{resolve_team, revision_from, ListQuery, Page};
use crate::state::AppState;

// ---- Force-repush (S5) -------------------------------------------------------------------

/// Force an immediate CP→RLS policy reconcile (admin:all governance). The 60 s reconcile is the
/// correctness backstop; this is an ops convenience for "apply now". Returns 202 once the kick
/// is queued; 503 when the RLS sync is not configured.
#[utoipa::path(
    post,
    path = "/api/v1/admin/rls/force-repush",
    tag = "Rate limit",
    responses(
        (status = 202, description = "Reconcile kick accepted"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody)
    )
)]
pub async fn force_repush(
    State(state): State<AppState>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    fp_core::services::rls_sync::authorize_repush(&ctx).map_err(|e| ApiError::new(e, rid))?;
    match &state.rls_repush {
        Some(notify) => {
            notify.notify_one();
            Ok(StatusCode::ACCEPTED)
        }
        None => Err(ApiError::new(
            DomainError::unavailable("rate-limit sync is not configured")
                .with_hint("set FLOWPLANE_RLS_ADMIN_URL to enable CP→RLS policy sync"),
            rid,
        )),
    }
}

// ---- Views -------------------------------------------------------------------------------

/// A rate-limit domain as returned over HTTP. A domain carries only its user-facing `name`
/// (the limit group) plus the optimistic-concurrency revision.
#[derive(Debug, Serialize, ToSchema)]
pub struct RateLimitDomainView {
    pub id: uuid::Uuid,
    pub name: String,
    /// Optimistic-concurrency revision; echo via If-Match on update/delete.
    pub revision: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<RateLimitDomain> for RateLimitDomainView {
    fn from(d: RateLimitDomain) -> Self {
        Self {
            id: d.id.as_uuid(),
            name: d.name,
            revision: d.version,
            created_at: d.created_at,
            updated_at: d.updated_at,
        }
    }
}

/// A rate-limit policy as returned over HTTP.
#[derive(Debug, Serialize, ToSchema)]
pub struct RateLimitPolicyView {
    pub id: uuid::Uuid,
    /// Owning domain (UUID).
    pub domain_id: uuid::Uuid,
    pub name: String,
    pub spec: RateLimitPolicySpec,
    /// Deterministic sorted-key form of the descriptor set — the RLS match key (read-only).
    pub descriptors_canonical: String,
    pub revision: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<RateLimitPolicy> for RateLimitPolicyView {
    fn from(p: RateLimitPolicy) -> Self {
        Self {
            id: p.id.as_uuid(),
            domain_id: p.domain_id.as_uuid(),
            name: p.name,
            spec: p.spec,
            descriptors_canonical: p.descriptors_canonical,
            revision: p.version,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

/// A per-team policy override as returned over HTTP. There is at most one override per policy
/// per team, so it has no name of its own.
#[derive(Debug, Serialize, ToSchema)]
pub struct RateLimitOverrideView {
    pub id: uuid::Uuid,
    /// Overridden policy (UUID).
    pub policy_id: uuid::Uuid,
    pub spec: RateLimitTeamOverrideSpec,
    pub revision: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<RateLimitTeamOverride> for RateLimitOverrideView {
    fn from(o: RateLimitTeamOverride) -> Self {
        Self {
            id: o.id.as_uuid(),
            policy_id: o.policy_id.as_uuid(),
            spec: o.spec,
            revision: o.version,
            created_at: o.created_at,
            updated_at: o.updated_at,
        }
    }
}

fn page<T, V: From<T>>(items: Vec<T>, total: i64, q: &ListQuery) -> Page<V> {
    Page {
        items: items.into_iter().map(V::from).collect(),
        total,
        limit: q.limit.clamp(1, 500),
        offset: q.offset.max(0),
    }
}

// ---- Request bodies ----------------------------------------------------------------------

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRateLimitDomainBody {
    /// User-facing domain name (1–253 chars).
    pub name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRateLimitDomainBody {
    /// New domain name.
    pub name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRateLimitPolicyBody {
    pub name: String,
    pub spec: RateLimitPolicySpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRateLimitPolicyBody {
    pub spec: RateLimitPolicySpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRateLimitOverrideBody {
    pub spec: RateLimitTeamOverrideSpec,
}

// ---- Domains -----------------------------------------------------------------------------

#[utoipa::path(get, path = "/api/v1/teams/{team}/rate-limit-domains", tag = "Rate limit",
    params(("team" = String, Path, description = "Team name or UUID"), ListQuery),
    responses(
        (status = 200, body = Page<RateLimitDomainView>),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn list_domains(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<RateLimitDomainView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_domains(&state.pool, &ctx, team, query.limit, query.offset, rid).await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(page(items, total, &query)))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/rate-limit-domains", tag = "Rate limit",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = CreateRateLimitDomainBody,
    responses(
        (status = 201, body = RateLimitDomainView),
        (status = 400, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    ))]
pub async fn create_domain(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<CreateRateLimitDomainBody>,
) -> Result<(StatusCode, Json<RateLimitDomainView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::create_domain(&state.pool, &ctx, team, &body.name, rid).await
    };
    let created = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(created.into())))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
    ),
    responses(
        (status = 200, body = RateLimitDomainView),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn get_domain(
    State(state): State<AppState>,
    Path((team, domain)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<RateLimitDomainView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_domain(&state.pool, &ctx, team, &domain, rid).await
    };
    run.await
        .map(|d| Json(d.into()))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(patch, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Current domain name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    request_body = UpdateRateLimitDomainBody,
    responses(
        (status = 200, body = RateLimitDomainView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn update_domain(
    State(state): State<AppState>,
    Path((team, domain)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<UpdateRateLimitDomainBody>,
) -> Result<Json<RateLimitDomainView>, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::update_domain(&state.pool, &ctx, team, &domain, &body.name, revision, rid).await
    };
    run.await
        .map(|d| Json(d.into()))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    responses(
        (status = 204),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn delete_domain(
    State(state): State<AppState>,
    Path((team, domain)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::delete_domain(&state.pool, &ctx, team, &domain, revision, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

// ---- Policies (nested under a domain) ----------------------------------------------------

#[utoipa::path(get, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}/policies", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
        ListQuery,
    ),
    responses(
        (status = 200, body = Page<RateLimitPolicyView>),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn list_policies(
    State(state): State<AppState>,
    Path((team, domain)): Path<(String, String)>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<RateLimitPolicyView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_policies(
            &state.pool,
            &ctx,
            team,
            &domain,
            query.limit,
            query.offset,
            rid,
        )
        .await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(page(items, total, &query)))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}/policies", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
    ),
    request_body = CreateRateLimitPolicyBody,
    responses(
        (status = 201, body = RateLimitPolicyView),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    ))]
pub async fn create_policy(
    State(state): State<AppState>,
    Path((team, domain)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<CreateRateLimitPolicyBody>,
) -> Result<(StatusCode, Json<RateLimitPolicyView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::create_policy(&state.pool, &ctx, team, &domain, &body.name, body.spec, rid).await
    };
    let created = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(created.into())))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}/policies/{name}", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
        ("name" = String, Path, description = "Policy name"),
    ),
    responses(
        (status = 200, body = RateLimitPolicyView),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn get_policy(
    State(state): State<AppState>,
    Path((team, domain, name)): Path<(String, String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<RateLimitPolicyView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_policy(&state.pool, &ctx, team, &domain, &name, rid).await
    };
    run.await
        .map(|p| Json(p.into()))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(patch, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}/policies/{name}", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
        ("name" = String, Path, description = "Policy name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    request_body = UpdateRateLimitPolicyBody,
    responses(
        (status = 200, body = RateLimitPolicyView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn update_policy(
    State(state): State<AppState>,
    Path((team, domain, name)): Path<(String, String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<UpdateRateLimitPolicyBody>,
) -> Result<Json<RateLimitPolicyView>, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::update_policy(
            &state.pool,
            &ctx,
            team,
            &domain,
            &name,
            body.spec,
            revision,
            rid,
        )
        .await
    };
    run.await
        .map(|p| Json(p.into()))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}/policies/{name}", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
        ("name" = String, Path, description = "Policy name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    responses(
        (status = 204),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn delete_policy(
    State(state): State<AppState>,
    Path((team, domain, name)): Path<(String, String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::delete_policy(&state.pool, &ctx, team, &domain, &name, revision, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}

// ---- Overrides (singleton sub-resource of a policy) --------------------------------------

#[utoipa::path(get, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}/policies/{policy}/override", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
        ("policy" = String, Path, description = "Policy name"),
    ),
    responses(
        (status = 200, body = RateLimitOverrideView),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn get_override(
    State(state): State<AppState>,
    Path((team, domain, policy)): Path<(String, String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<RateLimitOverrideView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_override(&state.pool, &ctx, team, &domain, &policy, rid).await
    };
    run.await
        .map(|o| Json(o.into()))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}/policies/{policy}/override", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
        ("policy" = String, Path, description = "Policy name"),
    ),
    request_body = SetRateLimitOverrideBody,
    responses(
        (status = 201, body = RateLimitOverrideView),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    ))]
pub async fn create_override(
    State(state): State<AppState>,
    Path((team, domain, policy)): Path<(String, String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<SetRateLimitOverrideBody>,
) -> Result<(StatusCode, Json<RateLimitOverrideView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::create_override(&state.pool, &ctx, team, &domain, &policy, body.spec, rid).await
    };
    let created = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((StatusCode::CREATED, Json(created.into())))
}

#[utoipa::path(patch, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}/policies/{policy}/override", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
        ("policy" = String, Path, description = "Policy name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    request_body = SetRateLimitOverrideBody,
    responses(
        (status = 200, body = RateLimitOverrideView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn update_override(
    State(state): State<AppState>,
    Path((team, domain, policy)): Path<(String, String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<SetRateLimitOverrideBody>,
) -> Result<Json<RateLimitOverrideView>, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::update_override(
            &state.pool,
            &ctx,
            team,
            &domain,
            &policy,
            body.spec,
            revision,
            rid,
        )
        .await
    };
    run.await
        .map(|o| Json(o.into()))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/rate-limit-domains/{domain}/policies/{policy}/override", tag = "Rate limit",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("domain" = String, Path, description = "Domain name"),
        ("policy" = String, Path, description = "Policy name"),
        ("If-Match" = i64, Header, description = "Current resource revision"),
    ),
    responses(
        (status = 204),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn delete_override(
    State(state): State<AppState>,
    Path((team, domain, policy)): Path<(String, String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::delete_override(&state.pool, &ctx, team, &domain, &policy, revision, rid).await
    };
    run.await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}
