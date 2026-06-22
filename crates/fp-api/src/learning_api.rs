//! Learning session REST endpoints (S8.3).

use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::resources::{resolve_team, Page};
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use fp_core::services::learning as svc;
use fp_core::PrincipalCtx;
use fp_domain::api_lifecycle::{
    CaptureSession, CaptureSessionSpec, CaptureSessionStatus, SpecVersion,
    DEFAULT_CAPTURE_MAX_BYTES, DEFAULT_CAPTURE_MAX_DISTINCT_PATHS,
    DEFAULT_CAPTURE_TARGET_SAMPLE_COUNT,
};
use fp_domain::{ApiDefinitionId, ListenerId, RequestId, RouteConfigId};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
pub struct LearningSessionView {
    pub id: uuid::Uuid,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_definition_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_config_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listener_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtual_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
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

impl From<CaptureSession> for LearningSessionView {
    fn from(value: CaptureSession) -> Self {
        Self {
            id: value.id.as_uuid(),
            name: value.name,
            status: value.status.as_str().into(),
            api_definition_id: value.api_definition_id.map(|id| id.as_uuid()),
            route_config_id: value.route_config_id.map(|id| id.as_uuid()),
            listener_id: value.listener_id.map(|id| id.as_uuid()),
            virtual_host: value.virtual_host,
            route: value.route,
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

#[derive(Debug, Serialize, ToSchema)]
pub struct LearnedSpecVersionView {
    pub id: uuid::Uuid,
    pub api_definition_id: uuid::Uuid,
    pub version: i64,
    pub source_kind: String,
    pub format: String,
    pub spec_hash: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<SpecVersion> for LearnedSpecVersionView {
    fn from(value: SpecVersion) -> Self {
        Self {
            id: value.id.as_uuid(),
            api_definition_id: value.api_definition_id.as_uuid(),
            version: value.version,
            source_kind: value.source_kind.as_str().into(),
            format: value.format.as_str().into(),
            spec_hash: value.spec_hash,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartLearningSessionBody {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_definition_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_config_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
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
    DEFAULT_CAPTURE_TARGET_SAMPLE_COUNT
}

fn default_max_bytes() -> i64 {
    DEFAULT_CAPTURE_MAX_BYTES
}

fn default_max_distinct_paths() -> i32 {
    DEFAULT_CAPTURE_MAX_DISTINCT_PATHS
}

impl StartLearningSessionBody {
    fn into_service(self) -> svc::StartLearningSessionInput {
        svc::StartLearningSessionInput {
            name: self.name,
            api: self.api,
            spec: CaptureSessionSpec {
                api_definition_id: self.api_definition_id.map(ApiDefinitionId::from),
                route_config_id: self.route_config_id.map(RouteConfigId::from),
                listener_id: self.listener_id.map(ListenerId::from),
                virtual_host: self.virtual_host,
                route: self.route,
                target_sample_count: self.target_sample_count,
                max_duration_seconds: self.max_duration_seconds,
                max_bytes: self.max_bytes,
                max_distinct_paths: self.max_distinct_paths,
            },
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct LearningSessionListQuery {
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
) -> Result<Option<CaptureSessionStatus>, fp_domain::DomainError> {
    raw.map(|value| CaptureSessionStatus::parse(&value))
        .transpose()
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/learning-sessions",
    tag = "LearningSessions",
    params(("team" = String, Path, description = "Team name or UUID"), LearningSessionListQuery),
    responses(
        (status = 200, body = Page<LearningSessionView>),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn list_learning_sessions(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<LearningSessionListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<LearningSessionView>>, ApiError> {
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
        items: items.into_iter().map(LearningSessionView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/learning-sessions",
    tag = "LearningSessions",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = StartLearningSessionBody,
    responses(
        (status = 201, body = LearningSessionView),
        (status = 400, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    ))]
pub async fn start_learning_session(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<StartLearningSessionBody>,
) -> Result<(axum::http::StatusCode, Json<LearningSessionView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::start_session(&state.pool, &ctx, team, body.into_service(), rid).await
    };
    let session = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(LearningSessionView::from(session)),
    ))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/learning-sessions/{session}",
    tag = "LearningSessions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("session" = String, Path, description = "Learning session name or UUID"),
    ),
    responses(
        (status = 200, body = LearningSessionView),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn get_learning_session(
    State(state): State<AppState>,
    Path((team, session)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<LearningSessionView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_session(&state.pool, &ctx, team, &session, rid).await
    };
    run.await
        .map(|v| Json(LearningSessionView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/learning-sessions/{session}/stop",
    tag = "LearningSessions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("session" = String, Path, description = "Learning session name or UUID"),
    ),
    responses(
        (status = 200, body = LearningSessionView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn stop_learning_session(
    State(state): State<AppState>,
    Path((team, session)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<LearningSessionView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::stop_session(&state.pool, &ctx, team, &session, rid).await
    };
    run.await
        .map(|v| Json(LearningSessionView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/learning-sessions/{session}/spec-version",
    tag = "LearningSessions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("session" = String, Path, description = "Completed learning session name or UUID"),
    ),
    responses(
        (status = 201, body = LearnedSpecVersionView),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn create_learned_spec_version(
    State(state): State<AppState>,
    Path((team, session)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<(axum::http::StatusCode, Json<LearnedSpecVersionView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::create_spec_version_from_session(&state.pool, &ctx, team, &session, rid).await
    };
    let spec = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(LearnedSpecVersionView::from(spec)),
    ))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/learning-sessions/{session}",
    tag = "LearningSessions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("session" = String, Path, description = "Learning session name or UUID"),
    ),
    responses(
        (status = 200, body = LearningSessionView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn cancel_learning_session(
    State(state): State<AppState>,
    Path((team, session)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<LearningSessionView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::cancel_session(&state.pool, &ctx, team, &session, rid).await
    };
    run.await
        .map(|v| Json(LearningSessionView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}
