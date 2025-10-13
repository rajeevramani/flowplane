use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, Request, StatusCode},
    Extension, Json,
};
use bytes::Bytes;
use http_body_util::BodyExt;
use serde::Serialize;
#[allow(unused_imports)]
use serde_json::json;
use utoipa::{IntoParams, ToSchema};

use crate::auth::authorization::require_resource_access;
use crate::auth::models::AuthContext;
use crate::storage::repositories::api_definition::UpdateApiDefinitionRequest;
use crate::storage::repository::ApiDefinitionData;
use crate::{
    api::{error::ApiError, routes::ApiState},
    platform_api::{
        materializer::{AppendRouteOutcome, CreateDefinitionOutcome, PlatformApiMaterializer},
        openapi_adapter,
    },
    validation::requests::api_definition::{
        AppendRouteBody, CreateApiDefinitionBody, UpdateApiDefinitionBody,
    },
};
use axum::response::Response;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "id": "api-def-abc123",
    "bootstrapUri": "/api/v1/api-definitions/api-def-abc123/bootstrap",
    "routes": ["route-xyz789", "route-uvw456"]
}))]
pub struct CreateApiDefinitionResponse {
    #[schema(example = "api-def-abc123")]
    id: String,
    #[schema(example = "/api/v1/api-definitions/api-def-abc123/bootstrap")]
    bootstrap_uri: String,
    #[schema(example = json!(["route-xyz789", "route-uvw456"]))]
    routes: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "apiId": "api-def-abc123",
    "routeId": "route-new999",
    "revision": 2,
    "bootstrapUri": "/api/v1/api-definitions/api-def-abc123/bootstrap"
}))]
pub struct AppendRouteResponse {
    #[schema(example = "api-def-abc123")]
    api_id: String,
    #[schema(example = "route-new999")]
    route_id: String,
    #[schema(example = 2)]
    revision: i64,
    #[schema(example = "/api/v1/api-definitions/api-def-abc123/bootstrap")]
    bootstrap_uri: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "id": "api-def-abc123",
    "team": "payments",
    "domain": "payments.example.com",
    "listenerIsolation": false,
    "bootstrapUri": "/api/v1/api-definitions/api-def-abc123/bootstrap",
    "version": 1,
    "createdAt": "2025-10-06T09:00:00Z",
    "updatedAt": "2025-10-06T09:00:00Z"
}))]
pub struct ApiDefinitionSummary {
    #[schema(example = "api-def-abc123")]
    id: String,
    #[schema(example = "payments")]
    team: String,
    #[schema(example = "payments.example.com")]
    domain: String,
    #[schema(example = false)]
    listener_isolation: bool,
    #[schema(example = "/api/v1/api-definitions/api-def-abc123/bootstrap")]
    bootstrap_uri: Option<String>,
    #[schema(example = 1)]
    version: i64,
    #[schema(example = "2025-10-06T09:00:00Z")]
    created_at: String,
    #[schema(example = "2025-10-06T09:00:00Z")]
    updated_at: String,
}

impl From<ApiDefinitionData> for ApiDefinitionSummary {
    fn from(row: ApiDefinitionData) -> Self {
        Self {
            id: row.id,
            team: row.team,
            domain: row.domain,
            listener_isolation: row.listener_isolation,
            bootstrap_uri: row.bootstrap_uri,
            version: row.version,
            created_at: row.created_at.to_rfc3339(),
            updated_at: row.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, serde::Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ListDefinitionsQuery {
    #[serde(default)]
    #[param(required = false)]
    pub team: Option<String>,
    #[serde(default)]
    #[param(required = false)]
    pub domain: Option<String>,
    #[serde(default)]
    #[param(required = false)]
    pub limit: Option<i32>,
    #[serde(default)]
    #[param(required = false)]
    pub offset: Option<i32>,
}

#[utoipa::path(
    get,
    path = "/api/v1/api-definitions",
    params(ListDefinitionsQuery),
    responses(
        (status = 200, description = "Successfully retrieved list of API definitions", body = [ApiDefinitionSummary]),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "API definition repository not configured")
    ),
    tag = "platform-api"
)]
pub async fn list_api_definitions_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(q): Query<ListDefinitionsQuery>,
) -> Result<Json<Vec<ApiDefinitionSummary>>, ApiError> {
    // Authorization: require api-definitions:read scope
    require_resource_access(&context, "api-definitions", "read", None)?;

    let repo = state.xds_state.api_definition_repository.as_ref().cloned().ok_or_else(|| {
        ApiError::service_unavailable("API definition repository is not configured")
    })?;

    let items = repo.list_definitions(q.team, q.limit, q.offset).await.map_err(ApiError::from)?;
    Ok(Json(items.into_iter().map(ApiDefinitionSummary::from).collect()))
}

#[utoipa::path(
    get,
    path = "/api/v1/api-definitions/{id}",
    params(("id" = String, Path, description = "API definition ID", example = "api-def-abc123")),
    responses(
        (status = 200, description = "Successfully retrieved API definition", body = ApiDefinitionSummary),
        (status = 404, description = "API definition not found with the specified ID"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "API definition repository not configured")
    ),
    tag = "platform-api"
)]
pub async fn get_api_definition_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiDefinitionSummary>, ApiError> {
    // Authorization: require api-definitions:read scope
    // Team-scoped validation will be added in subtask 24.4 (database-level filtering)
    require_resource_access(&context, "api-definitions", "read", None)?;

    let repo = state.xds_state.api_definition_repository.as_ref().cloned().ok_or_else(|| {
        ApiError::service_unavailable("API definition repository is not configured")
    })?;
    let row = repo.get_definition(&id).await.map_err(ApiError::from)?;
    Ok(Json(ApiDefinitionSummary::from(row)))
}

#[derive(Debug, serde::Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapQuery {
    #[serde(default)]
    #[param(required = false)]
    pub format: Option<String>, // yaml|json (default yaml)
    #[serde(default)]
    #[param(required = false)]
    pub scope: Option<String>, // all|team|allowlist (default all)
    #[serde(default)]
    #[param(required = false)]
    pub allowlist: Option<Vec<String>>, // names when scope=allowlist
    #[serde(default)]
    #[param(required = false)]
    pub include_default: Option<bool>, // default false in team scope
}

#[utoipa::path(
    get,
    path = "/api/v1/api-definitions/{id}/bootstrap",
    params(
        ("id" = String, Path, description = "API definition ID", example = "api-def-abc123"),
        BootstrapQuery
    ),
    responses(
        (status = 200, description = "Envoy bootstrap configuration in YAML or JSON format. The configuration includes admin interface, node metadata, dynamic resource discovery (ADS) configuration, and xDS cluster definition. Listeners are loaded dynamically via LDS from the control plane.", content_type = "application/yaml"),
        (status = 404, description = "API definition not found with the specified ID"),
        (status = 500, description = "Internal server error during bootstrap generation"),
        (status = 503, description = "API definition repository or listener repository not configured")
    ),
    tag = "platform-api"
)]
pub async fn get_bootstrap_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(q): Query<BootstrapQuery>,
) -> Result<Response, ApiError> {
    // Authorization: require api-definitions:read scope for bootstrap access
    require_resource_access(&context, "api-definitions", "read", None)?;

    let repo = state.xds_state.api_definition_repository.as_ref().cloned().ok_or_else(|| {
        ApiError::service_unavailable("API definition repository is not configured")
    })?;

    let def = repo.get_definition(&id).await.map_err(ApiError::from)?;

    let format = q.format.as_deref().unwrap_or("yaml").to_lowercase();
    let scope = q.scope.as_deref().unwrap_or("all").to_lowercase();
    let include_default = q.include_default.unwrap_or(false);
    let allowlist = q.allowlist.unwrap_or_default();

    // Build ADS bootstrap with node metadata for scoping
    let xds_addr = state.xds_state.config.bind_address.clone();
    let xds_port = state.xds_state.config.port;
    let node_id = format!("team={}/dp-{}", def.team, uuid::Uuid::new_v4());
    let node_cluster = format!("{}-cluster", def.team);

    // Gather listener information
    let listener_repo =
        state.xds_state.listener_repository.as_ref().cloned().ok_or_else(|| {
            ApiError::service_unavailable("Listener repository is not configured")
        })?;

    let mut listeners_info = Vec::new();

    // If isolated listener mode, get the generated listener
    if def.listener_isolation {
        if let Some(listener_id) = &def.generated_listener_id {
            if let Ok(listener) = listener_repo.get_by_id(listener_id).await {
                listeners_info.push(serde_json::json!({
                    "name": listener.name,
                    "address": listener.address,
                    "port": listener.port,
                    "protocol": listener.protocol,
                }));
            }
        }
    } else {
        // If shared listener mode, get all target listeners (or default)
        let target_listeners = def
            .target_listeners
            .clone()
            .unwrap_or_else(|| vec!["default-gateway-listener".to_string()]);

        for listener_name in target_listeners {
            if let Ok(listener) = listener_repo.get_by_name(&listener_name).await {
                listeners_info.push(serde_json::json!({
                    "name": listener.name,
                    "address": listener.address,
                    "port": listener.port,
                    "protocol": listener.protocol,
                }));
            }
        }
    }

    let metadata = match scope.as_str() {
        "team" => serde_json::json!({
            "team": def.team,
            "include_default": include_default,
        }),
        "allowlist" => serde_json::json!({
            "team": def.team,
            "listener_allowlist": allowlist,
        }),
        _ => serde_json::json!({}),
    };

    let bootstrap = serde_json::json!({
        "admin": {
            "access_log_path": "/tmp/envoy_admin.log",
            "address": { "socket_address": { "address": "127.0.0.1", "port_value": 9901 } }
        },
        "node": { "id": node_id, "cluster": node_cluster, "metadata": metadata },
        "dynamic_resources": {
            "lds_config": { "ads": {} },
            "cds_config": { "ads": {} },
            "ads_config": {
                "api_type": "GRPC",
                "transport_api_version": "V3",
                "grpc_services": [ { "envoy_grpc": { "cluster_name": "xds_cluster" } } ]
            }
        },
        "static_resources": {
            "clusters": [
                {
                    "name": "xds_cluster",
                    "type": "LOGICAL_DNS",
                    "dns_lookup_family": "V4_ONLY",
                    "connect_timeout": "1s",
                    "http2_protocol_options": {},
                    "load_assignment": {
                        "cluster_name": "xds_cluster",
                        "endpoints": [ { "lb_endpoints": [ { "endpoint": { "address": { "socket_address": { "address": xds_addr, "port_value": xds_port } } } } ] } ]
                    }
                }
            ]
        }
    });

    let response = if format == "json" {
        let body = serde_json::to_vec(&bootstrap)
            .map_err(|e| ApiError::service_unavailable(e.to_string()))?;
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    } else {
        let yaml = serde_yaml::to_string(&bootstrap)
            .map_err(|e| ApiError::service_unavailable(e.to_string()))?;
        Response::builder()
            .header(header::CONTENT_TYPE, "application/yaml")
            .body(axum::body::Body::from(yaml))
            .unwrap()
    };

    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/api-definitions",
    request_body = CreateApiDefinitionBody,
    responses(
        (status = 201, description = "API definition successfully created with routes and clusters", body = CreateApiDefinitionResponse),
        (status = 400, description = "Invalid request: validation error in payload (e.g., empty team, invalid domain, missing routes, malformed endpoint)"),
        (status = 409, description = "Conflict: domain already registered by another team or route collision detected"),
        (status = 403, description = "Forbidden: insufficient permissions"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "API definition repository not configured")
    ),
    tag = "platform-api"
)]
pub async fn create_api_definition_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<CreateApiDefinitionBody>,
) -> Result<(StatusCode, Json<CreateApiDefinitionResponse>), ApiError> {
    // Authorization: require api-definitions:write scope
    require_resource_access(&context, "api-definitions", "write", None)?;

    let spec = payload.into_spec().map_err(ApiError::from)?;

    let materializer =
        PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

    let outcome: CreateDefinitionOutcome =
        materializer.create_definition(spec).await.map_err(ApiError::from)?;

    let created_route_ids = outcome.routes.iter().map(|route| route.id.clone()).collect();

    Ok((
        StatusCode::CREATED,
        Json(CreateApiDefinitionResponse {
            id: outcome.definition.id,
            bootstrap_uri: outcome.bootstrap_uri,
            routes: created_route_ids,
        }),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/v1/api-definitions/{id}",
    params(("id" = String, Path, description = "API definition ID to update", example = "api-def-abc123")),
    request_body = UpdateApiDefinitionBody,
    responses(
        (status = 200, description = "API definition successfully updated. The version number is incremented and xDS cache is refreshed. When routes are provided, existing routes are deleted and replaced atomically (cascade update), triggering cleanup of orphaned native resources and xDS refresh.", body = ApiDefinitionSummary),
        (status = 400, description = "Invalid request: validation error (e.g., invalid domain format, empty routes array, invalid listener names)"),
        (status = 404, description = "API definition not found with the specified ID"),
        (status = 409, description = "Conflict: updated domain already registered for another API definition"),
        (status = 500, description = "Internal server error during update or xDS cache refresh"),
        (status = 503, description = "API definition repository not configured")
    ),
    tag = "platform-api"
)]
pub async fn update_api_definition_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(api_definition_id): Path<String>,
    Json(payload): Json<UpdateApiDefinitionBody>,
) -> Result<(StatusCode, Json<ApiDefinitionSummary>), ApiError> {
    // Authorization: require api-definitions:write scope
    require_resource_access(&context, "api-definitions", "write", None)?;

    // Validate request payload
    payload.validate_payload().map_err(ApiError::from)?;

    // Get repository
    let repo = state
        .xds_state
        .api_definition_repository
        .as_ref()
        .ok_or_else(|| {
            ApiError::service_unavailable("API definition repository is not configured")
        })?
        .clone();

    // Convert to repository request
    let update_request = UpdateApiDefinitionRequest {
        domain: payload.domain,
        tls_config: payload.tls,
        target_listeners: payload.target_listeners,
    };

    // Update the definition
    let updated =
        repo.update_definition(&api_definition_id, update_request).await.map_err(ApiError::from)?;

    // Handle route cascade updates if routes are provided
    if let Some(routes_payload) = payload.routes {
        tracing::info!(
            api_definition_id = %api_definition_id,
            route_count = routes_payload.len(),
            "Processing route cascade updates in PATCH endpoint"
        );

        // Convert RouteBody payloads to RouteSpec format
        let mut route_specs = Vec::with_capacity(routes_payload.len());
        for (idx, route_body) in routes_payload.into_iter().enumerate() {
            let route_spec =
                route_body.into_route_spec(Some(idx as i64), None).map_err(ApiError::from)?;
            route_specs.push(route_spec);
        }

        // Use the materializer to handle route cascade updates
        // This will delete existing routes, create new ones, and handle native resource cleanup
        let materializer =
            PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

        let _outcome = materializer
            .update_definition(&api_definition_id, route_specs)
            .await
            .map_err(ApiError::from)?;

        // Return updated definition from the outcome (includes incremented version)
        return Ok((StatusCode::OK, Json(ApiDefinitionSummary::from(_outcome.definition))));
    }

    // Trigger xDS snapshot updates to propagate changes to Envoy
    // Order matters: clusters -> routes -> platform API -> listeners
    tracing::info!(
        api_definition_id = %api_definition_id,
        "Triggering xDS updates after API definition update"
    );

    state.xds_state.refresh_clusters_from_repository().await.map_err(|err| {
        tracing::error!(error = %err, "Failed to refresh xDS caches after API definition update (clusters)");
        ApiError::from(err)
    })?;

    state.xds_state.refresh_routes_from_repository().await.map_err(|err| {
        tracing::error!(error = %err, "Failed to refresh xDS caches after API definition update (routes)");
        ApiError::from(err)
    })?;

    state.xds_state.refresh_platform_api_resources().await.map_err(|err| {
        tracing::error!(error = %err, "Failed to refresh xDS caches after API definition update (platform API)");
        ApiError::from(err)
    })?;

    // Refresh listeners if using listener isolation mode
    if updated.listener_isolation {
        state.xds_state.refresh_listeners_from_repository().await.map_err(|err| {
            tracing::error!(error = %err, "Failed to refresh xDS caches after API definition update (listeners)");
            ApiError::from(err)
        })?;
    }

    tracing::info!(
        api_definition_id = %api_definition_id,
        "xDS updates completed successfully"
    );

    // Return updated definition summary
    Ok((StatusCode::OK, Json(ApiDefinitionSummary::from(updated))))
}

#[utoipa::path(
    post,
    path = "/api/v1/api-definitions/{id}/routes",
    params(("id" = String, Path, description = "API definition ID", example = "api-def-abc123")),
    request_body = AppendRouteBody,
    responses(
        (status = 202, description = "Route successfully appended to API definition", body = AppendRouteResponse),
        (status = 400, description = "Invalid request: validation error in route payload (e.g., invalid path, missing cluster, timeout out of range)"),
        (status = 404, description = "API definition not found with the specified ID"),
        (status = 409, description = "Conflict: route with same match pattern already exists for this API definition"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "API definition repository not configured")
    ),
    tag = "platform-api"
)]
pub async fn append_route_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(api_definition_id): Path<String>,
    Json(payload): Json<AppendRouteBody>,
) -> Result<(StatusCode, Json<AppendRouteResponse>), ApiError> {
    // Authorization: require api-definitions:write scope
    require_resource_access(&context, "api-definitions", "write", None)?;

    let materializer =
        PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

    let route_spec = payload.into_route_spec(None).map_err(ApiError::from)?;

    let AppendRouteOutcome { definition, route, bootstrap_uri } =
        materializer.append_route(&api_definition_id, route_spec).await.map_err(ApiError::from)?;

    Ok((
        StatusCode::ACCEPTED,
        Json(AppendRouteResponse {
            api_id: definition.id,
            route_id: route.id,
            revision: definition.version,
            bootstrap_uri,
        }),
    ))
}

#[derive(Debug, serde::Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ImportOpenApiQuery {
    /// Team name for the API definition
    pub team: String,
    /// Enable dedicated listener for this API (default: false, uses shared listener)
    #[serde(default)]
    pub listener_isolation: Option<bool>,
    /// Port for the isolated listener (only used when listenerIsolation=true)
    #[serde(default)]
    pub port: Option<u32>,
}

/// Binary OpenAPI payload accepted by the import endpoint.
#[derive(Debug, ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct OpenApiSpecBody(pub Vec<u8>);

#[utoipa::path(
    post,
    path = "/api/v1/api-definitions/from-openapi",
    params(ImportOpenApiQuery),
    request_body(
        description = "OpenAPI 3.0 document in JSON or YAML format with optional x-flowplane-* extensions for filter configuration",
        content(
            (OpenApiSpecBody = "application/yaml"),
            (OpenApiSpecBody = "application/x-yaml"),
            (OpenApiSpecBody = "application/json")
        )
    ),
    responses(
        (status = 201, description = "API definition successfully created from OpenAPI document with routes and filters", body = CreateApiDefinitionResponse),
        (status = 400, description = "Invalid request: malformed OpenAPI spec, unsupported version, invalid x-flowplane extensions, or missing required fields"),
        (status = 409, description = "Conflict: domain from OpenAPI info already registered or route paths conflict with existing routes"),
        (status = 500, description = "Internal server error during OpenAPI parsing or materialization"),
        (status = 503, description = "API definition repository not configured")
    ),
    tag = "platform-api"
)]
pub async fn import_openapi_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(params): Query<ImportOpenApiQuery>,
    request: Request<Body>,
) -> Result<(StatusCode, Json<CreateApiDefinitionResponse>), ApiError> {
    // Authorization: require api-definitions:write scope for OpenAPI import
    require_resource_access(&context, "api-definitions", "write", None)?;

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

    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<mime::Mime>().ok());

    let document = parse_openapi_document(&bytes, content_type.as_ref())?;

    let listener_isolation = params.listener_isolation.unwrap_or(false);

    // Convert OpenAPI to Platform API definition spec
    let spec = openapi_adapter::openapi_to_api_definition_spec(
        document,
        params.team.clone(),
        listener_isolation,
        params.port,
    )
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;

    // Use Platform API materializer (benefits: FK tracking, source tagging, bootstrap gen)
    let materializer =
        PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

    let outcome: CreateDefinitionOutcome =
        materializer.create_definition(spec).await.map_err(ApiError::from)?;

    let created_route_ids = outcome.routes.iter().map(|route| route.id.clone()).collect();

    Ok((
        StatusCode::CREATED,
        Json(CreateApiDefinitionResponse {
            id: outcome.definition.id,
            bootstrap_uri: outcome.bootstrap_uri,
            routes: created_route_ids,
        }),
    ))
}

fn parse_openapi_document(
    bytes: &Bytes,
    mime: Option<&mime::Mime>,
) -> Result<openapiv3::OpenAPI, ApiError> {
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

    match serde_json::from_slice(bytes) {
        Ok(doc) => Ok(doc),
        Err(json_err) => match parse_yaml(bytes) {
            Ok(doc) => Ok(doc),
            Err(yaml_err) => Err(ApiError::BadRequest(format!(
                "Failed to parse OpenAPI document. JSON error: {}; YAML error: {:?}",
                json_err, yaml_err
            ))),
        },
    }
}

fn parse_yaml(bytes: &Bytes) -> Result<openapiv3::OpenAPI, ApiError> {
    let value: serde_json::Value = serde_yaml::from_slice(bytes)
        .map_err(|err| ApiError::BadRequest(format!("Invalid OpenAPI YAML document: {}", err)))?;
    serde_json::from_value(value)
        .map_err(|err| ApiError::BadRequest(format!("Invalid OpenAPI YAML structure: {}", err)))
}
