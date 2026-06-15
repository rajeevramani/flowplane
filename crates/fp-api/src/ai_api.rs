//! AI gateway REST endpoints.

use crate::error::ApiError;
use crate::resources::{resolve_team, revision_from, ListQuery, Page};
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use fp_core::services::ai as ai_svc;
use fp_core::PrincipalCtx;
use fp_domain::{AiProvider, AiProviderSpec, RequestId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct AiProviderView {
    pub id: uuid::Uuid,
    pub name: String,
    pub spec: AiProviderSpec,
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
    Json(body): Json<CreateAiProviderBody>,
) -> Result<(axum::http::StatusCode, Json<AiProviderView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::create_provider(&state.pool, &ctx, team, &body.name, body.spec, rid).await
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
    Json(body): Json<UpdateAiProviderBody>,
) -> Result<Json<AiProviderView>, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        ai_svc::update_provider(&state.pool, &ctx, team, &name, body.spec, revision, rid).await
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
