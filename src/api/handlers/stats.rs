//! Stats Dashboard HTTP handlers
//!
//! This module provides endpoints for the Envoy Stats Dashboard feature:
//! - Overview stats for the team dashboard
//! - Cluster-level stats
//! - Instance app management (enable/disable)

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::require_resource_access,
    auth::models::AuthContext,
    domain::{ClusterStats, EnvoyHealthStatus, StatsOverview, StatsSnapshot},
    services::{
        stats_data_source::{EnvoyAdminConfig, EnvoyAdminStats},
        team_stats_provider::{StatsProviderConfig, TeamStatsProvider},
    },
    storage::repositories::{
        app_ids, InstanceApp, InstanceAppRepository, SqlxInstanceAppRepository, SqlxTeamRepository,
    },
};

// === Response DTOs ===

/// Response for stats overview
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatsOverviewResponse {
    /// Team name
    pub team: String,
    /// Total requests per second
    pub total_rps: f64,
    /// Total active connections
    pub total_connections: u64,
    /// Error rate (0.0 - 1.0)
    pub error_rate: f64,
    /// P99 latency in milliseconds
    pub p99_latency_ms: f64,
    /// Number of healthy clusters
    pub healthy_clusters: u64,
    /// Number of degraded clusters
    pub degraded_clusters: u64,
    /// Number of unhealthy clusters
    pub unhealthy_clusters: u64,
    /// Total clusters
    pub total_clusters: u64,
    /// Overall health status
    pub health_status: String,
    /// When this data was collected
    pub timestamp: String,
}

impl StatsOverviewResponse {
    pub fn from_snapshot(team: &str, snapshot: &StatsSnapshot) -> Self {
        let overview = compute_overview(snapshot);
        Self {
            team: team.to_string(),
            total_rps: overview.total_rps,
            total_connections: overview.total_connections,
            error_rate: overview.error_rate,
            p99_latency_ms: overview.p99_latency_ms,
            healthy_clusters: overview.healthy_clusters,
            degraded_clusters: overview.degraded_clusters,
            unhealthy_clusters: overview.unhealthy_clusters,
            total_clusters: overview.total_clusters,
            health_status: format!("{:?}", snapshot.health_status).to_lowercase(),
            timestamp: snapshot.timestamp.to_rfc3339(),
        }
    }
}

/// Response for cluster stats list
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClustersStatsResponse {
    /// Team name
    pub team: String,
    /// Cluster stats
    pub clusters: Vec<ClusterStatsResponse>,
    /// Total count
    pub count: usize,
}

/// Single cluster stats response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterStatsResponse {
    /// Cluster name
    pub cluster_name: String,
    /// Health status
    pub health_status: String,
    /// Number of healthy hosts
    pub healthy_hosts: u64,
    /// Total hosts
    pub total_hosts: u64,
    /// Active connections
    pub active_connections: u64,
    /// Active requests
    pub active_requests: u64,
    /// Pending requests
    pub pending_requests: u64,
    /// Success rate (0.0 - 1.0)
    pub success_rate: Option<f64>,
    /// Circuit breaker open
    pub circuit_breaker_open: bool,
    /// Outlier ejections
    pub outlier_ejections: u64,
}

impl From<ClusterStats> for ClusterStatsResponse {
    fn from(c: ClusterStats) -> Self {
        let health_status = EnvoyHealthStatus::from_host_counts(c.healthy_hosts, c.total_hosts);
        Self {
            cluster_name: c.cluster_name,
            health_status: format!("{:?}", health_status).to_lowercase(),
            healthy_hosts: c.healthy_hosts,
            total_hosts: c.total_hosts,
            active_connections: c.upstream_cx_active,
            active_requests: c.upstream_rq_active,
            pending_requests: c.upstream_rq_pending,
            success_rate: c.success_rate,
            circuit_breaker_open: c.circuit_breaker_open,
            outlier_ejections: c.outlier_ejections,
        }
    }
}

/// Response for app status
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppStatusResponse {
    /// App ID
    pub app_id: String,
    /// Whether the app is enabled
    pub enabled: bool,
    /// App configuration
    pub config: Option<serde_json::Value>,
    /// Who enabled/disabled the app
    pub enabled_by: Option<String>,
    /// When the app was enabled
    pub enabled_at: Option<String>,
}

impl From<InstanceApp> for AppStatusResponse {
    fn from(app: InstanceApp) -> Self {
        Self {
            app_id: app.app_id,
            enabled: app.enabled,
            config: app.config,
            enabled_by: app.enabled_by,
            enabled_at: app.enabled_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Request to toggle app status
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetAppStatusRequest {
    /// Whether to enable the app
    pub enabled: bool,
    /// Optional configuration
    pub config: Option<serde_json::Value>,
}

/// Response for list of apps
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListAppsResponse {
    /// List of apps
    pub apps: Vec<AppStatusResponse>,
    /// Total count
    pub count: usize,
}

/// Check if stats dashboard is enabled response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatsEnabledResponse {
    /// Whether the stats dashboard is enabled
    pub enabled: bool,
}

// === Helper Functions ===

/// Helper to create a TeamStatsProvider from ApiState
async fn create_stats_provider(
    state: &ApiState,
) -> Result<
    TeamStatsProvider<EnvoyAdminStats, SqlxTeamRepository, SqlxInstanceAppRepository>,
    ApiError,
> {
    use std::sync::Arc;

    let pool = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database not available"))?
        .pool()
        .clone();

    let data_source =
        Arc::new(EnvoyAdminStats::new(EnvoyAdminConfig::default()).map_err(|e| {
            ApiError::internal(format!("Failed to create stats data source: {}", e))
        })?);

    let team_repo = Arc::new(SqlxTeamRepository::new(pool.clone()));
    let app_repo = Arc::new(SqlxInstanceAppRepository::new(pool));

    // Get base URL from environment or use default
    let envoy_admin_base_url =
        std::env::var("ENVOY_ADMIN_BASE_URL").unwrap_or_else(|_| "http://localhost".to_string());

    let config = StatsProviderConfig { envoy_admin_base_url, use_team_admin_port: true };

    Ok(TeamStatsProvider::new(data_source, state.stats_cache.clone(), team_repo, app_repo, config))
}

fn compute_overview(snapshot: &StatsSnapshot) -> StatsOverview {
    let total_requests = snapshot.response_codes.xx_2xx
        + snapshot.response_codes.xx_3xx
        + snapshot.response_codes.xx_4xx
        + snapshot.response_codes.xx_5xx;

    let error_rate = if total_requests > 0 {
        (snapshot.response_codes.xx_4xx + snapshot.response_codes.xx_5xx) as f64
            / total_requests as f64
    } else {
        0.0
    };

    let mut healthy_clusters = 0u64;
    let mut degraded_clusters = 0u64;
    let mut unhealthy_clusters = 0u64;

    for cluster in &snapshot.clusters {
        let status =
            EnvoyHealthStatus::from_host_counts(cluster.healthy_hosts, cluster.total_hosts);
        match status {
            EnvoyHealthStatus::Healthy => healthy_clusters += 1,
            EnvoyHealthStatus::Degraded => degraded_clusters += 1,
            EnvoyHealthStatus::Unhealthy => unhealthy_clusters += 1,
            EnvoyHealthStatus::Unknown => {}
        }
    }

    StatsOverview {
        total_rps: snapshot.requests.rps.unwrap_or(0.0),
        total_connections: snapshot.connections.downstream_cx_active,
        error_rate,
        p99_latency_ms: snapshot.latency.p99_ms.unwrap_or(0.0),
        healthy_clusters,
        degraded_clusters,
        unhealthy_clusters,
        total_clusters: snapshot.clusters.len() as u64,
    }
}

// === Stats Handlers ===

/// Check if stats dashboard is enabled
#[utoipa::path(
    get,
    path = "/api/v1/stats/enabled",
    tag = "System",
    responses(
        (status = 200, description = "Stats dashboard enabled status", body = StatsEnabledResponse),
        (status = 401, description = "Unauthorized")
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state), fields(user_id = ?context.user_id))]
pub async fn get_stats_enabled_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
) -> Result<Json<StatsEnabledResponse>, ApiError> {
    // Any authenticated user can check if stats is enabled
    require_resource_access(&context, "stats", "read", None)?;

    let pool = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database not available"))?
        .pool()
        .clone();

    let repo = SqlxInstanceAppRepository::new(pool);
    let enabled = repo.is_enabled(app_ids::STATS_DASHBOARD).await.map_err(ApiError::from)?;

    Ok(Json(StatsEnabledResponse { enabled }))
}

/// Get stats overview for a team
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/stats/overview",
    tag = "System",
    params(
        ("team" = String, Path, description = "Team name")
    ),
    responses(
        (status = 200, description = "Stats overview", body = StatsOverviewResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - stats dashboard not enabled or insufficient permissions"),
        (status = 404, description = "Team not found")
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state), fields(team = %team, user_id = ?context.user_id))]
pub async fn get_stats_overview_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
) -> Result<Json<StatsOverviewResponse>, ApiError> {
    // Check permission for this team
    require_resource_access(&context, "stats", "read", Some(&team))?;

    let provider = create_stats_provider(&state).await?;

    // This will check feature flag and fetch stats
    let snapshot = provider.get_stats(&team).await.map_err(|e| {
        // Convert FlowplaneError to appropriate ApiError
        match &e {
            crate::errors::FlowplaneError::NotFound { .. } => {
                ApiError::NotFound(format!("Team '{}' not found", team))
            }
            crate::errors::FlowplaneError::Internal { .. } => {
                // Check if it's because stats is disabled
                if e.to_string().contains("not enabled") {
                    ApiError::Forbidden(
                        "Stats dashboard is not enabled. Contact your administrator.".to_string(),
                    )
                } else {
                    ApiError::from(e)
                }
            }
            _ => ApiError::from(e),
        }
    })?;

    Ok(Json(StatsOverviewResponse::from_snapshot(&team, &snapshot)))
}

/// Get cluster stats for a team
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/stats/clusters",
    tag = "System",
    params(
        ("team" = String, Path, description = "Team name")
    ),
    responses(
        (status = 200, description = "Cluster stats", body = ClustersStatsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - stats dashboard not enabled or insufficient permissions"),
        (status = 404, description = "Team not found")
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state), fields(team = %team, user_id = ?context.user_id))]
pub async fn get_stats_clusters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
) -> Result<Json<ClustersStatsResponse>, ApiError> {
    // Check permission for this team
    require_resource_access(&context, "stats", "read", Some(&team))?;

    let provider = create_stats_provider(&state).await?;

    // This will check feature flag and fetch stats
    let clusters = provider.get_clusters(&team).await.map_err(|e| match &e {
        crate::errors::FlowplaneError::NotFound { .. } => {
            ApiError::NotFound(format!("Team '{}' not found", team))
        }
        crate::errors::FlowplaneError::Internal { .. } => {
            if e.to_string().contains("not enabled") {
                ApiError::Forbidden(
                    "Stats dashboard is not enabled. Contact your administrator.".to_string(),
                )
            } else {
                ApiError::from(e)
            }
        }
        _ => ApiError::from(e),
    })?;

    let cluster_responses: Vec<ClusterStatsResponse> =
        clusters.into_iter().map(ClusterStatsResponse::from).collect();
    let count = cluster_responses.len();

    Ok(Json(ClustersStatsResponse { team, clusters: cluster_responses, count }))
}

/// Get stats for a specific cluster
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/stats/clusters/{cluster}",
    tag = "System",
    params(
        ("team" = String, Path, description = "Team name"),
        ("cluster" = String, Path, description = "Cluster name")
    ),
    responses(
        (status = 200, description = "Cluster stats", body = ClusterStatsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - stats dashboard not enabled or insufficient permissions"),
        (status = 404, description = "Team or cluster not found")
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state), fields(team = %team, cluster = %cluster, user_id = ?context.user_id))]
pub async fn get_stats_cluster_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((team, cluster)): Path<(String, String)>,
) -> Result<Json<ClusterStatsResponse>, ApiError> {
    // Check permission for this team
    require_resource_access(&context, "stats", "read", Some(&team))?;

    let provider = create_stats_provider(&state).await?;

    // This will check feature flag and fetch stats
    let cluster_stats = provider.get_cluster(&team, &cluster).await.map_err(|e| match &e {
        crate::errors::FlowplaneError::NotFound { resource_type, .. } => {
            if resource_type == "Cluster" {
                ApiError::NotFound(format!("Cluster '{}' not found in team '{}'", cluster, team))
            } else {
                ApiError::NotFound(format!("Team '{}' not found", team))
            }
        }
        crate::errors::FlowplaneError::Internal { .. } => {
            if e.to_string().contains("not enabled") {
                ApiError::Forbidden(
                    "Stats dashboard is not enabled. Contact your administrator.".to_string(),
                )
            } else {
                ApiError::from(e)
            }
        }
        _ => ApiError::from(e),
    })?;

    Ok(Json(ClusterStatsResponse::from(cluster_stats)))
}

// === App Management Handlers (Admin Only) ===

/// List all instance apps
#[utoipa::path(
    get,
    path = "/api/v1/admin/apps",
    tag = "admin",
    responses(
        (status = 200, description = "List of apps", body = ListAppsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin access required")
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state), fields(user_id = ?context.user_id))]
pub async fn list_apps_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
) -> Result<Json<ListAppsResponse>, ApiError> {
    // Require admin access
    require_resource_access(&context, "admin", "read", None)?;

    let pool = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database not available"))?
        .pool()
        .clone();

    let repo = SqlxInstanceAppRepository::new(pool);
    let apps = repo.get_all_apps().await.map_err(ApiError::from)?;

    let app_responses: Vec<AppStatusResponse> =
        apps.into_iter().map(AppStatusResponse::from).collect();
    let count = app_responses.len();

    Ok(Json(ListAppsResponse { apps: app_responses, count }))
}

/// Get app status
#[utoipa::path(
    get,
    path = "/api/v1/admin/apps/{app_id}",
    tag = "admin",
    params(
        ("app_id" = String, Path, description = "App ID")
    ),
    responses(
        (status = 200, description = "App status", body = AppStatusResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin access required"),
        (status = 404, description = "App not found")
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state), fields(app_id = %app_id, user_id = ?context.user_id))]
pub async fn get_app_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(app_id): Path<String>,
) -> Result<Json<AppStatusResponse>, ApiError> {
    // Require admin access
    require_resource_access(&context, "admin", "read", None)?;

    let pool = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database not available"))?
        .pool()
        .clone();

    let repo = SqlxInstanceAppRepository::new(pool);
    let app = repo
        .get_app(&app_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("App '{}' not found", app_id)))?;

    Ok(Json(AppStatusResponse::from(app)))
}

/// Set app status (enable/disable)
#[utoipa::path(
    put,
    path = "/api/v1/admin/apps/{app_id}",
    tag = "admin",
    params(
        ("app_id" = String, Path, description = "App ID")
    ),
    request_body = SetAppStatusRequest,
    responses(
        (status = 200, description = "App status updated", body = AppStatusResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin access required")
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, body), fields(app_id = %app_id, user_id = ?context.user_id))]
pub async fn set_app_status_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(app_id): Path<String>,
    Json(body): Json<SetAppStatusRequest>,
) -> Result<Json<AppStatusResponse>, ApiError> {
    // Require admin access
    require_resource_access(&context, "admin", "write", None)?;

    let pool = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Database not available"))?
        .pool()
        .clone();

    let user_id = context.user_id.as_ref().map(|id| id.as_str()).unwrap_or("unknown");
    let repo = SqlxInstanceAppRepository::new(pool);

    let app = if body.enabled {
        repo.enable_app(&app_id, user_id, body.config).await.map_err(ApiError::from)?
    } else {
        repo.disable_app(&app_id, user_id).await.map_err(ApiError::from)?
    };

    tracing::info!(
        app_id = %app_id,
        enabled = %body.enabled,
        user_id = %user_id,
        "App status updated"
    );

    Ok(Json(AppStatusResponse::from(app)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ClusterStats;

    #[test]
    fn test_cluster_stats_response_from() {
        let stats = ClusterStats {
            cluster_name: "api-backend".to_string(),
            healthy_hosts: 3,
            total_hosts: 3,
            upstream_cx_active: 10,
            upstream_rq_active: 5,
            upstream_rq_pending: 2,
            success_rate: Some(0.95),
            circuit_breaker_open: false,
            outlier_ejections: 0,
            ..Default::default()
        };

        let response = ClusterStatsResponse::from(stats);

        assert_eq!(response.cluster_name, "api-backend");
        assert_eq!(response.health_status, "healthy");
        assert_eq!(response.active_connections, 10);
        assert_eq!(response.success_rate, Some(0.95));
    }

    #[test]
    fn test_overview_response_serialization() {
        let snapshot = StatsSnapshot::new("test-team".to_string());
        let response = StatsOverviewResponse::from_snapshot("test-team", &snapshot);

        let json = serde_json::to_string(&response).expect("serialize");
        assert!(json.contains("\"team\":\"test-team\""));
        assert!(json.contains("\"totalRps\""));
        assert!(json.contains("\"healthStatus\""));
    }
}
