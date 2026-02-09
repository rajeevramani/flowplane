//! Reporting API handlers for platform visibility and monitoring
//!
//! This module provides comprehensive reporting endpoints that allow users to view
//! the state of their API gateway infrastructure including routes, clusters, listeners,
//! API definitions, and audit trails.
//!
//! All endpoints respect token-based RBAC:
//! - `admin:all` scope grants access to all resources across teams
//! - `reports:read` scope allows read access to reporting endpoints
//! - Team-scoped tokens (`team:{name}:read`) only see their team's resources

use axum::{
    extract::{Query, State},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::{IntoParams, ToSchema};

use crate::api::error::ApiError;
use crate::api::handlers::team_access::{get_effective_team_ids, team_repo_from_state};
use crate::api::routes::ApiState;
use crate::auth::authorization::require_resource_access;
use crate::auth::models::AuthContext;
use crate::storage::repositories::ReportingRepository;
use crate::xds::ClusterSpec;

// ============================================================================
// Request/Response DTOs
// ============================================================================

/// Query parameters for listing route flows with pagination
#[derive(Debug, Clone, Deserialize, Default, IntoParams, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListRouteFlowsQuery {
    /// Maximum number of items to return (1-1000, default: 50)
    pub limit: Option<i64>,
    /// Number of items to skip (default: 0)
    pub offset: Option<i64>,
    /// Filter by team name (optional, admin tokens can filter by any team)
    pub team: Option<String>,
}

/// Listener information in route flow
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteFlowListener {
    /// Listener name
    pub name: String,
    /// Listener port
    pub port: u16,
    /// Listener address (e.g., "0.0.0.0" or "127.0.0.1")
    pub address: String,
}

/// Single route flow entry showing end-to-end request routing
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteFlowEntry {
    /// Route configuration name
    pub route_name: String,
    /// Path pattern (e.g., "/api/v1/*", "/health")
    pub path: String,
    /// Target cluster name
    pub cluster: String,
    /// Cluster endpoints (host:port format)
    pub endpoints: Vec<String>,
    /// Listener that receives requests for this route
    pub listener: RouteFlowListener,
    /// Team that owns this route (None if no team assigned)
    pub team: Option<String>,
}

use super::pagination::PaginatedResponse;

// ============================================================================
// Handlers
// ============================================================================

/// List all route flows showing end-to-end request routing
///
/// This endpoint provides a comprehensive view of how requests flow through the system:
/// listener → route → cluster → endpoint
///
/// # Authorization
/// - Requires `reports:read` scope
/// - Team-scoped tokens only see their team's routes
/// - `admin:all` scope grants access to all routes across teams
#[utoipa::path(
    get,
    path = "/api/v1/reports/route-flows",
    params(ListRouteFlowsQuery),
    responses(
        (status = 200, description = "Route flows retrieved successfully", body = PaginatedResponse<RouteFlowEntry>),
        (status = 403, description = "Insufficient permissions"),
        (status = 503, description = "Service unavailable")
    ),
    security(("bearerAuth" = [])),
    tag = "System"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, limit = ?params.limit, team = ?params.team))]
pub async fn list_route_flows_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<ListRouteFlowsQuery>,
) -> Result<Json<PaginatedResponse<RouteFlowEntry>>, ApiError> {
    // Authorization: require reports:read scope
    // Team filtering will be applied at the repository level based on context
    require_resource_access(&context, "reports", "read", None)?;

    let limit = params.limit.unwrap_or(50).clamp(1, 1000);
    let offset = params.offset.unwrap_or(0).max(0);

    // Get repository from state
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Cluster repository unavailable"))?;

    let reporting_repo = ReportingRepository::new(cluster_repo.pool().clone());

    // Extract team IDs from auth context for filtering
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo).await?;

    // Fetch route flows from repository
    let (rows, total) = reporting_repo
        .list_route_flows(&team_scopes, limit, offset)
        .await
        .map_err(ApiError::from)?;

    // Convert database rows to API response format
    let route_flows = rows
        .into_iter()
        .filter_map(|row| {
            // Parse cluster configuration to extract endpoints
            let cluster_spec: ClusterSpec = match serde_json::from_str(&row.cluster_configuration) {
                Ok(spec) => spec,
                Err(e) => {
                    tracing::warn!(
                        cluster = %row.cluster_name,
                        error = %e,
                        "Failed to parse cluster configuration"
                    );
                    return None;
                }
            };

            // Extract endpoint strings from cluster spec
            let endpoints: Vec<String> = cluster_spec
                .endpoints
                .iter()
                .map(|ep| match ep {
                    crate::xds::EndpointSpec::Address { host, port } => {
                        format!("{}:{}", host, port)
                    }
                    crate::xds::EndpointSpec::String(s) => s.clone(),
                })
                .collect();

            Some(RouteFlowEntry {
                route_name: row.route_name,
                path: row.path_prefix,
                cluster: row.cluster_name,
                endpoints,
                listener: RouteFlowListener {
                    name: row.listener_name,
                    port: row.listener_port.unwrap_or(0) as u16,
                    address: row.listener_address,
                },
                team: row.route_team,
            })
        })
        .collect();

    Ok(Json(PaginatedResponse::new(route_flows, total, limit, offset)))
}
