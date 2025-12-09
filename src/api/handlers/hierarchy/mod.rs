//! Hierarchical route filter attachment HTTP handlers
//!
//! This module provides endpoints for managing filter attachment at the
//! virtual host and route levels within a route configuration.
//!
//! API Endpoints:
//! - GET  /api/v1/route-configs/{route_config_name}/virtual-hosts - List virtual hosts
//! - GET  /api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/filters - List VH filters
//! - POST /api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/filters - Attach filter to VH
//! - DELETE /api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/filters/{filter_id} - Detach filter from VH
//! - GET  /api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes - List routes
//! - GET  /api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes/{route_name}/filters - List route filters
//! - POST /api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes/{route_name}/filters - Attach filter to route
//! - DELETE /api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes/{route_name}/filters/{filter_id} - Detach filter

mod types;

pub use types::*;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use tracing::{info, instrument};
use utoipa;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::{extract_team_scopes, has_admin_bypass, require_resource_access},
    auth::models::AuthContext,
    domain::FilterId,
    services::FilterService,
    storage::{RouteConfigData, RouteData, VirtualHostData},
};

// === Helper Functions ===

/// Verify that a route config belongs to one of the user's teams or is global.
/// Returns the route config if authorized, otherwise returns NotFound error (to avoid leaking existence).
async fn verify_route_config_access(
    route_config: RouteConfigData,
    team_scopes: &[String],
) -> Result<RouteConfigData, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(route_config);
    }

    // Check if route config is global (team = NULL) or belongs to one of user's teams
    match &route_config.team {
        None => Ok(route_config), // Global route config, accessible to all
        Some(route_team) => {
            if team_scopes.contains(route_team) {
                Ok(route_config)
            } else {
                // Record cross-team access attempt for security monitoring
                if let Some(from_team) = team_scopes.first() {
                    crate::observability::metrics::record_cross_team_access_attempt(
                        from_team, route_team, "routes",
                    )
                    .await;
                }

                // Return 404 to avoid leaking existence of other teams' resources
                Err(ApiError::NotFound(format!(
                    "Route config with name '{}' not found",
                    route_config.name
                )))
            }
        }
    }
}

/// Resolve a route config name to its data with team access verification
async fn resolve_route_config_with_access(
    state: &ApiState,
    route_name: &str,
    context: &AuthContext,
) -> Result<RouteConfigData, ApiError> {
    let route_config_repository =
        state.xds_state.route_config_repository.as_ref().ok_or_else(|| {
            ApiError::service_unavailable("Route config repository not available")
        })?;

    let route_config =
        route_config_repository.get_by_name(route_name).await.map_err(ApiError::from)?;

    // Extract team scopes for access verification
    let team_scopes =
        if has_admin_bypass(context) { Vec::new() } else { extract_team_scopes(context) };

    verify_route_config_access(route_config, &team_scopes).await
}

/// Resolve a virtual host by route config name and vhost name with team access verification
async fn resolve_virtual_host(
    state: &ApiState,
    route_name: &str,
    vhost_name: &str,
    context: &AuthContext,
) -> Result<VirtualHostData, ApiError> {
    // This verifies team access to the route config
    let route_config = resolve_route_config_with_access(state, route_name, context).await?;

    let vh_repository =
        state.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            ApiError::service_unavailable("Virtual host repository not available")
        })?;

    vh_repository
        .get_by_route_config_and_name(&route_config.id, vhost_name)
        .await
        .map_err(ApiError::from)
}

/// Resolve a route by route config name, vhost name, and route name with team access verification
async fn resolve_route(
    state: &ApiState,
    route_config_name: &str,
    vhost_name: &str,
    route_name: &str,
    context: &AuthContext,
) -> Result<RouteData, ApiError> {
    // This verifies team access to the route config via resolve_virtual_host
    let virtual_host = resolve_virtual_host(state, route_config_name, vhost_name, context).await?;

    let route_repository = state
        .xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Route repository not available"))?;

    route_repository.get_by_vh_and_name(&virtual_host.id, route_name).await.map_err(ApiError::from)
}

// === Virtual Host Handlers ===

/// List all virtual hosts for a route config
#[utoipa::path(
    get,
    path = "/api/v1/route-configs/{route_config_name}/virtual-hosts",
    params(
        ("route_config_name" = String, Path, description = "Route config name"),
    ),
    responses(
        (status = 200, description = "List of virtual hosts", body = ListVirtualHostsResponse),
        (status = 404, description = "Route config not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "hierarchy"
)]
#[instrument(skip(state, context), fields(route_config_name = %route_config_name, user_id = ?context.user_id))]
pub async fn list_virtual_hosts_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(route_config_name): Path<String>,
) -> Result<Json<ListVirtualHostsResponse>, ApiError> {
    require_resource_access(&context, "routes", "read", None)?;

    // Team access verification happens in resolve_route_config_with_access
    let route_config =
        resolve_route_config_with_access(&state, &route_config_name, &context).await?;

    let vh_repository =
        state.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            ApiError::service_unavailable("Virtual host repository not available")
        })?;

    let route_repository = state
        .xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Route repository not available"))?;

    let vh_filter_repository =
        state.xds_state.virtual_host_filter_repository.as_ref().ok_or_else(|| {
            ApiError::service_unavailable("Virtual host filter repository not available")
        })?;

    let virtual_hosts =
        vh_repository.list_by_route_config(&route_config.id).await.map_err(ApiError::from)?;

    // Build responses with counts
    let mut items = Vec::with_capacity(virtual_hosts.len());
    for vh in virtual_hosts {
        let route_count =
            route_repository.count_by_virtual_host(&vh.id).await.map_err(ApiError::from)?;
        let filter_count =
            vh_filter_repository.count_by_virtual_host(&vh.id).await.map_err(ApiError::from)?;
        items.push(VirtualHostResponse::from_data_with_counts(vh, route_count, filter_count));
    }

    Ok(Json(ListVirtualHostsResponse { route_config_name, virtual_hosts: items }))
}

/// List filters attached to a virtual host
#[utoipa::path(
    get,
    path = "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/filters",
    params(
        ("route_config_name" = String, Path, description = "Route config name"),
        ("vhost_name" = String, Path, description = "Virtual host name"),
    ),
    responses(
        (status = 200, description = "List of filters attached to virtual host", body = VirtualHostFiltersResponse),
        (status = 404, description = "Route config or virtual host not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "hierarchy"
)]
#[instrument(skip(state, context), fields(route_config_name = %route_config_name, vhost_name = %vhost_name, user_id = ?context.user_id))]
pub async fn list_virtual_host_filters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((route_config_name, vhost_name)): Path<(String, String)>,
) -> Result<Json<VirtualHostFiltersResponse>, ApiError> {
    require_resource_access(&context, "routes", "read", None)?;

    // Team access verification happens in resolve_virtual_host
    let virtual_host =
        resolve_virtual_host(&state, &route_config_name, &vhost_name, &context).await?;

    let service = FilterService::new(state.xds_state.clone());
    let filters =
        service.list_virtual_host_filters(&virtual_host.id).await.map_err(ApiError::from)?;

    let filter_responses: Vec<FilterResponse> =
        filters.into_iter().map(FilterResponse::from).collect();

    Ok(Json(VirtualHostFiltersResponse {
        route_config_name,
        virtual_host_name: vhost_name,
        filters: filter_responses,
    }))
}

/// Attach a filter to a virtual host
#[utoipa::path(
    post,
    path = "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/filters",
    params(
        ("route_config_name" = String, Path, description = "Route config name"),
        ("vhost_name" = String, Path, description = "Virtual host name"),
    ),
    request_body = AttachFilterRequest,
    responses(
        (status = 204, description = "Filter attached to virtual host"),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Route config, virtual host, or filter not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "hierarchy"
)]
#[instrument(skip(state, context, payload), fields(route_config_name = %route_config_name, vhost_name = %vhost_name, filter_id = %payload.filter_id, user_id = ?context.user_id))]
pub async fn attach_filter_to_virtual_host_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((route_config_name, vhost_name)): Path<(String, String)>,
    Json(payload): Json<AttachFilterRequest>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "routes", "write", None)?;

    // Team access verification happens in resolve_virtual_host
    let virtual_host =
        resolve_virtual_host(&state, &route_config_name, &vhost_name, &context).await?;
    let filter_id = FilterId::from_string(payload.filter_id);

    let service = FilterService::new(state.xds_state.clone());
    service
        .attach_filter_to_virtual_host(&virtual_host.id, &filter_id, payload.order)
        .await
        .map_err(ApiError::from)?;

    info!(
        route_config_name = %route_config_name,
        virtual_host_name = %vhost_name,
        virtual_host_id = %virtual_host.id,
        filter_id = %filter_id,
        "Filter attached to virtual host via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

/// Detach a filter from a virtual host
#[utoipa::path(
    delete,
    path = "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/filters/{filter_id}",
    params(
        ("route_config_name" = String, Path, description = "Route config name"),
        ("vhost_name" = String, Path, description = "Virtual host name"),
        ("filter_id" = String, Path, description = "Filter ID"),
    ),
    responses(
        (status = 204, description = "Filter detached from virtual host"),
        (status = 404, description = "Route config, virtual host, filter, or attachment not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "hierarchy"
)]
#[instrument(skip(state, context), fields(route_config_name = %route_config_name, vhost_name = %vhost_name, filter_id = %filter_id, user_id = ?context.user_id))]
pub async fn detach_filter_from_virtual_host_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((route_config_name, vhost_name, filter_id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "routes", "write", None)?;

    // Team access verification happens in resolve_virtual_host
    let virtual_host =
        resolve_virtual_host(&state, &route_config_name, &vhost_name, &context).await?;
    let filter_id = FilterId::from_string(filter_id);

    let service = FilterService::new(state.xds_state.clone());
    service
        .detach_filter_from_virtual_host(&virtual_host.id, &filter_id)
        .await
        .map_err(ApiError::from)?;

    info!(
        route_config_name = %route_config_name,
        virtual_host_name = %vhost_name,
        virtual_host_id = %virtual_host.id,
        filter_id = %filter_id,
        "Filter detached from virtual host via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

// === Route Rule Handlers ===

/// List all routes for a virtual host
#[utoipa::path(
    get,
    path = "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes",
    params(
        ("route_config_name" = String, Path, description = "Route config name"),
        ("vhost_name" = String, Path, description = "Virtual host name"),
    ),
    responses(
        (status = 200, description = "List of routes in virtual host", body = ListRouteRulesResponse),
        (status = 404, description = "Route config or virtual host not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "hierarchy"
)]
#[instrument(skip(state, context), fields(route_config_name = %route_config_name, vhost_name = %vhost_name, user_id = ?context.user_id))]
pub async fn list_route_rules_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((route_config_name, vhost_name)): Path<(String, String)>,
) -> Result<Json<ListRouteRulesResponse>, ApiError> {
    require_resource_access(&context, "routes", "read", None)?;

    // Team access verification happens in resolve_virtual_host
    let virtual_host =
        resolve_virtual_host(&state, &route_config_name, &vhost_name, &context).await?;

    let route_repository = state
        .xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Route repository not available"))?;

    let route_filter_repository =
        state.xds_state.route_filter_repository.as_ref().ok_or_else(|| {
            ApiError::service_unavailable("Route filter repository not available")
        })?;

    let routes =
        route_repository.list_by_virtual_host(&virtual_host.id).await.map_err(ApiError::from)?;

    // Build responses with filter counts
    let mut items = Vec::with_capacity(routes.len());
    for route in routes {
        let filter_count =
            route_filter_repository.count_by_route(&route.id).await.map_err(ApiError::from)?;
        items.push(RouteRuleResponse::from_data_with_count(route, filter_count));
    }

    Ok(Json(ListRouteRulesResponse {
        route_config_name,
        virtual_host_name: vhost_name,
        route_rules: items,
    }))
}

/// List filters attached to a route
#[utoipa::path(
    get,
    path = "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes/{route_name}/filters",
    params(
        ("route_config_name" = String, Path, description = "Route config name"),
        ("vhost_name" = String, Path, description = "Virtual host name"),
        ("route_name" = String, Path, description = "Route name"),
    ),
    responses(
        (status = 200, description = "List of filters attached to route", body = RouteRuleFiltersResponse),
        (status = 404, description = "Route config, virtual host, or route not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "hierarchy"
)]
#[instrument(skip(state, context), fields(route_config_name = %route_config_name, vhost_name = %vhost_name, route_name = %route_name, user_id = ?context.user_id))]
pub async fn list_route_rule_filters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((route_config_name, vhost_name, route_name)): Path<(String, String, String)>,
) -> Result<Json<RouteRuleFiltersResponse>, ApiError> {
    require_resource_access(&context, "routes", "read", None)?;

    // Team access verification happens in resolve_route
    let route =
        resolve_route(&state, &route_config_name, &vhost_name, &route_name, &context).await?;

    let service = FilterService::new(state.xds_state.clone());
    let filters = service.list_route_filters(&route.id).await.map_err(ApiError::from)?;

    let filter_responses: Vec<FilterResponse> =
        filters.into_iter().map(FilterResponse::from).collect();

    Ok(Json(RouteRuleFiltersResponse {
        route_config_name,
        virtual_host_name: vhost_name,
        route_name,
        filters: filter_responses,
    }))
}

/// Attach a filter to a route
#[utoipa::path(
    post,
    path = "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes/{route_name}/filters",
    params(
        ("route_config_name" = String, Path, description = "Route config name"),
        ("vhost_name" = String, Path, description = "Virtual host name"),
        ("route_name" = String, Path, description = "Route name"),
    ),
    request_body = AttachFilterRequest,
    responses(
        (status = 204, description = "Filter attached to route"),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Route config, virtual host, route, or filter not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "hierarchy"
)]
#[instrument(skip(state, context, payload), fields(route_config_name = %route_config_name, vhost_name = %vhost_name, route_name = %route_name, filter_id = %payload.filter_id, user_id = ?context.user_id))]
pub async fn attach_filter_to_route_rule_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((route_config_name, vhost_name, route_name)): Path<(String, String, String)>,
    Json(payload): Json<AttachFilterRequest>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "routes", "write", None)?;

    // Team access verification happens in resolve_route
    let route =
        resolve_route(&state, &route_config_name, &vhost_name, &route_name, &context).await?;
    let filter_id = FilterId::from_string(payload.filter_id);

    let service = FilterService::new(state.xds_state.clone());
    service
        .attach_filter_to_route(&route.id, &filter_id, payload.order)
        .await
        .map_err(ApiError::from)?;

    info!(
        route_config_name = %route_config_name,
        virtual_host_name = %vhost_name,
        route_name = %route_name,
        route_id = %route.id,
        filter_id = %filter_id,
        "Filter attached to route via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

/// Detach a filter from a route
#[utoipa::path(
    delete,
    path = "/api/v1/route-configs/{route_config_name}/virtual-hosts/{vhost_name}/routes/{route_name}/filters/{filter_id}",
    params(
        ("route_config_name" = String, Path, description = "Route config name"),
        ("vhost_name" = String, Path, description = "Virtual host name"),
        ("route_name" = String, Path, description = "Route name"),
        ("filter_id" = String, Path, description = "Filter ID"),
    ),
    responses(
        (status = 204, description = "Filter detached from route"),
        (status = 404, description = "Route config, virtual host, route, filter, or attachment not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "hierarchy"
)]
#[instrument(skip(state, context), fields(route_config_name = %route_config_name, vhost_name = %vhost_name, route_name = %route_name, filter_id = %filter_id, user_id = ?context.user_id))]
pub async fn detach_filter_from_route_rule_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((route_config_name, vhost_name, route_name, filter_id)): Path<(
        String,
        String,
        String,
        String,
    )>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "routes", "write", None)?;

    // Team access verification happens in resolve_route
    let route =
        resolve_route(&state, &route_config_name, &vhost_name, &route_name, &context).await?;
    let filter_id = FilterId::from_string(filter_id);

    let service = FilterService::new(state.xds_state.clone());
    service.detach_filter_from_route(&route.id, &filter_id).await.map_err(ApiError::from)?;

    info!(
        route_config_name = %route_config_name,
        virtual_host_name = %vhost_name,
        route_name = %route_name,
        route_id = %route.id,
        filter_id = %filter_id,
        "Filter detached from route via API"
    );

    Ok(StatusCode::NO_CONTENT)
}
