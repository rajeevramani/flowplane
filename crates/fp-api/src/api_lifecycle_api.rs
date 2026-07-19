//! API lifecycle REST endpoints (S8.2).

use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::resources::{resolve_team, revision_from, ListQuery, Page};
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use fp_core::services::api_lifecycle as svc;
use fp_core::PrincipalCtx;
use fp_domain::api_lifecycle::{ApiDefinition, ApiDefinitionSpec, ApiRouteBindingSpec, ApiTool};
use fp_domain::{ListenerId, RequestId, RouteConfigId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiDefinitionView {
    pub id: uuid::Uuid,
    pub name: String,
    pub display_name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_spec_version_id: Option<uuid::Uuid>,
    pub revision: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ApiDefinition> for ApiDefinitionView {
    fn from(value: ApiDefinition) -> Self {
        Self {
            id: value.id.as_uuid(),
            name: value.name,
            display_name: value.display_name,
            description: value.description,
            published_spec_version_id: value.published_spec_version_id.map(|id| id.as_uuid()),
            revision: value.version,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

/// One row of the definition list: the API plus enrichment (counts + latest/published
/// version numbers) served by a single aggregate query — no per-row `/status` call.
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiDefinitionListItemView {
    pub id: uuid::Uuid,
    pub name: String,
    pub display_name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_spec_version_id: Option<uuid::Uuid>,
    pub revision: i64,
    pub tool_count: i64,
    pub route_binding_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_version: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<fp_domain::api_lifecycle::ApiDefinitionOverview> for ApiDefinitionListItemView {
    fn from(value: fp_domain::api_lifecycle::ApiDefinitionOverview) -> Self {
        Self {
            id: value.api.id.as_uuid(),
            name: value.api.name,
            display_name: value.api.display_name,
            description: value.api.description,
            published_spec_version_id: value.api.published_spec_version_id.map(|id| id.as_uuid()),
            revision: value.api.version,
            tool_count: value.tool_count,
            route_binding_count: value.route_binding_count,
            latest_version: value.latest_version,
            published_version: value.published_version,
            created_at: value.api.created_at,
            updated_at: value.api.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiStatusView {
    pub api: ApiDefinitionView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_spec: Option<SpecVersionSummary>,
    pub tool_count: i64,
    pub route_binding_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SpecVersionSummary {
    pub id: uuid::Uuid,
    pub version: i64,
    pub source_kind: String,
    pub format: String,
    pub spec_hash: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One row of the spec-version list: metadata plus the version's latest review decision.
/// Content is never inlined here — fetch it via the version's `/content` endpoint.
#[derive(Debug, Serialize, ToSchema)]
pub struct SpecVersionListItemView {
    pub id: uuid::Uuid,
    pub version: i64,
    pub source_kind: String,
    pub format: String,
    pub spec_hash: String,
    /// Latest review decision (`submitted`, `reviewed`, `rejected`, `published`,
    /// `unpublished`), or absent for a version with no review events yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_decision: Option<String>,
    /// Provenance for learned versions: the capture session that produced this version
    /// (from the document's learning-source stamp). Absent for imported/manual versions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_session_id: Option<uuid::Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<svc::SpecVersionListItem> for SpecVersionListItemView {
    fn from(value: svc::SpecVersionListItem) -> Self {
        Self {
            id: value.meta.id.as_uuid(),
            version: value.meta.version,
            source_kind: value.meta.source_kind.as_str().into(),
            format: value.meta.format.as_str().into(),
            spec_hash: value.meta.spec_hash,
            latest_decision: value.latest_decision.map(|d| d.as_str().into()),
            capture_session_id: value.meta.capture_session_id,
            created_at: value.meta.created_at,
        }
    }
}

/// One route binding: typed IDs into gateway resources (consumers join names via the
/// resource endpoints — F2's dashboard data already carries them).
#[derive(Debug, Serialize, ToSchema)]
pub struct RouteBindingView {
    pub id: uuid::Uuid,
    pub name: String,
    pub api_definition_id: uuid::Uuid,
    pub route_config_id: uuid::Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listener_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtual_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<fp_domain::api_lifecycle::ApiRouteBinding> for RouteBindingView {
    fn from(value: fp_domain::api_lifecycle::ApiRouteBinding) -> Self {
        Self {
            id: value.id.as_uuid(),
            name: value.name,
            api_definition_id: value.api_definition_id.as_uuid(),
            route_config_id: value.route_config_id.as_uuid(),
            listener_id: value.listener_id.map(|id| id.as_uuid()),
            virtual_host: value.virtual_host,
            route: value.route,
            created_at: value.created_at,
        }
    }
}

/// One review event, rendered verbatim from the append-only history.
#[derive(Debug, Serialize, ToSchema)]
pub struct SpecReviewEventView {
    pub id: uuid::Uuid,
    /// One of `submitted`, `reviewed`, `rejected`, `published`, `unpublished`.
    pub decision: String,
    pub actor_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<uuid::Uuid>,
    pub reason: String,
    pub metadata: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<fp_domain::api_lifecycle::SpecVersionReviewEvent> for SpecReviewEventView {
    fn from(value: fp_domain::api_lifecycle::SpecVersionReviewEvent) -> Self {
        Self {
            id: value.id.as_uuid(),
            decision: value.decision.as_str().into(),
            actor_type: value.actor_type,
            actor_id: value.actor_id,
            reason: value.reason,
            metadata: value.metadata,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublishSpecView {
    pub spec: SpecVersionSummary,
    pub tool_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiToolView {
    pub id: uuid::Uuid,
    pub name: String,
    pub api_definition_id: uuid::Uuid,
    pub spec_version_id: uuid::Uuid,
    pub operation_id: String,
    pub method: String,
    pub path: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ApiTool> for ApiToolView {
    fn from(value: ApiTool) -> Self {
        Self {
            id: value.id.as_uuid(),
            name: value.name,
            api_definition_id: value.api_definition_id.as_uuid(),
            spec_version_id: value.spec_version_id.as_uuid(),
            operation_id: value.operation_id,
            method: value.method.as_str().into(),
            path: value.path,
            input_schema: value.input_schema,
            output_schema: value.output_schema,
            enabled: value.enabled,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateApiToolBody {
    pub enabled: bool,
}

impl From<svc::PublishSpecResult> for PublishSpecView {
    fn from(value: svc::PublishSpecResult) -> Self {
        Self {
            spec: SpecVersionSummary {
                id: value.spec.id.as_uuid(),
                version: value.spec.version,
                source_kind: value.spec.source_kind.as_str().into(),
                format: value.spec.format.as_str().into(),
                spec_hash: value.spec.spec_hash,
                created_at: value.spec.created_at,
            },
            tool_count: value.tool_count,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecReviewBody {
    #[serde(default)]
    pub reason: String,
}

impl From<svc::ApiStatus> for ApiStatusView {
    fn from(value: svc::ApiStatus) -> Self {
        Self {
            api: ApiDefinitionView::from(value.api),
            latest_spec: value.latest_spec.map(|spec| SpecVersionSummary {
                id: spec.id.as_uuid(),
                version: spec.version,
                source_kind: spec.source_kind.as_str().into(),
                format: spec.format.as_str().into(),
                spec_hash: spec.spec_hash,
                created_at: spec.created_at,
            }),
            tool_count: value.tool_count,
            route_binding_count: value.route_binding_count,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApiBody {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openapi: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_binding: Option<CreateRouteBindingBody>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRouteBindingBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub route_config_id: uuid::Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}

impl CreateApiBody {
    fn into_service(self) -> svc::CreateApiInput {
        let route_binding_name = self.route_binding.as_ref().and_then(|b| b.name.clone());
        let route_binding = self.route_binding.map(|b| ApiRouteBindingSpec {
            route_config_id: RouteConfigId::from(b.route_config_id),
            listener_id: b.listener_id.map(ListenerId::from),
            virtual_host: b.virtual_host,
            route: b.route,
        });
        svc::CreateApiInput {
            name: self.name,
            definition: ApiDefinitionSpec {
                display_name: self.display_name,
                description: self.description,
            },
            imported_spec: self.openapi,
            route_binding_name,
            route_binding,
        }
    }
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/api-definitions",
    tag = "ApiDefinitions",
    params(("team" = String, Path, description = "Team name or UUID"), ListQuery),
    responses(
        (status = 200, body = Page<ApiDefinitionListItemView>),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn list_apis(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<ApiDefinitionListItemView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_apis_overview(&state.pool, &ctx, team, query.limit, query.offset, rid).await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items
            .into_iter()
            .map(ApiDefinitionListItemView::from)
            .collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/api-definitions",
    tag = "ApiDefinitions",
    params(("team" = String, Path, description = "Team name or UUID")),
    request_body = CreateApiBody,
    responses(
        (status = 201, body = ApiStatusView),
        (status = 400, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    ))]
pub async fn create_api(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<CreateApiBody>,
) -> Result<(axum::http::StatusCode, Json<ApiStatusView>), ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::create_api(&state.pool, &ctx, team, body.into_service(), rid).await
    };
    let created = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(ApiStatusView::from(created)),
    ))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/api-definitions/{name}",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
    ),
    responses(
        (status = 200, body = ApiDefinitionView),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn get_api(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<ApiDefinitionView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_api(&state.pool, &ctx, team, &name, rid).await
    };
    run.await
        .map(|v| Json(ApiDefinitionView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/api-definitions/{name}/status",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
    ),
    responses(
        (status = 200, body = ApiStatusView),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn api_status(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<ApiStatusView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::api_status(&state.pool, &ctx, team, &name, rid).await
    };
    run.await
        .map(|v| Json(ApiStatusView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/api-definitions/{name}/specs",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
        ListQuery,
    ),
    responses(
        (status = 200, body = Page<SpecVersionListItemView>),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn list_spec_versions(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<SpecVersionListItemView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_spec_versions(
            &state.pool,
            &ctx,
            team,
            &name,
            query.limit,
            query.offset,
            rid,
        )
        .await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items
            .into_iter()
            .map(SpecVersionListItemView::from)
            .collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/api-definitions/{name}/specs/{version}/events",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
        ("version" = i64, Path, description = "Spec version"),
        ListQuery,
    ),
    responses(
        (status = 200, body = Page<SpecReviewEventView>),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn list_spec_review_events(
    State(state): State<AppState>,
    Path((team, name, version)): Path<(String, String, i64)>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<SpecReviewEventView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_spec_review_events(
            &state.pool,
            &ctx,
            team,
            svc::SpecEventsQuery {
                api: name,
                version,
                limit: query.limit,
                offset: query.offset,
            },
            rid,
        )
        .await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(SpecReviewEventView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/api-definitions/{name}/specs/{version}/content",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
        ("version" = i64, Path, description = "Spec version"),
        ("If-None-Match" = Option<String>, Header, description = "Prior ETag; matching hash returns 304"),
    ),
    responses(
        (status = 200, body = serde_json::Value,
            description = "Canonical serde_json encoding of the stored document — the bytes spec_hash was computed over. ETag: \"<spec_hash>\"; Cache-Control: private, no-store."),
        (status = 304, description = "If-None-Match matched the current spec_hash"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn get_spec_version_content(
    State(state): State<AppState>,
    Path((team, name, version)): Path<(String, String, i64)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::get_spec_version_content(&state.pool, &ctx, team, &name, version, rid).await
    };
    let spec = run.await.map_err(|e| ApiError::new(e, rid))?;
    let etag = format!("\"{}\"", spec.spec_hash);
    let base_headers = [
        (axum::http::header::ETAG, etag.clone()),
        (
            axum::http::header::CACHE_CONTROL,
            "private, no-store".to_string(),
        ),
    ];
    let revalidated = headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|raw| {
            raw.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*"
                    || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
                    || candidate.trim_matches('"') == spec.spec_hash
            })
        });
    if revalidated {
        return Ok((axum::http::StatusCode::NOT_MODIFIED, base_headers).into_response());
    }
    Ok((base_headers, Json(spec.spec)).into_response())
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/api-definitions/{name}/route-bindings",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
        ListQuery,
    ),
    responses(
        (status = 200, body = Page<RouteBindingView>),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn list_route_bindings(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<RouteBindingView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_route_bindings(
            &state.pool,
            &ctx,
            team,
            &name,
            query.limit,
            query.offset,
            rid,
        )
        .await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(RouteBindingView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(get, path = "/api/v1/teams/{team}/api-definitions/{name}/tools",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
        ListQuery,
    ),
    responses(
        (status = 200, body = Page<ApiToolView>,
            description = "Generated tools for this API including disabled rows; MCP tools/list continues to serve only enabled tools of the published version."),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn list_api_tools(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Query(query): Query<ListQuery>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Page<ApiToolView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::list_api_tools(
            &state.pool,
            &ctx,
            team,
            &name,
            query.limit,
            query.offset,
            rid,
        )
        .await
    };
    let (items, total) = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(Page {
        items: items.into_iter().map(ApiToolView::from).collect(),
        total,
        limit: query.limit.clamp(1, 500),
        offset: query.offset.max(0),
    }))
}

#[utoipa::path(patch, path = "/api/v1/teams/{team}/mcp/tools/{name}",
    tag = "McpTools",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "Generated api_tools.name"),
    ),
    request_body = UpdateApiToolBody,
    responses(
        (status = 200, body = ApiToolView),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    ))]
pub async fn update_mcp_tool(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<UpdateApiToolBody>,
) -> Result<Json<ApiToolView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::update_api_tool_enabled(&state.pool, &ctx, team, &name, body.enabled, rid).await
    };
    run.await
        .map(|tool| Json(ApiToolView::from(tool)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/api-definitions/{name}/specs/{version}/reject",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
        ("version" = i64, Path, description = "Spec version"),
    ),
    request_body = SpecReviewBody,
    responses(
        (status = 200, body = SpecVersionSummary),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn reject_spec_version(
    State(state): State<AppState>,
    Path((team, name, version)): Path<(String, String, i64)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<SpecReviewBody>,
) -> Result<Json<SpecVersionSummary>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::reject_spec_version(
            &state.pool,
            &ctx,
            team,
            svc::SpecReviewInput {
                api: name,
                version,
                reason: body.reason,
            },
            rid,
        )
        .await
    };
    let spec = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(SpecVersionSummary {
        id: spec.id.as_uuid(),
        version: spec.version,
        source_kind: spec.source_kind.as_str().into(),
        format: spec.format.as_str().into(),
        spec_hash: spec.spec_hash,
        created_at: spec.created_at,
    }))
}

#[utoipa::path(post, path = "/api/v1/teams/{team}/api-definitions/{name}/specs/{version}/publish",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
        ("version" = i64, Path, description = "Spec version"),
    ),
    request_body = SpecReviewBody,
    responses(
        (status = 200, body = PublishSpecView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn publish_spec_version(
    State(state): State<AppState>,
    Path((team, name, version)): Path<(String, String, i64)>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    ApiJson(body): ApiJson<SpecReviewBody>,
) -> Result<Json<PublishSpecView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::publish_spec_version(
            &state.pool,
            &ctx,
            team,
            svc::SpecReviewInput {
                api: name,
                version,
                reason: body.reason,
            },
            rid,
        )
        .await
    };
    run.await
        .map(|v| Json(PublishSpecView::from(v)))
        .map_err(|e| ApiError::new(e, rid))
}

#[utoipa::path(delete, path = "/api/v1/teams/{team}/api-definitions/{name}",
    tag = "ApiDefinitions",
    params(
        ("team" = String, Path, description = "Team name or UUID"),
        ("name" = String, Path, description = "API name"),
        ("If-Match" = i64, Header, description = "Current API revision"),
    ),
    responses(
        (status = 204),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    ))]
pub async fn delete_api(
    State(state): State<AppState>,
    Path((team, name)): Path<(String, String)>,
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<axum::http::StatusCode, ApiError> {
    let run = async {
        let revision = revision_from(&headers)?;
        let team = resolve_team(&state, &ctx, &team).await?;
        svc::delete_api(&state.pool, &ctx, team, &name, revision, rid).await
    };
    run.await
        .map(|_| axum::http::StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::new(e, rid))
}
