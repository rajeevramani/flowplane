//! MCP route handlers for enabling/disabling MCP on routes
//!
//! These handlers allow users to control MCP enablement at the route level.

mod types;

use crate::api::error::ApiError;
use crate::api::routes::ApiState;
use crate::auth::authorization::require_resource_access;
use crate::auth::models::AuthContext;
use crate::services::mcp_service::{EnableMcpRequest, McpService, McpServiceError};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::instrument;

pub use types::{
    BulkMcpDisableRequest, BulkMcpDisableResponse, BulkMcpEnableRequest, BulkMcpEnableResponse,
    EnableMcpRequestBody, McpStatusResponse, RefreshSchemaResponse,
};

/// Path parameters for route-level MCP endpoints
#[derive(Debug, Deserialize)]
pub struct TeamRoutePath {
    pub team: String,
    pub route_id: String,
}

/// Path parameters for team-level MCP endpoints
#[derive(Debug, Deserialize)]
pub struct TeamPath {
    pub team: String,
}

/// Convert McpServiceError to ApiError
fn to_api_error(e: McpServiceError) -> ApiError {
    match e {
        McpServiceError::NotFound(msg) => ApiError::NotFound(msg),
        McpServiceError::Validation(msg) => ApiError::BadRequest(msg),
        McpServiceError::Database(e) => ApiError::internal(format!("Database error: {}", e)),
        McpServiceError::Internal(msg) => ApiError::internal(msg),
    }
}

/// Get the database pool from ApiState
fn get_db_pool(state: &ApiState) -> Result<Arc<crate::storage::DbPool>, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database not available"))?;
    Ok(Arc::new(cluster_repo.pool().clone()))
}

/// Get MCP status for a route
///
/// Returns whether MCP is enabled on a route, its readiness status,
/// and any missing required fields.
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/routes/{route_id}/mcp/status",
    params(
        ("team" = String, Path, description = "Team name"),
        ("route_id" = String, Path, description = "Route ID")
    ),
    responses(
        (status = 200, description = "MCP status", body = McpStatusResponse),
        (status = 404, description = "Route not found"),
        (status = 403, description = "Access denied")
    ),
    tag = "mcp"
)]
#[instrument(skip(state, context), fields(team = %team, route_id = %route_id, user_id = ?context.user_id))]
pub async fn get_mcp_status_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamRoutePath { team, route_id }): Path<TeamRoutePath>,
) -> Result<Json<McpStatusResponse>, ApiError> {
    // Verify team access
    require_resource_access(&context, "mcp", "read", Some(&team))?;

    let db_pool = get_db_pool(&state)?;
    let service = McpService::new(db_pool);
    let status = service.get_status(&team, &route_id).await.map_err(to_api_error)?;

    Ok(Json(McpStatusResponse::from(status)))
}

/// Enable MCP on a route
///
/// Creates an MCP tool for the specified route. The route must have
/// complete metadata (operation_id, summary, description) for enablement.
#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/routes/{route_id}/mcp/enable",
    params(
        ("team" = String, Path, description = "Team name"),
        ("route_id" = String, Path, description = "Route ID")
    ),
    request_body = EnableMcpRequestBody,
    responses(
        (status = 201, description = "MCP enabled successfully", body = crate::api::handlers::mcp_tools::McpToolResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Route not found"),
        (status = 403, description = "Access denied")
    ),
    tag = "mcp"
)]
#[instrument(skip(state, context, body), fields(team = %team, route_id = %route_id, user_id = ?context.user_id))]
pub async fn enable_mcp_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamRoutePath { team, route_id }): Path<TeamRoutePath>,
    Json(body): Json<EnableMcpRequestBody>,
) -> Result<(StatusCode, Json<crate::api::handlers::mcp_tools::McpToolResponse>), ApiError> {
    // Verify team access
    require_resource_access(&context, "mcp", "write", Some(&team))?;

    let db_pool = get_db_pool(&state)?;
    let service = McpService::new(db_pool);
    let request = EnableMcpRequest {
        tool_name: body.tool_name,
        description: body.description,
        schema_source: body.schema_source,
        summary: body.summary,
        http_method: body.http_method,
    };

    let tool = service.enable(&team, &route_id, request).await.map_err(to_api_error)?;

    Ok((StatusCode::CREATED, Json(tool.into())))
}

/// Disable MCP on a route
///
/// Soft-disables the MCP tool for the specified route. The tool
/// definition is preserved but won't appear in tools/list.
#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/routes/{route_id}/mcp/disable",
    params(
        ("team" = String, Path, description = "Team name"),
        ("route_id" = String, Path, description = "Route ID")
    ),
    responses(
        (status = 204, description = "MCP disabled successfully"),
        (status = 404, description = "Route or MCP tool not found"),
        (status = 403, description = "Access denied")
    ),
    tag = "mcp"
)]
#[instrument(skip(state, context), fields(team = %team, route_id = %route_id, user_id = ?context.user_id))]
pub async fn disable_mcp_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamRoutePath { team, route_id }): Path<TeamRoutePath>,
) -> Result<StatusCode, ApiError> {
    // Verify team access
    require_resource_access(&context, "mcp", "write", Some(&team))?;

    let db_pool = get_db_pool(&state)?;
    let service = McpService::new(db_pool);
    service.disable(&team, &route_id).await.map_err(to_api_error)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Refresh MCP schema from learning module
///
/// Updates the route metadata with the latest learned schema
/// from the learning module's aggregated schemas.
#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/routes/{route_id}/mcp/refresh",
    params(
        ("team" = String, Path, description = "Team name"),
        ("route_id" = String, Path, description = "Route ID")
    ),
    responses(
        (status = 200, description = "Schema refresh result", body = RefreshSchemaResponse),
        (status = 404, description = "Route not found"),
        (status = 403, description = "Access denied")
    ),
    tag = "mcp"
)]
#[instrument(skip(state, context), fields(team = %team, route_id = %route_id, user_id = ?context.user_id))]
pub async fn refresh_mcp_schema_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamRoutePath { team, route_id }): Path<TeamRoutePath>,
) -> Result<Json<RefreshSchemaResponse>, ApiError> {
    // Verify team access
    require_resource_access(&context, "mcp", "write", Some(&team))?;

    let db_pool = get_db_pool(&state)?;
    let service = McpService::new(db_pool);
    let result = service.refresh_schema(&team, &route_id).await.map_err(to_api_error)?;

    Ok(Json(RefreshSchemaResponse::from(result)))
}

/// Bulk enable MCP on multiple routes
///
/// Enables MCP on multiple routes at once. Failed routes are reported
/// in the response without stopping the entire operation.
#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/mcp/bulk-enable",
    params(
        ("team" = String, Path, description = "Team name")
    ),
    request_body = BulkMcpEnableRequest,
    responses(
        (status = 200, description = "Bulk enable results", body = BulkMcpEnableResponse),
        (status = 403, description = "Access denied")
    ),
    tag = "mcp"
)]
#[instrument(skip(state, context, body), fields(team = %team, user_id = ?context.user_id))]
pub async fn bulk_enable_mcp_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Json(body): Json<BulkMcpEnableRequest>,
) -> Result<Json<BulkMcpEnableResponse>, ApiError> {
    // Verify team access
    require_resource_access(&context, "mcp", "write", Some(&team))?;

    let db_pool = get_db_pool(&state)?;
    let service = McpService::new(db_pool);
    let mut results = Vec::new();
    let mut succeeded = 0;
    let mut failed = 0;

    for route_id in body.route_ids {
        let request = EnableMcpRequest {
            tool_name: None,
            description: None,
            schema_source: None,
            summary: None,
            http_method: None,
        };

        match service.enable(&team, &route_id, request).await {
            Ok(tool) => {
                results.push(types::BulkEnableResult {
                    route_id,
                    success: true,
                    tool_name: Some(tool.name),
                    error: None,
                });
                succeeded += 1;
            }
            Err(e) => {
                results.push(types::BulkEnableResult {
                    route_id,
                    success: false,
                    tool_name: None,
                    error: Some(e.to_string()),
                });
                failed += 1;
            }
        }
    }

    Ok(Json(BulkMcpEnableResponse { results, succeeded, failed }))
}

/// Bulk disable MCP on multiple routes
///
/// Disables MCP on multiple routes at once. Failed routes are reported
/// in the response without stopping the entire operation.
#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/mcp/bulk-disable",
    params(
        ("team" = String, Path, description = "Team name")
    ),
    request_body = BulkMcpDisableRequest,
    responses(
        (status = 200, description = "Bulk disable results", body = BulkMcpDisableResponse),
        (status = 403, description = "Access denied")
    ),
    tag = "mcp"
)]
#[instrument(skip(state, context, body), fields(team = %team, user_id = ?context.user_id))]
pub async fn bulk_disable_mcp_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Json(body): Json<BulkMcpDisableRequest>,
) -> Result<Json<BulkMcpDisableResponse>, ApiError> {
    // Verify team access
    require_resource_access(&context, "mcp", "write", Some(&team))?;

    let db_pool = get_db_pool(&state)?;
    let service = McpService::new(db_pool);
    let mut results = Vec::new();
    let mut succeeded = 0;
    let mut failed = 0;

    for route_id in body.route_ids {
        match service.disable(&team, &route_id).await {
            Ok(()) => {
                results.push(types::BulkDisableResult { route_id, success: true, error: None });
                succeeded += 1;
            }
            Err(e) => {
                results.push(types::BulkDisableResult {
                    route_id,
                    success: false,
                    error: Some(e.to_string()),
                });
                failed += 1;
            }
        }
    }

    Ok(Json(BulkMcpDisableResponse { results, succeeded, failed }))
}
