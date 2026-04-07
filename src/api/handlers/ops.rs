//! Ops diagnostic REST handlers
//!
//! Team-scoped GET endpoints for gateway diagnostics. These wrap the typed
//! service functions in `services::ops_service` so the CLI and UI can access
//! the same diagnostic data that MCP tools provide.

use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    api::{
        error::ApiError,
        handlers::team_access::{
            get_db_pool, require_resource_access_resolved, resolve_team_name, TeamPath,
        },
        routes::ApiState,
    },
    auth::models::AuthContext,
    services::ops_service,
};

// =============================================================================
// Query parameter structs
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct TraceQuery {
    pub path: String,
    pub port: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyQuery {
    pub scope: Option<String>,
    pub name: Option<String>,
    pub limit: Option<i64>,
    pub include_details: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct XdsStatusQuery {
    pub dataplane: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NackHistoryQuery {
    pub dataplane: Option<String>,
    #[serde(rename = "type")]
    pub type_url: Option<String>,
    pub since: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub resource_type: Option<String>,
    pub action: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct LearningHealthPath {
    pub team: String,
    pub id: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// GET /api/v1/teams/{team}/ops/trace?path=...&port=...
pub async fn ops_trace_handler(
    State(state): State<ApiState>,
    Extension(auth_context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Query(query): Query<TraceQuery>,
) -> Result<Json<Value>, ApiError> {
    require_resource_access_resolved(&state, &auth_context, "routes", "read", Some(&team)).await?;
    let pool = get_db_pool(&state)?;
    let team_id = resolve_team_name(&state, &team, auth_context.org_id.as_ref()).await?;

    let result = ops_service::trace_request(&pool, &team_id, &query.path, query.port).await?;
    Ok(Json(
        serde_json::to_value(result)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {}", e)))?,
    ))
}

/// GET /api/v1/teams/{team}/ops/topology?scope=...&name=...&limit=...&includeDetails=...
pub async fn ops_topology_handler(
    State(state): State<ApiState>,
    Extension(auth_context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Query(query): Query<TopologyQuery>,
) -> Result<Json<Value>, ApiError> {
    require_resource_access_resolved(&state, &auth_context, "clusters", "read", Some(&team))
        .await?;
    let pool = get_db_pool(&state)?;
    let team_id = resolve_team_name(&state, &team, auth_context.org_id.as_ref()).await?;

    let result = ops_service::topology(
        &pool,
        &team_id,
        query.scope.as_deref(),
        query.name.as_deref(),
        query.limit,
        query.include_details.unwrap_or(false),
    )
    .await?;
    Ok(Json(
        serde_json::to_value(result)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {}", e)))?,
    ))
}

/// GET /api/v1/teams/{team}/ops/validate
pub async fn ops_validate_handler(
    State(state): State<ApiState>,
    Extension(auth_context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
) -> Result<Json<Value>, ApiError> {
    require_resource_access_resolved(&state, &auth_context, "clusters", "read", Some(&team))
        .await?;
    let pool = get_db_pool(&state)?;
    let team_id = resolve_team_name(&state, &team, auth_context.org_id.as_ref()).await?;

    let result = ops_service::config_validate(&pool, &team_id).await?;
    Ok(Json(
        serde_json::to_value(result)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {}", e)))?,
    ))
}

/// GET /api/v1/teams/{team}/ops/xds/status?dataplane=...
pub async fn ops_xds_status_handler(
    State(state): State<ApiState>,
    Extension(auth_context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Query(query): Query<XdsStatusQuery>,
) -> Result<Json<Value>, ApiError> {
    require_resource_access_resolved(&state, &auth_context, "clusters", "read", Some(&team))
        .await?;
    let pool = get_db_pool(&state)?;
    let team_id = resolve_team_name(&state, &team, auth_context.org_id.as_ref()).await?;

    let result =
        ops_service::xds_delivery_status(&pool, &team_id, query.dataplane.as_deref()).await?;
    Ok(Json(
        serde_json::to_value(result)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {}", e)))?,
    ))
}

/// GET /api/v1/teams/{team}/ops/xds/nacks?dataplane=...&type=...&since=...&limit=...
pub async fn ops_nack_history_handler(
    State(state): State<ApiState>,
    Extension(auth_context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Query(query): Query<NackHistoryQuery>,
) -> Result<Json<Value>, ApiError> {
    require_resource_access_resolved(&state, &auth_context, "clusters", "read", Some(&team))
        .await?;
    let pool = get_db_pool(&state)?;
    let team_id = resolve_team_name(&state, &team, auth_context.org_id.as_ref()).await?;

    let result = ops_service::nack_history(
        &pool,
        &team_id,
        query.dataplane.as_deref(),
        query.type_url.as_deref(),
        query.since.as_deref(),
        query.limit,
    )
    .await?;
    Ok(Json(
        serde_json::to_value(result)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {}", e)))?,
    ))
}

/// GET /api/v1/teams/{team}/ops/audit?resource_type=...&action=...&limit=...
pub async fn ops_audit_handler(
    State(state): State<ApiState>,
    Extension(auth_context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Value>, ApiError> {
    require_resource_access_resolved(&state, &auth_context, "audit", "read", Some(&team)).await?;
    let pool = get_db_pool(&state)?;
    let team_id = resolve_team_name(&state, &team, auth_context.org_id.as_ref()).await?;
    let org_id = auth_context.org_id.as_ref().map(|o| o.as_str());

    let result = ops_service::audit_query(
        &pool,
        &team_id,
        org_id,
        query.resource_type.as_deref(),
        query.action.as_deref(),
        query.limit,
    )
    .await?;
    Ok(Json(
        serde_json::to_value(result)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {}", e)))?,
    ))
}

/// GET /api/v1/teams/{team}/ops/learning/{id}/health
pub async fn ops_learning_health_handler(
    State(state): State<ApiState>,
    Extension(auth_context): Extension<AuthContext>,
    Path(path): Path<LearningHealthPath>,
) -> Result<Json<Value>, ApiError> {
    require_resource_access_resolved(
        &state,
        &auth_context,
        "learning-sessions",
        "read",
        Some(&path.team),
    )
    .await?;

    let xds_state = &state.xds_state;
    let org_id = auth_context.org_id.as_ref();

    let args = serde_json::json!({ "id": path.id });
    let result = crate::mcp::tools::learning::execute_ops_learning_session_health(
        xds_state, &path.team, org_id, args,
    )
    .await?;

    Ok(Json(
        serde_json::to_value(&result.content)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {}", e)))?,
    ))
}
