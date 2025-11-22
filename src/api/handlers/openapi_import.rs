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
use utoipa::{IntoParams, ToSchema};

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::{
        authorization::{extract_team_scopes, has_admin_bypass, require_resource_access},
        models::AuthContext,
    },
    openapi::{build_gateway_plan, GatewayOptions},
    storage::{
        repositories::{
            cluster_references::ClusterReferencesRepository,
            import_metadata::{
                CreateImportMetadataRequest, ImportMetadataData, ImportMetadataRepository,
            },
        },
        CreateClusterRequest, CreateListenerRequest, CreateRouteRepositoryRequest,
    },
    xds::XdsState,
};

// === Request/Response DTOs ===

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ImportOpenApiQuery {
    /// Team name for multi-tenancy isolation
    #[param(required = true, example = "payments")]
    pub team: String,
    /// Optional custom port for isolated listener (if not using shared listener)
    #[param(required = false, example = 10000)]
    pub port: Option<u16>,
    /// Whether to use the shared default gateway listener
    #[param(required = false, example = false)]
    #[serde(default)]
    pub shared_listener: bool,
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
    tag = "openapi-import"
)]
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
    let listener_name = if params.shared_listener {
        crate::openapi::defaults::DEFAULT_GATEWAY_LISTENER.to_string()
    } else {
        format!("{}-listener", gateway_name)
    };

    let gateway_options = GatewayOptions {
        name: gateway_name.clone(),
        bind_address: "0.0.0.0".to_string(),
        port: params.port.unwrap_or(10000),
        protocol: "HTTP".to_string(),
        shared_listener: params.shared_listener,
        listener_name: listener_name.clone(),
    };

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
        materialize_routes_from_virtual_host(
            &state.xds_state,
            &db_pool,
            &import_id,
            &params.team,
            virtual_host,
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
    tag = "openapi-import"
)]
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
    tag = "openapi-import"
)]
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
    let route_repo = state
        .xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Route repository not configured".to_string()))?;
    let listener_repo = state
        .xds_state
        .listener_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Listener repository not configured".to_string()))?;
    let cluster_ref_repo = ClusterReferencesRepository::new(db_pool);

    let routes = route_repo.list_by_import(&id).await.map_err(ApiError::from)?;
    let listener_count = listener_repo.count_by_import(&id).await.map_err(ApiError::from)?;
    let cluster_refs = cluster_ref_repo.get_by_import(&id).await.map_err(ApiError::from)?;

    Ok(Json(ImportDetailsResponse {
        id: import_data.id,
        spec_name: import_data.spec_name,
        spec_version: import_data.spec_version,
        spec_checksum: import_data.spec_checksum,
        team: import_data.team,
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
    tag = "openapi-import"
)]
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
    mut route_request: CreateRouteRepositoryRequest,
) -> std::result::Result<(), ApiError> {
    let route_repo = xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Route repository not configured".to_string()))?;

    // Add import metadata
    route_request.import_id = Some(import_id.to_string());
    route_request.team = Some(team.to_string());
    route_request.route_order = Some(0);

    route_repo.create(route_request).await.map_err(ApiError::from)?;

    Ok(())
}

async fn materialize_routes_from_virtual_host(
    xds_state: &XdsState,
    _db_pool: &crate::storage::DbPool,
    import_id: &str,
    team: &str,
    virtual_host: crate::xds::route::VirtualHostConfig,
) -> std::result::Result<usize, ApiError> {
    let route_repo = xds_state
        .route_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Route repository not configured".to_string()))?;
    let mut count = 0;

    for (idx, route_rule) in virtual_host.routes.iter().enumerate() {
        let route_name = route_rule
            .name
            .clone()
            .unwrap_or_else(|| format!("openapi-{}-route-{}", import_id, idx));
        let cluster_name = match &route_rule.action {
            crate::xds::route::RouteActionConfig::Cluster { name, .. } => name.clone(),
            _ => continue,
        };

        let route_config = crate::xds::route::RouteConfig {
            name: route_name.clone(),
            virtual_hosts: vec![virtual_host.clone()],
        };

        let configuration = serde_json::to_value(&route_config).map_err(|err| {
            ApiError::Internal(format!("Failed to serialize route config: {}", err))
        })?;

        let route_request = CreateRouteRepositoryRequest {
            name: route_name,
            path_prefix: "/".to_string(),
            cluster_name,
            configuration,
            team: Some(team.to_string()),
            import_id: Some(import_id.to_string()),
            route_order: Some(idx as i64),
            headers: None,
        };

        route_repo.create(route_request).await.map_err(ApiError::from)?;
        count += 1;
    }

    Ok(count)
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
