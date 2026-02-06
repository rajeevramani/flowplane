//! Filter configuration HTTP handlers
//!
//! This module provides CRUD operations for filters through the REST API.
//! Basic CRUD operations are delegated to the internal API layer (FilterOperations)
//! which provides unified validation, access control, and XDS state synchronization.

mod filter_types;
mod types;
mod validation;

// Re-export public types
pub use filter_types::{
    get_filter_type_handler, list_filter_types_handler, reload_filter_schemas_handler,
    FilterTypeFormSection, FilterTypeInfo, FilterTypeUiHints, FilterTypesResponse,
};
pub use types::{
    AttachFilterRequest, ClusterCreationConfig, ClusterMode, ConfigureFilterRequest,
    ConfigureFilterResponse, CreateFilterRequest, FilterConfigurationItem,
    FilterConfigurationsResponse, FilterInstallationItem, FilterInstallationsResponse,
    FilterResponse, FilterStatusResponse, InstallFilterRequest, InstallFilterResponse,
    ListFiltersQuery, ListenerFiltersResponse, RouteFiltersResponse, ScopeType,
    UpdateFilterRequest,
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use tracing::{info, instrument};

use crate::{
    api::{
        error::ApiError,
        handlers::team_access::{get_effective_team_scopes, verify_team_access},
        routes::ApiState,
    },
    auth::authorization::require_resource_access,
    auth::models::AuthContext,
    domain::{FilterId, ListenerId, RouteConfigId},
    internal_api::auth::InternalAuthContext,
    internal_api::filters::FilterOperations,
    internal_api::types::{
        ListFiltersRequest as InternalListFiltersRequest,
        UpdateFilterRequest as InternalUpdateFilterRequest,
    },
    services::FilterService,
};

use validation::{
    filter_response_from_data, filter_response_from_data_with_count, require_filter_repository,
    validate_create_filter_request, validate_update_filter_request,
};

// === Helper Functions ===

/// Resolve a route name to its database ID (UUID)
///
/// The public API uses route names as identifiers, but the database
/// uses UUIDs for foreign key relationships. This function looks up
/// the route config by name and returns its internal UUID.
async fn resolve_route_config_id(
    state: &ApiState,
    route_name: &str,
) -> Result<RouteConfigId, ApiError> {
    let route_config_repository =
        state.xds_state.route_config_repository.as_ref().ok_or_else(|| {
            ApiError::service_unavailable("Route config repository not available")
        })?;

    let route_config =
        route_config_repository.get_by_name(route_name).await.map_err(ApiError::from)?;

    Ok(route_config.id)
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
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(user_id = ?context.user_id))]
pub async fn list_filters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<ListFiltersQuery>,
) -> Result<Json<Vec<FilterResponse>>, ApiError> {
    require_resource_access(&context, "filters", "read", None)?;

    // Create internal API request
    let internal_request = InternalListFiltersRequest {
        limit: params.limit,
        offset: params.offset,
        filter_type: None,
        include_defaults: true,
    };

    // Delegate to internal API layer for team-scoped listing
    let ops = FilterOperations::new(state.xds_state.clone());
    let auth = InternalAuthContext::from_rest(&context);
    let result = ops.list(internal_request, &auth).await?;

    // Build responses with attachment counts (REST-specific enhancement)
    let repository = require_filter_repository(&state)?;
    let mut responses = Vec::with_capacity(result.filters.len());
    for filter_data in result.filters {
        let filter_id = filter_data.id.clone();
        let attachment_count = repository.count_attachments(&filter_id).await.ok(); // Ignore errors, return None for count
        let response = filter_response_from_data_with_count(filter_data, attachment_count)?;
        responses.push(response);
    }

    Ok(Json(responses))
}

#[utoipa::path(
    post,
    path = "/api/v1/filters",
    request_body = CreateFilterRequest,
    responses(
        (status = 201, description = "Filter created", body = FilterResponse),
        (status = 400, description = "Validation error"),
        (status = 409, description = "Cluster already exists (when creating)"),
        (status = 503, description = "Filter repository unavailable"),
    ),
    tag = "Filters"
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
        cluster_config = ?payload.cluster_config,
        "Creating filter - received payload"
    );
    validate_create_filter_request(&payload, state.filter_schema_registry.as_ref()).await?;

    // Verify user has write access to the specified team
    require_resource_access(&context, "filters", "write", Some(&payload.team))?;

    let service = FilterService::new(state.xds_state.clone());

    let created = service
        .create_filter_with_cluster(
            payload.name.clone(),
            payload.filter_type,
            payload.description.clone(),
            payload.config.clone(),
            payload.team.clone(),
            payload.cluster_config.clone(),
        )
        .await
        .map_err(ApiError::from)?;

    info!(
        filter_id = %created.id,
        filter_name = %created.name,
        cluster_created = ?payload.cluster_config.as_ref().map(|cc| cc.mode == ClusterMode::Create),
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
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(filter_id = %id, user_id = ?context.user_id))]
pub async fn get_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<FilterResponse>, ApiError> {
    require_resource_access(&context, "filters", "read", None)?;

    // Delegate to internal API layer (includes team access verification)
    let ops = FilterOperations::new(state.xds_state.clone());
    let auth = InternalAuthContext::from_rest(&context);
    let filter = ops.get(&id, &auth).await?;

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
    tag = "Filters"
)]
#[instrument(skip(state, context, payload), fields(filter_id = %id, user_id = ?context.user_id))]
pub async fn update_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateFilterRequest>,
) -> Result<Json<FilterResponse>, ApiError> {
    // REST-specific validation
    validate_update_filter_request(&payload)?;

    // Verify user has write access (general scope check)
    require_resource_access(&context, "filters", "write", None)?;

    // Create internal API request
    let internal_request = InternalUpdateFilterRequest {
        name: payload.name.clone(),
        description: payload.description.clone(),
        config: payload.config.clone(),
    };

    // Delegate to internal API layer (includes team access verification)
    let ops = FilterOperations::new(state.xds_state.clone());
    let auth = InternalAuthContext::from_rest(&context);
    let result = ops.update(&id, internal_request, &auth).await?;

    let response = filter_response_from_data(result.data)?;

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
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(filter_id = %id, user_id = ?context.user_id))]
pub async fn delete_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Verify user has write access (general scope check)
    require_resource_access(&context, "filters", "write", None)?;

    // Delegate to internal API layer (includes team access verification)
    let ops = FilterOperations::new(state.xds_state.clone());
    let auth = InternalAuthContext::from_rest(&context);
    ops.delete(&id, &auth).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/route-configs/{route_config_id}/filters",
    params(
        ("route_config_id" = String, Path, description = "Route config name"),
    ),
    request_body = AttachFilterRequest,
    responses(
        (status = 204, description = "Filter attached to route config"),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Route config or filter not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
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
    let route_config_id = resolve_route_config_id(&state, &route_name).await?;
    let filter_id = FilterId::from_string(payload.filter_id);

    let service = FilterService::new(state.xds_state.clone());

    service
        .attach_filter_to_route_config(&route_config_id, &filter_id, payload.order, None)
        .await
        .map_err(ApiError::from)?;

    info!(
        route_name = %route_name,
        route_config_id = %route_config_id,
        filter_id = %filter_id,
        "Filter attached to route config via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/route-configs/{route_config_id}/filters/{filter_id}",
    params(
        ("route_config_id" = String, Path, description = "Route config name"),
        ("filter_id" = String, Path, description = "Filter ID"),
    ),
    responses(
        (status = 204, description = "Filter detached from route config"),
        (status = 404, description = "Route config, filter, or attachment not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(route_name = %route_name, filter_id = %filter_id, user_id = ?context.user_id))]
pub async fn detach_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((route_name, filter_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "routes", "write", None)?;

    // Resolve route name to internal UUID for database foreign key
    let route_config_id = resolve_route_config_id(&state, &route_name).await?;
    let filter_id = FilterId::from_string(filter_id);

    let service = FilterService::new(state.xds_state.clone());

    service
        .detach_filter_from_route_config(&route_config_id, &filter_id)
        .await
        .map_err(ApiError::from)?;

    info!(
        route_name = %route_name,
        route_config_id = %route_config_id,
        filter_id = %filter_id,
        "Filter detached from route config via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/route-configs/{route_config_id}/filters",
    params(
        ("route_config_id" = String, Path, description = "Route config name"),
    ),
    responses(
        (status = 200, description = "Filters attached to route config", body = RouteFiltersResponse),
        (status = 404, description = "Route config not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(route_name = %route_name, user_id = ?context.user_id))]
pub async fn list_route_filters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(route_name): Path<String>,
) -> Result<Json<RouteFiltersResponse>, ApiError> {
    require_resource_access(&context, "routes", "read", None)?;

    // Resolve route name to internal UUID for database query
    let route_config_id = resolve_route_config_id(&state, &route_name).await?;

    let service = FilterService::new(state.xds_state.clone());

    let filters =
        service.list_route_config_filters(&route_config_id).await.map_err(ApiError::from)?;

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
    tag = "Filters"
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
    tag = "Filters"
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
    tag = "Filters"
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

// ============================================================================
// Filter Installation Handlers (Filter Install/Configure Redesign)
// ============================================================================

use crate::storage::VirtualHostData;

/// Resolve a virtual host by route config name and vhost name
async fn resolve_virtual_host_data(
    state: &ApiState,
    route_config_name: &str,
    vhost_name: &str,
) -> Result<VirtualHostData, ApiError> {
    let route_config_id = resolve_route_config_id(state, route_config_name).await?;

    let vh_repository =
        state.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            ApiError::service_unavailable("Virtual host repository not available")
        })?;

    vh_repository
        .get_by_route_config_and_name(&route_config_id, vhost_name)
        .await
        .map_err(ApiError::from)
}

/// Resolve a route by route config name, vhost name, and route name
async fn resolve_route_id(
    state: &ApiState,
    route_config_name: &str,
    vhost_name: &str,
    route_name: &str,
) -> Result<crate::domain::RouteId, ApiError> {
    let virtual_host = resolve_virtual_host_data(state, route_config_name, vhost_name).await?;

    let route_repository = state
        .xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Route repository not available"))?;

    let route = route_repository
        .get_by_vh_and_name(&virtual_host.id, route_name)
        .await
        .map_err(ApiError::from)?;

    Ok(route.id)
}

#[utoipa::path(
    post,
    path = "/api/v1/filters/{filter_id}/installations",
    params(
        ("filter_id" = String, Path, description = "Filter ID or name"),
    ),
    request_body = InstallFilterRequest,
    responses(
        (status = 201, description = "Filter installed on listener", body = InstallFilterResponse),
        (status = 400, description = "Validation error - filter type incompatible with listener"),
        (status = 404, description = "Filter or listener not found"),
        (status = 409, description = "Filter already installed on this listener"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
)]
#[instrument(skip(state, context, payload), fields(filter_id = %filter_id, listener_name = %payload.listener_name, user_id = ?context.user_id))]
pub async fn install_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(filter_id): Path<String>,
    Json(payload): Json<InstallFilterRequest>,
) -> Result<(StatusCode, Json<InstallFilterResponse>), ApiError> {
    require_resource_access(&context, "filters", "write", None)?;

    let team_scopes = get_effective_team_scopes(&context);
    let repository = require_filter_repository(&state)?;

    // Get the filter and verify access
    let filter_id = FilterId::from_string(filter_id);
    let filter = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;
    let filter = verify_team_access(filter, &team_scopes).await?;

    // Resolve listener name to ID
    let listener_id = resolve_listener_id(&state, &payload.listener_name).await?;

    // Get listener details for response
    let listener_repository = state
        .xds_state
        .listener_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Listener repository not available"))?;
    let listener = listener_repository.get_by_id(&listener_id).await.map_err(ApiError::from)?;

    let service = FilterService::new(state.xds_state.clone());

    // Install the filter on the listener (reuses existing attach_filter_to_listener)
    service
        .attach_filter_to_listener(&listener_id, &filter_id, payload.order)
        .await
        .map_err(ApiError::from)?;

    // Determine the actual order (if not specified, get the max+1)
    let order = payload.order.unwrap_or(1);

    info!(
        filter_id = %filter.id,
        filter_name = %filter.name,
        listener_id = %listener_id,
        listener_name = %listener.name,
        order = order,
        "Filter installed on listener via API"
    );

    let response = InstallFilterResponse {
        filter_id: filter.id.to_string(),
        listener_id: listener_id.to_string(),
        listener_name: listener.name,
        order,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/filters/{filter_id}/installations/{listener_id}",
    params(
        ("filter_id" = String, Path, description = "Filter ID or name"),
        ("listener_id" = String, Path, description = "Listener ID or name"),
    ),
    responses(
        (status = 204, description = "Filter uninstalled from listener"),
        (status = 404, description = "Filter, listener, or installation not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(filter_id = %filter_id, listener_id = %listener_id, user_id = ?context.user_id))]
pub async fn uninstall_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((filter_id, listener_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "filters", "write", None)?;

    let team_scopes = get_effective_team_scopes(&context);
    let repository = require_filter_repository(&state)?;

    // Get the filter and verify access
    let filter_id = FilterId::from_string(filter_id);
    let filter = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;
    let _filter = verify_team_access(filter, &team_scopes).await?;

    // Resolve listener name to ID
    let listener_id = resolve_listener_id(&state, &listener_id).await?;

    let service = FilterService::new(state.xds_state.clone());

    // Uninstall the filter from the listener (reuses existing detach_filter_from_listener)
    service.detach_filter_from_listener(&listener_id, &filter_id).await.map_err(ApiError::from)?;

    info!(
        filter_id = %filter_id,
        listener_id = %listener_id,
        "Filter uninstalled from listener via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/filters/{filter_id}/installations",
    params(
        ("filter_id" = String, Path, description = "Filter ID or name"),
    ),
    responses(
        (status = 200, description = "List of listener installations", body = FilterInstallationsResponse),
        (status = 404, description = "Filter not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(filter_id = %filter_id, user_id = ?context.user_id))]
pub async fn list_filter_installations_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(filter_id): Path<String>,
) -> Result<Json<FilterInstallationsResponse>, ApiError> {
    require_resource_access(&context, "filters", "read", None)?;

    let team_scopes = get_effective_team_scopes(&context);
    let repository = require_filter_repository(&state)?;

    // Get the filter and verify access
    let filter_id = FilterId::from_string(filter_id);
    let filter = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;
    let filter = verify_team_access(filter, &team_scopes).await?;

    // Get all listeners this filter is installed on
    let installations =
        repository.list_filter_installations(&filter_id).await.map_err(ApiError::from)?;
    let installations: Vec<types::FilterInstallationItem> =
        installations.into_iter().map(|i| i.into()).collect();

    let response = FilterInstallationsResponse {
        filter_id: filter.id.to_string(),
        filter_name: filter.name,
        installations,
    };

    Ok(Json(response))
}

// ============================================================================
// Filter Configuration Handlers (Filter Install/Configure Redesign)
// ============================================================================

#[utoipa::path(
    post,
    path = "/api/v1/filters/{filter_id}/configurations",
    params(
        ("filter_id" = String, Path, description = "Filter ID or name"),
    ),
    request_body = ConfigureFilterRequest,
    responses(
        (status = 201, description = "Filter configured for scope", body = ConfigureFilterResponse),
        (status = 400, description = "Validation error - filter not installed on relevant listeners"),
        (status = 404, description = "Filter or scope not found"),
        (status = 409, description = "Filter already configured for this scope"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
)]
#[instrument(skip(state, context, payload), fields(filter_id = %filter_id, scope_type = %payload.scope_type, scope_id = %payload.scope_id, user_id = ?context.user_id))]
pub async fn configure_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(filter_id): Path<String>,
    Json(payload): Json<ConfigureFilterRequest>,
) -> Result<(StatusCode, Json<ConfigureFilterResponse>), ApiError> {
    require_resource_access(&context, "filters", "write", None)?;

    let team_scopes = get_effective_team_scopes(&context);
    let repository = require_filter_repository(&state)?;

    // Get the filter and verify access
    let filter_id = FilterId::from_string(filter_id);
    let filter = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;
    let filter = verify_team_access(filter, &team_scopes).await?;

    let service = FilterService::new(state.xds_state.clone());

    // Configure based on scope type
    let scope_name = match payload.scope_type {
        ScopeType::RouteConfig => {
            let route_config_id = resolve_route_config_id(&state, &payload.scope_id).await?;
            service
                .attach_filter_to_route_config(
                    &route_config_id,
                    &filter_id,
                    None,
                    payload.settings.clone(),
                )
                .await
                .map_err(ApiError::from)?;
            payload.scope_id.clone()
        }
        ScopeType::VirtualHost => {
            // Path format: "route-config-name/vhost-name"
            let parts: Vec<&str> = payload.scope_id.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err(ApiError::validation(
                    "Virtual host scope_id must be in format 'route-config-name/vhost-name'",
                ));
            }
            let virtual_host = resolve_virtual_host_data(&state, parts[0], parts[1]).await?;

            service
                .attach_filter_to_virtual_host(
                    &virtual_host.id,
                    &filter_id,
                    None,
                    payload.settings.clone(),
                )
                .await
                .map_err(ApiError::from)?;
            parts[1].to_string()
        }
        ScopeType::Route => {
            // Path format: "route-config-name/vhost-name/route-name"
            let parts: Vec<&str> = payload.scope_id.splitn(3, '/').collect();
            if parts.len() != 3 {
                return Err(ApiError::validation(
                    "Route scope_id must be in format 'route-config-name/vhost-name/route-name'",
                ));
            }
            let route_id = resolve_route_id(&state, parts[0], parts[1], parts[2]).await?;

            service
                .attach_filter_to_route(&route_id, &filter_id, None, payload.settings.clone())
                .await
                .map_err(ApiError::from)?;
            parts[2].to_string()
        }
    };

    info!(
        filter_id = %filter.id,
        filter_name = %filter.name,
        scope_type = %payload.scope_type,
        scope_id = %payload.scope_id,
        "Filter configured for scope via API"
    );

    let response = ConfigureFilterResponse {
        filter_id: filter.id.to_string(),
        scope_type: payload.scope_type,
        scope_id: payload.scope_id,
        scope_name,
        settings: payload.settings,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/filters/{filter_id}/configurations/{scope_type}/{scope_id}",
    params(
        ("filter_id" = String, Path, description = "Filter ID or name"),
        ("scope_type" = String, Path, description = "Scope type: route-config, virtual-host, or route"),
        ("scope_id" = String, Path, description = "Scope ID (URL-encoded if contains slashes)"),
    ),
    responses(
        (status = 204, description = "Filter configuration removed"),
        (status = 404, description = "Filter, scope, or configuration not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(filter_id = %filter_id, scope_type = %scope_type, scope_id = %scope_id, user_id = ?context.user_id))]
pub async fn remove_filter_configuration_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((filter_id, scope_type, scope_id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    require_resource_access(&context, "filters", "write", None)?;

    let team_scopes = get_effective_team_scopes(&context);
    let repository = require_filter_repository(&state)?;

    // Get the filter and verify access
    let filter_id = FilterId::from_string(filter_id);
    let filter = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;
    let _filter = verify_team_access(filter, &team_scopes).await?;

    // Parse scope type
    let scope_type: ScopeType = scope_type.parse().map_err(|e: String| ApiError::validation(&e))?;

    let service = FilterService::new(state.xds_state.clone());

    // Remove configuration based on scope type
    match scope_type {
        ScopeType::RouteConfig => {
            let route_config_id = resolve_route_config_id(&state, &scope_id).await?;
            service
                .detach_filter_from_route_config(&route_config_id, &filter_id)
                .await
                .map_err(ApiError::from)?;
        }
        ScopeType::VirtualHost => {
            let parts: Vec<&str> = scope_id.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err(ApiError::validation(
                    "Virtual host scope_id must be in format 'route-config-name/vhost-name'",
                ));
            }
            let virtual_host = resolve_virtual_host_data(&state, parts[0], parts[1]).await?;

            service
                .detach_filter_from_virtual_host(&virtual_host.id, &filter_id)
                .await
                .map_err(ApiError::from)?;
        }
        ScopeType::Route => {
            let parts: Vec<&str> = scope_id.splitn(3, '/').collect();
            if parts.len() != 3 {
                return Err(ApiError::validation(
                    "Route scope_id must be in format 'route-config-name/vhost-name/route-name'",
                ));
            }
            let route_id = resolve_route_id(&state, parts[0], parts[1], parts[2]).await?;

            service
                .detach_filter_from_route(&route_id, &filter_id)
                .await
                .map_err(ApiError::from)?;
        }
    }

    info!(
        filter_id = %filter_id,
        scope_type = %scope_type,
        scope_id = %scope_id,
        "Filter configuration removed via API"
    );

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/filters/{filter_id}/configurations",
    params(
        ("filter_id" = String, Path, description = "Filter ID or name"),
    ),
    responses(
        (status = 200, description = "List of filter configurations", body = FilterConfigurationsResponse),
        (status = 404, description = "Filter not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(filter_id = %filter_id, user_id = ?context.user_id))]
pub async fn list_filter_configurations_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(filter_id): Path<String>,
) -> Result<Json<FilterConfigurationsResponse>, ApiError> {
    require_resource_access(&context, "filters", "read", None)?;

    let team_scopes = get_effective_team_scopes(&context);
    let repository = require_filter_repository(&state)?;

    // Get the filter and verify access
    let filter_id = FilterId::from_string(filter_id);
    let filter = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;
    let filter = verify_team_access(filter, &team_scopes).await?;

    // Get all configurations for this filter
    let configurations =
        repository.list_filter_configurations(&filter_id).await.map_err(ApiError::from)?;
    let configurations: Vec<types::FilterConfigurationItem> =
        configurations.into_iter().map(|c| c.into()).collect();

    let response = FilterConfigurationsResponse {
        filter_id: filter.id.to_string(),
        filter_name: filter.name,
        configurations,
    };

    Ok(Json(response))
}

// ============================================================================
// Filter Status Handler (Combined view)
// ============================================================================

#[utoipa::path(
    get,
    path = "/api/v1/filters/{filter_id}/status",
    params(
        ("filter_id" = String, Path, description = "Filter ID or name"),
    ),
    responses(
        (status = 200, description = "Filter status with installations and configurations", body = FilterStatusResponse),
        (status = 404, description = "Filter not found"),
        (status = 503, description = "Repository unavailable"),
    ),
    tag = "Filters"
)]
#[instrument(skip(state, context), fields(filter_id = %filter_id, user_id = ?context.user_id))]
pub async fn get_filter_status_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(filter_id): Path<String>,
) -> Result<Json<FilterStatusResponse>, ApiError> {
    require_resource_access(&context, "filters", "read", None)?;

    let team_scopes = get_effective_team_scopes(&context);
    let repository = require_filter_repository(&state)?;

    // Get the filter and verify access
    let filter_id = FilterId::from_string(filter_id);
    let filter = repository.get_by_id(&filter_id).await.map_err(ApiError::from)?;
    let filter = verify_team_access(filter, &team_scopes).await?;

    // Get installations and configurations
    let installations =
        repository.list_filter_installations(&filter_id).await.map_err(ApiError::from)?;
    let installations: Vec<types::FilterInstallationItem> =
        installations.into_iter().map(|i| i.into()).collect();
    let configurations =
        repository.list_filter_configurations(&filter_id).await.map_err(ApiError::from)?;
    let configurations: Vec<types::FilterConfigurationItem> =
        configurations.into_iter().map(|c| c.into()).collect();

    let response = FilterStatusResponse {
        filter_id: filter.id.to_string(),
        filter_name: filter.name,
        filter_type: filter.filter_type,
        description: filter.description,
        installations,
        configurations,
    };

    Ok(Json(response))
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, Query, State};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Extension;

    use crate::api::test_utils::{
        admin_auth_context, create_test_state, minimal_auth_context,
        readonly_resource_auth_context, team_auth_context,
    };
    use crate::domain::filter::{FilterConfig, HeaderMutationEntry, HeaderMutationFilterConfig};

    // Use test_utils helpers:
    // - admin_auth_context() -> admin_auth_context()
    // - team_auth_context(team) -> team_auth_context(team)
    // - readonly_resource_auth_context("filters") -> readonly_resource_auth_context("filters")
    // - minimal_auth_context() -> minimal_auth_context()

    async fn setup_state_with_team(
        team_name: &str,
    ) -> (crate::storage::test_helpers::TestDatabase, ApiState) {
        use crate::auth::team::CreateTeamRequest;
        use crate::storage::repositories::{SqlxTeamRepository, TeamRepository};

        let (_db, state) = create_test_state().await;

        // Create the team
        let cluster_repo = state.xds_state.cluster_repository.as_ref().unwrap();
        let pool = cluster_repo.pool().clone();
        let team_repo = SqlxTeamRepository::new(pool);

        // Use get_or_create pattern to handle seed data
        if team_repo.get_team_by_name(team_name).await.ok().flatten().is_none() {
            team_repo
                .create_team(CreateTeamRequest {
                    name: team_name.to_string(),
                    display_name: format!("Test Team {}", team_name),
                    description: Some("Test team".to_string()),
                    owner_user_id: None,
                    settings: None,
                })
                .await
                .expect("create team");
        }

        (_db, state)
    }

    fn sample_create_filter_request(team: &str) -> CreateFilterRequest {
        CreateFilterRequest {
            team: team.to_string(),
            name: "test-filter".to_string(),
            filter_type: "header_mutation".to_string(),
            description: Some("A test header mutation filter".to_string()),
            config: FilterConfig::HeaderMutation(HeaderMutationFilterConfig {
                request_headers_to_add: vec![HeaderMutationEntry {
                    key: "X-Test-Header".to_string(),
                    value: "test-value".to_string(),
                    append: false,
                }],
                request_headers_to_remove: vec![],
                response_headers_to_add: vec![],
                response_headers_to_remove: vec![],
            }),
            cluster_config: None,
        }
    }

    fn empty_list_query() -> ListFiltersQuery {
        ListFiltersQuery { limit: None, offset: None }
    }

    // === Filter CRUD Tests ===

    #[tokio::test]
    async fn test_list_filters_empty() {
        let (_db, state) = create_test_state().await;

        let result = list_filters_handler(
            State(state),
            Extension(admin_auth_context()),
            Query(empty_list_query()),
        )
        .await;

        assert!(result.is_ok());
        let Json(filters) = result.unwrap();
        assert!(filters.is_empty());
    }

    #[tokio::test]
    async fn test_list_filters_requires_read_scope() {
        let (_db, state) = create_test_state().await;

        let result = list_filters_handler(
            State(state),
            Extension(minimal_auth_context()),
            Query(empty_list_query()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_create_filter_with_admin_auth_context() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        let result =
            create_filter_handler(State(state), Extension(admin_auth_context()), Json(body)).await;

        assert!(result.is_ok());
        let (status, Json(response)) = result.unwrap();
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(response.name, "test-filter");
        assert_eq!(response.filter_type, "header_mutation");
    }

    #[tokio::test]
    async fn test_create_filter_with_team_auth_context() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        let result = create_filter_handler(
            State(state),
            Extension(team_auth_context("test-team")),
            Json(body),
        )
        .await;

        assert!(result.is_ok());
        let (status, _) = result.unwrap();
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_create_filter_fails_without_write_scope() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        let result = create_filter_handler(
            State(state),
            Extension(readonly_resource_auth_context("filters")),
            Json(body),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_create_filter_validates_team_exists() {
        let (_db, state) = create_test_state().await; // No team created
        let body = sample_create_filter_request("non-existent-team");

        let result =
            create_filter_handler(State(state), Extension(admin_auth_context()), Json(body)).await;

        // Should fail because team doesn't exist
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_filter_returns_details() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        // Create filter
        let (_, Json(created)) = create_filter_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(body),
        )
        .await
        .expect("create filter");

        // Get the filter
        let result = get_filter_handler(
            State(state),
            Extension(admin_auth_context()),
            Path(created.id.clone()),
        )
        .await;

        assert!(result.is_ok());
        let Json(filter) = result.unwrap();
        assert_eq!(filter.id, created.id);
        assert_eq!(filter.name, "test-filter");
    }

    #[tokio::test]
    async fn test_get_filter_not_found() {
        let (_db, state) = create_test_state().await;

        let result = get_filter_handler(
            State(state),
            Extension(admin_auth_context()),
            Path("non-existent-filter-id".to_string()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_update_filter_changes_description() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        // Create filter
        let (_, Json(created)) = create_filter_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(body),
        )
        .await
        .expect("create filter");

        // Update the filter
        let update_body = UpdateFilterRequest {
            name: None,
            description: Some("Updated description".to_string()),
            config: None,
        };

        let result = update_filter_handler(
            State(state),
            Extension(admin_auth_context()),
            Path(created.id.clone()),
            Json(update_body),
        )
        .await;

        assert!(result.is_ok());
        let Json(filter) = result.unwrap();
        assert_eq!(filter.description, Some("Updated description".to_string()));
    }

    #[tokio::test]
    async fn test_update_filter_requires_write_scope() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        // Create filter
        let (_, Json(created)) = create_filter_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(body),
        )
        .await
        .expect("create filter");

        // Try to update with readonly context
        let update_body = UpdateFilterRequest { name: None, description: None, config: None };

        let result = update_filter_handler(
            State(state),
            Extension(readonly_resource_auth_context("filters")),
            Path(created.id.clone()),
            Json(update_body),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_delete_filter_removes_record() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        // Create filter
        let (_, Json(created)) = create_filter_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(body),
        )
        .await
        .expect("create filter");

        // Delete the filter
        let result = delete_filter_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Path(created.id.clone()),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StatusCode::NO_CONTENT);

        // Verify it's gone
        let get_result =
            get_filter_handler(State(state), Extension(admin_auth_context()), Path(created.id))
                .await;

        assert!(get_result.is_err());
    }

    #[tokio::test]
    async fn test_delete_filter_requires_write_scope() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        // Create filter
        let (_, Json(created)) = create_filter_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(body),
        )
        .await
        .expect("create filter");

        // Try to delete with readonly context
        let result = delete_filter_handler(
            State(state),
            Extension(readonly_resource_auth_context("filters")),
            Path(created.id),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_delete_filter_not_found() {
        let (_db, state) = create_test_state().await;

        let result = delete_filter_handler(
            State(state),
            Extension(admin_auth_context()),
            Path("non-existent-filter-id".to_string()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_list_filters_returns_created_filters() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        // Create filter
        let _ = create_filter_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(body),
        )
        .await
        .expect("create filter");

        // List filters
        let result = list_filters_handler(
            State(state),
            Extension(admin_auth_context()),
            Query(empty_list_query()),
        )
        .await;

        assert!(result.is_ok());
        let Json(filters) = result.unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].name, "test-filter");
    }

    #[tokio::test]
    async fn test_list_filters_with_pagination() {
        let (_db, state) = setup_state_with_team("test-team").await;

        // Create multiple filters
        for i in 0..5 {
            let mut body = sample_create_filter_request("test-team");
            body.name = format!("test-filter-{}", i);
            let _ = create_filter_handler(
                State(state.clone()),
                Extension(admin_auth_context()),
                Json(body),
            )
            .await
            .expect("create filter");
        }

        // List with limit
        let result = list_filters_handler(
            State(state),
            Extension(admin_auth_context()),
            Query(ListFiltersQuery { limit: Some(2), offset: Some(0) }),
        )
        .await;

        assert!(result.is_ok());
        let Json(filters) = result.unwrap();
        assert_eq!(filters.len(), 2);
    }

    // === Filter Status Tests ===

    #[tokio::test]
    async fn test_get_filter_status_returns_installations_and_configurations() {
        let (_db, state) = setup_state_with_team("test-team").await;
        let body = sample_create_filter_request("test-team");

        // Create filter
        let (_, Json(created)) = create_filter_handler(
            State(state.clone()),
            Extension(admin_auth_context()),
            Json(body),
        )
        .await
        .expect("create filter");

        // Get status
        let result = get_filter_status_handler(
            State(state),
            Extension(admin_auth_context()),
            Path(created.id.clone()),
        )
        .await;

        assert!(result.is_ok());
        let Json(status) = result.unwrap();
        assert_eq!(status.filter_id, created.id);
        assert_eq!(status.filter_name, "test-filter");
        assert!(status.installations.is_empty());
        assert!(status.configurations.is_empty());
    }

    #[tokio::test]
    async fn test_get_filter_status_not_found() {
        let (_db, state) = create_test_state().await;

        let result = get_filter_status_handler(
            State(state),
            Extension(admin_auth_context()),
            Path("non-existent-filter-id".to_string()),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
