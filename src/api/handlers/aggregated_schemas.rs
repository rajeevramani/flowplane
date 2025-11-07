//! Aggregated Schema HTTP handlers
//!
//! This module provides REST API endpoints for retrieving, comparing, and exporting
//! aggregated API schemas learned from traffic observations.

use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::{extract_team_scopes, require_resource_access},
    auth::models::AuthContext,
};

// === DTOs ===

/// Query parameters for listing aggregated schemas
#[derive(Debug, Deserialize, IntoParams, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ListAggregatedSchemasQuery {
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

async fn verify_schema_access(
    schema: crate::storage::repositories::AggregatedSchemaData,
    team_scopes: &[String],
) -> Result<crate::storage::repositories::AggregatedSchemaData, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(schema);
    }

    // Check if schema belongs to one of user's teams
    if team_scopes.contains(&schema.team) {
        Ok(schema)
    } else {
        // Record cross-team access attempt for security monitoring
        if let Some(from_team) = team_scopes.first() {
            crate::observability::metrics::record_cross_team_access_attempt(
                from_team,
                &schema.team,
                "aggregated_schemas",
            )
            .await;
        }

        // Return 404 to avoid leaking existence of other teams' resources
        Err(ApiError::NotFound(format!("Aggregated schema with ID '{}' not found", schema.id)))
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
    tag = "aggregated-schemas"
)]
pub async fn list_aggregated_schemas_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<ListAggregatedSchemasQuery>,
) -> Result<Json<Vec<AggregatedSchemaResponse>>, ApiError> {
    // Authorization
    require_resource_access(&context, "aggregated-schemas", "read", None)?;

    // Extract team from context
    let team_scopes = extract_team_scopes(&context);
    let team = team_scopes.first().ok_or_else(|| {
        ApiError::BadRequest("Team scope required for aggregated schemas".to_string())
    })?;

    // Get repository
    let repo = state
        .xds_state
        .aggregated_schema_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    // List schemas with filters
    let schemas = repo
        .list_filtered(
            team,
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
    tag = "aggregated-schemas"
)]
pub async fn get_aggregated_schema_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<i64>,
) -> Result<Json<AggregatedSchemaResponse>, ApiError> {
    // Authorization
    require_resource_access(&context, "aggregated-schemas", "read", None)?;

    // Extract team scopes
    let team_scopes = extract_team_scopes(&context);

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

    // Verify team access
    let authorized_schema = verify_schema_access(schema, &team_scopes).await?;

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
    tag = "aggregated-schemas"
)]
pub async fn compare_aggregated_schemas_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<i64>,
    Query(query): Query<CompareSchemaQuery>,
) -> Result<Json<SchemaComparisonResponse>, ApiError> {
    // Authorization
    require_resource_access(&context, "aggregated-schemas", "read", None)?;

    // Extract team scopes
    let team_scopes = extract_team_scopes(&context);

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

    // Verify team access to current schema
    let authorized_current = verify_schema_access(current_schema.clone(), &team_scopes).await?;

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
    tag = "aggregated-schemas"
)]
pub async fn export_aggregated_schema_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<i64>,
    Query(query): Query<ExportSchemaQuery>,
) -> Result<Json<OpenApiExportResponse>, ApiError> {
    // Authorization
    require_resource_access(&context, "aggregated-schemas", "read", None)?;

    // Extract team scopes
    let team_scopes = extract_team_scopes(&context);

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

    // Verify team access
    let authorized_schema = verify_schema_access(schema, &team_scopes).await?;

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

// === OpenAPI Export Logic ===

fn build_openapi_spec(
    schema: &crate::storage::repositories::AggregatedSchemaData,
    include_metadata: bool,
) -> OpenApiExportResponse {
    let mut path_item = serde_json::json!({});

    // Build operation object for this HTTP method
    let method_key = schema.http_method.to_lowercase();
    let mut operation = serde_json::json!({
        "summary": format!("{} {}", schema.http_method, schema.path),
        "operationId": format!("{}_{}", method_key, schema.path.replace('/', "_").replace(['{', '}'], "")),
        "responses": {}
    });

    // Add request body if present
    if let Some(ref req_schema) = schema.request_schema {
        // Clone and strip internal attributes from request schema
        let mut cleaned_req_schema = req_schema.clone();
        strip_internal_attributes(&mut cleaned_req_schema);

        let mut request_body = serde_json::json!({
            "required": true,
            "content": {
                "application/json": {
                    "schema": cleaned_req_schema
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
                // Clone and strip internal attributes from response schema
                let mut cleaned_resp_schema = resp_schema.clone();
                strip_internal_attributes(&mut cleaned_resp_schema);

                let mut response_obj = serde_json::json!({
                    "description": format!("Response with status {}", status_code),
                    "content": {
                        "application/json": {
                            "schema": cleaned_resp_schema
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

    path_item[&method_key] = operation;

    let mut paths = serde_json::json!({});
    paths[&schema.path] = path_item;

    OpenApiExportResponse {
        openapi: "3.1.0".to_string(),
        info: OpenApiInfo {
            title: format!("API Schema - {} {}", schema.http_method, schema.path),
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
}
