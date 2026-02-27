// ! Aggregated API schema repository for managing consensus schemas
//!
//! This module provides CRUD operations for aggregated schema resources, handling storage,
//! retrieval, and versioning of aggregated API schemas derived from multiple observations.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row};
use tracing::instrument;

/// Database row structure for aggregated API schemas
#[derive(Debug, Clone, FromRow)]
struct AggregatedSchemaRow {
    pub id: i64,
    pub team: String,
    pub path: String,
    pub http_method: String,
    pub version: i64,
    pub previous_version_id: Option<i64>,
    pub request_schema: Option<String>,   // JSON Schema as string
    pub response_schemas: Option<String>, // JSON object as string
    pub request_headers: Option<String>,  // JSON array of {name, example}
    pub response_headers: Option<String>, // JSON array of {name, example}
    pub sample_count: i64,
    pub confidence_score: f64,
    pub breaking_changes: Option<String>, // JSON array as string
    pub first_observed: chrono::DateTime<chrono::Utc>,
    pub last_observed: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Aggregated API schema data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedSchemaData {
    pub id: i64,
    pub team: String,
    pub path: String,
    pub http_method: String,
    pub version: i64,
    pub previous_version_id: Option<i64>,
    pub request_schema: Option<serde_json::Value>,
    pub response_schemas: Option<serde_json::Value>,
    pub request_headers: Option<serde_json::Value>, // JSON array of {name, example}
    pub response_headers: Option<serde_json::Value>, // JSON array of {name, example}
    pub sample_count: i64,
    pub confidence_score: f64,
    pub breaking_changes: Option<Vec<serde_json::Value>>,
    pub first_observed: chrono::DateTime<chrono::Utc>,
    pub last_observed: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<AggregatedSchemaRow> for AggregatedSchemaData {
    type Error = FlowplaneError;

    fn try_from(row: AggregatedSchemaRow) -> Result<Self> {
        let request_schema =
            row.request_schema.as_ref().map(|s| serde_json::from_str(s)).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid request_schema JSON: {}", e)),
            )?;

        let response_schemas =
            row.response_schemas.as_ref().map(|s| serde_json::from_str(s)).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid response_schemas JSON: {}", e)),
            )?;

        let request_headers =
            row.request_headers.as_ref().map(|s| serde_json::from_str(s)).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid request_headers JSON: {}", e)),
            )?;

        let response_headers =
            row.response_headers.as_ref().map(|s| serde_json::from_str(s)).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid response_headers JSON: {}", e)),
            )?;

        let breaking_changes =
            row.breaking_changes.as_ref().map(|s| serde_json::from_str(s)).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid breaking_changes JSON: {}", e)),
            )?;

        Ok(Self {
            id: row.id,
            team: row.team,
            path: row.path,
            http_method: row.http_method,
            version: row.version,
            previous_version_id: row.previous_version_id,
            request_schema,
            response_schemas,
            request_headers,
            response_headers,
            sample_count: row.sample_count,
            confidence_score: row.confidence_score,
            breaking_changes,
            first_observed: row.first_observed,
            last_observed: row.last_observed,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

impl crate::api::handlers::team_access::TeamOwned for AggregatedSchemaData {
    fn team(&self) -> Option<&str> {
        Some(&self.team)
    }

    fn resource_name(&self) -> &str {
        &self.path
    }

    fn resource_type() -> &'static str {
        "Aggregated schema"
    }

    fn resource_type_metric() -> &'static str {
        "aggregated_schemas"
    }

    fn identifier_label() -> &'static str {
        "path"
    }
}

/// Request to create a new aggregated schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAggregatedSchemaRequest {
    pub team: String,
    pub path: String,
    pub http_method: String,
    pub request_schema: Option<serde_json::Value>,
    pub response_schemas: Option<serde_json::Value>,
    pub request_headers: Option<serde_json::Value>, // JSON array of {name, example}
    pub response_headers: Option<serde_json::Value>, // JSON array of {name, example}
    pub sample_count: i64,
    pub confidence_score: f64,
    pub breaking_changes: Option<Vec<serde_json::Value>>,
    pub first_observed: chrono::DateTime<chrono::Utc>,
    pub last_observed: chrono::DateTime<chrono::Utc>,
    pub previous_version_id: Option<i64>,
}

/// Repository for aggregated schema data access
#[derive(Debug, Clone)]
pub struct AggregatedSchemaRepository {
    pool: DbPool,
}

impl AggregatedSchemaRepository {
    /// Create a new aggregated schema repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new aggregated schema
    ///
    /// Uses a transaction to atomically determine the next version number and insert,
    /// preventing race conditions where concurrent aggregations could get the same version.
    #[instrument(skip(self, request), fields(team = %request.team, path = %request.path, method = %request.http_method), name = "db_create_aggregated_schema")]
    pub async fn create(
        &self,
        request: CreateAggregatedSchemaRequest,
    ) -> Result<AggregatedSchemaData> {
        let now = chrono::Utc::now();

        let request_schema_json =
            request.request_schema.as_ref().map(serde_json::to_string).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid request_schema: {}", e)),
            )?;

        let response_schemas_json =
            request.response_schemas.as_ref().map(serde_json::to_string).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid response_schemas: {}", e)),
            )?;

        let breaking_changes_json =
            request.breaking_changes.as_ref().map(serde_json::to_string).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid breaking_changes: {}", e)),
            )?;

        let request_headers_json =
            request.request_headers.as_ref().map(serde_json::to_string).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid request_headers: {}", e)),
            )?;

        let response_headers_json =
            request.response_headers.as_ref().map(serde_json::to_string).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid response_headers: {}", e)),
            )?;

        // Use a transaction to atomically determine version and insert
        // This prevents race conditions where concurrent aggregations get the same version
        let mut tx = self.pool.begin().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to start transaction for schema creation".to_string(),
        })?;

        // Determine next version number within the transaction
        let version_result = sqlx::query(
            "SELECT COALESCE(MAX(version), 0) + 1 as next_version
             FROM aggregated_api_schemas
             WHERE team = $1 AND path = $2 AND http_method = $3",
        )
        .bind(&request.team)
        .bind(&request.path)
        .bind(&request.http_method)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to determine next version number".to_string(),
        })?;

        let version: i64 = version_result.get("next_version");

        let result = sqlx::query(
            "INSERT INTO aggregated_api_schemas (
                team, path, http_method, version, previous_version_id,
                request_schema, response_schemas, request_headers, response_headers,
                sample_count, confidence_score,
                breaking_changes, first_observed, last_observed, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            RETURNING id",
        )
        .bind(&request.team)
        .bind(&request.path)
        .bind(&request.http_method)
        .bind(version)
        .bind(request.previous_version_id)
        .bind(&request_schema_json)
        .bind(&response_schemas_json)
        .bind(&request_headers_json)
        .bind(&response_headers_json)
        .bind(request.sample_count)
        .bind(request.confidence_score)
        .bind(&breaking_changes_json)
        .bind(request.first_observed)
        .bind(request.last_observed)
        .bind(now)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %request.team, "Failed to create aggregated schema");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create aggregated schema for team '{}'", request.team),
            }
        })?;

        let id: i64 = result.get("id");

        // Commit the transaction
        tx.commit().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to commit transaction for schema creation".to_string(),
        })?;

        tracing::info!(
            id = id,
            team = %request.team,
            path = %request.path,
            method = %request.http_method,
            version = version,
            "Created new aggregated schema"
        );

        self.get_by_id(id).await
    }

    /// Create multiple aggregated schemas in a single transaction
    ///
    /// This is atomic: either all schemas are created or none are.
    /// Used by session aggregation to ensure all-or-nothing semantics.
    #[instrument(skip(self, requests), fields(request_count = requests.len()), name = "db_create_aggregated_schemas_batch")]
    pub async fn create_batch(
        &self,
        requests: Vec<CreateAggregatedSchemaRequest>,
    ) -> Result<Vec<i64>> {
        if requests.is_empty() {
            return Ok(vec![]);
        }

        let now = chrono::Utc::now();

        // Start a single transaction for all inserts
        let mut tx = self.pool.begin().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to start transaction for batch schema creation".to_string(),
        })?;

        let mut created_ids = Vec::with_capacity(requests.len());

        for request in requests {
            let request_schema_json =
                request.request_schema.as_ref().map(serde_json::to_string).transpose().map_err(
                    |e| FlowplaneError::validation(format!("Invalid request_schema: {}", e)),
                )?;

            let response_schemas_json =
                request.response_schemas.as_ref().map(serde_json::to_string).transpose().map_err(
                    |e| FlowplaneError::validation(format!("Invalid response_schemas: {}", e)),
                )?;

            let breaking_changes_json =
                request.breaking_changes.as_ref().map(serde_json::to_string).transpose().map_err(
                    |e| FlowplaneError::validation(format!("Invalid breaking_changes: {}", e)),
                )?;

            let request_headers_json =
                request.request_headers.as_ref().map(serde_json::to_string).transpose().map_err(
                    |e| FlowplaneError::validation(format!("Invalid request_headers: {}", e)),
                )?;

            let response_headers_json =
                request.response_headers.as_ref().map(serde_json::to_string).transpose().map_err(
                    |e| FlowplaneError::validation(format!("Invalid response_headers: {}", e)),
                )?;

            // Determine next version number within the transaction
            let version_result = sqlx::query(
                "SELECT COALESCE(MAX(version), 0) + 1 as next_version
                 FROM aggregated_api_schemas
                 WHERE team = $1 AND path = $2 AND http_method = $3",
            )
            .bind(&request.team)
            .bind(&request.path)
            .bind(&request.http_method)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to determine next version number".to_string(),
            })?;

            let version: i64 = version_result.get("next_version");

            let result = sqlx::query(
                "INSERT INTO aggregated_api_schemas (
                    team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, request_headers, response_headers,
                    sample_count, confidence_score,
                    breaking_changes, first_observed, last_observed, created_at, updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
                RETURNING id",
            )
            .bind(&request.team)
            .bind(&request.path)
            .bind(&request.http_method)
            .bind(version)
            .bind(request.previous_version_id)
            .bind(&request_schema_json)
            .bind(&response_schemas_json)
            .bind(&request_headers_json)
            .bind(&response_headers_json)
            .bind(request.sample_count)
            .bind(request.confidence_score)
            .bind(&breaking_changes_json)
            .bind(request.first_observed)
            .bind(request.last_observed)
            .bind(now)
            .bind(now)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, team = %request.team, "Failed to create aggregated schema in batch");
                FlowplaneError::Database {
                    source: e,
                    context: format!(
                        "Failed to create aggregated schema for team '{}' path '{}'",
                        request.team, request.path
                    ),
                }
            })?;

            let id: i64 = result.get("id");
            created_ids.push(id);

            tracing::debug!(
                id = id,
                team = %request.team,
                path = %request.path,
                method = %request.http_method,
                version = version,
                "Created aggregated schema in batch"
            );
        }

        // Commit the transaction - all or nothing
        tx.commit().await.map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to commit transaction for batch schema creation".to_string(),
        })?;

        tracing::info!(
            created_count = created_ids.len(),
            "Successfully created batch of aggregated schemas"
        );

        Ok(created_ids)
    }

    /// Get aggregated schema by ID
    #[instrument(skip(self), fields(id = %id), name = "db_get_aggregated_schema_by_id")]
    pub async fn get_by_id(&self, id: i64) -> Result<AggregatedSchemaData> {
        let row = sqlx::query_as::<sqlx::Postgres, AggregatedSchemaRow>(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, request_headers, response_headers,
                    sample_count, confidence_score,
                    breaking_changes, first_observed, last_observed, created_at, updated_at
             FROM aggregated_api_schemas WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, id = %id, "Failed to get aggregated schema by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get aggregated schema with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => row.try_into(),
            None => Err(FlowplaneError::not_found_msg(format!(
                "Aggregated schema with ID '{}' not found",
                id
            ))),
        }
    }

    /// Get multiple aggregated schemas by IDs
    ///
    /// Returns schemas ordered by path and http_method for consistent output.
    /// Returns only the schemas that exist; caller should verify expected count.
    #[instrument(skip(self), fields(ids_count = ids.len()), name = "db_get_aggregated_schemas_by_ids")]
    pub async fn get_by_ids(&self, ids: &[i64]) -> Result<Vec<AggregatedSchemaData>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        // Build dynamic IN clause with positional placeholders
        let placeholders: String = ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let query = format!(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, request_headers, response_headers,
                    sample_count, confidence_score,
                    breaking_changes, first_observed, last_observed, created_at, updated_at
             FROM aggregated_api_schemas
             WHERE id IN ({})
             ORDER BY path, http_method",
            placeholders
        );

        let mut query_builder = sqlx::query_as::<sqlx::Postgres, AggregatedSchemaRow>(&query);
        for id in ids {
            query_builder = query_builder.bind(id);
        }

        let rows = query_builder.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, ids_count = ids.len(), "Failed to get aggregated schemas by IDs");
            FlowplaneError::Database {
                source: e,
                context: "Failed to get aggregated schemas by IDs".to_string(),
            }
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// Get latest aggregated schema for a specific endpoint and team
    #[instrument(skip(self), fields(team = %team, path = %path, method = %http_method), name = "db_get_latest_aggregated_schema")]
    pub async fn get_latest(
        &self,
        team: &str,
        path: &str,
        http_method: &str,
    ) -> Result<Option<AggregatedSchemaData>> {
        let row = sqlx::query_as::<sqlx::Postgres, AggregatedSchemaRow>(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, request_headers, response_headers,
                    sample_count, confidence_score,
                    breaking_changes, first_observed, last_observed, created_at, updated_at
             FROM aggregated_api_schemas
             WHERE team = $1 AND path = $2 AND http_method = $3
             ORDER BY version DESC
             LIMIT 1",
        )
        .bind(team)
        .bind(path)
        .bind(http_method)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, path = %path, method = %http_method, "Failed to get latest aggregated schema");
            FlowplaneError::Database {
                source: e,
                context: "Failed to get latest aggregated schema for endpoint".to_string(),
            }
        })?;

        row.map(|r| r.try_into()).transpose()
    }

    /// List all aggregated schemas for a team
    #[instrument(skip(self), fields(team = %team), name = "db_list_aggregated_schemas_by_team")]
    pub async fn list_by_team(&self, team: &str) -> Result<Vec<AggregatedSchemaData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, AggregatedSchemaRow>(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, request_headers, response_headers,
                    sample_count, confidence_score,
                    breaking_changes, first_observed, last_observed, created_at, updated_at
             FROM aggregated_api_schemas
             WHERE team = $1
             ORDER BY created_at DESC",
        )
        .bind(team)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list aggregated schemas");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list aggregated schemas for team '{}'", team),
            }
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// List latest versions only for a team
    #[instrument(skip(self), fields(team = %team), name = "db_list_latest_aggregated_schemas")]
    pub async fn list_latest_by_team(&self, team: &str) -> Result<Vec<AggregatedSchemaData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, AggregatedSchemaRow>(
            "SELECT a.id, a.team, a.path, a.http_method, a.version, a.previous_version_id,
                    a.request_schema, a.response_schemas, a.request_headers, a.response_headers,
                    a.sample_count, a.confidence_score,
                    a.breaking_changes, a.first_observed, a.last_observed, a.created_at, a.updated_at
             FROM aggregated_api_schemas a
             INNER JOIN (
                 SELECT team, path, http_method, MAX(version) as max_version
                 FROM aggregated_api_schemas
                 WHERE team = $1
                 GROUP BY team, path, http_method
             ) latest
             ON a.team = latest.team
                AND a.path = latest.path
                AND a.http_method = latest.http_method
                AND a.version = latest.max_version
             ORDER BY a.created_at DESC",
        )
        .bind(team)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list latest aggregated schemas");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list latest aggregated schemas for team '{}'", team),
            }
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// Get version history for a specific endpoint
    #[instrument(skip(self), fields(team = %team, path = %path, method = %http_method), name = "db_get_schema_version_history")]
    pub async fn get_version_history(
        &self,
        team: &str,
        path: &str,
        http_method: &str,
    ) -> Result<Vec<AggregatedSchemaData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, AggregatedSchemaRow>(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, request_headers, response_headers,
                    sample_count, confidence_score,
                    breaking_changes, first_observed, last_observed, created_at, updated_at
             FROM aggregated_api_schemas
             WHERE team = $1 AND path = $2 AND http_method = $3
             ORDER BY version DESC",
        )
        .bind(team)
        .bind(path)
        .bind(http_method)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, path = %path, method = %http_method, "Failed to get version history");
            FlowplaneError::Database {
                source: e,
                context: "Failed to get version history for endpoint".to_string(),
            }
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// Get a specific version of a schema
    #[instrument(skip(self), fields(team = %team, path = %path, method = %http_method, version = %version), name = "db_get_schema_by_version")]
    pub async fn get_by_version(
        &self,
        team: &str,
        path: &str,
        http_method: &str,
        version: i64,
    ) -> Result<Option<AggregatedSchemaData>> {
        let row = sqlx::query_as::<sqlx::Postgres, AggregatedSchemaRow>(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, request_headers, response_headers,
                    sample_count, confidence_score,
                    breaking_changes, first_observed, last_observed, created_at, updated_at
             FROM aggregated_api_schemas
             WHERE team = $1 AND path = $2 AND http_method = $3 AND version = $4",
        )
        .bind(team)
        .bind(path)
        .bind(http_method)
        .bind(version)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, path = %path, method = %http_method, version = %version, "Failed to get schema by version");
            FlowplaneError::Database {
                source: e,
                context: "Failed to get schema by version".to_string(),
            }
        })?;

        row.map(|r| r.try_into()).transpose()
    }

    /// List aggregated schemas with filters
    #[instrument(skip(self), fields(team = %team), name = "db_list_aggregated_schemas_filtered")]
    pub async fn list_filtered(
        &self,
        team: &str,
        path_search: Option<&str>,
        http_method: Option<&str>,
        min_confidence: Option<f64>,
    ) -> Result<Vec<AggregatedSchemaData>> {
        let mut query = String::from(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, request_headers, response_headers,
                    sample_count, confidence_score,
                    breaking_changes, first_observed, last_observed, created_at, updated_at
             FROM aggregated_api_schemas
             WHERE team = $1",
        );

        let mut bind_count = 1;

        if path_search.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND path LIKE ${}", bind_count));
        }

        if http_method.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND http_method = ${}", bind_count));
        }

        if min_confidence.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND confidence_score >= ${}", bind_count));
        }

        query.push_str(" ORDER BY created_at DESC");

        let mut query_builder =
            sqlx::query_as::<sqlx::Postgres, AggregatedSchemaRow>(&query).bind(team);

        if let Some(search) = path_search {
            query_builder = query_builder.bind(format!("%{}%", search));
        }

        if let Some(method) = http_method {
            query_builder = query_builder.bind(method);
        }

        if let Some(confidence) = min_confidence {
            query_builder = query_builder.bind(confidence);
        }

        let rows = query_builder.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list filtered aggregated schemas");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list filtered aggregated schemas for team '{}'", team),
            }
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::{TestDatabase, TEST_TEAM_ID};

    #[tokio::test]
    async fn test_create_and_get_aggregated_schema() {
        let _db = TestDatabase::new("agg_schema_create_get").await;
        let pool = _db.pool.clone();
        let repo = AggregatedSchemaRepository::new(pool);

        let request = CreateAggregatedSchemaRequest {
            team: TEST_TEAM_ID.to_string(),
            path: "/users/{id}".to_string(),
            http_method: "GET".to_string(),
            request_schema: None,
            response_schemas: Some(serde_json::json!({
                "200": {"type": "object", "properties": {"id": {"type": "integer"}}}
            })),
            request_headers: None,
            response_headers: None,
            sample_count: 10,
            confidence_score: 0.95,
            breaking_changes: None,
            first_observed: chrono::Utc::now(),
            last_observed: chrono::Utc::now(),
            previous_version_id: None,
        };

        let created = repo.create(request).await.unwrap();

        assert_eq!(created.team, TEST_TEAM_ID);
        assert_eq!(created.path, "/users/{id}");
        assert_eq!(created.http_method, "GET");
        assert_eq!(created.version, 1);
        assert_eq!(created.sample_count, 10);
        assert_eq!(created.confidence_score, 0.95);

        let retrieved = repo.get_by_id(created.id).await.unwrap();
        assert_eq!(retrieved.id, created.id);
        assert_eq!(retrieved.team, created.team);
    }

    #[tokio::test]
    async fn test_version_increment() {
        let _db = TestDatabase::new("agg_schema_version_inc").await;
        let pool = _db.pool.clone();
        let repo = AggregatedSchemaRepository::new(pool);

        let now = chrono::Utc::now();

        // Create version 1
        let request1 = CreateAggregatedSchemaRequest {
            team: TEST_TEAM_ID.to_string(),
            path: "/users".to_string(),
            http_method: "POST".to_string(),
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: None,
            request_headers: None,
            response_headers: None,
            sample_count: 5,
            confidence_score: 0.8,
            breaking_changes: None,
            first_observed: now,
            last_observed: now,
            previous_version_id: None,
        };

        let v1 = repo.create(request1).await.unwrap();
        assert_eq!(v1.version, 1);

        // Create version 2
        let request2 = CreateAggregatedSchemaRequest {
            team: TEST_TEAM_ID.to_string(),
            path: "/users".to_string(),
            http_method: "POST".to_string(),
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: None,
            request_headers: None,
            response_headers: None,
            sample_count: 10,
            confidence_score: 0.9,
            breaking_changes: Some(vec![serde_json::json!({"type": "field_added"})]),
            first_observed: now,
            last_observed: now,
            previous_version_id: Some(v1.id),
        };

        let v2 = repo.create(request2).await.unwrap();
        assert_eq!(v2.version, 2);
        assert_eq!(v2.previous_version_id, Some(v1.id));
    }

    #[tokio::test]
    async fn test_get_latest() {
        let _db = TestDatabase::new("agg_schema_get_latest").await;
        let pool = _db.pool.clone();
        let repo = AggregatedSchemaRepository::new(pool);

        let now = chrono::Utc::now();

        // Create multiple versions
        for i in 1..=3 {
            let request = CreateAggregatedSchemaRequest {
                team: TEST_TEAM_ID.to_string(),
                path: "/products".to_string(),
                http_method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(serde_json::json!({"version": i})),
                request_headers: None,
                response_headers: None,
                sample_count: i * 5,
                confidence_score: 0.8,
                breaking_changes: None,
                first_observed: now,
                last_observed: now,
                previous_version_id: None,
            };

            repo.create(request).await.unwrap();
        }

        let latest = repo.get_latest(TEST_TEAM_ID, "/products", "GET").await.unwrap().unwrap();

        assert_eq!(latest.version, 3);
        assert_eq!(latest.sample_count, 15);
    }

    #[tokio::test]
    async fn test_list_latest_by_team() {
        let _db = TestDatabase::new("agg_schema_list_latest").await;
        let pool = _db.pool.clone();
        let repo = AggregatedSchemaRepository::new(pool);

        let now = chrono::Utc::now();

        // Create multiple endpoints with multiple versions
        for path in &["/users", "/products", "/orders"] {
            for version in 1..=2 {
                let request = CreateAggregatedSchemaRequest {
                    team: TEST_TEAM_ID.to_string(),
                    path: path.to_string(),
                    http_method: "GET".to_string(),
                    request_schema: None,
                    response_schemas: Some(serde_json::json!({"version": version})),
                    request_headers: None,
                    response_headers: None,
                    sample_count: version * 5,
                    confidence_score: 0.8,
                    breaking_changes: None,
                    first_observed: now,
                    last_observed: now,
                    previous_version_id: None,
                };

                repo.create(request).await.unwrap();
            }
        }

        let latest = repo.list_latest_by_team(TEST_TEAM_ID).await.unwrap();

        // Should get 3 endpoints, each at version 2
        assert_eq!(latest.len(), 3);
        for schema in latest {
            assert_eq!(schema.version, 2);
            assert_eq!(schema.sample_count, 10);
        }
    }

    #[tokio::test]
    async fn test_get_by_ids_returns_all_requested() {
        let _db = TestDatabase::new("agg_schema_get_by_ids").await;
        let pool = _db.pool.clone();
        let repo = AggregatedSchemaRepository::new(pool);

        let now = chrono::Utc::now();

        // Create 3 schemas
        let mut ids: Vec<i64> = Vec::new();
        for path in ["/users", "/products", "/orders"] {
            let request = CreateAggregatedSchemaRequest {
                team: TEST_TEAM_ID.to_string(),
                path: path.to_string(),
                http_method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(serde_json::json!({"type": "object"})),
                request_headers: None,
                response_headers: None,
                sample_count: 10,
                confidence_score: 0.9,
                breaking_changes: None,
                first_observed: now,
                last_observed: now,
                previous_version_id: None,
            };
            let schema = repo.create(request).await.unwrap();
            ids.push(schema.id);
        }

        // Fetch all three
        let result = repo.get_by_ids(&ids).await.unwrap();
        assert_eq!(result.len(), 3);

        // Fetch first two
        let result = repo.get_by_ids(&ids[0..2]).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn test_get_by_ids_empty_returns_empty() {
        let _db = TestDatabase::new("agg_schema_get_by_ids_empty").await;
        let pool = _db.pool.clone();
        let repo = AggregatedSchemaRepository::new(pool);

        let result = repo.get_by_ids(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_get_by_ids_orders_by_path() {
        let _db = TestDatabase::new("agg_schema_get_by_ids_order").await;
        let pool = _db.pool.clone();
        let repo = AggregatedSchemaRepository::new(pool);

        let now = chrono::Utc::now();

        // Create schemas in non-alphabetical order
        let mut ids: Vec<i64> = Vec::new();
        for path in ["/zebra", "/apple", "/mango"] {
            let request = CreateAggregatedSchemaRequest {
                team: TEST_TEAM_ID.to_string(),
                path: path.to_string(),
                http_method: "GET".to_string(),
                request_schema: None,
                response_schemas: None,
                request_headers: None,
                response_headers: None,
                sample_count: 5,
                confidence_score: 0.8,
                breaking_changes: None,
                first_observed: now,
                last_observed: now,
                previous_version_id: None,
            };
            let schema = repo.create(request).await.unwrap();
            ids.push(schema.id);
        }

        // Fetch all and verify order
        let result = repo.get_by_ids(&ids).await.unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].path, "/apple");
        assert_eq!(result[1].path, "/mango");
        assert_eq!(result[2].path, "/zebra");
    }

    #[tokio::test]
    async fn test_get_by_ids_partial_match() {
        let _db = TestDatabase::new("agg_schema_get_by_ids_partial").await;
        let pool = _db.pool.clone();
        let repo = AggregatedSchemaRepository::new(pool);

        let now = chrono::Utc::now();

        let request = CreateAggregatedSchemaRequest {
            team: TEST_TEAM_ID.to_string(),
            path: "/test".to_string(),
            http_method: "GET".to_string(),
            request_schema: None,
            response_schemas: None,
            request_headers: None,
            response_headers: None,
            sample_count: 5,
            confidence_score: 0.8,
            breaking_changes: None,
            first_observed: now,
            last_observed: now,
            previous_version_id: None,
        };

        let schema = repo.create(request).await.unwrap();

        // Request existing + non-existing ID
        let result = repo.get_by_ids(&[schema.id, 99999]).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, schema.id);
    }
}
