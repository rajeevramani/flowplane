//! Route metadata repository for managing OpenAPI-extracted route information
//!
//! This module provides CRUD operations for route metadata, which stores OpenAPI
//! specifications and other metadata about routes to enable MCP tool generation.

use crate::domain::{RouteId, RouteMetadataId, RouteMetadataSourceType};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;

/// Database row structure for route metadata
#[derive(Debug, Clone, FromRow)]
struct RouteMetadataRow {
    pub id: String,
    pub route_id: String,
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub tags: Option<String>,
    pub http_method: Option<String>,
    pub request_body_schema: Option<String>,
    pub response_schemas: Option<String>,
    pub learning_schema_id: Option<i64>,
    pub enriched_from_learning: bool,
    pub source_type: String,
    pub confidence: Option<f64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Route metadata data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteMetadataData {
    pub id: RouteMetadataId,
    pub route_id: RouteId,
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub http_method: Option<String>,
    pub request_body_schema: Option<serde_json::Value>,
    pub response_schemas: Option<serde_json::Value>,
    pub learning_schema_id: Option<i64>,
    pub enriched_from_learning: bool,
    pub source_type: RouteMetadataSourceType,
    pub confidence: Option<f64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<RouteMetadataRow> for RouteMetadataData {
    type Error = FlowplaneError;

    fn try_from(row: RouteMetadataRow) -> Result<Self> {
        let tags = row.tags.as_ref().and_then(|t| serde_json::from_str::<Vec<String>>(t).ok());

        let request_body_schema =
            row.request_body_schema.as_ref().and_then(|s| serde_json::from_str(s).ok());

        let response_schemas =
            row.response_schemas.as_ref().and_then(|s| serde_json::from_str(s).ok());

        let source_type = row
            .source_type
            .parse()
            .map_err(|e| FlowplaneError::validation(format!("Invalid source_type: {}", e)))?;

        Ok(Self {
            id: RouteMetadataId::from_string(row.id),
            route_id: RouteId::from_string(row.route_id),
            operation_id: row.operation_id,
            summary: row.summary,
            description: row.description,
            tags,
            http_method: row.http_method,
            request_body_schema,
            response_schemas,
            learning_schema_id: row.learning_schema_id,
            enriched_from_learning: row.enriched_from_learning,
            source_type,
            confidence: row.confidence,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Create route metadata request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRouteMetadataRequest {
    pub route_id: RouteId,
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub http_method: Option<String>,
    pub request_body_schema: Option<serde_json::Value>,
    pub response_schemas: Option<serde_json::Value>,
    pub learning_schema_id: Option<i64>,
    pub enriched_from_learning: bool,
    pub source_type: RouteMetadataSourceType,
    pub confidence: Option<f64>,
}

/// Update route metadata request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRouteMetadataRequest {
    pub operation_id: Option<Option<String>>,
    pub summary: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub tags: Option<Option<Vec<String>>>,
    pub http_method: Option<Option<String>>,
    pub request_body_schema: Option<Option<serde_json::Value>>,
    pub response_schemas: Option<Option<serde_json::Value>>,
    pub learning_schema_id: Option<Option<i64>>,
    pub enriched_from_learning: Option<bool>,
    pub source_type: Option<RouteMetadataSourceType>,
    pub confidence: Option<Option<f64>>,
}

/// Repository for route metadata data access
#[derive(Debug, Clone)]
pub struct RouteMetadataRepository {
    pool: DbPool,
}

impl RouteMetadataRepository {
    /// Create a new route metadata repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create new route metadata
    #[instrument(skip(self, request), fields(route_id = %request.route_id), name = "db_create_route_metadata")]
    pub async fn create(&self, request: CreateRouteMetadataRequest) -> Result<RouteMetadataData> {
        let id = RouteMetadataId::new();
        let now = chrono::Utc::now();

        let tags_json = request
            .tags
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| FlowplaneError::validation(format!("Invalid tags JSON: {}", e)))?;

        let request_body_schema_json =
            request.request_body_schema.as_ref().map(serde_json::to_string).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid request_body_schema JSON: {}", e)),
            )?;

        let response_schemas_json =
            request.response_schemas.as_ref().map(serde_json::to_string).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid response_schemas JSON: {}", e)),
            )?;

        let result = sqlx::query(
            "INSERT INTO route_metadata (
                id, route_id, operation_id, summary, description, tags, http_method,
                request_body_schema, response_schemas, learning_schema_id,
                enriched_from_learning, source_type, confidence, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(&id)
        .bind(&request.route_id)
        .bind(&request.operation_id)
        .bind(&request.summary)
        .bind(&request.description)
        .bind(&tags_json)
        .bind(&request.http_method)
        .bind(&request_body_schema_json)
        .bind(&response_schemas_json)
        .bind(request.learning_schema_id)
        .bind(request.enriched_from_learning)
        .bind(request.source_type.to_string())
        .bind(request.confidence)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %request.route_id, "Failed to create route metadata");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create route metadata for route '{}'", request.route_id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create route metadata"));
        }

        tracing::info!(
            route_metadata_id = %id,
            route_id = %request.route_id,
            "Created route metadata"
        );

        self.get_by_id(&id).await?.ok_or_else(|| {
            FlowplaneError::internal("Failed to retrieve newly created route metadata")
        })
    }

    /// Get route metadata by ID
    #[instrument(skip(self), fields(route_metadata_id = %id), name = "db_get_route_metadata_by_id")]
    pub async fn get_by_id(&self, id: &RouteMetadataId) -> Result<Option<RouteMetadataData>> {
        let row = sqlx::query_as::<sqlx::Postgres, RouteMetadataRow>(
            "SELECT id, route_id, operation_id, summary, description, tags, http_method,
                    request_body_schema, response_schemas, learning_schema_id,
                    enriched_from_learning, source_type, confidence, created_at, updated_at
             FROM route_metadata WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_metadata_id = %id, "Failed to get route metadata by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get route metadata with ID '{}'", id),
            }
        })?;

        row.map(RouteMetadataData::try_from).transpose()
    }

    /// Get route metadata by route ID
    #[instrument(skip(self), fields(route_id = %route_id), name = "db_get_route_metadata_by_route_id")]
    pub async fn get_by_route_id(&self, route_id: &RouteId) -> Result<Option<RouteMetadataData>> {
        let row = sqlx::query_as::<sqlx::Postgres, RouteMetadataRow>(
            "SELECT id, route_id, operation_id, summary, description, tags, http_method,
                    request_body_schema, response_schemas, learning_schema_id,
                    enriched_from_learning, source_type, confidence, created_at, updated_at
             FROM route_metadata WHERE route_id = $1",
        )
        .bind(route_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %route_id, "Failed to get route metadata by route ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get route metadata for route '{}'", route_id),
            }
        })?;

        row.map(RouteMetadataData::try_from).transpose()
    }

    /// List route metadata by team (joins with routes table)
    #[instrument(skip(self), fields(team = %team), name = "db_list_route_metadata_by_team")]
    pub async fn list_by_team(&self, team: &str) -> Result<Vec<RouteMetadataData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, RouteMetadataRow>(
            "SELECT rm.id, rm.route_id, rm.operation_id, rm.summary, rm.description, rm.tags, rm.http_method,
                    rm.request_body_schema, rm.response_schemas, rm.learning_schema_id,
                    rm.enriched_from_learning, rm.source_type, rm.confidence, rm.created_at, rm.updated_at
             FROM route_metadata rm
             INNER JOIN routes r ON rm.route_id = r.id
             WHERE r.team = $1
             ORDER BY rm.created_at DESC",
        )
        .bind(team)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list route metadata by team");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list route metadata for team '{}'", team),
            }
        })?;

        rows.into_iter().map(RouteMetadataData::try_from).collect()
    }

    /// Update route metadata
    #[instrument(skip(self, request), fields(route_metadata_id = %id), name = "db_update_route_metadata")]
    pub async fn update(
        &self,
        id: &RouteMetadataId,
        request: UpdateRouteMetadataRequest,
    ) -> Result<RouteMetadataData> {
        // Get current metadata
        let current = self.get_by_id(id).await?.ok_or_else(|| {
            FlowplaneError::not_found_msg(format!("Route metadata with ID '{}' not found", id))
        })?;

        let new_operation_id = request.operation_id.unwrap_or(current.operation_id);
        let new_summary = request.summary.unwrap_or(current.summary);
        let new_description = request.description.unwrap_or(current.description);
        let new_tags = request.tags.unwrap_or(current.tags);
        let new_http_method = request.http_method.unwrap_or(current.http_method);
        let new_request_body_schema =
            request.request_body_schema.unwrap_or(current.request_body_schema);
        let new_response_schemas = request.response_schemas.unwrap_or(current.response_schemas);
        let new_learning_schema_id =
            request.learning_schema_id.unwrap_or(current.learning_schema_id);
        let new_enriched_from_learning =
            request.enriched_from_learning.unwrap_or(current.enriched_from_learning);
        let new_source_type = request.source_type.unwrap_or(current.source_type);
        let new_confidence = request.confidence.unwrap_or(current.confidence);

        let now = chrono::Utc::now();

        let tags_json = new_tags
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| FlowplaneError::validation(format!("Invalid tags JSON: {}", e)))?;

        let request_body_schema_json =
            new_request_body_schema.as_ref().map(serde_json::to_string).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid request_body_schema JSON: {}", e)),
            )?;

        let response_schemas_json =
            new_response_schemas.as_ref().map(serde_json::to_string).transpose().map_err(|e| {
                FlowplaneError::validation(format!("Invalid response_schemas JSON: {}", e))
            })?;

        let result = sqlx::query(
            "UPDATE route_metadata SET
                operation_id = $1, summary = $2, description = $3, tags = $4, http_method = $5,
                request_body_schema = $6, response_schemas = $7, learning_schema_id = $8,
                enriched_from_learning = $9, source_type = $10, confidence = $11, updated_at = $12
             WHERE id = $13",
        )
        .bind(&new_operation_id)
        .bind(&new_summary)
        .bind(&new_description)
        .bind(&tags_json)
        .bind(&new_http_method)
        .bind(&request_body_schema_json)
        .bind(&response_schemas_json)
        .bind(new_learning_schema_id)
        .bind(new_enriched_from_learning)
        .bind(new_source_type.to_string())
        .bind(new_confidence)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_metadata_id = %id, "Failed to update route metadata");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update route metadata with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Route metadata with ID '{}' not found",
                id
            )));
        }

        tracing::info!(
            route_metadata_id = %id,
            "Updated route metadata"
        );

        self.get_by_id(id).await?.ok_or_else(|| {
            FlowplaneError::not_found_msg(format!("Route metadata with ID '{}' not found", id))
        })
    }

    /// Delete route metadata
    #[instrument(skip(self), fields(route_metadata_id = %id), name = "db_delete_route_metadata")]
    pub async fn delete(&self, id: &RouteMetadataId) -> Result<()> {
        // Check if metadata exists first
        let _metadata = self.get_by_id(id).await?.ok_or_else(|| {
            FlowplaneError::not_found_msg(format!("Route metadata with ID '{}' not found", id))
        })?;

        let result = sqlx::query("DELETE FROM route_metadata WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, route_metadata_id = %id, "Failed to delete route metadata");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete route metadata with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Route metadata with ID '{}' not found",
                id
            )));
        }

        tracing::info!(
            route_metadata_id = %id,
            "Deleted route metadata"
        );

        Ok(())
    }
}
