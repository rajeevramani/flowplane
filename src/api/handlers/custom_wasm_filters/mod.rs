//! Custom WASM filter HTTP handlers
//!
//! Provides CRUD operations for user-uploaded WASM filters that can be
//! used as custom filter types in the system.

pub mod types;

use super::team_access::TeamPath;
pub use types::{
    CreateCustomWasmFilterRequest, CustomFilterPath, CustomWasmFilterResponse,
    UpdateCustomWasmFilterRequest,
};

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use tracing::{info, instrument, warn};

use crate::{
    api::{
        error::ApiError,
        handlers::{
            pagination::{PaginatedResponse, PaginationQuery},
            team_access::{
                get_effective_team_ids, require_resource_access_resolved, team_repo_from_state,
                verify_team_access,
            },
        },
        routes::ApiState,
    },
    auth::models::AuthContext,
    domain::CustomWasmFilterId,
    services::CustomWasmFilterService,
    storage::UpdateCustomWasmFilterRequest as DbUpdateRequest,
};

/// Register a custom WASM filter schema in the shared registry
async fn register_custom_schema(state: &ApiState, data: &crate::storage::CustomWasmFilterData) {
    if let Some(ref registry) = state.filter_schema_registry {
        let schema = CustomWasmFilterService::generate_schema_definition(data);
        let filter_type = schema.name.clone();
        let mut registry = registry.write().await;
        match registry.register_custom_schema(schema) {
            Ok(()) => {
                info!(filter_type = %filter_type, "Registered custom WASM filter schema");
            }
            Err(e) => {
                warn!(
                    filter_type = %filter_type,
                    error = %e,
                    "Failed to register custom WASM filter schema"
                );
            }
        }
    }
}

/// Unregister a custom WASM filter schema from the shared registry
async fn unregister_custom_schema(state: &ApiState, filter_type: &str) {
    if let Some(ref registry) = state.filter_schema_registry {
        let mut registry = registry.write().await;
        if registry.unregister_custom_schema(filter_type).is_some() {
            info!(filter_type = %filter_type, "Unregistered custom WASM filter schema");
        }
    }
}

/// Verify the user has access to the specified team for custom-wasm-filters resource
async fn verify_team_access_for_filters(
    state: &ApiState,
    context: &AuthContext,
    team: &str,
    action: &str,
) -> Result<(), ApiError> {
    require_resource_access_resolved(
        state,
        context,
        "custom-wasm-filters",
        action,
        Some(team),
        context.org_id.as_ref(),
    )
    .await
}

/// Get the custom WASM filter service
fn get_service(state: &ApiState) -> Result<CustomWasmFilterService, ApiError> {
    if state.xds_state.custom_wasm_filter_repository.is_some() {
        Ok(CustomWasmFilterService::new(state.xds_state.clone()))
    } else {
        Err(ApiError::service_unavailable("Custom WASM filter repository unavailable"))
    }
}

// === Handler Implementations ===

/// Create a new custom WASM filter
#[utoipa::path(
    post,
    path = "/api/v1/teams/{team}/custom-filters",
    request_body = CreateCustomWasmFilterRequest,
    responses(
        (status = 201, description = "Custom filter created", body = CustomWasmFilterResponse),
        (status = 400, description = "Validation error"),
        (status = 409, description = "Filter with this name already exists"),
        (status = 503, description = "Service unavailable")
    ),
    params(
        ("team" = String, Path, description = "Team name")
    ),
    tag = "Custom WASM Filters",
    security(("bearerAuth" = []))
)]
#[instrument(skip(state, payload), fields(team = %team, filter_name = %payload.name, user_id = ?context.user_id))]
pub async fn create_custom_wasm_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Json(payload): Json<CreateCustomWasmFilterRequest>,
) -> Result<(StatusCode, Json<CustomWasmFilterResponse>), ApiError> {
    use validator::Validate;
    payload.validate().map_err(ApiError::from)?;

    // Verify user has write access to the specified team
    verify_team_access_for_filters(&state, &context, &team, "write").await?;

    // Decode base64 WASM binary
    let wasm_binary = BASE64.decode(&payload.wasm_binary_base64).map_err(|e| {
        ApiError::BadRequest(format!("Invalid base64 encoding for WASM binary: {}", e))
    })?;

    let service = get_service(&state)?;

    let created = service
        .create_custom_filter(
            payload.name,
            payload.display_name,
            payload.description,
            wasm_binary,
            payload.config_schema,
            payload.per_route_config_schema,
            payload.ui_hints,
            payload.attachment_points,
            payload.runtime,
            payload.failure_policy,
            team,
            context.user_id.as_ref().map(|id| id.to_string()),
        )
        .await
        .map_err(ApiError::from)?;

    // Register the schema in the filter schema registry so it can be used immediately
    register_custom_schema(&state, &created).await;

    Ok((StatusCode::CREATED, Json(CustomWasmFilterResponse::from_data(&created))))
}

/// List custom WASM filters for a team
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/custom-filters",
    responses(
        (status = 200, description = "List of custom filters", body = PaginatedResponse<CustomWasmFilterResponse>),
        (status = 503, description = "Service unavailable")
    ),
    params(
        ("team" = String, Path, description = "Team name"),
        PaginationQuery
    ),
    tag = "Custom WASM Filters",
    security(("bearerAuth" = []))
)]
#[instrument(skip(state), fields(team = %team, user_id = ?context.user_id))]
pub async fn list_custom_wasm_filters_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(TeamPath { team }): Path<TeamPath>,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<PaginatedResponse<CustomWasmFilterResponse>>, ApiError> {
    // Verify user has read access
    verify_team_access_for_filters(&state, &context, &team, "read").await?;

    let service = get_service(&state)?;

    let filters = service
        .list_custom_filters(std::slice::from_ref(&team), query.limit, query.offset)
        .await
        .map_err(ApiError::from)?;

    let total = service.count_by_team(&team).await.map_err(ApiError::from)?;

    let items = filters.iter().map(CustomWasmFilterResponse::from_data).collect();

    Ok(Json(PaginatedResponse::new(items, total, query.limit, query.offset)))
}

/// Get a custom WASM filter by ID
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/custom-filters/{id}",
    responses(
        (status = 200, description = "Custom filter details", body = CustomWasmFilterResponse),
        (status = 404, description = "Filter not found"),
        (status = 503, description = "Service unavailable")
    ),
    params(
        ("team" = String, Path, description = "Team name"),
        ("id" = String, Path, description = "Custom filter ID")
    ),
    tag = "Custom WASM Filters",
    security(("bearerAuth" = []))
)]
#[instrument(skip(state), fields(team = %team, filter_id = %id, user_id = ?context.user_id))]
pub async fn get_custom_wasm_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(CustomFilterPath { team, id }): Path<CustomFilterPath>,
) -> Result<Json<CustomWasmFilterResponse>, ApiError> {
    // Verify user has read access
    verify_team_access_for_filters(&state, &context, &team, "read").await?;

    let service = get_service(&state)?;

    let filter_id = CustomWasmFilterId::from_string(id);
    let filter = service.get_custom_filter(&filter_id).await.map_err(ApiError::from)?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let filter = verify_team_access(filter, &team_scopes).await?;

    Ok(Json(CustomWasmFilterResponse::from_data(&filter)))
}

/// Update a custom WASM filter's metadata
#[utoipa::path(
    put,
    path = "/api/v1/teams/{team}/custom-filters/{id}",
    request_body = UpdateCustomWasmFilterRequest,
    responses(
        (status = 200, description = "Custom filter updated", body = CustomWasmFilterResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Filter not found"),
        (status = 503, description = "Service unavailable")
    ),
    params(
        ("team" = String, Path, description = "Team name"),
        ("id" = String, Path, description = "Custom filter ID")
    ),
    tag = "Custom WASM Filters",
    security(("bearerAuth" = []))
)]
#[instrument(skip(state, payload), fields(team = %team, filter_id = %id, user_id = ?context.user_id))]
pub async fn update_custom_wasm_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(CustomFilterPath { team, id }): Path<CustomFilterPath>,
    Json(payload): Json<UpdateCustomWasmFilterRequest>,
) -> Result<Json<CustomWasmFilterResponse>, ApiError> {
    use validator::Validate;
    payload.validate().map_err(ApiError::from)?;

    // Verify user has write access
    verify_team_access_for_filters(&state, &context, &team, "write").await?;

    let service = get_service(&state)?;

    // First verify the filter exists and belongs to this team
    let filter_id = CustomWasmFilterId::from_string(id);
    let existing = service.get_custom_filter(&filter_id).await.map_err(ApiError::from)?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let _existing = verify_team_access(existing, &team_scopes).await?;

    // Build database update request
    let db_request = DbUpdateRequest {
        display_name: payload.display_name,
        description: payload.description,
        config_schema: payload.config_schema,
        per_route_config_schema: payload.per_route_config_schema,
        ui_hints: payload.ui_hints,
        attachment_points: payload.attachment_points,
    };

    let updated =
        service.update_custom_filter(&filter_id, db_request).await.map_err(ApiError::from)?;

    Ok(Json(CustomWasmFilterResponse::from_data(&updated)))
}

/// Delete a custom WASM filter
#[utoipa::path(
    delete,
    path = "/api/v1/teams/{team}/custom-filters/{id}",
    responses(
        (status = 204, description = "Custom filter deleted"),
        (status = 404, description = "Filter not found"),
        (status = 409, description = "Filter is in use"),
        (status = 503, description = "Service unavailable")
    ),
    params(
        ("team" = String, Path, description = "Team name"),
        ("id" = String, Path, description = "Custom filter ID")
    ),
    tag = "Custom WASM Filters",
    security(("bearerAuth" = []))
)]
#[instrument(skip(state), fields(team = %team, filter_id = %id, user_id = ?context.user_id))]
pub async fn delete_custom_wasm_filter_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(CustomFilterPath { team, id }): Path<CustomFilterPath>,
) -> Result<StatusCode, ApiError> {
    // Verify user has write access
    verify_team_access_for_filters(&state, &context, &team, "write").await?;

    let service = get_service(&state)?;

    // First verify the filter exists and belongs to this team
    let filter_id = CustomWasmFilterId::from_string(id);
    let existing = service.get_custom_filter(&filter_id).await.map_err(ApiError::from)?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let existing = verify_team_access(existing, &team_scopes).await?;

    // Store the filter type before deletion for schema unregistration
    // Note: The service will check for filter instances and return 409 if any exist
    let filter_type = format!("custom_wasm_{}", existing.id);

    service.delete_custom_filter(&filter_id).await.map_err(ApiError::from)?;

    // Unregister the schema from the filter schema registry
    unregister_custom_schema(&state, &filter_type).await;

    Ok(StatusCode::NO_CONTENT)
}

/// Download the WASM binary for a custom filter
#[utoipa::path(
    get,
    path = "/api/v1/teams/{team}/custom-filters/{id}/download",
    responses(
        (status = 200, description = "WASM binary", content_type = "application/wasm"),
        (status = 404, description = "Filter not found"),
        (status = 503, description = "Service unavailable")
    ),
    params(
        ("team" = String, Path, description = "Team name"),
        ("id" = String, Path, description = "Custom filter ID")
    ),
    tag = "Custom WASM Filters",
    security(("bearerAuth" = []))
)]
#[instrument(skip(state), fields(team = %team, filter_id = %id, user_id = ?context.user_id))]
pub async fn download_wasm_binary_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(CustomFilterPath { team, id }): Path<CustomFilterPath>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify user has read access
    verify_team_access_for_filters(&state, &context, &team, "read").await?;

    let service = get_service(&state)?;

    // First verify the filter exists and belongs to this team
    let filter_id = CustomWasmFilterId::from_string(id.clone());
    let existing = service.get_custom_filter(&filter_id).await.map_err(ApiError::from)?;

    // Verify team access using unified verifier
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let existing = verify_team_access(existing, &team_scopes).await?;

    // Get the binary
    let binary = service.get_wasm_binary(&filter_id).await.map_err(ApiError::from)?;

    // Return with appropriate headers
    let headers = [
        (axum::http::header::CONTENT_TYPE, "application/wasm".to_string()),
        (
            axum::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}.wasm\"", existing.name),
        ),
    ];

    Ok((headers, Bytes::from(binary)))
}
