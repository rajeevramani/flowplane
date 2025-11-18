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

use crate::auth::authorization::{extract_team_scopes, require_resource_access};
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

// === Helper Functions ===

/// Verify that the user has access to the given API definition based on team scopes.
/// Returns the API definition if access is allowed, otherwise returns 404 to avoid leaking existence.
async fn verify_api_definition_access(
    definition: ApiDefinitionData,
    team_scopes: &[String],
) -> Result<ApiDefinitionData, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(definition);
    }

    // Check if definition belongs to one of user's teams
    if team_scopes.contains(&definition.team) {
        Ok(definition)
    } else {
        // Record cross-team access attempt for security monitoring
        if let Some(from_team) = team_scopes.first() {
            crate::observability::metrics::record_cross_team_access_attempt(
                from_team,
                &definition.team,
                "api-definitions",
            )
            .await;
        }

        // Return 404 to avoid leaking existence of other teams' resources
        Err(ApiError::NotFound(format!("API definition with ID '{}' not found", definition.id)))
    }
}

// === Response DTOs ===

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "id": "api-def-abc123",
    "bootstrapUri": "/api/v1/teams/payments/bootstrap",
    "routes": ["route-xyz789", "route-uvw456"]
}))]
pub struct CreateApiDefinitionResponse {
    #[schema(example = "api-def-abc123")]
    id: String,
    #[schema(example = "/api/v1/teams/payments/bootstrap")]
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
    "bootstrapUri": "/api/v1/teams/payments/bootstrap"
}))]
pub struct AppendRouteResponse {
    #[schema(example = "api-def-abc123")]
    api_id: String,
    #[schema(example = "route-new999")]
    route_id: String,
    #[schema(example = 2)]
    revision: i64,
    #[schema(example = "/api/v1/teams/payments/bootstrap")]
    bootstrap_uri: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "id": "api-def-abc123",
    "team": "payments",
    "domain": "payments.example.com",
    "listenerIsolation": false,
    "bootstrapUri": "/api/v1/teams/payments/bootstrap",
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
            id: row.id.to_string(),
            team: row.team,
            domain: row.domain,
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

    // Extract team scopes from auth context
    let team_scopes = extract_team_scopes(&context);

    // Determine which team to filter by:
    // - Admin/resource-level users: use query param (if provided) or list all
    // - Team-scoped users: only list their teams' definitions
    let filter_team = if team_scopes.is_empty() {
        // Admin or resource-level scope - honor query param
        q.team
    } else {
        // Team-scoped user - only show their teams
        // If query param provided, verify it's in their scopes
        if let Some(requested_team) = q.team {
            if !team_scopes.contains(&requested_team) {
                // User requested a team they don't have access to - return empty list
                return Ok(Json(vec![]));
            }
            Some(requested_team)
        } else {
            // No specific team requested - list all their teams
            // Note: For now we'll filter client-side. Ideally we'd have list_by_teams() in repo
            None
        }
    };

    let items =
        repo.list_definitions(filter_team, q.limit, q.offset).await.map_err(ApiError::from)?;

    // Apply client-side team filtering for team-scoped users
    let filtered_items = if team_scopes.is_empty() {
        // Admin/resource-level - show all results
        items
    } else {
        // Team-scoped - filter to only their teams
        items.into_iter().filter(|def| team_scopes.contains(&def.team)).collect()
    };

    Ok(Json(filtered_items.into_iter().map(ApiDefinitionSummary::from).collect()))
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
    require_resource_access(&context, "api-definitions", "read", None)?;

    let repo = state.xds_state.api_definition_repository.as_ref().cloned().ok_or_else(|| {
        ApiError::service_unavailable("API definition repository is not configured")
    })?;

    let definition = repo
        .get_definition(&crate::domain::ApiDefinitionId::from_str_unchecked(&id))
        .await
        .map_err(ApiError::from)?;

    // Verify team access
    let team_scopes = extract_team_scopes(&context);
    let verified_definition = verify_api_definition_access(definition, &team_scopes).await?;

    Ok(Json(ApiDefinitionSummary::from(verified_definition)))
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

    // Extract team from auth context
    // - Team-scoped users: must create definitions for their team only
    // - Admin/resource-level users: can use any team (from payload)
    let team_scopes = extract_team_scopes(&context);
    let team = if team_scopes.is_empty() {
        // Admin or resource-level scope - use team from payload
        payload.team.clone()
    } else {
        // Team-scoped user - must use their team (only one team scope supported)
        let user_team = team_scopes.into_iter().next().ok_or_else(|| {
            ApiError::Forbidden("Team-scoped users must have exactly one team scope".to_string())
        })?;

        // Verify payload team matches user's team (if provided)
        if !payload.team.is_empty() && payload.team != user_team {
            return Err(ApiError::Forbidden(format!(
                "Team-scoped users can only create definitions for their own team '{}', not '{}'",
                user_team, payload.team
            )));
        }

        user_team
    };

    let mut spec = payload.into_spec().map_err(ApiError::from)?;
    // Override team with the one extracted from auth context
    spec.team = team;

    let materializer =
        PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

    let outcome: CreateDefinitionOutcome =
        materializer.create_definition(spec).await.map_err(ApiError::from)?;

    let created_route_ids = outcome.routes.iter().map(|route| route.id.to_string()).collect();

    Ok((
        StatusCode::CREATED,
        Json(CreateApiDefinitionResponse {
            id: outcome.definition.id.to_string(),
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

    // Get existing definition and verify team access
    let existing_definition = repo
        .get_definition(&crate::domain::ApiDefinitionId::from_str_unchecked(&api_definition_id))
        .await
        .map_err(ApiError::from)?;

    let team_scopes = extract_team_scopes(&context);
    verify_api_definition_access(existing_definition, &team_scopes).await?;

    // Convert to repository request
    let update_request =
        UpdateApiDefinitionRequest { domain: payload.domain, tls_config: payload.tls };

    // Update the definition
    let updated = repo
        .update_definition(
            &crate::domain::ApiDefinitionId::from_str_unchecked(&api_definition_id),
            update_request,
        )
        .await
        .map_err(ApiError::from)?;

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

    // Refresh listeners
    state.xds_state.refresh_listeners_from_repository().await.map_err(|err| {
        tracing::error!(error = %err, "Failed to refresh xDS caches after API definition update (listeners)");
        ApiError::from(err)
    })?;

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

    // Verify team access to the API definition before appending route
    let repo = state
        .xds_state
        .api_definition_repository
        .as_ref()
        .ok_or_else(|| {
            ApiError::service_unavailable("API definition repository is not configured")
        })?
        .clone();

    let existing_definition = repo
        .get_definition(&crate::domain::ApiDefinitionId::from_str_unchecked(&api_definition_id))
        .await
        .map_err(ApiError::from)?;

    let team_scopes = extract_team_scopes(&context);
    verify_api_definition_access(existing_definition, &team_scopes).await?;

    let materializer =
        PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

    let route_spec = payload.into_route_spec(None).map_err(ApiError::from)?;

    let AppendRouteOutcome { definition, route, bootstrap_uri } =
        materializer.append_route(&api_definition_id, route_spec).await.map_err(ApiError::from)?;

    Ok((
        StatusCode::ACCEPTED,
        Json(AppendRouteResponse {
            api_id: definition.id.to_string(),
            route_id: route.id.to_string(),
            revision: definition.version,
            bootstrap_uri,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/api-definitions/{id}",
    params(
        ("id" = String, Path, description = "API definition ID")
    ),
    responses(
        (status = 204, description = "API definition successfully deleted, including all routes, generated listeners, routes, and clusters"),
        (status = 403, description = "Forbidden: user does not have access to this team's API definition"),
        (status = 404, description = "API definition not found or user does not have access"),
        (status = 500, description = "Internal server error during deletion"),
        (status = 503, description = "API definition repository not configured")
    ),
    tag = "platform-api"
)]
pub async fn delete_api_definition_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(api_definition_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Authorization: require api-definitions:write scope (delete is a write operation)
    require_resource_access(&context, "api-definitions", "write", None)?;

    // Verify team access to the API definition before deleting
    let repo = state
        .xds_state
        .api_definition_repository
        .as_ref()
        .ok_or_else(|| {
            ApiError::service_unavailable("API definition repository is not configured")
        })?
        .clone();

    let existing_definition = repo
        .get_definition(&crate::domain::ApiDefinitionId::from_str_unchecked(&api_definition_id))
        .await
        .map_err(ApiError::from)?;

    let team_scopes = extract_team_scopes(&context);
    verify_api_definition_access(existing_definition, &team_scopes).await?;

    // Use materializer to properly clean up all associated resources
    let materializer =
        PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

    materializer.delete_definition(&api_definition_id).await.map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, serde::Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ImportOpenApiQuery {
    /// Team name for the API definition (only used for admin/resource-level users, ignored for team-scoped users)
    #[serde(default)]
    pub team: Option<String>,
    /// Port for the API listener
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

    // Extract team from auth context
    // - Team-scoped users: must create definitions for their team only
    // - Admin/resource-level users: can provide team in query param, or use domain as fallback
    let team_scopes = extract_team_scopes(&context);
    let extracted_team = if !team_scopes.is_empty() {
        // Team-scoped user - use their team (ignore query param)
        Some(team_scopes.into_iter().next().ok_or_else(|| {
            ApiError::Forbidden("Team-scoped users must have exactly one team scope".to_string())
        })?)
    } else {
        // Admin or resource-level scope - use team from query param (if provided)
        params.team.clone()
    };

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

    // Team is REQUIRED for proper multi-tenancy and xDS filtering
    // Team-scoped users: automatically use their team
    // Admin users: must explicitly specify team in query param
    let team_param = extracted_team.ok_or_else(|| {
        ApiError::BadRequest(
            "team parameter is required. Specify ?team=<team-name> in the request URL".to_string(),
        )
    })?;

    // Convert OpenAPI to Platform API definition spec
    let spec = openapi_adapter::openapi_to_api_definition_spec(document, team_param, params.port)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;

    // Use Platform API materializer (benefits: FK tracking, source tagging, bootstrap gen)
    let materializer =
        PlatformApiMaterializer::new(state.xds_state.clone()).map_err(ApiError::from)?;

    let outcome: CreateDefinitionOutcome =
        materializer.create_definition(spec).await.map_err(ApiError::from)?;

    let created_route_ids = outcome.routes.iter().map(|route| route.id.to_string()).collect();

    Ok((
        StatusCode::CREATED,
        Json(CreateApiDefinitionResponse {
            id: outcome.definition.id.to_string(),
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
