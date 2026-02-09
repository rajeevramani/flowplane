//! Audit log API handlers
//!
//! This module provides admin-only endpoints for querying audit logs.

use axum::{
    extract::{Query, State},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tracing::instrument;
use utoipa::IntoParams;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::{authorization::has_admin_bypass, models::AuthContext},
    storage::repositories::{AuditLogEntry, AuditLogFilters, AuditLogRepository},
};

/// Query parameters for listing audit logs
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListAuditLogsQuery {
    /// Filter by resource type (e.g., "auth.token", "platform.api", "secrets")
    #[param(required = false, example = "auth.token")]
    pub resource_type: Option<String>,
    /// Filter by action
    #[param(required = false, example = "create")]
    pub action: Option<String>,
    /// Filter by user ID
    #[param(required = false)]
    pub user_id: Option<String>,
    /// Filter by organization ID
    #[param(required = false)]
    pub org_id: Option<String>,
    /// Filter by team ID
    #[param(required = false)]
    pub team_id: Option<String>,
    /// Filter by start date (ISO 8601 format)
    #[param(required = false, example = "2024-01-01T00:00:00Z")]
    pub start_date: Option<String>,
    /// Filter by end date (ISO 8601 format)
    #[param(required = false, example = "2024-12-31T23:59:59Z")]
    pub end_date: Option<String>,
    /// Maximum number of results (default: 50, max: 1000)
    #[param(required = false, example = 50)]
    pub limit: Option<i32>,
    /// Pagination offset (default: 0)
    #[param(required = false, example = 0)]
    pub offset: Option<i32>,
}

use super::pagination::PaginatedResponse;

/// List audit logs with optional filtering and pagination
///
/// **Admin only**: This endpoint requires admin:all scope
#[utoipa::path(
    get,
    path = "/api/v1/audit-logs",
    params(ListAuditLogsQuery),
    responses(
        (status = 200, description = "Audit logs retrieved successfully", body = PaginatedResponse<AuditLogEntry>),
        (status = 400, description = "Invalid query parameters"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin access required"),
        (status = 503, description = "Service unavailable")
    ),
    security(("bearerAuth" = ["admin:all"])),
    tag = "Administration"
)]
#[instrument(skip(state), fields(user_id = ?auth_context.user_id, resource_type = ?query.resource_type, action = ?query.action))]
pub async fn list_audit_logs(
    State(state): State<ApiState>,
    Extension(auth_context): Extension<AuthContext>,
    Query(query): Query<ListAuditLogsQuery>,
) -> Result<Json<PaginatedResponse<AuditLogEntry>>, ApiError> {
    // Check admin privileges
    if !has_admin_bypass(&auth_context) {
        return Err(ApiError::forbidden("Admin privileges required to access audit logs"));
    }

    // Get repository from state
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Cluster repository not available"))?;

    let pool = cluster_repo.pool().clone();
    let audit_repo = AuditLogRepository::new(pool);

    // Parse date filters
    let start_date = if let Some(date_str) = query.start_date {
        Some(
            DateTime::parse_from_rfc3339(&date_str)
                .map_err(|e| ApiError::BadRequest(format!("Invalid start_date format: {}", e)))?
                .with_timezone(&Utc),
        )
    } else {
        None
    };

    let end_date = if let Some(date_str) = query.end_date {
        Some(
            DateTime::parse_from_rfc3339(&date_str)
                .map_err(|e| ApiError::BadRequest(format!("Invalid end_date format: {}", e)))?
                .with_timezone(&Utc),
        )
    } else {
        None
    };

    // Build filters
    let filters = AuditLogFilters {
        resource_type: query.resource_type,
        action: query.action,
        user_id: query.user_id,
        org_id: query.org_id,
        team_id: query.team_id,
        start_date,
        end_date,
    };

    // Query logs
    let entries = audit_repo
        .query_logs(Some(filters.clone()), query.limit, query.offset)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to query audit logs: {}", e)))?;

    // Get total count for pagination
    let total = audit_repo
        .count_logs(Some(filters))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to count audit logs: {}", e)))?;

    let limit = query.limit.unwrap_or(50).min(1000) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    Ok(Json(PaginatedResponse::new(entries, total, limit, offset)))
}
