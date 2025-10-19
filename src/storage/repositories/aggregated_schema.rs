// ! Aggregated API schema repository for managing consensus schemas
//!
//! This module provides CRUD operations for aggregated schema resources, handling storage,
//! retrieval, and versioning of aggregated API schemas derived from multiple observations.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, Sqlite};
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

/// Request to create a new aggregated schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAggregatedSchemaRequest {
    pub team: String,
    pub path: String,
    pub http_method: String,
    pub request_schema: Option<serde_json::Value>,
    pub response_schemas: Option<serde_json::Value>,
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
    #[instrument(skip(self, request), fields(team = %request.team, path = %request.path, method = %request.http_method), name = "db_create_aggregated_schema")]
    pub async fn create(
        &self,
        request: CreateAggregatedSchemaRequest,
    ) -> Result<AggregatedSchemaData> {
        let now = chrono::Utc::now();

        // Determine next version number
        let version =
            self.get_next_version(&request.team, &request.path, &request.http_method).await?;

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

        let result = sqlx::query(
            "INSERT INTO aggregated_api_schemas (
                team, path, http_method, version, previous_version_id,
                request_schema, response_schemas, sample_count, confidence_score,
                breaking_changes, first_observed, last_observed, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            RETURNING id",
        )
        .bind(&request.team)
        .bind(&request.path)
        .bind(&request.http_method)
        .bind(version)
        .bind(request.previous_version_id)
        .bind(&request_schema_json)
        .bind(&response_schemas_json)
        .bind(request.sample_count)
        .bind(request.confidence_score)
        .bind(&breaking_changes_json)
        .bind(request.first_observed)
        .bind(request.last_observed)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %request.team, "Failed to create aggregated schema");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create aggregated schema for team '{}'", request.team),
            }
        })?;

        let id: i64 = result.get("id");

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

    /// Get aggregated schema by ID
    #[instrument(skip(self), fields(id = %id), name = "db_get_aggregated_schema_by_id")]
    pub async fn get_by_id(&self, id: i64) -> Result<AggregatedSchemaData> {
        let row = sqlx::query_as::<Sqlite, AggregatedSchemaRow>(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, sample_count, confidence_score,
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

    /// Get latest aggregated schema for a specific endpoint and team
    #[instrument(skip(self), fields(team = %team, path = %path, method = %http_method), name = "db_get_latest_aggregated_schema")]
    pub async fn get_latest(
        &self,
        team: &str,
        path: &str,
        http_method: &str,
    ) -> Result<Option<AggregatedSchemaData>> {
        let row = sqlx::query_as::<Sqlite, AggregatedSchemaRow>(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, sample_count, confidence_score,
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
                context: format!("Failed to get latest aggregated schema for endpoint"),
            }
        })?;

        row.map(|r| r.try_into()).transpose()
    }

    /// List all aggregated schemas for a team
    #[instrument(skip(self), fields(team = %team), name = "db_list_aggregated_schemas_by_team")]
    pub async fn list_by_team(&self, team: &str) -> Result<Vec<AggregatedSchemaData>> {
        let rows = sqlx::query_as::<Sqlite, AggregatedSchemaRow>(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, sample_count, confidence_score,
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
        let rows = sqlx::query_as::<Sqlite, AggregatedSchemaRow>(
            "SELECT a.id, a.team, a.path, a.http_method, a.version, a.previous_version_id,
                    a.request_schema, a.response_schemas, a.sample_count, a.confidence_score,
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
        let rows = sqlx::query_as::<Sqlite, AggregatedSchemaRow>(
            "SELECT id, team, path, http_method, version, previous_version_id,
                    request_schema, response_schemas, sample_count, confidence_score,
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

    /// Get next version number for an endpoint
    async fn get_next_version(&self, team: &str, path: &str, http_method: &str) -> Result<i64> {
        let result = sqlx::query(
            "SELECT COALESCE(MAX(version), 0) + 1 as next_version
             FROM aggregated_api_schemas
             WHERE team = $1 AND path = $2 AND http_method = $3",
        )
        .bind(team)
        .bind(path)
        .bind(http_method)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to determine next version number".to_string(),
        })?;

        Ok(result.get("next_version"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn setup_test_db() -> DbPool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        pool
    }

    #[tokio::test]
    async fn test_create_and_get_aggregated_schema() {
        let pool = setup_test_db().await;
        let repo = AggregatedSchemaRepository::new(pool);

        let request = CreateAggregatedSchemaRequest {
            team: "test-team".to_string(),
            path: "/users/{id}".to_string(),
            http_method: "GET".to_string(),
            request_schema: None,
            response_schemas: Some(serde_json::json!({
                "200": {"type": "object", "properties": {"id": {"type": "integer"}}}
            })),
            sample_count: 10,
            confidence_score: 0.95,
            breaking_changes: None,
            first_observed: chrono::Utc::now(),
            last_observed: chrono::Utc::now(),
            previous_version_id: None,
        };

        let created = repo.create(request).await.unwrap();

        assert_eq!(created.team, "test-team");
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
        let pool = setup_test_db().await;
        let repo = AggregatedSchemaRepository::new(pool);

        let now = chrono::Utc::now();

        // Create version 1
        let request1 = CreateAggregatedSchemaRequest {
            team: "test-team".to_string(),
            path: "/users".to_string(),
            http_method: "POST".to_string(),
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: None,
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
            team: "test-team".to_string(),
            path: "/users".to_string(),
            http_method: "POST".to_string(),
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: None,
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
        let pool = setup_test_db().await;
        let repo = AggregatedSchemaRepository::new(pool);

        let now = chrono::Utc::now();

        // Create multiple versions
        for i in 1..=3 {
            let request = CreateAggregatedSchemaRequest {
                team: "test-team".to_string(),
                path: "/products".to_string(),
                http_method: "GET".to_string(),
                request_schema: None,
                response_schemas: Some(serde_json::json!({"version": i})),
                sample_count: i * 5,
                confidence_score: 0.8,
                breaking_changes: None,
                first_observed: now,
                last_observed: now,
                previous_version_id: None,
            };

            repo.create(request).await.unwrap();
        }

        let latest = repo.get_latest("test-team", "/products", "GET").await.unwrap().unwrap();

        assert_eq!(latest.version, 3);
        assert_eq!(latest.sample_count, 15);
    }

    #[tokio::test]
    async fn test_list_latest_by_team() {
        let pool = setup_test_db().await;
        let repo = AggregatedSchemaRepository::new(pool);

        let now = chrono::Utc::now();

        // Create multiple endpoints with multiple versions
        for path in &["/users", "/products", "/orders"] {
            for version in 1..=2 {
                let request = CreateAggregatedSchemaRequest {
                    team: "test-team".to_string(),
                    path: path.to_string(),
                    http_method: "GET".to_string(),
                    request_schema: None,
                    response_schemas: Some(serde_json::json!({"version": version})),
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

        let latest = repo.list_latest_by_team("test-team").await.unwrap();

        // Should get 3 endpoints, each at version 2
        assert_eq!(latest.len(), 3);
        for schema in latest {
            assert_eq!(schema.version, 2);
            assert_eq!(schema.sample_count, 10);
        }
    }
}
