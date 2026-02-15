//! Aggregated Schemas MCP Tools
//!
//! Control Plane tools for discovering and inspecting API schemas learned through traffic analysis.

use crate::domain::OrgId;
use crate::internal_api::{AggregatedSchemaOperations, InternalAuthContext, ListSchemasRequest};
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::xds::XdsState;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::instrument;

/// Validate that a team belongs to the caller's org. Returns McpError on failure.
async fn validate_team_in_org(
    db_pool: &crate::storage::DbPool,
    team: &str,
    org_id: &OrgId,
) -> Result<(), McpError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT COUNT(*) FROM teams WHERE name = $1 AND org_id = $2")
            .bind(team)
            .bind(org_id.as_str())
            .fetch_optional(db_pool)
            .await
            .map_err(|e| McpError::InternalError(format!("Failed to validate team: {}", e)))?;

    let count = row.map(|r| r.0).unwrap_or(0);
    if count == 0 {
        return Err(McpError::Forbidden(format!("Team '{}' not found in your organization", team)));
    }
    Ok(())
}

/// Tool definition for listing aggregated schemas
pub fn cp_list_aggregated_schemas_tool() -> Tool {
    Tool::new(
        "cp_list_aggregated_schemas",
        r#"List aggregated API schemas discovered through learning sessions.

PURPOSE: Discover API endpoints and their structures as learned from actual traffic patterns.
These schemas represent the consensus view of API structure based on observed requests and responses.

SCHEMA PROPERTIES:
- path: The API endpoint path (e.g., /api/users)
- http_method: HTTP method (GET, POST, PUT, DELETE, etc.)
- request_schema: JSON Schema for request body (if applicable)
- response_schemas: JSON Schemas for responses by status code
- sample_count: Number of traffic samples used to build the schema
- confidence_score: Reliability score from 0.0 to 1.0 (higher = more reliable)
- version: Schema version number for the endpoint
- breaking_changes: Description of breaking changes from previous version

FILTERING OPTIONS:
- path: Search pattern for API path (e.g., '/api/users' or '/v1/')
- http_method: Filter by HTTP method (GET, POST, etc.)
- min_confidence: Minimum confidence score (0.0 to 1.0)
- latest_only: Only return latest version of each endpoint (default: true)

USE CASES:
- Discover available API endpoints
- Review API structure and evolution
- Identify high-confidence schemas for documentation
- Find endpoints with breaking changes
- Export schemas for validation or code generation

VERSIONING: Each endpoint can have multiple schema versions. Use latest_only=true to
see only the current version, or latest_only=false to see the full version history.

CONFIDENCE SCORES:
- 0.9-1.0: Very reliable, based on many consistent samples
- 0.7-0.9: Good confidence, suitable for most uses
- 0.5-0.7: Moderate confidence, may need review
- Below 0.5: Low confidence, recently observed or inconsistent

RELATED TOOLS: cp_get_aggregated_schema (get specific schema by ID)"#,
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Search pattern for API path (e.g., '/api/users' or '/v1/')"
                },
                "http_method": {
                    "type": "string",
                    "description": "Filter by HTTP method",
                    "enum": ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"]
                },
                "min_confidence": {
                    "type": "number",
                    "description": "Minimum confidence score (0.0 to 1.0). Higher values indicate more reliable schemas.",
                    "minimum": 0.0,
                    "maximum": 1.0
                },
                "latest_only": {
                    "type": "boolean",
                    "description": "If true, only return the latest version of each endpoint (default: true)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of schemas to return (1-1000, default: 100)",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 100
                },
                "offset": {
                    "type": "integer",
                    "description": "Offset for pagination (default: 0)",
                    "minimum": 0,
                    "default": 0
                }
            }
        }),
    )
}

/// Tool definition for getting a specific aggregated schema
pub fn cp_get_aggregated_schema_tool() -> Tool {
    Tool::new(
        "cp_get_aggregated_schema",
        r#"Get a specific aggregated API schema by ID.

PURPOSE: Retrieve detailed schema information for a specific API endpoint version.

RETURNS:
- id: Internal schema identifier
- path: API endpoint path
- http_method: HTTP method
- request_schema: JSON Schema for request body (if applicable)
- response_schemas: JSON Schemas for responses by status code (e.g., "200", "404")
- sample_count: Number of traffic samples used to build this schema
- confidence_score: Reliability score from 0.0 to 1.0
- version: Version number for this endpoint's schema
- breaking_changes: Description of breaking changes from previous version (if any)
- previous_version_id: ID of the previous schema version (if exists)
- first_observed: When this schema version was first seen
- last_observed: When this schema version was last confirmed
- created_at: When the schema was stored
- updated_at: When the schema was last updated

SCHEMA STRUCTURE:
- request_schema: OpenAPI-style JSON Schema describing request body structure
- response_schemas: Map of status codes to JSON Schemas describing response structure

WHEN TO USE:
- Get full details of a discovered endpoint
- Review request/response structures
- Check confidence and sample count before using schema
- Understand breaking changes from previous versions
- Get version history linkage (previous_version_id)

EXAMPLE USE CASE:
After listing schemas, get detailed structure for a specific endpoint to understand
its request/response format, validate against actual implementation, or generate
client code.

RELATED TOOLS: cp_list_aggregated_schemas (discover schemas)"#,
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "description": "The aggregated schema ID"
                }
            },
            "required": ["id"]
        }),
    )
}

/// Tool definition for exporting aggregated schemas as OpenAPI 3.1 specification
pub fn cp_export_schema_openapi_tool() -> Tool {
    Tool::new(
        "cp_export_schema_openapi",
        r#"Export aggregated API schemas as an OpenAPI 3.1 specification.

PURPOSE: Generate a unified OpenAPI document from learned API schemas, suitable for
documentation, code generation, or API validation.

PARAMETERS:
- schema_ids (required): Array of schema IDs to include in the export
- title (optional): Title for the OpenAPI spec (default: "Flowplane API")
- version (optional): Version string (default: "1.0.0")
- description (optional): Description for the API
- include_metadata (optional): Include Flowplane extensions like x-flowplane-sample-count (default: false)

EXAMPLE:
{ "schema_ids": [1, 2, 3], "title": "Orders API", "version": "2.0.0" }

RETURNS:
- openapi: "3.1.0"
- info: { title, version, description }
- paths: OpenAPI path definitions with operations, parameters, request/response schemas
- components: Shared schema components

WORKFLOW:
1. Use cp_list_aggregated_schemas to discover available schemas
2. Select schema IDs of interest
3. Call cp_export_schema_openapi with those IDs

Authorization: Requires aggregated-schemas:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "schema_ids": {
                    "type": "array",
                    "description": "Array of schema IDs to include in the export",
                    "items": { "type": "integer" },
                    "minItems": 1
                },
                "title": {
                    "type": "string",
                    "description": "Title for the OpenAPI spec (default: 'Flowplane API')"
                },
                "version": {
                    "type": "string",
                    "description": "Version string (default: '1.0.0')"
                },
                "description": {
                    "type": "string",
                    "description": "Description for the API"
                },
                "include_metadata": {
                    "type": "boolean",
                    "description": "Include Flowplane extensions (x-flowplane-*). Default: false",
                    "default": false
                }
            },
            "required": ["schema_ids"]
        }),
    )
}

/// Execute cp_export_schema_openapi: export schemas as OpenAPI 3.1.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_export_schema_openapi")]
pub async fn execute_export_schema_openapi(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Validate team belongs to caller's org
    if let Some(oid) = org_id {
        let pool = xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| McpError::InternalError("Database not available".to_string()))?
            .pool();
        validate_team_in_org(pool, team, oid).await?;
    }

    let schema_ids: Vec<i64> = args
        .get("schema_ids")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            McpError::InvalidParams("Missing required parameter: schema_ids".to_string())
        })?
        .iter()
        .filter_map(|v| v.as_i64())
        .collect();

    if schema_ids.is_empty() {
        return Err(McpError::InvalidParams("schema_ids must contain at least one ID".to_string()));
    }

    let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("Flowplane API").to_string();
    let version = args.get("version").and_then(|v| v.as_str()).unwrap_or("1.0.0").to_string();
    let description = args.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());
    let include_metadata = args.get("include_metadata").and_then(|v| v.as_bool()).unwrap_or(false);

    // Verify auth context
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    // Fetch schemas via internal API
    let ops = AggregatedSchemaOperations::new(xds_state.clone());
    let mut schemas = Vec::new();
    for id in &schema_ids {
        let schema = ops.get(*id, &auth).await?;
        schemas.push(schema);
    }

    // Build the OpenAPI export using the existing REST handler's logic
    let export_request = crate::api::handlers::aggregated_schemas::ExportMultipleSchemasRequest {
        schema_ids,
        title,
        version,
        description,
        include_metadata,
    };

    let repo = xds_state
        .aggregated_schema_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Schema repository unavailable".to_string()))?;

    // Fetch full schema data for the export builder
    let schema_data = repo.get_by_ids(&export_request.schema_ids).await.map_err(|e| {
        McpError::InternalError(format!("Failed to fetch schemas for export: {}", e))
    })?;

    let openapi_response = crate::api::handlers::aggregated_schemas::build_unified_openapi_spec(
        &schema_data,
        &export_request,
    );

    let output = json!({
        "success": true,
        "openapi": openapi_response.openapi,
        "info": {
            "title": openapi_response.info.title,
            "version": openapi_response.info.version,
            "description": openapi_response.info.description
        },
        "paths": openapi_response.paths,
        "components": openapi_response.components,
        "schema_count": schema_data.len(),
        "message": format!("Exported {} schema(s) as OpenAPI 3.1", schema_data.len())
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute list aggregated schemas operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_list_aggregated_schemas")]
pub async fn execute_list_aggregated_schemas(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let path = args.get("path").and_then(|v| v.as_str()).map(String::from);
    let http_method = args.get("http_method").and_then(|v| v.as_str()).map(String::from);
    let min_confidence = args.get("min_confidence").and_then(|v| v.as_f64());
    let latest_only = args.get("latest_only").and_then(|v| v.as_bool());
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32);
    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32);

    // Use internal API layer
    let ops = AggregatedSchemaOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let req = ListSchemasRequest { path, http_method, min_confidence, latest_only, limit, offset };

    let response = ops.list(req, &auth).await?;

    let result = json!({
        "schemas": response.schemas.iter().map(|s| {
            // Parse JSON schemas
            let request_schema = s.request_schema.as_ref().and_then(|v| {
                serde_json::from_value(v.clone()).ok()
            }).unwrap_or(json!(null));

            let response_schemas = s.response_schemas.as_ref().and_then(|v| {
                serde_json::from_value(v.clone()).ok()
            }).unwrap_or(json!({}));

            json!({
                "id": s.id,
                "path": s.path,
                "http_method": s.http_method,
                "request_schema": request_schema,
                "response_schemas": response_schemas,
                "sample_count": s.sample_count,
                "confidence_score": s.confidence_score,
                "version": s.version,
                "breaking_changes": s.breaking_changes,
                "previous_version_id": s.previous_version_id,
                "first_observed": s.first_observed.to_rfc3339(),
                "last_observed": s.last_observed.to_rfc3339(),
                "created_at": s.created_at.to_rfc3339(),
                "updated_at": s.updated_at.to_rfc3339()
            })
        }).collect::<Vec<_>>(),
        "count": response.count
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

/// Execute get aggregated schema operation using the internal API layer.
#[instrument(skip(xds_state, args), fields(team = %team), name = "mcp_execute_get_aggregated_schema")]
pub async fn execute_get_aggregated_schema(
    xds_state: &Arc<XdsState>,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let id = args
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: id".to_string()))?;

    // Use internal API layer
    let ops = AggregatedSchemaOperations::new(xds_state.clone());
    let team_repo = xds_state
        .team_repository
        .as_ref()
        .ok_or_else(|| McpError::InternalError("Team repository unavailable".to_string()))?;
    let auth = InternalAuthContext::from_mcp(team, org_id.cloned(), None)
        .resolve_teams(team_repo)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to resolve teams: {}", e)))?;

    let schema = ops.get(id, &auth).await?;

    // Parse JSON schemas
    let request_schema = schema
        .request_schema
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(json!(null));

    let response_schemas = schema
        .response_schemas
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(json!({}));

    let result = json!({
        "id": schema.id,
        "team": schema.team,
        "path": schema.path,
        "http_method": schema.http_method,
        "request_schema": request_schema,
        "response_schemas": response_schemas,
        "sample_count": schema.sample_count,
        "confidence_score": schema.confidence_score,
        "version": schema.version,
        "breaking_changes": schema.breaking_changes,
        "previous_version_id": schema.previous_version_id,
        "first_observed": schema.first_observed.to_rfc3339(),
        "last_observed": schema.last_observed.to_rfc3339(),
        "created_at": schema.created_at.to_rfc3339(),
        "updated_at": schema.updated_at.to_rfc3339()
    });

    let result_text =
        serde_json::to_string_pretty(&result).map_err(McpError::SerializationError)?;

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text: result_text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cp_list_aggregated_schemas_tool_definition() {
        let tool = cp_list_aggregated_schemas_tool();
        assert_eq!(tool.name, "cp_list_aggregated_schemas");
        assert!(tool.description.as_ref().unwrap().contains("aggregated"));
        assert!(tool.description.as_ref().unwrap().contains("schema"));
        assert!(tool.description.as_ref().unwrap().contains("API"));

        // Verify input schema has expected properties
        let properties = &tool.input_schema["properties"];
        assert!(properties.get("path").is_some());
        assert!(properties.get("http_method").is_some());
        assert!(properties.get("min_confidence").is_some());
        assert!(properties.get("latest_only").is_some());

        // Verify no required parameters
        assert!(
            tool.input_schema.get("required").is_none()
                || tool.input_schema["required"].as_array().is_none_or(|a| a.is_empty())
        );
    }

    #[test]
    fn test_cp_get_aggregated_schema_tool_definition() {
        let tool = cp_get_aggregated_schema_tool();
        assert_eq!(tool.name, "cp_get_aggregated_schema");
        assert!(tool.description.as_ref().unwrap().contains("specific"));
        assert!(tool.description.as_ref().unwrap().contains("schema"));

        // Verify required parameters
        let required = tool.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("id")));

        // Verify id property exists and is correct type
        let properties = &tool.input_schema["properties"];
        let id_prop = properties.get("id").unwrap();
        assert_eq!(id_prop["type"], "integer");
    }

    #[test]
    fn test_tool_descriptions_mention_confidence() {
        let list_tool = cp_list_aggregated_schemas_tool();
        let get_tool = cp_get_aggregated_schema_tool();

        // Both tools should explain confidence scores
        assert!(list_tool.description.as_ref().unwrap().contains("confidence"));
        assert!(get_tool.description.as_ref().unwrap().contains("confidence"));
    }

    #[test]
    fn test_list_tool_has_http_method_enum() {
        let tool = cp_list_aggregated_schemas_tool();
        let properties = &tool.input_schema["properties"];
        let http_method = properties.get("http_method").unwrap();

        let enum_values = http_method["enum"].as_array().unwrap();
        assert!(enum_values.contains(&json!("GET")));
        assert!(enum_values.contains(&json!("POST")));
        assert!(enum_values.contains(&json!("PUT")));
        assert!(enum_values.contains(&json!("DELETE")));
        assert!(enum_values.contains(&json!("PATCH")));
    }

    #[test]
    fn test_cp_export_schema_openapi_tool_definition() {
        let tool = cp_export_schema_openapi_tool();
        assert_eq!(tool.name, "cp_export_schema_openapi");
        assert!(tool.description.as_ref().unwrap().contains("OpenAPI"));
        assert!(tool.description.as_ref().unwrap().contains("3.1"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["schema_ids"].is_object());
        assert!(schema["properties"]["title"].is_object());
        assert!(schema["properties"]["version"].is_object());
        assert!(schema["properties"]["include_metadata"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("schema_ids")));
    }
}
