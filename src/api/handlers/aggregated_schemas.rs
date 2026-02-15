//! Aggregated Schema HTTP handlers
//!
//! This module provides REST API endpoints for retrieving, comparing, and exporting
//! aggregated API schemas learned from traffic observations.

use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::{IntoParams, ToSchema};

use crate::{
    api::{
        error::ApiError,
        handlers::team_access::{
            get_effective_team_ids, require_resource_access_resolved, team_repo_from_state,
            verify_team_access,
        },
        routes::ApiState,
    },
    auth::authorization::{extract_team_scopes, require_resource_access},
    auth::models::AuthContext,
};

// === DTOs ===

/// Query parameters for listing aggregated schemas
#[derive(Debug, Deserialize, IntoParams, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ListAggregatedSchemasQuery {
    /// Team to filter schemas by (required for team-scoped users, optional for admins)
    #[serde(default)]
    #[schema(example = "engineering")]
    pub team: Option<String>,

    /// Text search in API path (substring match)
    #[serde(default)]
    #[schema(example = "users")]
    pub path: Option<String>,

    /// Filter by HTTP method (exact match)
    #[serde(default)]
    #[schema(example = "GET")]
    pub http_method: Option<String>,

    /// Filter by minimum confidence score (0.0 to 1.0)
    #[serde(default)]
    #[schema(example = 0.8, minimum = 0.0, maximum = 1.0)]
    pub min_confidence: Option<f64>,
}

/// Query parameters for schema comparison
#[derive(Debug, Deserialize, IntoParams, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct CompareSchemaQuery {
    /// Version number to compare with current schema
    #[schema(example = 1)]
    pub with_version: i64,
}

/// Query parameters for OpenAPI export
#[derive(Debug, Deserialize, IntoParams, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ExportSchemaQuery {
    /// Include Flowplane-specific metadata extensions (x-flowplane-*)
    #[serde(default = "default_true")]
    #[schema(example = true)]
    pub include_metadata: bool,
}

fn default_true() -> bool {
    true
}

/// Request body for exporting multiple schemas as a unified OpenAPI document
#[derive(Debug, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportMultipleSchemasRequest {
    /// List of schema IDs to include in the unified export
    #[schema(example = json!([1, 2, 3]))]
    pub schema_ids: Vec<i64>,

    /// Title for the exported OpenAPI specification
    #[schema(example = "My API")]
    pub title: String,

    /// Version for the exported OpenAPI specification
    #[schema(example = "1.0.0")]
    pub version: String,

    /// Optional description for the API
    #[serde(default)]
    #[schema(example = "API learned from traffic analysis")]
    pub description: Option<String>,

    /// Include Flowplane-specific metadata extensions (x-flowplane-*)
    #[serde(default = "default_true")]
    #[schema(example = true)]
    pub include_metadata: bool,
}

/// Aggregated schema response
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedSchemaResponse {
    pub id: i64,
    pub team: String,
    pub path: String,
    pub http_method: String,
    pub version: i64,
    pub previous_version_id: Option<i64>,
    pub request_schema: Option<serde_json::Value>,
    pub response_schemas: Option<serde_json::Value>,
    pub sample_count: i64,
    pub confidence_score: f64,
    pub breaking_changes: Option<Vec<serde_json::Value>>,
    pub first_observed: String,
    pub last_observed: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Schema comparison response
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SchemaComparisonResponse {
    pub current_schema: AggregatedSchemaResponse,
    pub compared_schema: AggregatedSchemaResponse,
    pub differences: SchemaDifferences,
}

/// Differences between two schemas
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDifferences {
    pub version_change: i64,
    pub sample_count_change: i64,
    pub confidence_change: f64,
    pub has_breaking_changes: bool,
    pub breaking_changes: Option<Vec<serde_json::Value>>,
}

/// OpenAPI 3.1 export response
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiExportResponse {
    pub openapi: String,
    pub info: OpenApiInfo,
    pub paths: serde_json::Value,
    pub components: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct OpenApiInfo {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
}

// === Helper Functions ===

/// Strip internal attributes from schema JSON
///
/// Removes internal metadata fields (confidence, presence_count, sample_count)
/// from inferred schemas to avoid exposing implementation details in API responses.
///
/// This function recursively processes schema objects and arrays to remove:
/// - `confidence`: Field presence confidence score (0.0-1.0)
/// - `presence_count`: Number of times field was observed
/// - `sample_count`: Total number of samples processed
fn strip_internal_attributes(schema: &mut serde_json::Value) {
    match schema {
        serde_json::Value::Object(map) => {
            // Remove internal attributes at this level
            map.remove("confidence");
            map.remove("presence_count");
            map.remove("sample_count");

            // Recursively process ALL nested values in the object
            // This handles properties, items, status codes (200, 201), and any other nested objects
            for (_, value) in map.iter_mut() {
                strip_internal_attributes(value);
            }
        }
        serde_json::Value::Array(arr) => {
            // Process each element in the array
            for item in arr.iter_mut() {
                strip_internal_attributes(item);
            }
        }
        _ => {
            // Primitive values don't need processing
        }
    }
}

fn schema_response_from_data(
    data: crate::storage::repositories::AggregatedSchemaData,
) -> AggregatedSchemaResponse {
    // Strip internal attributes from schemas before returning
    let request_schema = data.request_schema.map(|mut schema| {
        strip_internal_attributes(&mut schema);
        schema
    });

    let response_schemas = data.response_schemas.map(|mut schemas| {
        strip_internal_attributes(&mut schemas);
        schemas
    });

    AggregatedSchemaResponse {
        id: data.id,
        team: data.team,
        path: data.path,
        http_method: data.http_method,
        version: data.version,
        previous_version_id: data.previous_version_id,
        request_schema,
        response_schemas,
        sample_count: data.sample_count,
        confidence_score: data.confidence_score,
        breaking_changes: data.breaking_changes,
        first_observed: data.first_observed.to_rfc3339(),
        last_observed: data.last_observed.to_rfc3339(),
        created_at: data.created_at.to_rfc3339(),
        updated_at: data.updated_at.to_rfc3339(),
    }
}

// === Handlers ===

/// List aggregated schemas with optional filters
#[utoipa::path(
    get,
    path = "/api/v1/aggregated-schemas",
    params(ListAggregatedSchemasQuery),
    responses(
        (status = 200, description = "List of aggregated schemas", body = Vec<AggregatedSchemaResponse>),
        (status = 400, description = "Bad request - invalid parameters"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 503, description = "Repository unavailable")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, path = ?query.path, http_method = ?query.http_method))]
pub async fn list_aggregated_schemas_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<ListAggregatedSchemasQuery>,
) -> Result<Json<Vec<AggregatedSchemaResponse>>, ApiError> {
    // Determine team: use query parameter if provided, otherwise fall back to first team scope
    let team = if let Some(ref requested_team) = query.team {
        // Verify user has access to the requested team
        require_resource_access_resolved(
            &state,
            &context,
            "aggregated-schemas",
            "read",
            Some(requested_team),
            context.org_id.as_ref(),
        )
        .await?;
        requested_team.clone()
    } else {
        // Fall back to first team scope from auth context
        require_resource_access(&context, "aggregated-schemas", "read", None)?;
        let team_scopes = extract_team_scopes(&context);
        team_scopes
            .first()
            .ok_or_else(|| {
                ApiError::BadRequest(
                    "Team scope required for aggregated schemas. Provide 'team' query parameter."
                        .to_string(),
                )
            })?
            .clone()
    };

    // Resolve team name to UUID
    use crate::storage::repositories::TeamRepository as _;
    let team_repo = crate::api::handlers::team_access::team_repo_from_state(&state)?;
    let team_ids = team_repo
        .resolve_team_ids(context.org_id.as_ref(), &[team])
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to resolve team ID: {}", e)))?;
    let team = team_ids
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::NotFound("Team not found".to_string()))?;

    // Validate min_confidence bounds
    if let Some(min_conf) = query.min_confidence {
        if !(0.0..=1.0).contains(&min_conf) {
            return Err(ApiError::BadRequest(
                "min_confidence must be between 0.0 and 1.0".to_string(),
            ));
        }
    }

    // Get repository
    let repo = state
        .xds_state
        .aggregated_schema_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    // List schemas with filters
    let schemas = repo
        .list_filtered(
            &team,
            query.path.as_deref(),
            query.http_method.as_deref(),
            query.min_confidence,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list aggregated schemas");
            ApiError::Internal(format!("Failed to list aggregated schemas: {}", e))
        })?;

    tracing::info!(
        count = schemas.len(),
        team = %team,
        "Listed aggregated schemas"
    );

    let responses: Vec<AggregatedSchemaResponse> =
        schemas.into_iter().map(schema_response_from_data).collect();

    Ok(Json(responses))
}

/// Get aggregated schema by ID
#[utoipa::path(
    get,
    path = "/api/v1/aggregated-schemas/{id}",
    params(
        ("id" = i64, Path, description = "Aggregated schema ID")
    ),
    responses(
        (status = 200, description = "Aggregated schema details", body = AggregatedSchemaResponse),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Schema not found"),
        (status = 503, description = "Repository unavailable")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state), fields(schema_id = %id, user_id = ?context.user_id))]
pub async fn get_aggregated_schema_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<i64>,
) -> Result<Json<AggregatedSchemaResponse>, ApiError> {
    // Authorization
    require_resource_access(&context, "aggregated-schemas", "read", None)?;

    // Get repository
    let repo = state
        .xds_state
        .aggregated_schema_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    // Get schema by ID
    let schema = repo.get_by_id(id).await.map_err(|e| {
        tracing::error!(error = %e, id = %id, "Failed to get aggregated schema");
        ApiError::from(e)
    })?;

    // Verify team access based on user's scope type
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let is_admin = crate::auth::authorization::has_admin_bypass(&context);
    let has_global_resource_scope = context.has_scope("aggregated-schemas:read")
        || context.has_scope("aggregated-schemas:write");

    let authorized_schema = if is_admin || has_global_resource_scope {
        // Admin or global scope - allow access to all schemas
        schema
    } else {
        // Team-scoped user - verify access
        verify_team_access(schema, &team_scopes).await?
    };

    tracing::info!(
        id = id,
        team = %authorized_schema.team,
        "Retrieved aggregated schema"
    );

    let response = schema_response_from_data(authorized_schema);

    Ok(Json(response))
}

/// Compare schema versions
#[utoipa::path(
    get,
    path = "/api/v1/aggregated-schemas/{id}/compare",
    params(
        ("id" = i64, Path, description = "Current schema ID"),
        CompareSchemaQuery
    ),
    responses(
        (status = 200, description = "Schema comparison result", body = SchemaComparisonResponse),
        (status = 400, description = "Bad request - invalid version"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Schema or version not found"),
        (status = 503, description = "Repository unavailable")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state), fields(schema_id = %id, compare_version = %query.with_version, user_id = ?context.user_id))]
pub async fn compare_aggregated_schemas_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<i64>,
    Query(query): Query<CompareSchemaQuery>,
) -> Result<Json<SchemaComparisonResponse>, ApiError> {
    // Authorization
    require_resource_access(&context, "aggregated-schemas", "read", None)?;

    // Get repository
    let repo = state
        .xds_state
        .aggregated_schema_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    // Get current schema
    let current_schema = repo.get_by_id(id).await.map_err(|e| {
        tracing::error!(error = %e, id = %id, "Failed to get current schema");
        ApiError::from(e)
    })?;

    // Verify team access based on user's scope type
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let is_admin = crate::auth::authorization::has_admin_bypass(&context);
    let has_global_resource_scope = context.has_scope("aggregated-schemas:read")
        || context.has_scope("aggregated-schemas:write");

    let authorized_current = if is_admin || has_global_resource_scope {
        // Admin or global scope - allow access to all schemas
        current_schema.clone()
    } else {
        // Team-scoped user - verify access
        verify_team_access(current_schema.clone(), &team_scopes).await?
    };

    // Get comparison version (same path, method, different version)
    let compared_schema = repo
        .get_by_version(
            &authorized_current.team,
            &authorized_current.path,
            &authorized_current.http_method,
            query.with_version,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, version = query.with_version, "Failed to get comparison schema");
            ApiError::Internal(format!("Failed to get comparison schema: {}", e))
        })?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Version {} not found for endpoint {} {}",
                query.with_version, authorized_current.http_method, authorized_current.path
            ))
        })?;

    // Calculate differences
    let differences = SchemaDifferences {
        version_change: authorized_current.version - compared_schema.version,
        sample_count_change: authorized_current.sample_count - compared_schema.sample_count,
        confidence_change: authorized_current.confidence_score - compared_schema.confidence_score,
        has_breaking_changes: authorized_current.breaking_changes.is_some(),
        breaking_changes: authorized_current.breaking_changes.clone(),
    };

    tracing::info!(
        current_id = id,
        current_version = authorized_current.version,
        compared_version = compared_schema.version,
        team = %authorized_current.team,
        "Compared schema versions"
    );

    let response = SchemaComparisonResponse {
        current_schema: schema_response_from_data(authorized_current),
        compared_schema: schema_response_from_data(compared_schema),
        differences,
    };

    Ok(Json(response))
}

/// Export schema as OpenAPI 3.1 specification
#[utoipa::path(
    get,
    path = "/api/v1/aggregated-schemas/{id}/export",
    params(
        ("id" = i64, Path, description = "Schema ID to export"),
        ExportSchemaQuery
    ),
    responses(
        (status = 200, description = "OpenAPI 3.1 specification", body = OpenApiExportResponse),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Schema not found"),
        (status = 503, description = "Repository unavailable")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state), fields(schema_id = %id, include_metadata = %query.include_metadata, user_id = ?context.user_id))]
pub async fn export_aggregated_schema_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<i64>,
    Query(query): Query<ExportSchemaQuery>,
) -> Result<Json<OpenApiExportResponse>, ApiError> {
    // Authorization
    require_resource_access(&context, "aggregated-schemas", "read", None)?;

    // Get repository
    let repo = state
        .xds_state
        .aggregated_schema_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    // Get schema by ID
    let schema = repo.get_by_id(id).await.map_err(|e| {
        tracing::error!(error = %e, id = %id, "Failed to get schema for export");
        ApiError::from(e)
    })?;

    // Verify team access based on user's scope type
    // - Admin users (admin:all) can access all schemas
    // - Users with global aggregated-schemas:read can access all schemas
    // - Users with team:X:aggregated-schemas:read can only access their team's schemas
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let is_admin = crate::auth::authorization::has_admin_bypass(&context);
    let has_global_resource_scope = context.has_scope("aggregated-schemas:read")
        || context.has_scope("aggregated-schemas:write");

    let authorized_schema = if is_admin || has_global_resource_scope {
        // Admin or global scope - allow access to all schemas
        schema
    } else {
        // Team-scoped user - verify access
        verify_team_access(schema, &team_scopes).await?
    };

    // Build OpenAPI 3.1 specification
    let openapi = build_openapi_spec(&authorized_schema, query.include_metadata);

    tracing::info!(
        id = id,
        team = %authorized_schema.team,
        include_metadata = query.include_metadata,
        "Exported schema as OpenAPI"
    );

    Ok(Json(openapi))
}

/// Export multiple schemas as a unified OpenAPI 3.1 specification
#[utoipa::path(
    post,
    path = "/api/v1/aggregated-schemas/export",
    request_body = ExportMultipleSchemasRequest,
    responses(
        (status = 200, description = "Unified OpenAPI 3.1 specification", body = OpenApiExportResponse),
        (status = 400, description = "Bad request - invalid parameters or empty schema list"),
        (status = 403, description = "Forbidden - insufficient permissions or cross-team access"),
        (status = 404, description = "One or more schemas not found"),
        (status = 503, description = "Repository unavailable")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state, body), fields(schema_count = body.schema_ids.len(), user_id = ?context.user_id))]
pub async fn export_multiple_schemas_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(body): Json<ExportMultipleSchemasRequest>,
) -> Result<Json<OpenApiExportResponse>, ApiError> {
    // Validate non-empty
    if body.schema_ids.is_empty() {
        return Err(ApiError::BadRequest("At least one schema ID is required".to_string()));
    }

    // Authorization
    require_resource_access(&context, "aggregated-schemas", "read", None)?;

    // Get repository
    let repo = state
        .xds_state
        .aggregated_schema_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    // Fetch all schemas by IDs
    let schemas = repo.get_by_ids(&body.schema_ids).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to get schemas for export");
        ApiError::from(e)
    })?;

    // Verify we found all requested schemas
    if schemas.len() != body.schema_ids.len() {
        let found_ids: Vec<i64> = schemas.iter().map(|s| s.id).collect();
        let missing: Vec<i64> =
            body.schema_ids.iter().filter(|id| !found_ids.contains(id)).copied().collect();
        return Err(ApiError::NotFound(format!("Schemas not found: {:?}", missing)));
    }

    // Verify team access based on user's scope type
    // - Admin users (admin:all) can access all schemas
    // - Users with global aggregated-schemas:read can access all schemas
    // - Users with team:X:aggregated-schemas:read can only access their team's schemas
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;
    let is_admin = crate::auth::authorization::has_admin_bypass(&context);
    let has_global_resource_scope = context.has_scope("aggregated-schemas:read")
        || context.has_scope("aggregated-schemas:write");

    if !is_admin && !has_global_resource_scope {
        // Team-scoped user - verify access to each schema
        for schema in &schemas {
            verify_team_access(schema.clone(), &team_scopes).await?;
        }
    }
    // Admin or global scope - allow access to all schemas (no verification needed)

    // Build unified OpenAPI spec
    let openapi = build_unified_openapi_spec(&schemas, &body);

    tracing::info!(
        schema_count = schemas.len(),
        title = %body.title,
        version = %body.version,
        include_metadata = body.include_metadata,
        "Exported unified OpenAPI specification"
    );

    Ok(Json(openapi))
}

// === OpenAPI Export Logic ===

fn build_openapi_spec(
    schema: &crate::storage::repositories::AggregatedSchemaData,
    include_metadata: bool,
) -> OpenApiExportResponse {
    use super::openapi_utils::{
        build_path_parameters, convert_schema_to_openapi, extract_path_parameters,
        generate_operation_id, generate_semantic_summary, infer_param_type, parse_path_with_query,
    };

    let mut path_item = serde_json::json!({});

    // Parse path to extract base path and query parameters
    let parsed = parse_path_with_query(&schema.path);

    // Extract path template parameters (e.g., {customerId})
    let path_param_names = extract_path_parameters(&parsed.base_path);

    // Build operation object for this HTTP method
    let method_key = schema.http_method.to_lowercase();
    let operation_id = generate_operation_id(&schema.http_method, &schema.path);

    // BUG-007 FIX: Use semantic summary generation for better API documentation
    let summary = generate_semantic_summary(&schema.http_method, &schema.path);

    let mut operation = serde_json::json!({
        "summary": summary,
        "operationId": operation_id,
        "responses": {}
    });

    // Build parameters list: path params first, then query params
    let mut all_params: Vec<serde_json::Value> = Vec::new();

    // Add path parameters (required)
    if !path_param_names.is_empty() {
        all_params.extend(build_path_parameters(&path_param_names));
    }

    // Add query parameters
    for (name, value) in &parsed.query_params {
        let param_type = infer_param_type(value);
        let mut param = serde_json::json!({
            "name": name,
            "in": "query",
            "required": false,
            "schema": {
                "type": param_type
            }
        });

        // Add example with appropriate type
        if !value.is_empty() {
            let example = match param_type {
                "integer" => value
                    .parse::<i64>()
                    .map(serde_json::Value::from)
                    .unwrap_or_else(|_| serde_json::Value::String(value.clone())),
                "number" => value
                    .parse::<f64>()
                    .map(|n| serde_json::json!(n))
                    .unwrap_or_else(|_| serde_json::Value::String(value.clone())),
                "boolean" => serde_json::Value::Bool(value.eq_ignore_ascii_case("true")),
                _ => serde_json::Value::String(value.clone()),
            };
            param["example"] = example;
        }

        all_params.push(param);
    }

    // Add parameters to operation if any exist
    if !all_params.is_empty() {
        operation["parameters"] = serde_json::json!(all_params);
    }

    // Add request body if present
    if let Some(ref req_schema) = schema.request_schema {
        // Clone, strip internal attributes, and convert to OpenAPI format
        let mut cleaned_req_schema = req_schema.clone();
        strip_internal_attributes(&mut cleaned_req_schema);
        let converted_req_schema = convert_schema_to_openapi(&cleaned_req_schema);

        let mut request_body = serde_json::json!({
            "required": true,
            "content": {
                "application/json": {
                    "schema": converted_req_schema
                }
            }
        });

        if include_metadata {
            request_body["content"]["application/json"]["schema"]["x-flowplane-sample-count"] =
                serde_json::json!(schema.sample_count);
            request_body["content"]["application/json"]["schema"]["x-flowplane-confidence"] =
                serde_json::json!(schema.confidence_score);
        }

        operation["requestBody"] = request_body;
    }

    // Add responses
    if let Some(ref resp_schemas) = schema.response_schemas {
        if let Some(resp_map) = resp_schemas.as_object() {
            for (status_code, resp_schema) in resp_map {
                // Clone, strip internal attributes, and convert to OpenAPI format
                let mut cleaned_resp_schema = resp_schema.clone();
                strip_internal_attributes(&mut cleaned_resp_schema);
                let converted_resp_schema = convert_schema_to_openapi(&cleaned_resp_schema);

                let mut response_obj = serde_json::json!({
                    "description": format!("Response with status {}", status_code),
                    "content": {
                        "application/json": {
                            "schema": converted_resp_schema
                        }
                    }
                });

                if include_metadata {
                    response_obj["content"]["application/json"]["schema"]
                        ["x-flowplane-sample-count"] = serde_json::json!(schema.sample_count);
                    response_obj["content"]["application/json"]["schema"]
                        ["x-flowplane-confidence"] = serde_json::json!(schema.confidence_score);
                    response_obj["content"]["application/json"]["schema"]
                        ["x-flowplane-first-observed"] =
                        serde_json::json!(schema.first_observed.to_rfc3339());
                    response_obj["content"]["application/json"]["schema"]
                        ["x-flowplane-last-observed"] =
                        serde_json::json!(schema.last_observed.to_rfc3339());
                }

                operation["responses"][status_code] = response_obj;
            }
        }
    }

    // Ensure responses object is not empty (OpenAPI requires at least one response)
    if operation["responses"].as_object().is_some_and(|m| m.is_empty()) {
        operation["responses"]["default"] = serde_json::json!({
            "description": "Response not captured during learning"
        });
    }

    path_item[&method_key] = operation;

    let mut paths = serde_json::json!({});
    paths[&parsed.base_path] = path_item;

    OpenApiExportResponse {
        openapi: "3.1.0".to_string(),
        info: OpenApiInfo {
            title: format!("API Schema - {} {}", schema.http_method, parsed.base_path),
            version: schema.version.to_string(),
            description: Some(format!(
                "Learned from {} samples with {:.1}% confidence",
                schema.sample_count,
                schema.confidence_score * 100.0
            )),
        },
        paths,
        components: serde_json::json!({
            "schemas": {}
        }),
    }
}

/// Build a unified OpenAPI 3.1 specification from multiple schemas
pub fn build_unified_openapi_spec(
    schemas: &[crate::storage::repositories::AggregatedSchemaData],
    options: &ExportMultipleSchemasRequest,
) -> OpenApiExportResponse {
    use super::openapi_utils::{
        aggregate_query_params, build_path_parameters, build_query_parameters,
        convert_schema_to_openapi, extract_path_parameters, generate_operation_id,
        generate_semantic_summary, parse_path_with_query,
    };
    use std::collections::HashMap;

    let mut paths = serde_json::json!({});

    // Aggregate query parameters from all schemas for path/method deduplication
    let aggregated_params = aggregate_query_params(schemas);

    // STEP 1: Group all schemas by (base_path, method) to merge response_schemas
    // This fixes the bug where only the first status code was exported
    let mut grouped_schemas: HashMap<
        (String, String),
        Vec<&crate::storage::repositories::AggregatedSchemaData>,
    > = HashMap::new();

    for schema in schemas {
        let parsed = parse_path_with_query(&schema.path);
        let method_key = schema.http_method.to_lowercase();
        let key = (parsed.base_path.clone(), method_key);
        grouped_schemas.entry(key).or_default().push(schema);
    }

    // STEP 2: Process each unique (path, method) combination
    for ((base_path, method_key), endpoint_schemas) in &grouped_schemas {
        // Use first schema as representative for request schema, metadata
        let representative = endpoint_schemas[0];

        // Extract path template parameters (e.g., {customerId})
        let path_param_names = extract_path_parameters(base_path);

        let operation_id = generate_operation_id(&representative.http_method, &representative.path);

        // BUG-007 FIX: Use semantic summary generation for better API documentation
        let summary = generate_semantic_summary(&representative.http_method, &representative.path);

        // Build operation object
        let mut operation = serde_json::json!({
            "summary": summary,
            "operationId": operation_id,
            "responses": {}
        });

        // Build parameters list: path params first, then query params
        let mut all_params: Vec<serde_json::Value> = Vec::new();

        // Add path parameters (required)
        if !path_param_names.is_empty() {
            all_params.extend(build_path_parameters(&path_param_names));
        }

        // Add aggregated query parameters if available
        if let Some(method_params) = aggregated_params.get(base_path) {
            if let Some(path_info) = method_params.get(method_key) {
                if !path_info.query_params.is_empty() {
                    all_params.extend(build_query_parameters(&path_info.query_params));
                }
            }
        }

        // Add parameters to operation if any exist
        if !all_params.is_empty() {
            operation["parameters"] = serde_json::json!(all_params);
        }

        // Add request body from first schema that has one
        for schema in endpoint_schemas.iter() {
            if let Some(ref req_schema) = schema.request_schema {
                let mut cleaned_req = req_schema.clone();
                strip_internal_attributes(&mut cleaned_req);
                let converted_req = convert_schema_to_openapi(&cleaned_req);

                let mut request_body = serde_json::json!({
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": converted_req
                        }
                    }
                });

                if options.include_metadata {
                    request_body["content"]["application/json"]["schema"]
                        ["x-flowplane-sample-count"] = serde_json::json!(schema.sample_count);
                    request_body["content"]["application/json"]["schema"]
                        ["x-flowplane-confidence"] = serde_json::json!(schema.confidence_score);
                }

                operation["requestBody"] = request_body;
                break; // Use first available request schema
            }
        }

        // STEP 3: Merge response_schemas from ALL records for this endpoint
        // This is the key fix - collect all status codes across all records
        for schema in endpoint_schemas.iter() {
            if let Some(ref resp_schemas) = schema.response_schemas {
                if let Some(resp_map) = resp_schemas.as_object() {
                    for (status_code, resp_schema) in resp_map {
                        let mut cleaned_resp = resp_schema.clone();
                        strip_internal_attributes(&mut cleaned_resp);
                        let converted_resp = convert_schema_to_openapi(&cleaned_resp);

                        let mut response_obj = serde_json::json!({
                            "description": format!("Response with status {}", status_code),
                            "content": {
                                "application/json": {
                                    "schema": converted_resp
                                }
                            }
                        });

                        if options.include_metadata {
                            response_obj["content"]["application/json"]["schema"]
                                ["x-flowplane-sample-count"] =
                                serde_json::json!(schema.sample_count);
                            response_obj["content"]["application/json"]["schema"]
                                ["x-flowplane-confidence"] =
                                serde_json::json!(schema.confidence_score);
                            response_obj["content"]["application/json"]["schema"]
                                ["x-flowplane-first-observed"] =
                                serde_json::json!(schema.first_observed.to_rfc3339());
                            response_obj["content"]["application/json"]["schema"]
                                ["x-flowplane-last-observed"] =
                                serde_json::json!(schema.last_observed.to_rfc3339());
                        }

                        operation["responses"][status_code] = response_obj;
                    }
                }
            }
        }

        // Ensure responses object is not empty (OpenAPI requires at least one response)
        if operation["responses"].as_object().is_some_and(|m| m.is_empty()) {
            operation["responses"]["default"] = serde_json::json!({
                "description": "Response not captured during learning"
            });
        }

        // Merge into paths using base_path (handle same path with different methods)
        if !paths[base_path].is_object() {
            paths[base_path] = serde_json::json!({});
        }
        paths[base_path][method_key] = operation;
    }

    // Build description using base paths (not full paths with query strings)
    let description = if options.include_metadata {
        let summary = schemas
            .iter()
            .map(|s| {
                let parsed = parse_path_with_query(&s.path);
                format!(
                    "{} {} ({} samples, {:.0}% confidence)",
                    s.http_method,
                    parsed.base_path,
                    s.sample_count,
                    s.confidence_score * 100.0
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        Some(format!(
            "{}\n\nEndpoints: {}",
            options.description.as_deref().unwrap_or("API learned from traffic"),
            summary
        ))
    } else {
        options.description.clone()
    };

    OpenApiExportResponse {
        openapi: "3.1.0".to_string(),
        info: OpenApiInfo {
            title: options.title.clone(),
            version: options.version.clone(),
            description,
        },
        paths,
        components: serde_json::json!({
            "schemas": {}
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_internal_attributes_from_simple_object() {
        let mut schema = serde_json::json!({
            "type": "object",
            "confidence": 0.95,
            "presence_count": 100,
            "sample_count": 105,
            "properties": {
                "name": {
                    "type": "string",
                    "confidence": 1.0,
                    "presence_count": 105,
                    "sample_count": 105
                }
            }
        });

        strip_internal_attributes(&mut schema);

        // Root level internal attributes should be removed
        assert!(schema.get("confidence").is_none());
        assert!(schema.get("presence_count").is_none());
        assert!(schema.get("sample_count").is_none());

        // Type should remain
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));

        // Nested property internal attributes should also be removed
        let name_prop = schema.get("properties").and_then(|p| p.get("name"));
        assert!(name_prop.is_some());
        assert!(name_prop.unwrap().get("confidence").is_none());
        assert!(name_prop.unwrap().get("presence_count").is_none());
        assert!(name_prop.unwrap().get("sample_count").is_none());
        assert_eq!(name_prop.unwrap().get("type").and_then(|v| v.as_str()), Some("string"));
    }

    #[test]
    fn test_strip_internal_attributes_from_nested_objects() {
        let mut schema = serde_json::json!({
            "type": "object",
            "confidence": 0.90,
            "properties": {
                "user": {
                    "type": "object",
                    "confidence": 0.95,
                    "presence_count": 50,
                    "properties": {
                        "email": {
                            "type": "string",
                            "confidence": 1.0,
                            "sample_count": 50
                        }
                    }
                }
            }
        });

        strip_internal_attributes(&mut schema);

        // Check root
        assert!(schema.get("confidence").is_none());

        // Check first level nesting
        let user = schema.get("properties").and_then(|p| p.get("user"));
        assert!(user.is_some());
        assert!(user.unwrap().get("confidence").is_none());
        assert!(user.unwrap().get("presence_count").is_none());

        // Check second level nesting
        let email = user.unwrap().get("properties").and_then(|p| p.get("email"));
        assert!(email.is_some());
        assert!(email.unwrap().get("confidence").is_none());
        assert!(email.unwrap().get("sample_count").is_none());
        assert_eq!(email.unwrap().get("type").and_then(|v| v.as_str()), Some("string"));
    }

    #[test]
    fn test_strip_internal_attributes_from_array_items() {
        let mut schema = serde_json::json!({
            "type": "array",
            "confidence": 0.85,
            "sample_count": 20,
            "items": {
                "type": "object",
                "confidence": 0.90,
                "presence_count": 18,
                "properties": {
                    "id": {
                        "type": "integer",
                        "confidence": 1.0
                    }
                }
            }
        });

        strip_internal_attributes(&mut schema);

        // Array level
        assert!(schema.get("confidence").is_none());
        assert!(schema.get("sample_count").is_none());

        // Items level
        let items = schema.get("items");
        assert!(items.is_some());
        assert!(items.unwrap().get("confidence").is_none());
        assert!(items.unwrap().get("presence_count").is_none());

        // Nested property in items
        let id = items.unwrap().get("properties").and_then(|p| p.get("id"));
        assert!(id.is_some());
        assert!(id.unwrap().get("confidence").is_none());
        assert_eq!(id.unwrap().get("type").and_then(|v| v.as_str()), Some("integer"));
    }

    #[test]
    fn test_strip_internal_attributes_preserves_other_fields() {
        let mut schema = serde_json::json!({
            "type": "object",
            "confidence": 0.95,
            "required": ["id", "name"],
            "sample_count": 100,
            "properties": {
                "id": {
                    "type": "integer",
                    "format": "int64",
                    "confidence": 1.0
                },
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 255,
                    "presence_count": 95
                }
            }
        });

        strip_internal_attributes(&mut schema);

        // Internal attributes removed
        assert!(schema.get("confidence").is_none());
        assert!(schema.get("sample_count").is_none());

        // Other fields preserved
        assert!(schema.get("required").is_some());
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));

        // Property constraints preserved
        let id = schema.get("properties").and_then(|p| p.get("id"));
        assert_eq!(id.and_then(|v| v.get("format")).and_then(|v| v.as_str()), Some("int64"));

        let name = schema.get("properties").and_then(|p| p.get("name"));
        assert_eq!(name.and_then(|v| v.get("minLength")).and_then(|v| v.as_u64()), Some(1));
        assert_eq!(name.and_then(|v| v.get("maxLength")).and_then(|v| v.as_u64()), Some(255));

        // But internal attributes removed from properties
        assert!(id.unwrap().get("confidence").is_none());
        assert!(name.unwrap().get("presence_count").is_none());
    }

    // Helper to create test schema data
    fn create_test_schema_data(
        id: i64,
        path: &str,
        method: &str,
        sample_count: i64,
        confidence: f64,
    ) -> crate::storage::repositories::AggregatedSchemaData {
        crate::storage::repositories::AggregatedSchemaData {
            id,
            team: "test-team".to_string(),
            path: path.to_string(),
            http_method: method.to_string(),
            version: 1,
            previous_version_id: None,
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: Some(serde_json::json!({
                "200": {"type": "object", "properties": {"id": {"type": "integer"}}}
            })),
            sample_count,
            confidence_score: confidence,
            breaking_changes: None,
            first_observed: chrono::Utc::now(),
            last_observed: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_build_unified_openapi_spec_merges_paths() {
        let schemas = vec![
            create_test_schema_data(1, "/users", "GET", 10, 0.9),
            create_test_schema_data(2, "/users", "POST", 20, 0.85),
            create_test_schema_data(3, "/products/{id}", "GET", 15, 0.95),
        ];

        let options = ExportMultipleSchemasRequest {
            schema_ids: vec![1, 2, 3],
            title: "Test API".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test description".to_string()),
            include_metadata: false,
        };

        let result = build_unified_openapi_spec(&schemas, &options);

        assert_eq!(result.openapi, "3.1.0");
        assert_eq!(result.info.title, "Test API");
        assert_eq!(result.info.version, "1.0.0");

        // Verify paths are merged correctly
        let paths = result.paths.as_object().unwrap();
        assert!(paths.contains_key("/users"));
        assert!(paths.contains_key("/products/{id}"));

        // Verify /users has both GET and POST
        let users_path = paths.get("/users").unwrap().as_object().unwrap();
        assert!(users_path.contains_key("get"));
        assert!(users_path.contains_key("post"));
    }

    #[test]
    fn test_build_unified_openapi_spec_includes_metadata_when_requested() {
        let schemas = vec![create_test_schema_data(1, "/users", "GET", 50, 0.95)];

        let options = ExportMultipleSchemasRequest {
            schema_ids: vec![1],
            title: "Test".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            include_metadata: true,
        };

        let result = build_unified_openapi_spec(&schemas, &options);

        // Verify description includes endpoint summary
        assert!(result.info.description.is_some());
        let desc = result.info.description.as_ref().unwrap();
        assert!(desc.contains("GET /users"));
        assert!(desc.contains("50 samples"));
        assert!(desc.contains("95% confidence"));

        // Verify x-flowplane-* extensions are present in response schemas
        let paths = result.paths.as_object().unwrap();
        let users = paths.get("/users").unwrap();
        let get_op = users.get("get").unwrap();
        let responses = get_op.get("responses").unwrap();
        let resp_200 = responses.get("200").unwrap();
        let content = resp_200.get("content").unwrap();
        let json = content.get("application/json").unwrap();
        let schema = json.get("schema").unwrap();
        assert!(schema.get("x-flowplane-sample-count").is_some());
        assert!(schema.get("x-flowplane-confidence").is_some());
    }

    #[test]
    fn test_build_unified_openapi_spec_without_metadata() {
        let schemas = vec![create_test_schema_data(1, "/users", "GET", 50, 0.95)];

        let options = ExportMultipleSchemasRequest {
            schema_ids: vec![1],
            title: "Test".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Custom description".to_string()),
            include_metadata: false,
        };

        let result = build_unified_openapi_spec(&schemas, &options);

        // Verify custom description is used as-is
        assert_eq!(result.info.description, Some("Custom description".to_string()));

        // Verify x-flowplane-* extensions are NOT present
        let paths = result.paths.as_object().unwrap();
        let users = paths.get("/users").unwrap();
        let get_op = users.get("get").unwrap();
        let responses = get_op.get("responses").unwrap();
        let resp_200 = responses.get("200").unwrap();
        let content = resp_200.get("content").unwrap();
        let json = content.get("application/json").unwrap();
        let schema = json.get("schema").unwrap();
        assert!(schema.get("x-flowplane-sample-count").is_none());
        assert!(schema.get("x-flowplane-confidence").is_none());
    }

    #[test]
    fn test_build_unified_openapi_spec_single_path_multiple_methods() {
        let schemas = vec![
            create_test_schema_data(1, "/items", "GET", 10, 0.9),
            create_test_schema_data(2, "/items", "POST", 20, 0.85),
            create_test_schema_data(3, "/items", "PUT", 15, 0.88),
            create_test_schema_data(4, "/items", "DELETE", 5, 0.75),
        ];

        let options = ExportMultipleSchemasRequest {
            schema_ids: vec![1, 2, 3, 4],
            title: "Items API".to_string(),
            version: "2.0.0".to_string(),
            description: None,
            include_metadata: false,
        };

        let result = build_unified_openapi_spec(&schemas, &options);

        // Verify single path with all methods
        let paths = result.paths.as_object().unwrap();
        assert_eq!(paths.len(), 1);

        let items = paths.get("/items").unwrap().as_object().unwrap();
        assert!(items.contains_key("get"));
        assert!(items.contains_key("post"));
        assert!(items.contains_key("put"));
        assert!(items.contains_key("delete"));
    }

    #[test]
    fn test_openapi_export_with_no_response_schema_adds_default() {
        // Create schema with request but no response (simulates request-only capture)
        let schema = crate::storage::repositories::AggregatedSchemaData {
            id: 1,
            team: "test-team".to_string(),
            path: "/users".to_string(),
            http_method: "PATCH".to_string(),
            version: 1,
            previous_version_id: None,
            request_schema: Some(
                serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}}}),
            ),
            response_schemas: None, // No response captured
            sample_count: 5,
            confidence_score: 0.8,
            breaking_changes: None,
            first_observed: chrono::Utc::now(),
            last_observed: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let options = ExportMultipleSchemasRequest {
            schema_ids: vec![1],
            title: "Test API".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            include_metadata: false,
        };

        let result = build_unified_openapi_spec(&[schema], &options);

        // Verify responses has a default entry instead of being empty
        let paths = result.paths.as_object().unwrap();
        let users = paths.get("/users").unwrap();
        let patch_op = users.get("patch").unwrap();
        let responses = patch_op.get("responses").unwrap().as_object().unwrap();

        // Should have at least one response (the default)
        assert!(!responses.is_empty(), "responses should not be empty");
        assert!(
            responses.contains_key("default"),
            "should have default response when no responses captured"
        );

        let default_response = responses.get("default").unwrap();
        assert!(
            default_response.get("description").is_some(),
            "default response should have description"
        );
    }

    /// Helper to create test schema data with specific status code
    fn create_test_schema_with_status(
        id: i64,
        path: &str,
        method: &str,
        status_code: &str,
        sample_count: i64,
        confidence: f64,
    ) -> crate::storage::repositories::AggregatedSchemaData {
        crate::storage::repositories::AggregatedSchemaData {
            id,
            team: "test-team".to_string(),
            path: path.to_string(),
            http_method: method.to_string(),
            version: 1,
            previous_version_id: None,
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: Some(serde_json::json!({
                status_code: {"type": "object", "properties": {"id": {"type": "integer"}}}
            })),
            sample_count,
            confidence_score: confidence,
            breaking_changes: None,
            first_observed: chrono::Utc::now(),
            last_observed: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_build_unified_openapi_spec_merges_multiple_status_codes_same_endpoint() {
        // BUG REPRODUCTION: Same endpoint (POST /customers) with different status codes
        // should have ALL status codes in the exported OpenAPI responses
        let schemas = vec![
            create_test_schema_with_status(1, "/v2/api/customers", "POST", "201", 10, 0.9),
            create_test_schema_with_status(2, "/v2/api/customers", "POST", "400", 5, 0.85),
            create_test_schema_with_status(3, "/v2/api/customers", "POST", "500", 2, 0.7),
        ];

        let options = ExportMultipleSchemasRequest {
            schema_ids: vec![1, 2, 3],
            title: "Customer API".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            include_metadata: false,
        };

        let result = build_unified_openapi_spec(&schemas, &options);

        // Verify single path exists
        let paths = result.paths.as_object().unwrap();
        assert!(paths.contains_key("/v2/api/customers"), "Should have /v2/api/customers path");

        // Verify POST operation exists
        let customers = paths.get("/v2/api/customers").unwrap();
        let post_op = customers.get("post").unwrap();
        let responses = post_op.get("responses").unwrap().as_object().unwrap();

        // THIS IS THE BUG: Currently only 201 is exported, 400 and 500 are dropped
        // After fix, all three status codes should be present
        assert!(
            responses.contains_key("201"),
            "Should have 201 response - got: {:?}",
            responses.keys().collect::<Vec<_>>()
        );
        assert!(
            responses.contains_key("400"),
            "Should have 400 response - got: {:?}",
            responses.keys().collect::<Vec<_>>()
        );
        assert!(
            responses.contains_key("500"),
            "Should have 500 response - got: {:?}",
            responses.keys().collect::<Vec<_>>()
        );

        // Verify each response has correct structure
        for status_code in ["201", "400", "500"] {
            let resp = responses.get(status_code).unwrap();
            assert!(resp.get("description").is_some());
            assert!(resp.get("content").is_some());
        }
    }

    #[test]
    fn test_openapi_export_with_empty_response_schemas_map_adds_default() {
        // Create schema with empty response_schemas map
        let schema = crate::storage::repositories::AggregatedSchemaData {
            id: 1,
            team: "test-team".to_string(),
            path: "/users".to_string(),
            http_method: "PUT".to_string(),
            version: 1,
            previous_version_id: None,
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: Some(serde_json::json!({})), // Empty map
            sample_count: 3,
            confidence_score: 0.7,
            breaking_changes: None,
            first_observed: chrono::Utc::now(),
            last_observed: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let options = ExportMultipleSchemasRequest {
            schema_ids: vec![1],
            title: "Test API".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            include_metadata: false,
        };

        let result = build_unified_openapi_spec(&[schema], &options);

        // Verify responses has a default entry
        let paths = result.paths.as_object().unwrap();
        let users = paths.get("/users").unwrap();
        let put_op = users.get("put").unwrap();
        let responses = put_op.get("responses").unwrap().as_object().unwrap();

        assert!(!responses.is_empty(), "responses should not be empty");
        assert!(
            responses.contains_key("default"),
            "should have default response when response_schemas is empty"
        );
    }

    /// Tests for team access verification using unified pattern
    mod access_verification_tests {
        use super::*;
        use crate::api::handlers::team_access::verify_team_access;

        fn sample_schema(
            team: &str,
            id: i64,
        ) -> crate::storage::repositories::AggregatedSchemaData {
            crate::storage::repositories::AggregatedSchemaData {
                id,
                team: team.to_string(),
                path: "/api/test".to_string(),
                http_method: "GET".to_string(),
                version: 1,
                previous_version_id: None,
                request_schema: Some(serde_json::json!({"type": "object"})),
                response_schemas: Some(serde_json::json!({
                    "200": {"type": "object", "properties": {"id": {"type": "integer"}}}
                })),
                sample_count: 10,
                confidence_score: 0.9,
                breaking_changes: None,
                first_observed: chrono::Utc::now(),
                last_observed: chrono::Utc::now(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }
        }

        #[tokio::test]
        async fn test_verify_access_same_team() {
            let schema = sample_schema("team-a", 1);
            let team_scopes = vec!["team-a".to_string()];
            let result = verify_team_access(schema, &team_scopes).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_verify_access_different_team() {
            let schema = sample_schema("team-a", 1);
            let team_scopes = vec!["team-b".to_string()];
            let result = verify_team_access(schema, &team_scopes).await;
            assert!(result.is_err());
            match result {
                Err(ApiError::NotFound(_)) => {} // Expected - return 404 to avoid leaking info
                _ => panic!("Expected NotFound error for cross-team access"),
            }
        }

        #[tokio::test]
        async fn test_verify_access_empty_scopes_denied() {
            let schema = sample_schema("any-team", 1);
            let team_scopes: Vec<String> = vec![]; // Empty scopes = no access (admin bypass removed)
            let result = verify_team_access(schema, &team_scopes).await;
            assert!(result.is_err(), "empty scopes should not bypass team access");
        }
    }
}
