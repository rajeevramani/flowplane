//! Discovery-session REST endpoints (S9).

use crate::error::ApiError;
use crate::resources::{resolve_team, Page};
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use fp_core::services::discovery as svc;
use fp_core::PrincipalCtx;
use fp_domain::{DiscoverySession, DiscoverySessionSpec, DiscoverySessionStatus, RequestId};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
pub struct DiscoverySessionView {
    pub id: uuid::Uuid,
    pub name: String,
    pub status: String,
    pub listener_port: i32,
    pub upstream_host: String,
    pub upstream_port: i32,
    pub upstream_tls: bool,
    pub validated_upstream_ip: String,
    pub validated_upstream_port: i32,
    pub target_sample_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<i32>,
    pub max_bytes: i64,
    pub max_distinct_paths: i32,
    pub sample_count: i64,
    pub byte_count: i64,
    pub path_count: i64,
    pub drop_count: i64,
    pub started_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<DiscoverySession> for DiscoverySessionView {
    fn from(value: DiscoverySession) -> Self {
        Self {
            id: value.id.as_uuid(),
            name: value.name,
            status: value.status.as_str().into(),
            listener_port: value.listener_port,
            upstream_host: value.upstream_host,
            upstream_port: value.upstream_port,
            upstream_tls: value.upstream_tls,
            validated_upstream_ip: value.validated_upstream_ip,
            validated_upstream_port: value.validated_upstream_port,
            target_sample_count: value.target_sample_count,
            max_duration_seconds: value.max_duration_seconds,
            max_bytes: value.max_bytes,
            max_distinct_paths: value.max_distinct_paths,
            sample_count: value.sample_count,
            byte_count: value.byte_count,
            path_count: value.path_count,
            drop_count: value.drop_count,
            started_at: value.started_at,
            completed_at: value.completed_at,
            cancelled_at: value.cancelled_at,
            updated_at: value.updated_at,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartDiscoverySessionBody {
    pub name: String,
    pub upstream: String,
    pub listener_port: i32,
    #[serde(default)]
    pub upstream_tls: bool,
    #[serde(default = "default_target_sample_count")]
    pub target_sample_count: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<i32>,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: i64,
    #[serde(default = "default_max_distinct_paths")]
    pub max_distinct_paths: i32,
}

fn default_target_sample_count() -> i32 {
    fp_domain::api_lifecycle::DEFAULT_CAPTURE_TARGET_SAMPLE_COUNT
}

fn default_max_bytes() -> i64 {
    fp_domain::api_lifecycle::DEFAULT_CAPTURE_MAX_BYTES
}

fn default_max_distinct_paths() -> i32 {
    fp_domain::api_lifecycle::DEFAULT_CAPTURE_MAX_DISTINCT_PATHS
}

impl StartDiscoverySessionBody {
    fn into_service(self) -> Result<svc::StartDiscoveryInput, fp_domain::DomainError> {
        let (upstream_host, upstream_port) = parse_upstream(&self.upstream)?;
        Ok(svc::StartDiscoveryInput {
            name: self.name,
            spec: DiscoverySessionSpec {
                listener_port: self.listener_port,
                upstream_host,
                upstream_port,
                upstream_tls: self.upstream_tls,
                target_sample_count: self.target_sample_count,
                max_duration_seconds: self.max_duration_seconds,
                max_bytes: self.max_bytes,
                max_distinct_paths: self.max_distinct_paths,
            },
        })
    }
}

fn parse_upstream(raw: &str) -> Result<(String, i32), fp_domain::DomainError> {
    if raw.contains("://") || raw.contains('/') || raw.contains('@') {
        return Err(fp_domain::DomainError::validation(
            "discovery upstream must be host:port without scheme, path, or credentials",
        ));
    }
    let (host, port) = raw.rsplit_once(':').ok_or_else(|| {
        fp_domain::DomainError::validation("discovery upstream must include host:port")
    })?;
    let port = port.parse::<i32>().map_err(|_| {
        fp_domain::DomainError::validation("discovery upstream port must be an integer")
    })?;
    Ok((host.to_string(), port))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct DiscoverySessionListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

fn default_limit() -> i64 {
    50
}

fn parse_status(
    raw: Option<String>,
) -> Result<Option<DiscoverySessionStatus>, fp_domain::DomainError> {
    raw.map(|value| DiscoverySessionStatus::parse(&value))
        .transpose()
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/learning-discovery-sessions",
    tag = "DiscoverySessions",
    params(("team" = String, Path, description = "Team name or UUID"), DiscoverySessionListQuery),
    responses((status = 200, body = Page<DiscoverySessionView>)))]
pub async fn list_discovery_sessions(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<DiscoverySessionListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<DiscoverySessionView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        let status = parse_status(query.status)?;
        svc::list_sessions(
            &state.pool,
            &ctx,
            team,
            status,
            query.limit,
            query.offset,
            rid,
        )
        .await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(DiscoverySessionView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/learning-discovery-sessions",
    tag = "DiscoverySessions",
    request_body = StartDiscoverySessionBody,
    responses((status = 201, body = DiscoverySessionView)))]
pub async fn start_discovery_session(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<StartDiscoverySessionBody>,
) -> Result<(axum::http::StatusCode, Json<DiscoverySessionView>), ApiError> {
    let input = body.into_service().map_err(|e| ApiError::new(e, rid))?;
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::start_session_with_policy(
            &state.pool,
            &ctx,
            team,
            input,
            rid,
            &state.discovery_forwarding_policy,
        )
        .await
    };
    let session = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(DiscoverySessionView::from(session)),
    ))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/learning-discovery-sessions/{session}",
    tag = "DiscoverySessions",
    responses((status = 200, body = DiscoverySessionView)))]
pub async fn get_discovery_session(
    State(state): State<AppState>,
    Path((team, session)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<DiscoverySessionView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_session(&state.pool, &ctx, team, &session, rid).await
    };
    run.await
        .map(|v| Json(DiscoverySessionView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/learning-discovery-sessions/{session}/stop",
    tag = "DiscoverySessions",
    responses((status = 200, body = DiscoverySessionView)))]
pub async fn stop_discovery_session(
    State(state): State<AppState>,
    Path((team, session)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<DiscoverySessionView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::stop_session(&state.pool, &ctx, team, &session, rid).await
    };
    run.await
        .map(|v| Json(DiscoverySessionView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}
