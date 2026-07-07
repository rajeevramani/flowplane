//! Gateway-resource REST endpoints (spec/01 conventions with the v2 fixes: PATCH for
//! partial-life updates, If-Match revisions, uniform list envelope, per-team names).
//! Every handler carries its OpenAPI declaration — the router and the document are split
//! from the same `routes!` registration (spec/10 §9), so they cannot drift.

use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use fp_core::services::{clusters as cluster_svc, gateway as gateway_svc};
use fp_core::PrincipalCtx;
use fp_domain::authz::TeamRef;
use fp_domain::gateway::cluster::{Cluster, ClusterSpec};
use fp_domain::gateway::listener::{Listener, ListenerSpec};
use fp_domain::gateway::route_config::{RouteConfig, RouteConfigSpec};
use fp_domain::{DomainError, DomainResult, ErrorCode, RequestId};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa::{IntoParams, ToSchema};

/// Resolve `{team}` (name within the caller's org, or UUID) to a TeamRef. Unknown or
/// foreign teams are indistinguishable: `not_found`.
pub async fn resolve_team(
    state: &AppState,
    ctx: &PrincipalCtx,
    raw: &str,
) -> DomainResult<TeamRef> {
    let not_found = || DomainError::not_found("team", raw);
    if let Ok(team_id) = fp_domain::TeamId::from_str(raw) {
        return fp_storage::repos::identity::resolve_team_ref(&state.pool, team_id)
            .await?
            .ok_or_else(not_found);
    }
    let org_id = match ctx {
        PrincipalCtx::User {
            org: Some((org_id, _)),
            ..
        } => *org_id,
        PrincipalCtx::Agent { org_id, .. } => *org_id,
        // D-014: ambiguous (multi-org, no selector) fails closed with a clear "pick an org",
        // distinct from genuinely-no-access which stays 404 (anti-enumeration).
        PrincipalCtx::User {
            org: None,
            org_selector_required: true,
            ..
        } => return Err(DomainError::org_selector_required()),
        PrincipalCtx::User { org: None, .. } => return Err(not_found()),
    };
    fp_storage::repos::identity::resolve_team_by_name(&state.pool, org_id, raw)
        .await?
        .ok_or_else(not_found)
}

/// Revision from `If-Match` (plain integer). Required on update/delete.
pub fn revision_from(headers: &HeaderMap) -> DomainResult<i64> {
    headers
        .get(axum::http::header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().trim_matches('"').parse::<i64>().ok())
        .ok_or_else(|| {
            DomainError::new(
                ErrorCode::ValidationFailed,
                "this operation requires the resource revision",
            )
            .with_hint("read the resource and send its revision as: If-Match: <revision>")
        })
}

/// Optional revision from `If-Match`, for create-or-replace endpoints where the header is
/// optional: absent → `None` (create or replace); present → `Some(n)` (must match the current
/// revision). A present-but-malformed header is an error rather than being silently ignored, so
/// a client that meant to guard a write never falls through to a last-writer-wins replace.
pub fn optional_revision_from(headers: &HeaderMap) -> DomainResult<Option<i64>> {
    let Some(value) = headers.get(axum::http::header::IF_MATCH) else {
        return Ok(None);
    };
    // The header IS present: it must be a valid integer revision. Anything unparseable —
    // non-UTF-8, empty (`If-Match: ""`), or non-numeric — is an error, never a silent
    // fall-through to a last-writer-wins replace (that is what an *omitted* header means).
    value
        .to_str()
        .ok()
        .map(|v| v.trim().trim_matches('"'))
        .filter(|v| !v.is_empty())
        .and_then(|v| v.parse::<i64>().ok())
        .map(Some)
        .ok_or_else(|| {
            DomainError::new(
                ErrorCode::ValidationFailed,
                "If-Match must be an integer revision",
            )
            .with_hint("send If-Match: <revision>, or omit it to create or replace")
        })
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListQuery {
    /// Max items (default 50, cap 500).
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Items to skip.
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Uniform list envelope (spec/01).
#[derive(Debug, Serialize, ToSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

macro_rules! views {
    ($view:ident, $domain:ty, $spec:ty) => {
        #[derive(Debug, Serialize, ToSchema)]
        pub struct $view {
            /// Stable identifier (UUID).
            pub id: uuid::Uuid,
            pub name: String,
            pub spec: $spec,
            /// Optimistic-concurrency revision; echo via If-Match on update/delete.
            pub revision: i64,
            pub created_at: chrono::DateTime<chrono::Utc>,
            pub updated_at: chrono::DateTime<chrono::Utc>,
        }

        impl From<$domain> for $view {
            fn from(value: $domain) -> Self {
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
    };
}

views!(ClusterView, Cluster, ClusterSpec);
views!(ListenerView, Listener, ListenerSpec);
views!(RouteConfigView, RouteConfig, RouteConfigSpec);

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateClusterBody {
    pub name: String,
    pub spec: ClusterSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateClusterBody {
    pub spec: ClusterSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateListenerBody {
    pub name: String,
    pub spec: ListenerSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateListenerBody {
    pub spec: ListenerSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRouteConfigBody {
    pub name: String,
    pub spec: RouteConfigSpec,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRouteConfigBody {
    pub spec: RouteConfigSpec,
}

macro_rules! endpoints {
    ($mod_name:ident, $segment:literal, $tag:literal,
     view: $view:ident, create: $create_body:ident, update: $update_body:ident,
     svc_create: $svc_create:path, svc_get: $svc_get:path, svc_list: $svc_list:path,
     svc_update: $svc_update:path, svc_delete: $svc_delete:path
     $(, egress: $egress:ident)?
     $(, rls: $rls:ident)?) => {
        pub mod $mod_name {
            use super::*;

            #[utoipa::path(get, path = concat!("/api/v1/teams/{team}/", $segment),
                tag = $tag,
                params(("team" = String, Path, description = "Team name or UUID"), ListQuery),
                responses(
                    (status = 200, body = Page<$view>),
                    (status = 401, body = crate::error::ErrorBody),
                    (status = 404, body = crate::error::ErrorBody),
                ))]
            pub async fn list(
                State(state): State<AppState>,
                Path(team): Path<String>,
                Query(query): Query<ListQuery>,
                Extension(ctx): Extension<PrincipalCtx>,
                Extension(rid): Extension<RequestId>,
            ) -> Result<Json<Page<$view>>, ApiError> {
                let run = async {
                    let team = resolve_team(&state, &ctx, &team).await?;
                    $svc_list(&state.pool, &ctx, team, query.limit, query.offset, rid).await
                };
                let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
                Ok(Json(Page {
                    items: items.into_iter().map($view::from).collect(),
                    total,
                    limit: query.limit.clamp(1, 500),
                    offset: query.offset.max(0),
                }))
            }

            #[utoipa::path(post, path = concat!("/api/v1/teams/{team}/", $segment),
                tag = $tag,
                params(("team" = String, Path, description = "Team name or UUID")),
                request_body = $create_body,
                responses(
                    (status = 201, body = $view),
                    (status = 400, body = crate::error::ErrorBody),
                    (status = 409, body = crate::error::ErrorBody),
                    (status = 422, body = crate::error::ErrorBody),
                ))]
            pub async fn create(
                State(state): State<AppState>,
                Path(team): Path<String>,
                Extension(ctx): Extension<PrincipalCtx>,
                Extension(rid): Extension<RequestId>,
                ApiJson(body): ApiJson<$create_body>,
            ) -> Result<(axum::http::StatusCode, Json<$view>), ApiError> {
                let run = async {
                    let team = resolve_team(&state, &ctx, &team).await?;
                    $svc_create(
                        &state.pool,
                        &ctx,
                        team,
                        &body.name,
                        body.spec,
                        rid
                        $(, &state.$egress)?
                        $(, state.$rls)?
                    ).await
                };
                let created = run.await.map_err(|e| ApiError::new(e, rid))?;
                Ok((axum::http::StatusCode::CREATED, Json($view::from(created))))
            }

            #[utoipa::path(get, path = concat!("/api/v1/teams/{team}/", $segment, "/{name}"),
                tag = $tag,
                params(
                    ("team" = String, Path, description = "Team name or UUID"),
                    ("name" = String, Path, description = "Resource name"),
                ),
                responses(
                    (status = 200, body = $view),
                    (status = 404, body = crate::error::ErrorBody),
                ))]
            pub async fn get(
                State(state): State<AppState>,
                Path((team, name)): Path<(String, String)>,
                Extension(ctx): Extension<PrincipalCtx>,
                Extension(rid): Extension<RequestId>,
            ) -> Result<Json<$view>, ApiError> {
                let run = async {
                    let team = resolve_team(&state, &ctx, &team).await?;
                    $svc_get(&state.pool, &ctx, team, &name, rid).await
                };
                run.await.map(|v| Json($view::from(v))).map_err(|e| ApiError::new(e, rid))
            }

            #[utoipa::path(patch, path = concat!("/api/v1/teams/{team}/", $segment, "/{name}"),
                tag = $tag,
                params(
                    ("team" = String, Path, description = "Team name or UUID"),
                    ("name" = String, Path, description = "Resource name"),
                    ("If-Match" = i64, Header, description = "Current resource revision"),
                ),
                request_body = $update_body,
                responses(
                    (status = 200, body = $view),
                    (status = 404, body = crate::error::ErrorBody),
                    (status = 409, body = crate::error::ErrorBody),
                ))]
            pub async fn update(
                State(state): State<AppState>,
                Path((team, name)): Path<(String, String)>,
                headers: HeaderMap,
                Extension(ctx): Extension<PrincipalCtx>,
                Extension(rid): Extension<RequestId>,
                ApiJson(body): ApiJson<$update_body>,
            ) -> Result<Json<$view>, ApiError> {
                let run = async {
                    let revision = revision_from(&headers)?;
                    let team = resolve_team(&state, &ctx, &team).await?;
                    $svc_update(
                        &state.pool,
                        &ctx,
                        team,
                        &name,
                        body.spec,
                        revision,
                        rid
                        $(, &state.$egress)?
                        $(, state.$rls)?
                    ).await
                };
                run.await.map(|v| Json($view::from(v))).map_err(|e| ApiError::new(e, rid))
            }

            #[utoipa::path(delete, path = concat!("/api/v1/teams/{team}/", $segment, "/{name}"),
                tag = $tag,
                params(
                    ("team" = String, Path, description = "Team name or UUID"),
                    ("name" = String, Path, description = "Resource name"),
                    ("If-Match" = i64, Header, description = "Current resource revision"),
                ),
                responses(
                    (status = 204),
                    (status = 404, body = crate::error::ErrorBody),
                    (status = 409, body = crate::error::ErrorBody),
                ))]
            pub async fn delete(
                State(state): State<AppState>,
                Path((team, name)): Path<(String, String)>,
                headers: HeaderMap,
                Extension(ctx): Extension<PrincipalCtx>,
                Extension(rid): Extension<RequestId>,
            ) -> Result<axum::http::StatusCode, ApiError> {
                let run = async {
                    let revision = revision_from(&headers)?;
                    let team = resolve_team(&state, &ctx, &team).await?;
                    $svc_delete(&state.pool, &ctx, team, &name, revision, rid).await
                };
                run.await
                    .map(|_| axum::http::StatusCode::NO_CONTENT)
                    .map_err(|e| ApiError::new(e, rid))
            }
        }
    };
}

endpoints!(clusters, "clusters", "Clusters",
    view: ClusterView, create: CreateClusterBody, update: UpdateClusterBody,
    svc_create: cluster_svc::create_cluster_with_egress_policy, svc_get: cluster_svc::get_cluster,
    svc_list: cluster_svc::list_clusters, svc_update: cluster_svc::update_cluster_with_egress_policy,
    svc_delete: cluster_svc::delete_cluster,
    egress: egress_policy);

endpoints!(listeners, "listeners", "Listeners",
    view: ListenerView, create: CreateListenerBody, update: UpdateListenerBody,
    svc_create: gateway_svc::create_listener, svc_get: gateway_svc::get_listener,
    svc_list: gateway_svc::list_listeners, svc_update: gateway_svc::update_listener,
    svc_delete: gateway_svc::delete_listener,
    rls: rls_grpc_configured);

endpoints!(route_configs, "route-configs", "RouteConfigs",
    view: RouteConfigView, create: CreateRouteConfigBody, update: UpdateRouteConfigBody,
    svc_create: gateway_svc::create_route_config, svc_get: gateway_svc::get_route_config,
    svc_list: gateway_svc::list_route_configs, svc_update: gateway_svc::update_route_config,
    svc_delete: gateway_svc::delete_route_config);
