//! Filter configuration HTTP handlers
//!
//! This module provides CRUD operations for filters through the REST API,
//! with validation, team isolation, and XDS state synchronization.

mod types;
mod validation;

// Re-export public types
pub use types::{
    AttachFilterRequest, CreateFilterRequest, FilterResponse, ListFiltersQuery,
    ListenerFiltersResponse, RouteFiltersResponse, UpdateFilterRequest,
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use tracing::{info, instrument};

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::{extract_team_scopes, has_admin_bypass, require_resource_access},
    auth::models::AuthContext,
    domain::{FilterId, ListenerId, RouteId},
    services::FilterService,
};

use validation::{
    filter_response_from_data, require_filter_repository, validate_create_filter_request,
    validate_update_filter_request, verify_filter_access,
};

// === Helper Functions ===

/// Resolve a route name to its database ID (UUID)
///
/// The public API uses route names as identifiers, but the database
/// uses UUIDs for foreign key relationships. This function looks up
/// the route by name and returns its internal UUID.
async fn resolve_route_id(state: &ApiState, route_name: &str) -> Result<RouteId, ApiError> {
    let route_repository = state
        .xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Route repository not available"))?;

    let route = route_repository.get_by_name(route_name).await.map_err(ApiError::from)?;

    Ok(route.id)
}

/// Resolve a listener name to its database ID (UUID)
///
/// The public API uses listener names as identifiers, but the database
/// uses UUIDs for foreign key relationships. This function looks up
/// the listener by name and returns its internal UUID.
async fn resolve_listener_id(
    state: &ApiState,
    listener_name: &str,
) -> Result<ListenerId, ApiError> {
    let listener_repository = state
        .xds_state
        .listener_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Listener repository not available"))?;

    let listener = listener_repository.get_by_name(listener_name).await.map_err(ApiError::from)?;

    Ok(listener.id)
}

// === Handler Implementations ===

#[utoipa::path(
    get,
    path = "/api/v1/filters",
    params(
        ("limit" = Option<i32>, Query, description = "Maximum number of filters to return"),
        ("offset" = Option<i32>, Query, description = "Offset for paginated results"),
    ),
    responses(
        (status = 200, description = "List of filters", body = [FilterResponse]),
        (status = 503, description = "Filter repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context), fields(user_id = ?context.user_id))]
pub async fn list_filters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<ListFiltersQuery>,
) -> Result<Json<Vec<FilterResponse>>, ApiError> {
    require_resource_access(&context, "filters", "read", None)?;

    // Admin users see all filters, regular users see only their team's filters
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };
    let repository = require_filter_repository(&state)?;

    let filters = repository
        .list_by_teams(&team_scopes, params.limit, params.offset)
        .await
        .map_err(ApiError::from)?;

    let responses: Result<Vec<FilterResponse>, ApiError> =
        filters.into_iter().map(filter_response_from_data).collect();

    Ok(Json(responses?))
}

#[utoipa::path(
    post,
    path = "/api/v1/filters",
    request_body = CreateFilterRequest,
    responses(
        (status = 201, description = "Filter created", body = FilterResponse),
        (status = 400, description = "Validation error"),
        (status = 503, description = "Filter repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context, payload), fields(team = %payload.team, filter_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<CreateFilterRequest>,
) -> Result<(StatusCode, Json<FilterResponse>), ApiError> {
    info!(
        filter_type = ?payload.filter_type,
        config = ?payload.config,
        "Creating filter - received payload"
    );
    validate_create_filter_request(&payload)?;

    // Verify user has write access to the specified team
    require_resource_access(&context, "filters", "write", Some(&payload.team))?;

    let service = FilterService::new(state.xds_state.clone());

    let created = service
        .create_filter(
            payload.name.clone(),
            payload.filter_type,
            payload.description.clone(),
            payload.config.clone(),
            payload.team.clone(),
        )
        .await
        .map_err(ApiError::from)?;

    info!(
        filter_id = %created.id,
        filter_name = %created.name,
        "Filter created via API"
    );

    let response = filter_response_from_data(created)?;

    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/filters/{id}",
    params(
        ("id" = String, Path, description = "Filter ID"),
    ),
    responses(
        (status = 200, description = "Filter details", body = FilterResponse),
        (status = 404, description = "Filter not found"),
        (status = 503, description = "Filter repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context), fields(filter_id = %id, user_id = ?context.user_id))]
pub async fn get_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<FilterResponse>, ApiError> {
    require_resource_access(&context, "filters", "read", None)?;

    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };
    let repository = require_filter_repository(&state)?;

    let filter_id = FilterId::from_string(id);
    let filter = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;

    // Verify access to this filter
    let filter = verify_filter_access(filter, &team_scopes).await?;

    let response = filter_response_from_data(filter)?;

    Ok(Json(response))
}

#[utoipa::path(
    put,
    path = "/api/v1/filters/{id}",
    params(
        ("id" = String, Path, description = "Filter ID"),
    ),
    request_body = UpdateFilterRequest,
    responses(
        (status = 200, description = "Filter updated", body = FilterResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Filter not found"),
        (status = 503, description = "Filter repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context, payload), fields(filter_id = %id, user_id = ?context.user_id))]
pub async fn update_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateFilterRequest>,
) -> Result<Json<FilterResponse>, ApiError> {
    validate_update_filter_request(&payload)?;

    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };
    let repository = require_filter_repository(&state)?;

    let filter_id = FilterId::from_string(id);

    // Get existing filter and verify access
    let existing = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;
    let existing = verify_filter_access(existing, &team_scopes).await?;

    // Verify user has write access to the filter's team
    require_resource_access(&context, "filters", "write", Some(&existing.team))?;

    let service = FilterService::new(state.xds_state.clone());

    let updated = service
        .update_filter(
            &filter_id,
            payload.name.clone(),
            payload.description.clone(),
            payload.config.clone(),
        )
        .await
        .map_err(ApiError::from)?;

    info!(
        filter_id = %updated.id,
        filter_name = %updated.name,
        "Filter updated via API"
    );

    let response = filter_response_from_data(updated)?;

    Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/api/v1/filters/{id}",
    params(
        ("id" = String, Path, description = "Filter ID"),
    ),
    responses(
        (status = 204, description = "Filter deleted"),
        (status = 404, description = "Filter not found"),
        (status = 409, description = "Filter is attached to routes"),
        (status = 503, description = "Filter repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context), fields(filter_id = %id, user_id = ?context.user_id))]
pub async fn delete_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let team_scopes =
        if has_admin_bypass(&context) { Vec::new() } else { extract_team_scopes(&context) };
    let repository = require_filter_repository(&state)?;

    let filter_id = FilterId::from_string(id);

    // Get existing filter and verify access
    let existing = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;
    let existing = verify_filter_access(existing, &team_scopes).await?;

    // Verify user has write access to the filter's team
    require_resource_access(&context, "filters", "write", Some(&existing.team))?;

    let service = FilterService::new(state.xds_state.clone());

    service.delete_filter(&filter_id).await.map_err(ApiError::from)?;

    info!(
        filter_id = %filter_id,
        filter_name = %existing.name,
        "Filter deleted via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/routes/{route_id}/filters",
    params(
        ("route_id" = String, Path, description = "Route ID"),
    ),
    request_body = AttachFilterRequest,
    responses(
        (status = 204, description = "Filter attached to route"),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Route or filter not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context, payload), fields(route_name = %route_name, filter_id = %payload.filter_id, user_id = ?context.user_id))]
pub async fn attach_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(route_name): Path<String>,
    Json(payload): Json<AttachFilterRequest>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "routes", "write", None)?;

    // Resolve route name to internal UUID for database foreign key
    let route_id = resolve_route_id(&state, &route_name).await?;
    let filter_id = FilterId::from_string(payload.filter_id);

    let service = FilterService::new(state.xds_state.clone());

    service
        .attach_filter_to_route(&route_id, &filter_id, payload.order)
        .await
        .map_err(ApiError::from)?;

    info!(
        route_name = %route_name,
        route_id = %route_id,
        filter_id = %filter_id,
        "Filter attached to route via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/routes/{route_id}/filters/{filter_id}",
    params(
        ("route_id" = String, Path, description = "Route ID"),
        ("filter_id" = String, Path, description = "Filter ID"),
    ),
    responses(
        (status = 204, description = "Filter detached from route"),
        (status = 404, description = "Route, filter, or attachment not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context), fields(route_name = %route_name, filter_id = %filter_id, user_id = ?context.user_id))]
pub async fn detach_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((route_name, filter_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "routes", "write", None)?;

    // Resolve route name to internal UUID for database foreign key
    let route_id = resolve_route_id(&state, &route_name).await?;
    let filter_id = FilterId::from_string(filter_id);

    let service = FilterService::new(state.xds_state.clone());

    service.detach_filter_from_route(&route_id, &filter_id).await.map_err(ApiError::from)?;

    info!(
        route_name = %route_name,
        route_id = %route_id,
        filter_id = %filter_id,
        "Filter detached from route via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/routes/{route_id}/filters",
    params(
        ("route_id" = String, Path, description = "Route ID"),
    ),
    responses(
        (status = 200, description = "Filters attached to route", body = RouteFiltersResponse),
        (status = 404, description = "Route not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context), fields(route_name = %route_name, user_id = ?context.user_id))]
pub async fn list_route_filters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(route_name): Path<String>,
) -> Result<Json<RouteFiltersResponse>, ApiError> {
    require_resource_access(&context, "routes", "read", None)?;

    // Resolve route name to internal UUID for database query
    let route_id = resolve_route_id(&state, &route_name).await?;

    let service = FilterService::new(state.xds_state.clone());

    let filters = service.list_route_filters(&route_id).await.map_err(ApiError::from)?;

    let filter_responses: Result<Vec<FilterResponse>, ApiError> =
        filters.into_iter().map(filter_response_from_data).collect();

    // Return the route name (public identifier) in the response, not the internal UUID
    let response = RouteFiltersResponse { route_id: route_name, filters: filter_responses? };

    Ok(Json(response))
}

// === Listener Filter Handlers ===

#[utoipa::path(
    post,
    path = "/api/v1/listeners/{listener_id}/filters",
    params(
        ("listener_id" = String, Path, description = "Listener ID"),
    ),
    request_body = AttachFilterRequest,
    responses(
        (status = 204, description = "Filter attached to listener"),
        (status = 400, description = "Validation error - filter type incompatible with listener attachment"),
        (status = 404, description = "Listener or filter not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context, payload), fields(listener_name = %listener_name, filter_id = %payload.filter_id, user_id = ?context.user_id))]
pub async fn attach_filter_to_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(listener_name): Path<String>,
    Json(payload): Json<AttachFilterRequest>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "listeners", "write", None)?;

    // Resolve listener name to internal UUID for database foreign key
    let listener_id = resolve_listener_id(&state, &listener_name).await?;
    let filter_id = FilterId::from_string(payload.filter_id);

    let service = FilterService::new(state.xds_state.clone());

    service
        .attach_filter_to_listener(&listener_id, &filter_id, payload.order)
        .await
        .map_err(ApiError::from)?;

    info!(
        listener_name = %listener_name,
        listener_id = %listener_id,
        filter_id = %filter_id,
        "Filter attached to listener via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/listeners/{listener_id}/filters/{filter_id}",
    params(
        ("listener_id" = String, Path, description = "Listener ID"),
        ("filter_id" = String, Path, description = "Filter ID"),
    ),
    responses(
        (status = 204, description = "Filter detached from listener"),
        (status = 404, description = "Listener, filter, or attachment not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context), fields(listener_name = %listener_name, filter_id = %filter_id, user_id = ?context.user_id))]
pub async fn detach_filter_from_listener_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((listener_name, filter_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "listeners", "write", None)?;

    // Resolve listener name to internal UUID for database foreign key
    let listener_id = resolve_listener_id(&state, &listener_name).await?;
    let filter_id = FilterId::from_string(filter_id);

    let service = FilterService::new(state.xds_state.clone());

    service.detach_filter_from_listener(&listener_id, &filter_id).await.map_err(ApiError::from)?;

    info!(
        listener_name = %listener_name,
        listener_id = %listener_id,
        filter_id = %filter_id,
        "Filter detached from listener via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/listeners/{listener_id}/filters",
    params(
        ("listener_id" = String, Path, description = "Listener ID"),
    ),
    responses(
        (status = 200, description = "Filters attached to listener", body = ListenerFiltersResponse),
        (status = 404, description = "Listener not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context), fields(listener_name = %listener_name, user_id = ?context.user_id))]
pub async fn list_listener_filters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(listener_name): Path<String>,
) -> Result<Json<ListenerFiltersResponse>, ApiError> {
    require_resource_access(&context, "listeners", "read", None)?;

    // Resolve listener name to internal UUID for database query
    let listener_id = resolve_listener_id(&state, &listener_name).await?;

    let service = FilterService::new(state.xds_state.clone());

    let filters = service.list_listener_filters(&listener_id).await.map_err(ApiError::from)?;

    let filter_responses: Result<Vec<FilterResponse>, ApiError> =
        filters.into_iter().map(filter_response_from_data).collect();

    // Return the listener name (public identifier) in the response, not the internal UUID
    let response =
        ListenerFiltersResponse { listener_id: listener_name, filters: filter_responses? };

    Ok(Json(response))
}
