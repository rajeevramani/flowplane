//! Route Views HTTP handlers
//!
//! This module provides endpoints for the route list view UI,
//! returning flattened route data extracted from configuration JSON.

use axum::{
    extract::{Query, State},
    Extension, Json,
};
use tracing::{error, instrument};

use crate::{
    api::{
        dto::route_view::{
            PaginationDto, RouteListQueryParams, RouteListResponseDto, RouteListStatsDto,
            RouteListViewDto,
        },
        error::ApiError,
        handlers::team_access::get_effective_team_scopes,
        routes::ApiState,
    },
    auth::authorization::require_resource_access,
    auth::models::AuthContext,
    errors::Error,
    services::route_view_extractor::RouteViewExtractor,
};

/// List routes with pagination, search, and filtering for UI display.
///
/// Returns a flattened view of routes with computed fields extracted from
/// configuration JSON. Supports pagination, search, and MCP filter query params.
///
/// This handler uses an optimized JOIN query to fetch all related data in a single
/// database call, avoiding the N+1 query problem.
#[utoipa::path(
    get,
    path = "/api/v1/route-views",
    params(RouteListQueryParams),
    responses(
        (status = 200, description = "Paginated list of routes with stats", body = RouteListResponseDto),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Routes"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, page = ?params.page, page_size = ?params.page_size, search = ?params.search))]
pub async fn list_route_views_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<RouteListQueryParams>,
) -> Result<Json<RouteListResponseDto>, ApiError> {
    // Authorization: require routes:read scope
    require_resource_access(&context, "routes", "read", None)?;

    // Extract team scopes from auth context for filtering
    // Empty team_scopes = admin bypass (query all teams)
    let team_scopes = get_effective_team_scopes(&context);

    // Parse query parameters
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * page_size;
    let search = params.search.as_deref();
    let mcp_filter = params.mcp_filter.as_deref();

    // Get repositories
    let route_repo = state
        .xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::from(Error::internal("Route repository not available")))?;

    let virtual_host_repo =
        state.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            ApiError::from(Error::internal("Virtual host repository not available"))
        })?;

    let route_config_repo =
        state.xds_state.route_config_repository.as_ref().ok_or_else(|| {
            ApiError::from(Error::internal("Route config repository not available"))
        })?;

    let mcp_tool_repo = state
        .xds_state
        .mcp_tool_repository
        .as_ref()
        .ok_or_else(|| ApiError::from(Error::internal("MCP tool repository not available")))?;

    // Get total count for pagination (with all filters applied)
    // Uses *_by_teams methods which handle admin bypass (empty teams = all data)
    let total_count = route_repo
        .count_by_teams_with_mcp_filter(&team_scopes, search, mcp_filter)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to count routes");
            ApiError::from(e)
        })?;

    // Get paginated routes with all related data in a single query (optimized)
    // Uses *_by_teams methods which handle admin bypass (empty teams = all data)
    let routes_with_related = route_repo
        .list_by_teams_paginated_with_related(
            &team_scopes,
            Some(page_size),
            Some(offset),
            search,
            mcp_filter,
        )
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to list routes with related data");
            ApiError::from(e)
        })?;

    // Build route view DTOs using the pre-loaded related data
    // Note: Route metadata is not available in this context (would require additional DB pool)
    let items: Vec<RouteListViewDto> = routes_with_related
        .iter()
        .map(|data| RouteViewExtractor::extract_from_related_data(data, None))
        .collect();

    // Calculate pagination
    let total_pages = ((total_count as f64) / (page_size as f64)).ceil() as i32;

    // Compute stats using efficient count queries
    // Uses *_by_teams methods which handle admin bypass (empty teams = all data)
    let total_routes = route_repo.count_by_teams(&team_scopes).await.map_err(|e| {
        error!(error = %e, "Failed to count total routes");
        ApiError::from(e)
    })?;

    let total_virtual_hosts =
        virtual_host_repo.count_by_teams(&team_scopes).await.map_err(|e| {
            error!(error = %e, "Failed to count virtual hosts");
            ApiError::from(e)
        })?;

    let total_route_configs =
        route_config_repo.count_by_teams(&team_scopes).await.map_err(|e| {
            error!(error = %e, "Failed to count route configs");
            ApiError::from(e)
        })?;

    let mcp_enabled_count =
        mcp_tool_repo.count_enabled_by_teams(&team_scopes).await.map_err(|e| {
            error!(error = %e, "Failed to count enabled MCP tools");
            ApiError::from(e)
        })?;

    // Compute unique clusters and domains from the loaded data
    let unique_clusters = RouteViewExtractor::count_unique_clusters(&items);
    let unique_domains = RouteViewExtractor::count_unique_domains(&items);

    let stats = RouteListStatsDto {
        total_routes,
        total_virtual_hosts,
        total_route_configs,
        mcp_enabled_count,
        unique_clusters,
        unique_domains,
    };

    let pagination = PaginationDto { page, page_size, total_count, total_pages };

    let response = RouteListResponseDto { items, stats, pagination };

    Ok(Json(response))
}

/// Get statistics for the route list view.
///
/// Returns aggregate statistics for the current team's routes.
#[utoipa::path(
    get,
    path = "/api/v1/route-views/stats",
    responses(
        (status = 200, description = "Route statistics", body = RouteListStatsDto),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Routes"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id))]
pub async fn get_route_stats_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
) -> Result<Json<RouteListStatsDto>, ApiError> {
    // Authorization: require routes:read scope
    require_resource_access(&context, "routes", "read", None)?;

    // Extract team scopes from auth context for filtering
    // Empty team_scopes = admin bypass (query all teams)
    let team_scopes = get_effective_team_scopes(&context);

    // Get repositories
    let route_repo = state
        .xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::from(Error::internal("Route repository not available")))?;

    let virtual_host_repo =
        state.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            ApiError::from(Error::internal("Virtual host repository not available"))
        })?;

    let route_config_repo =
        state.xds_state.route_config_repository.as_ref().ok_or_else(|| {
            ApiError::from(Error::internal("Route config repository not available"))
        })?;

    let mcp_tool_repo = state
        .xds_state
        .mcp_tool_repository
        .as_ref()
        .ok_or_else(|| ApiError::from(Error::internal("MCP tool repository not available")))?;

    // Compute basic stats using efficient count queries
    // Uses *_by_teams methods which handle admin bypass (empty teams = all data)
    let total_routes = route_repo.count_by_teams(&team_scopes).await.map_err(|e| {
        error!(error = %e, "Failed to count routes");
        ApiError::from(e)
    })?;

    let total_virtual_hosts =
        virtual_host_repo.count_by_teams(&team_scopes).await.map_err(|e| {
            error!(error = %e, "Failed to count virtual hosts");
            ApiError::from(e)
        })?;

    let total_route_configs =
        route_config_repo.count_by_teams(&team_scopes).await.map_err(|e| {
            error!(error = %e, "Failed to count route configs");
            ApiError::from(e)
        })?;

    let mcp_enabled_count =
        mcp_tool_repo.count_enabled_by_teams(&team_scopes).await.map_err(|e| {
            error!(error = %e, "Failed to count enabled MCP tools");
            ApiError::from(e)
        })?;

    // For unique clusters and domains, we need to fetch route data and compute from JSON
    // Use a reasonable limit (100 routes) for stats computation to avoid loading entire DB
    // Uses *_by_teams methods which handle admin bypass (empty teams = all data)
    let routes_for_stats = route_repo
        .list_by_teams_paginated_with_related(&team_scopes, Some(100), Some(0), None, None)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to load routes for stats computation");
            ApiError::from(e)
        })?;

    // Convert to view DTOs to use the extractor methods for unique counts
    let views: Vec<RouteListViewDto> = routes_for_stats
        .iter()
        .map(|data| RouteViewExtractor::extract_from_related_data(data, None))
        .collect();

    let unique_clusters = RouteViewExtractor::count_unique_clusters(&views);
    let unique_domains = RouteViewExtractor::count_unique_domains(&views);

    let stats = RouteListStatsDto {
        total_routes,
        total_virtual_hosts,
        total_route_configs,
        mcp_enabled_count,
        unique_clusters,
        unique_domains,
    };

    Ok(Json(stats))
}
