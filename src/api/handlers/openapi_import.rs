//! OpenAPI Import Handlers - Direct-to-Routes Materialization
//!
//! This module provides handlers for importing OpenAPI specifications directly into
//! the routes, clusters, and listeners tables without intermediate api_definitions tables.
//!
//! **Key Features:**
//! - Direct route materialization (no dual storage)
//! - Cluster deduplication across imports
//! - Import tracking via import_metadata table
//! - Cascade delete support
//! - Team isolation

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, Request, StatusCode},
    Extension, Json,
};
use bytes::Bytes;
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::instrument;
use utoipa::{IntoParams, ToSchema};

use std::collections::HashMap;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::{
        authorization::{extract_team_scopes, has_admin_bypass, require_resource_access},
        models::AuthContext,
    },
    domain::{RouteConfigId, RouteMetadataSourceType},
    openapi::{build_gateway_plan, GatewayOptions, RouteMetadataEntry},
    storage::{
        repositories::{
            cluster_references::ClusterReferencesRepository,
            import_metadata::{
                CreateImportMetadataRequest, ImportMetadataData, ImportMetadataRepository,
            },
            route::RouteRepository,
            route_metadata::{CreateRouteMetadataRequest, RouteMetadataRepository},
        },
        CreateClusterRequest, CreateListenerRequest, CreateRouteConfigRepositoryRequest,
    },
    xds::XdsState,
};

// === Request/Response DTOs ===

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "snake_case")]
pub struct ImportOpenApiQuery {
    /// Team name for multi-tenancy isolation
    #[param(required = true, example = "payments")]
    pub team: String,
    /// Listener mode: "existing" to use an existing listener, "new" to create a new one
    #[param(required = true, example = "existing")]
    pub listener_mode: String,
    /// Name of an existing listener to use (required when listener_mode="existing")
    #[param(required = false, example = "my-api-listener")]
    pub existing_listener_name: Option<String>,
    /// Name for the new listener (required when listener_mode="new")
    #[param(required = false, example = "petstore-listener")]
    pub new_listener_name: Option<String>,
    /// Bind address for the new listener (default: 0.0.0.0)
    #[param(required = false, example = "0.0.0.0")]
    pub new_listener_address: Option<String>,
    /// Port for the new listener (default: 10000)
    #[param(required = false, example = 10000)]
    pub new_listener_port: Option<u16>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportResponse {
    /// Import ID
    #[schema(example = "550e8400-e29b-41d4-a716-446655440000")]
    pub import_id: String,
    /// Specification name extracted from OpenAPI
    #[schema(example = "Payments API")]
    pub spec_name: String,
    /// Specification version
    #[schema(example = "1.0.0")]
    pub spec_version: Option<String>,
    /// Number of routes created
    #[schema(example = 5)]
    pub routes_created: usize,
    /// Number of clusters created
    #[schema(example = 3)]
    pub clusters_created: usize,
    /// Number of existing clusters reused
    #[schema(example = 2)]
    pub clusters_reused: usize,
    /// Listener name (if created)
    #[schema(example = "payments-listener")]
    pub listener_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListImportsResponse {
    pub imports: Vec<ImportSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub id: String,
    pub spec_name: String,
    pub spec_version: Option<String>,
    pub team: String,
    pub listener_name: Option<String>,
    pub imported_at: String,
    pub updated_at: String,
}

impl From<ImportMetadataData> for ImportSummary {
    fn from(data: ImportMetadataData) -> Self {
        Self {
            id: data.id,
            spec_name: data.spec_name,
            spec_version: data.spec_version,
            team: data.team,
            listener_name: data.listener_name,
            imported_at: data.imported_at.to_rfc3339(),
            updated_at: data.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportDetailsResponse {
    pub id: String,
    pub spec_name: String,
    pub spec_version: Option<String>,
    pub spec_checksum: Option<String>,
    pub team: String,
    pub listener_name: Option<String>,
    pub imported_at: String,
    pub updated_at: String,
    pub route_count: usize,
    pub cluster_count: usize,
    pub listener_count: usize,
}

#[derive(Debug, ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct OpenApiSpecBody(pub Vec<u8>);

// === Core Import Logic ===

/// Import OpenAPI spec and materialize routes directly to routes table
#[utoipa::path(
    post,
    path = "/api/v1/openapi/import",
    params(ImportOpenApiQuery),
    request_body(
        description = "OpenAPI 3.0 document in JSON or YAML format",
        content(
            (OpenApiSpecBody = "application/yaml"),
            (OpenApiSpecBody = "application/x-yaml"),
            (OpenApiSpecBody = "application/json")
        )
    ),
    responses(
        (status = 201, description = "OpenAPI spec successfully imported", body = ImportResponse),
        (status = 400, description = "Invalid OpenAPI spec or parameters"),
        (status = 500, description = "Internal server error")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state, request), fields(team = %params.team, listener_mode = %params.listener_mode, user_id = ?context.user_id))]
pub async fn import_openapi_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<ImportOpenApiQuery>,
    request: Request<Body>,
) -> std::result::Result<(StatusCode, Json<ImportResponse>), ApiError> {
    // Authorization: require openapi-import:write scope for the target team
    require_resource_access(&context, "openapi-import", "write", Some(&params.team))?;

    // Team-scoped users validation (skip for admins)
    if !has_admin_bypass(&context) {
        let team_scopes = extract_team_scopes(&context);
        if !team_scopes.is_empty() && !team_scopes.contains(&params.team) {
            return Err(ApiError::Forbidden(format!(
                "Cannot import for team '{}' - not in your team scopes",
                params.team
            )));
        }
    }

    // Read request body
    let (parts, body) = request.into_parts();
    let collected = body
        .collect()
        .await
        .map_err(|err| ApiError::BadRequest(format!("Failed to read body: {}", err)))?;
    let bytes = collected.to_bytes();

    if bytes.is_empty() {
        return Err(ApiError::BadRequest(
            "OpenAPI specification body must not be empty".to_string(),
        ));
    }

    // Parse OpenAPI document
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<mime::Mime>().ok());

    let document = parse_openapi_document(&bytes, content_type.as_ref())?;

    // Build gateway plan from OpenAPI
    let spec_name = document.info.title.clone();
    let spec_version = Some(document.info.version.clone());
    let spec_checksum = Some(compute_checksum(&bytes));

    let gateway_name = sanitize_gateway_name(&spec_name);

    // Determine listener mode from query parameters
    let listener_mode = if params.listener_mode == "existing" {
        let listener_name = params.existing_listener_name.clone().ok_or_else(|| {
            ApiError::BadRequest(
                "existing_listener_name is required when listener_mode is 'existing'".to_string(),
            )
        })?;
        crate::openapi::ListenerMode::Existing { name: listener_name }
    } else {
        let listener_name = params
            .new_listener_name
            .clone()
            .unwrap_or_else(|| format!("{}-listener", gateway_name));
        let address = params.new_listener_address.clone().unwrap_or_else(|| "0.0.0.0".to_string());
        let port = params.new_listener_port.unwrap_or(10000);
        crate::openapi::ListenerMode::New { name: listener_name, address, port }
    };

    let listener_name = match &listener_mode {
        crate::openapi::ListenerMode::Existing { name } => name.clone(),
        crate::openapi::ListenerMode::New { name, .. } => name.clone(),
    };

    let gateway_options =
        GatewayOptions { name: gateway_name.clone(), protocol: "HTTP".to_string(), listener_mode };

    let plan = build_gateway_plan(document, gateway_options)
        .map_err(|err| ApiError::BadRequest(format!("Failed to build gateway plan: {}", err)))?;

    // Get database pool from cluster repository
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Cluster repository not configured".to_string()))?;
    let db_pool = cluster_repo.pool().clone();

    // Create import metadata record
    let import_repo = ImportMetadataRepository::new(db_pool.clone());
    let import_metadata = import_repo
        .create(CreateImportMetadataRequest {
            spec_name: spec_name.clone(),
            spec_version: spec_version.clone(),
            spec_checksum: spec_checksum.clone(),
            team: params.team.clone(),
            source_content: None, // TODO: optionally store source
            listener_name: Some(listener_name.clone()),
        })
        .await
        .map_err(ApiError::from)?;

    let import_id = import_metadata.id.clone();

    // Materialize clusters with deduplication
    let (clusters_created, clusters_reused) = materialize_clusters(
        &state.xds_state,
        &db_pool,
        &import_id,
        &params.team,
        &plan.cluster_requests,
    )
    .await?;

    // Materialize routes
    let routes_count = if let Some(route_request) = plan.route_request {
        materialize_route(&state.xds_state, &db_pool, &import_id, &params.team, route_request)
            .await?;
        1
    } else if let Some(virtual_host) = plan.default_virtual_host {
        // For existing listener mode, merge virtual host into the listener's route config
        materialize_routes_from_virtual_host(
            &state.xds_state,
            &db_pool,
            &import_id,
            &params.team,
            virtual_host,
            &listener_name,
        )
        .await?
    } else {
        0
    };

    // Materialize listener (if needed)
    let listener_created = if let Some(listener_request) = plan.listener_request {
        materialize_listener(
            &state.xds_state,
            &db_pool,
            &import_id,
            &params.team,
            listener_request,
        )
        .await?;
        Some(listener_name)
    } else {
        None
    };

    // Create route metadata from OpenAPI operation metadata
    // This happens after route hierarchy sync so routes are in the database
    if !plan.route_metadata.is_empty() {
        create_route_metadata_from_plan(&db_pool, &params.team, &plan.route_metadata).await?;
    }

    // Trigger xDS refresh
    state.xds_state.refresh_routes_from_repository().await.map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(ImportResponse {
            import_id,
            spec_name,
            spec_version,
            routes_created: routes_count,
            clusters_created,
            clusters_reused,
            listener_name: listener_created,
        }),
    ))
}

/// List all imports for a team
#[utoipa::path(
    get,
    path = "/api/v1/openapi/imports",
    params(
        ("team" = String, Query, description = "Team name to filter imports", example = "payments")
    ),
    responses(
        (status = 200, description = "Successfully retrieved imports", body = ListImportsResponse),
        (status = 500, description = "Internal server error")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state, query), fields(user_id = ?context.user_id))]
pub async fn list_imports_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<serde_json::Value>,
) -> std::result::Result<Json<ListImportsResponse>, ApiError> {
    let team = query.get("team").and_then(|v| v.as_str());

    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Cluster repository not configured".to_string()))?;
    let db_pool = cluster_repo.pool().clone();
    let import_repo = ImportMetadataRepository::new(db_pool);

    // Admin users can list all imports when no team is specified
    if has_admin_bypass(&context) {
        let imports = if let Some(team) = team {
            // Admin requesting specific team's imports
            import_repo.list_by_team(team).await.map_err(ApiError::from)?
        } else {
            // Admin requesting all imports across all teams
            import_repo.list_all().await.map_err(ApiError::from)?
        };

        return Ok(Json(ListImportsResponse {
            imports: imports.into_iter().map(ImportSummary::from).collect(),
        }));
    }

    // Non-admin users must specify a team
    let team =
        team.ok_or_else(|| ApiError::BadRequest("team parameter is required".to_string()))?;

    // Authorization: require openapi-import:read scope for the target team
    require_resource_access(&context, "openapi-import", "read", Some(team))?;

    // Team-scoped users validation
    let team_scopes = extract_team_scopes(&context);
    if !team_scopes.is_empty() && !team_scopes.contains(&team.to_string()) {
        return Err(ApiError::Forbidden(format!(
            "Cannot list imports for team '{}' - not in your team scopes",
            team
        )));
    }

    let imports = import_repo.list_by_team(team).await.map_err(ApiError::from)?;

    Ok(Json(ListImportsResponse {
        imports: imports.into_iter().map(ImportSummary::from).collect(),
    }))
}

/// Get import details by ID
#[utoipa::path(
    get,
    path = "/api/v1/openapi/imports/{id}",
    params(
        ("id" = String, Path, description = "Import ID", example = "550e8400-e29b-41d4-a716-446655440000")
    ),
    responses(
        (status = 200, description = "Successfully retrieved import details", body = ImportDetailsResponse),
        (status = 404, description = "Import not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state), fields(import_id = %id, user_id = ?context.user_id))]
pub async fn get_import_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> std::result::Result<Json<ImportDetailsResponse>, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Cluster repository not configured".to_string()))?;
    let db_pool = cluster_repo.pool().clone();

    let import_repo = ImportMetadataRepository::new(db_pool.clone());
    let import_data = import_repo
        .get_by_id(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Import with ID '{}' not found", id)))?;

    // Authorization: require openapi-import:read scope for the import's team
    require_resource_access(&context, "openapi-import", "read", Some(&import_data.team))?;

    // Team-scoped users validation (skip for admins)
    if !has_admin_bypass(&context) {
        let team_scopes = extract_team_scopes(&context);
        if !team_scopes.is_empty() && !team_scopes.contains(&import_data.team) {
            return Err(ApiError::NotFound(format!("Import with ID '{}' not found", id)));
        }
    }

    // Count routes and clusters
    let route_config_repo =
        state.xds_state.route_config_repository.as_ref().ok_or_else(|| {
            ApiError::Internal("Route config repository not configured".to_string())
        })?;
    let listener_repo = state
        .xds_state
        .listener_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Listener repository not configured".to_string()))?;
    let cluster_ref_repo = ClusterReferencesRepository::new(db_pool);

    let routes = route_config_repo.list_by_import(&id).await.map_err(ApiError::from)?;
    let listener_count = listener_repo.count_by_import(&id).await.map_err(ApiError::from)?;
    let cluster_refs = cluster_ref_repo.get_by_import(&id).await.map_err(ApiError::from)?;

    Ok(Json(ImportDetailsResponse {
        id: import_data.id,
        spec_name: import_data.spec_name,
        spec_version: import_data.spec_version,
        spec_checksum: import_data.spec_checksum,
        team: import_data.team,
        listener_name: import_data.listener_name,
        imported_at: import_data.imported_at.to_rfc3339(),
        updated_at: import_data.updated_at.to_rfc3339(),
        route_count: routes.len(),
        cluster_count: cluster_refs.len(),
        listener_count: listener_count as usize,
    }))
}

/// Delete import and cascade to routes/clusters/listeners
#[utoipa::path(
    delete,
    path = "/api/v1/openapi/imports/{id}",
    params(
        ("id" = String, Path, description = "Import ID to delete", example = "550e8400-e29b-41d4-a716-446655440000")
    ),
    responses(
        (status = 204, description = "Import successfully deleted"),
        (status = 404, description = "Import not found"),
        (status = 500, description = "Internal server error")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state), fields(import_id = %id, user_id = ?context.user_id))]
pub async fn delete_import_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> std::result::Result<StatusCode, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Cluster repository not configured".to_string()))?;
    let db_pool = cluster_repo.pool().clone();

    let import_repo = ImportMetadataRepository::new(db_pool.clone());

    // Get import to verify team access
    let import_data = import_repo
        .get_by_id(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Import with ID '{}' not found", id)))?;

    // Authorization: require openapi-import:delete scope for the import's team
    require_resource_access(&context, "openapi-import", "delete", Some(&import_data.team))?;

    // Team-scoped users validation (skip for admins)
    if !has_admin_bypass(&context) {
        let team_scopes = extract_team_scopes(&context);
        if !team_scopes.is_empty() && !team_scopes.contains(&import_data.team) {
            return Err(ApiError::NotFound(format!("Import with ID '{}' not found", id)));
        }
    }

    // Delete import cascade logic
    delete_import_cascade(&state.xds_state, &db_pool, &id).await?;

    // Trigger xDS refresh
    state.xds_state.refresh_routes_from_repository().await.map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

// === Helper Functions ===

/// Create route_metadata records from OpenAPI operation metadata
///
/// This function looks up routes by name (created during hierarchy sync) and
/// creates corresponding metadata records with OpenAPI operation information.
async fn create_route_metadata_from_plan(
    db_pool: &crate::storage::DbPool,
    team: &str,
    route_metadata_map: &HashMap<String, RouteMetadataEntry>,
) -> std::result::Result<usize, ApiError> {
    let route_repo = RouteRepository::new(db_pool.clone());
    let route_metadata_repo = RouteMetadataRepository::new(db_pool.clone());

    let mut created_count = 0;

    // Get all routes for this team to match by name
    let routes = route_repo.list_by_team(team).await.map_err(|e| {
        ApiError::Internal(format!("Failed to list routes for team '{}': {}", team, e))
    })?;

    for (route_name, metadata_entry) in route_metadata_map {
        // Find the route with this name
        let route = routes.iter().find(|r| r.name == *route_name);

        if let Some(route) = route {
            // Check if metadata already exists for this route
            let existing = route_metadata_repo.get_by_route_id(&route.id).await.map_err(|e| {
                ApiError::Internal(format!("Failed to check existing metadata: {}", e))
            })?;

            if existing.is_some() {
                tracing::debug!(
                    route_id = %route.id,
                    route_name = %route_name,
                    "Route metadata already exists, skipping"
                );
                continue;
            }

            // Create the route metadata record
            let create_request = CreateRouteMetadataRequest {
                route_id: route.id.clone(),
                operation_id: metadata_entry.operation_id.clone(),
                summary: metadata_entry.summary.clone(),
                description: metadata_entry.description.clone(),
                tags: metadata_entry.tags.clone(),
                http_method: Some(metadata_entry.http_method.clone()),
                request_body_schema: metadata_entry.request_body_schema.clone(),
                response_schemas: metadata_entry.response_schemas.clone(),
                learning_schema_id: None,
                enriched_from_learning: false,
                source_type: RouteMetadataSourceType::Openapi,
                confidence: Some(1.0), // OpenAPI metadata has highest confidence
            };

            match route_metadata_repo.create(create_request).await {
                Ok(_) => {
                    created_count += 1;
                    tracing::info!(
                        route_id = %route.id,
                        route_name = %route_name,
                        operation_id = ?metadata_entry.operation_id,
                        "Created route metadata from OpenAPI"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        route_id = %route.id,
                        route_name = %route_name,
                        error = %e,
                        "Failed to create route metadata, continuing"
                    );
                }
            }
        } else {
            tracing::debug!(
                route_name = %route_name,
                "Route not found for metadata entry, may not have been synced yet"
            );
        }
    }

    tracing::info!(
        team = %team,
        metadata_count = created_count,
        "Created route metadata records from OpenAPI import"
    );

    Ok(created_count)
}

async fn materialize_clusters(
    xds_state: &XdsState,
    db_pool: &crate::storage::DbPool,
    import_id: &str,
    team: &str,
    cluster_requests: &[CreateClusterRequest],
) -> std::result::Result<(usize, usize), ApiError> {
    let cluster_repo = xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Cluster repository not configured".to_string()))?;
    let cluster_ref_repo = ClusterReferencesRepository::new(db_pool.clone());

    let mut created = 0;
    let mut reused = 0;

    for mut cluster_request in cluster_requests.iter().cloned() {
        // Check if cluster already exists (by name)
        let existing = cluster_repo.get_by_name(&cluster_request.name).await;

        if let Ok(existing_cluster) = existing {
            // Cluster exists - add reference using cluster ID
            cluster_ref_repo
                .add_reference(existing_cluster.id.as_ref(), import_id, 1)
                .await
                .map_err(ApiError::from)?;
            reused += 1;
        } else {
            // Create new cluster
            cluster_request.team = Some(team.to_string());
            cluster_request.import_id = Some(import_id.to_string());

            let created_cluster =
                cluster_repo.create(cluster_request.clone()).await.map_err(ApiError::from)?;
            cluster_ref_repo
                .add_reference(created_cluster.id.as_ref(), import_id, 1)
                .await
                .map_err(ApiError::from)?;
            created += 1;
        }
    }

    Ok((created, reused))
}

async fn materialize_route(
    xds_state: &XdsState,
    _db_pool: &crate::storage::DbPool,
    import_id: &str,
    team: &str,
    mut route_request: CreateRouteConfigRepositoryRequest,
) -> std::result::Result<(), ApiError> {
    let route_config_repo = xds_state
        .route_config_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Route config repository not configured".to_string()))?;

    // Add import metadata
    route_request.import_id = Some(import_id.to_string());
    route_request.team = Some(team.to_string());
    route_request.route_order = Some(0);

    // Parse the configuration for hierarchy sync
    let xds_config: crate::xds::route::RouteConfig =
        serde_json::from_value(route_request.configuration.clone()).map_err(|e| {
            ApiError::Internal(format!("Failed to parse route configuration for sync: {}", e))
        })?;

    let created = route_config_repo.create(route_request).await.map_err(ApiError::from)?;

    // Sync route hierarchy (extract virtual hosts and routes to database tables)
    if let Some(ref sync_service) = xds_state.route_hierarchy_sync_service {
        let route_config_id = RouteConfigId::from_string(created.id.to_string());
        if let Err(err) = sync_service.sync(&route_config_id, &xds_config).await {
            tracing::error!(error = %err, route_config_id = %created.id, "Failed to sync route hierarchy after OpenAPI import");
            // Continue anyway - the route config was created, hierarchy sync is optional
        }
    }

    Ok(())
}

async fn materialize_routes_from_virtual_host(
    xds_state: &XdsState,
    _db_pool: &crate::storage::DbPool,
    _import_id: &str,
    _team: &str,
    virtual_host: crate::xds::route::VirtualHostConfig,
    listener_name: &str,
) -> std::result::Result<usize, ApiError> {
    let route_config_repo = xds_state
        .route_config_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Route config repository not configured".to_string()))?;
    let listener_repo = xds_state
        .listener_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Listener repository not configured".to_string()))?;

    // Get the existing listener to find its route config name
    let listener = listener_repo.get_by_name(listener_name).await.map_err(|e| {
        ApiError::BadRequest(format!("Listener '{}' not found: {}", listener_name, e))
    })?;

    // Parse the listener configuration to find the route_config_name
    let listener_config: crate::xds::listener::ListenerConfig =
        serde_json::from_str(&listener.configuration).map_err(|e| {
            ApiError::Internal(format!("Failed to parse listener configuration: {}", e))
        })?;

    // Find the HTTP connection manager and get the route_config_name
    let route_config_name = listener_config
        .filter_chains
        .iter()
        .flat_map(|fc| fc.filters.iter())
        .find_map(|f| match &f.filter_type {
            crate::xds::listener::FilterType::HttpConnectionManager {
                route_config_name, ..
            } => route_config_name.clone(),
            _ => None,
        })
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Listener '{}' has no HTTP connection manager with route_config_name",
                listener_name
            ))
        })?;

    // Get the existing route config
    let existing_route_config =
        route_config_repo.get_by_name(&route_config_name).await.map_err(|e| {
            ApiError::Internal(format!("Route config '{}' not found: {}", route_config_name, e))
        })?;

    // Parse the existing route configuration
    let existing_config: serde_json::Value =
        serde_json::from_str(&existing_route_config.configuration).map_err(|e| {
            ApiError::Internal(format!("Failed to parse route configuration: {}", e))
        })?;

    let mut route_config: crate::xds::route::RouteConfig = serde_json::from_value(existing_config)
        .map_err(|e| ApiError::Internal(format!("Failed to deserialize route config: {}", e)))?;

    // Check if a virtual host with the same name already exists
    let vh_exists = route_config.virtual_hosts.iter().any(|vh| vh.name == virtual_host.name);

    if vh_exists {
        tracing::debug!(
            virtual_host_name = %virtual_host.name,
            route_config_name = %route_config_name,
            "Virtual host already exists in route config, skipping"
        );
        return Ok(0);
    }

    // Add the new virtual host to the existing route config
    route_config.virtual_hosts.push(virtual_host);

    // Serialize the updated route config
    let updated_configuration = serde_json::to_value(&route_config)
        .map_err(|e| ApiError::Internal(format!("Failed to serialize route config: {}", e)))?;

    // Update the route config in the database
    let update_request = crate::storage::UpdateRouteConfigRepositoryRequest {
        path_prefix: None,
        cluster_name: None,
        configuration: Some(updated_configuration),
        team: None,
    };

    route_config_repo
        .update(&existing_route_config.id, update_request)
        .await
        .map_err(ApiError::from)?;

    tracing::info!(
        route_config_name = %route_config_name,
        virtual_hosts_count = route_config.virtual_hosts.len(),
        "Added virtual host to existing route config"
    );

    // Sync route hierarchy (extract virtual hosts and routes to database tables)
    if let Some(ref sync_service) = xds_state.route_hierarchy_sync_service {
        let route_config_id = RouteConfigId::from_string(existing_route_config.id.to_string());
        if let Err(err) = sync_service.sync(&route_config_id, &route_config).await {
            tracing::error!(error = %err, route_config_id = %existing_route_config.id, "Failed to sync route hierarchy after adding virtual host");
            // Continue anyway - the route config was updated, hierarchy sync is optional
        }
    }

    // Return 1 to indicate virtual host was added
    Ok(1)
}

async fn materialize_listener(
    xds_state: &XdsState,
    _db_pool: &crate::storage::DbPool,
    import_id: &str,
    team: &str,
    mut listener_request: CreateListenerRequest,
) -> std::result::Result<(), ApiError> {
    let listener_repo = xds_state
        .listener_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Listener repository not configured".to_string()))?;

    // Add import metadata
    listener_request.team = Some(team.to_string());
    listener_request.import_id = Some(import_id.to_string());

    listener_repo.create(listener_request).await.map_err(ApiError::from)?;

    Ok(())
}

async fn cleanup_virtual_host_from_route_config(
    xds_state: &XdsState,
    spec_name: &str,
    listener_name: &str,
) -> std::result::Result<(), ApiError> {
    let route_config_repo = xds_state
        .route_config_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Route config repository not configured".to_string()))?;
    let listener_repo = xds_state
        .listener_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Listener repository not configured".to_string()))?;

    // Derive virtual host name (matches creation logic in openapi/mod.rs:93)
    // CRITICAL: Must sanitize spec_name the same way it's done during import
    let gateway_name = sanitize_gateway_name(spec_name);
    let virtual_host_name = format!("{}-vh", gateway_name);

    // Get the listener to find its route config name
    let listener = listener_repo.get_by_name(listener_name).await.map_err(|e| {
        ApiError::Internal(format!("Failed to get listener '{}': {}", listener_name, e))
    })?;

    // Parse the listener configuration to find the route_config_name
    let listener_config: crate::xds::listener::ListenerConfig =
        serde_json::from_str(&listener.configuration).map_err(|e| {
            ApiError::Internal(format!("Failed to parse listener configuration: {}", e))
        })?;

    // Find the HTTP connection manager and get the route_config_name
    let route_config_name = listener_config
        .filter_chains
        .iter()
        .flat_map(|fc| fc.filters.iter())
        .find_map(|f| match &f.filter_type {
            crate::xds::listener::FilterType::HttpConnectionManager {
                route_config_name, ..
            } => route_config_name.clone(),
            _ => None,
        })
        .ok_or_else(|| {
            ApiError::Internal(format!(
                "Listener '{}' has no HTTP connection manager with route_config_name",
                listener_name
            ))
        })?;

    // Get the existing route config
    let existing_route_config =
        route_config_repo.get_by_name(&route_config_name).await.map_err(|e| {
            ApiError::Internal(format!("Route config '{}' not found: {}", route_config_name, e))
        })?;

    // Parse the existing route configuration
    let mut route_config: crate::xds::route::RouteConfig =
        serde_json::from_str(&existing_route_config.configuration).map_err(|e| {
            ApiError::Internal(format!("Failed to parse route configuration: {}", e))
        })?;

    // Remove the virtual host
    let original_count = route_config.virtual_hosts.len();
    route_config.virtual_hosts.retain(|vh| vh.name != virtual_host_name);

    if route_config.virtual_hosts.len() < original_count {
        // Serialize the updated route config
        let updated_configuration = serde_json::to_value(&route_config)
            .map_err(|e| ApiError::Internal(format!("Failed to serialize route config: {}", e)))?;

        // Update the route config in the database
        let update_request = crate::storage::UpdateRouteConfigRepositoryRequest {
            path_prefix: None,
            cluster_name: None,
            configuration: Some(updated_configuration),
            team: None,
        };

        route_config_repo
            .update(&existing_route_config.id, update_request)
            .await
            .map_err(ApiError::from)?;

        tracing::info!(
            virtual_host_name = %virtual_host_name,
            route_config_name = %route_config_name,
            remaining_vh_count = route_config.virtual_hosts.len(),
            "Removed virtual host from route config during import deletion"
        );
    } else {
        tracing::debug!(
            virtual_host_name = %virtual_host_name,
            route_config_name = %route_config_name,
            "Virtual host not found in route config, nothing to clean up"
        );
    }

    Ok(())
}

async fn delete_import_cascade(
    xds_state: &XdsState,
    db_pool: &crate::storage::DbPool,
    import_id: &str,
) -> std::result::Result<(), ApiError> {
    let import_repo = ImportMetadataRepository::new(db_pool.clone());
    let cluster_ref_repo = ClusterReferencesRepository::new(db_pool.clone());
    let cluster_repo = xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Cluster repository not configured".to_string()))?;

    // Get import metadata to check if we need to clean up virtual hosts
    let import_metadata = import_repo
        .get_by_id(import_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("Import '{}' not found", import_id)))?;

    // Clean up virtual hosts from existing route configs (if using existing listener)
    if let Some(listener_name) = &import_metadata.listener_name {
        cleanup_virtual_host_from_route_config(
            xds_state,
            &import_metadata.spec_name,
            listener_name,
        )
        .await?;
    }

    // Get orphaned clusters before deleting references
    let orphaned_clusters =
        cluster_ref_repo.delete_by_import(import_id).await.map_err(ApiError::from)?;

    // Delete orphaned clusters
    for cluster_id in orphaned_clusters {
        let cluster_id_obj = crate::domain::ClusterId::from_string(cluster_id);
        cluster_repo.delete(&cluster_id_obj).await.map_err(ApiError::from)?;
    }

    // Delete import metadata (CASCADE deletes routes/listeners via FK)
    import_repo.delete(import_id).await.map_err(ApiError::from)?;

    Ok(())
}

fn parse_openapi_document(
    bytes: &Bytes,
    mime: Option<&mime::Mime>,
) -> std::result::Result<openapiv3::OpenAPI, ApiError> {
    if let Some(mime) = mime {
        if mime.subtype() == mime::JSON {
            return serde_json::from_slice(bytes).map_err(|err| {
                ApiError::BadRequest(format!("Invalid OpenAPI JSON document: {}", err))
            });
        }

        if mime.subtype() == "yaml"
            || mime.subtype() == "x-yaml"
            || mime.suffix().map(|name| name == "yaml").unwrap_or(false)
        {
            return parse_yaml(bytes);
        }
    }

    // Try JSON first, then YAML
    match serde_json::from_slice(bytes) {
        Ok(doc) => Ok(doc),
        Err(json_err) => match parse_yaml(bytes) {
            Ok(doc) => Ok(doc),
            Err(_) => Err(ApiError::BadRequest(format!(
                "Failed to parse OpenAPI spec as JSON ({}). YAML parsing also failed.",
                json_err
            ))),
        },
    }
}

fn parse_yaml(bytes: &Bytes) -> std::result::Result<openapiv3::OpenAPI, ApiError> {
    serde_yaml::from_slice(bytes)
        .map_err(|err| ApiError::BadRequest(format!("Invalid OpenAPI YAML document: {}", err)))
}

fn compute_checksum(bytes: &Bytes) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn sanitize_gateway_name(spec_name: &str) -> String {
    spec_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(48)
        .collect()
}
