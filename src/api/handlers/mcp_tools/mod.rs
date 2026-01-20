//! MCP tool management HTTP handlers
//!
//! Provides REST API endpoints for managing MCP tools, which represent callable
//! tools exposed to AI assistants via the Model Context Protocol.

pub mod types;

pub use types::{ListMcpToolsQuery, ListMcpToolsResponse, McpToolResponse, UpdateMcpToolBody};

use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use tracing::instrument;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::{authorization::require_resource_access, models::AuthContext},
    storage::UpdateMcpToolRequest,
};

// === Handler Implementations ===

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/mcp/tools",
    params(
        ("team" = String, Path, description = "Team name"),
        ListMcpToolsQuery
    ),
    responses(
        (status = 200, description = "List of MCP tools", body = ListMcpToolsResponse),
        (status = 403, description = "Forbidden - insufficient permissions"),
    ),
    tag = "MCP Tools"
)]
#[instrument(skip(state), fields(team = %team, user_id = ?context.user_id))]
pub async fn list_mcp_tools_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    Query(query): Query<ListMcpToolsQuery>,
) -> Result<Json<ListMcpToolsResponse>, ApiError> {
    // Authorization: require mcp:read scope
    require_resource_access(&context, "mcp", "read", Some(&team))?;

    let repo = state
        .xds_state
        .mcp_tool_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("MCP tool repository unavailable"))?;

    // Get all tools for the team
    let enabled_only = query.enabled.unwrap_or(false);
    let mut tools = repo.list_by_team(&team, enabled_only).await.map_err(ApiError::from)?;

    // Apply category filter if specified
    if let Some(category) = query.category {
        tools.retain(|t| t.category == category);
    }

    // Apply search filter if specified
    if let Some(ref search) = query.search {
        let search_lower = search.to_lowercase();
        tools.retain(|t| t.name.to_lowercase().contains(&search_lower));
    }

    // Calculate pagination
    let total = tools.len() as i64;
    let limit = query.limit.unwrap_or(100).min(1000); // Cap at 1000
    let offset = query.offset.unwrap_or(0);

    // Apply pagination
    let start = offset as usize;
    let end = (offset + limit) as usize;
    let paginated_tools =
        if start < tools.len() { tools[start..end.min(tools.len())].to_vec() } else { vec![] };

    let responses: Vec<McpToolResponse> =
        paginated_tools.into_iter().map(McpToolResponse::from).collect();

    Ok(Json(ListMcpToolsResponse { tools: responses, total, limit, offset }))
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/mcp/tools/{name}",
    params(
        ("team" = String, Path, description = "Team name"),
        ("name" = String, Path, description = "Tool name"),
    ),
    responses(
        (status = 200, description = "MCP tool details", body = McpToolResponse),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Tool not found"),
    ),
    tag = "MCP Tools"
)]
#[instrument(skip(state), fields(team = %team, tool_name = %name, user_id = ?context.user_id))]
pub async fn get_mcp_tool_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((team, name)): Path<(String, String)>,
) -> Result<Json<McpToolResponse>, ApiError> {
    // Authorization: require mcp:read scope
    require_resource_access(&context, "mcp", "read", Some(&team))?;

    let repo = state
        .xds_state
        .mcp_tool_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("MCP tool repository unavailable"))?;

    let tool = repo.get_by_name(&team, &name).await.map_err(ApiError::from)?.ok_or_else(|| {
        ApiError::NotFound(format!("MCP tool '{}' not found in team '{}'", name, team))
    })?;

    // Verify team ownership (should already be correct from get_by_name, but double-check)
    if tool.team != team {
        return Err(ApiError::NotFound(format!(
            "MCP tool '{}' not found in team '{}'",
            name, team
        )));
    }

    Ok(Json(McpToolResponse::from(tool)))
}

#[utoipa::path(
    patch,
    path = "/api/v1/teams/{team}/mcp/tools/{name}",
    params(
        ("team" = String, Path, description = "Team name"),
        ("name" = String, Path, description = "Tool name"),
    ),
    request_body = UpdateMcpToolBody,
    responses(
        (status = 200, description = "MCP tool updated", body = McpToolResponse),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Tool not found"),
    ),
    tag = "MCP Tools"
)]
#[instrument(skip(state, payload), fields(team = %team, tool_name = %name, user_id = ?context.user_id))]
pub async fn update_mcp_tool_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((team, name)): Path<(String, String)>,
    Json(payload): Json<UpdateMcpToolBody>,
) -> Result<Json<McpToolResponse>, ApiError> {
    // Authorization: require mcp:write scope
    require_resource_access(&context, "mcp", "write", Some(&team))?;

    let repo = state
        .xds_state
        .mcp_tool_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("MCP tool repository unavailable"))?;

    // Get existing tool to verify ownership and get ID
    let existing =
        repo.get_by_name(&team, &name).await.map_err(ApiError::from)?.ok_or_else(|| {
            ApiError::NotFound(format!("MCP tool '{}' not found in team '{}'", name, team))
        })?;

    // Verify team ownership
    if existing.team != team {
        return Err(ApiError::NotFound(format!(
            "MCP tool '{}' not found in team '{}'",
            name, team
        )));
    }

    // Build update request - pass all provided fields
    let update_request = UpdateMcpToolRequest {
        name: payload.name,
        description: payload.description.map(Some),
        category: payload.category,
        source_type: None,
        input_schema: payload.input_schema,
        output_schema: payload.output_schema.map(Some),
        learned_schema_id: None,
        schema_source: None,
        route_id: None,
        http_method: payload.http_method.map(Some),
        http_path: payload.http_path.map(Some),
        cluster_name: payload.cluster_name.map(Some),
        listener_port: payload.listener_port.map(Some),
        enabled: payload.enabled,
        confidence: None,
    };

    let updated = repo.update(&existing.id, update_request).await.map_err(ApiError::from)?;

    Ok(Json(McpToolResponse::from(updated)))
}

// === Learned Schema Handlers ===

pub use types::{
    ApplyLearnedSchemaRequest, ApplyLearnedSchemaResponse, CheckLearnedSchemaResponse,
    LearnedSchemaInfoResponse,
};

use std::sync::Arc;

use crate::domain::RouteId;
use crate::services::{McpService, McpServiceError};

/// Get the database pool from ApiState
fn get_db_pool(state: &ApiState) -> Result<Arc<sqlx::SqlitePool>, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Database not available".to_string()))?;
    Ok(Arc::new(cluster_repo.pool().clone()))
}

/// Convert McpServiceError to ApiError
fn to_api_error(e: McpServiceError) -> ApiError {
    match e {
        McpServiceError::NotFound(msg) => ApiError::NotFound(msg),
        McpServiceError::Validation(msg) => ApiError::BadRequest(msg),
        McpServiceError::Database(e) => ApiError::Internal(format!("Database error: {}", e)),
        McpServiceError::Internal(msg) => ApiError::Internal(msg),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/mcp/routes/{route_id}/learned-schema",
    params(
        ("team" = String, Path, description = "Team name"),
        ("route_id" = String, Path, description = "Route ID"),
    ),
    responses(
        (status = 200, description = "Learned schema availability", body = CheckLearnedSchemaResponse),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Route not found"),
    ),
    tag = "MCP Tools"
)]
#[instrument(skip(state), fields(team = %team, route_id = %route_id, user_id = ?context.user_id))]
pub async fn check_learned_schema_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((team, route_id)): Path<(String, String)>,
) -> Result<Json<CheckLearnedSchemaResponse>, ApiError> {
    // Authorization: require mcp:read scope
    require_resource_access(&context, "mcp", "read", Some(&team))?;

    let db_pool = get_db_pool(&state)?;
    let mcp_service = McpService::new(db_pool);
    let route_id = RouteId::from_string(route_id);

    let availability = mcp_service
        .check_learned_schema_availability(&team, &route_id)
        .await
        .map_err(to_api_error)?;

    Ok(Json(CheckLearnedSchemaResponse {
        available: availability.available,
        schema: availability.schema.map(|s| LearnedSchemaInfoResponse {
            id: s.id,
            confidence: s.confidence,
            sample_count: s.sample_count,
            version: s.version,
            last_observed: s.last_observed.to_rfc3339(),
        }),
        current_source: availability.current_source.to_string(),
        can_apply: availability.can_apply,
        requires_force: availability.requires_force,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/mcp/routes/{route_id}/apply-learned",
    params(
        ("team" = String, Path, description = "Team name"),
        ("route_id" = String, Path, description = "Route ID"),
    ),
    request_body = ApplyLearnedSchemaRequest,
    responses(
        (status = 200, description = "Learned schema applied", body = ApplyLearnedSchemaResponse),
        (status = 400, description = "Validation error - MCP not enabled or low confidence"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Route or learned schema not found"),
        (status = 409, description = "Conflict - requires force to override OpenAPI"),
    ),
    tag = "MCP Tools"
)]
#[instrument(skip(state, payload), fields(team = %team, route_id = %route_id, user_id = ?context.user_id))]
pub async fn apply_learned_schema_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((team, route_id)): Path<(String, String)>,
    Json(payload): Json<ApplyLearnedSchemaRequest>,
) -> Result<Json<ApplyLearnedSchemaResponse>, ApiError> {
    // Authorization: require mcp:write scope
    require_resource_access(&context, "mcp", "write", Some(&team))?;

    let db_pool = get_db_pool(&state)?;
    let mcp_service = McpService::new(db_pool);
    let route_id = RouteId::from_string(route_id);
    let force = payload.force.unwrap_or(false);

    let result =
        mcp_service.apply_learned_schema(&team, &route_id, force).await.map_err(|e| match &e {
            McpServiceError::Validation(msg) if msg.contains("force=true") => {
                ApiError::Conflict(msg.clone())
            }
            _ => to_api_error(e),
        })?;

    Ok(Json(ApplyLearnedSchemaResponse {
        success: true,
        previous_source: result.previous_source.to_string(),
        learned_schema_id: result.learned_schema_id,
        confidence: result.confidence,
        sample_count: result.sample_count,
    }))
}
