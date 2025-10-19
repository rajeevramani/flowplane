//! Inferred schema repository for managing individual schema observations
//!
//! This module provides CRUD operations for inferred schema resources, handling storage
//! and retrieval of individual schema observations from learning sessions.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, Sqlite};
use std::collections::HashMap;
use tracing::instrument;

/// Type alias for endpoint grouping key: (http_method, path_pattern, status_code)
type EndpointKey = (String, String, Option<i64>);

/// Type alias for grouped schemas map
type GroupedSchemas = HashMap<EndpointKey, Vec<InferredSchemaData>>;

/// Database row structure for inferred schemas
#[derive(Debug, Clone, FromRow)]
struct InferredSchemaRow {
    pub id: i64,
    pub team: String,
    pub session_id: String,
    pub http_method: String,
    pub path_pattern: String,
    pub request_schema: Option<String>,  // JSON Schema as string
    pub response_schema: Option<String>, // JSON Schema as string
    pub response_status_code: Option<i64>,
    pub sample_count: i64,
    pub confidence: f64,
    pub first_seen_at: chrono::DateTime<chrono::Utc>,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Inferred schema data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferredSchemaData {
    pub id: i64,
    pub team: String,
    pub session_id: String,
    pub http_method: String,
    pub path_pattern: String,
    pub request_schema: Option<serde_json::Value>,
    pub response_schema: Option<serde_json::Value>,
    pub response_status_code: Option<i64>,
    pub sample_count: i64,
    pub confidence: f64,
    pub first_seen_at: chrono::DateTime<chrono::Utc>,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<InferredSchemaRow> for InferredSchemaData {
    type Error = FlowplaneError;

    fn try_from(row: InferredSchemaRow) -> Result<Self> {
        let request_schema =
            row.request_schema.as_ref().map(|s| serde_json::from_str(s)).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid request_schema JSON: {}", e)),
            )?;

        let response_schema =
            row.response_schema.as_ref().map(|s| serde_json::from_str(s)).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid response_schema JSON: {}", e)),
            )?;

        Ok(Self {
            id: row.id,
            team: row.team,
            session_id: row.session_id,
            http_method: row.http_method,
            path_pattern: row.path_pattern,
            request_schema,
            response_schema,
            response_status_code: row.response_status_code,
            sample_count: row.sample_count,
            confidence: row.confidence,
            first_seen_at: row.first_seen_at,
            last_seen_at: row.last_seen_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Repository for inferred schema data access
#[derive(Debug, Clone)]
pub struct InferredSchemaRepository {
    pool: DbPool,
}

impl InferredSchemaRepository {
    /// Create a new inferred schema repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Get inferred schema by ID
    #[instrument(skip(self), fields(id = %id), name = "db_get_inferred_schema_by_id")]
    pub async fn get_by_id(&self, id: i64) -> Result<InferredSchemaData> {
        let row = sqlx::query_as::<Sqlite, InferredSchemaRow>(
            "SELECT id, team, session_id, http_method, path_pattern,
                    request_schema, response_schema, response_status_code,
                    sample_count, confidence, first_seen_at, last_seen_at,
                    created_at, updated_at
             FROM inferred_schemas WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, id = %id, "Failed to get inferred schema by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get inferred schema with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => row.try_into(),
            None => Err(FlowplaneError::not_found_msg(format!(
                "Inferred schema with ID '{}' not found",
                id
            ))),
        }
    }

    /// List all inferred schemas for a learning session
    #[instrument(skip(self), fields(session_id = %session_id), name = "db_list_inferred_schemas_by_session")]
    pub async fn list_by_session_id(&self, session_id: &str) -> Result<Vec<InferredSchemaData>> {
        let rows = sqlx::query_as::<Sqlite, InferredSchemaRow>(
            "SELECT id, team, session_id, http_method, path_pattern,
                    request_schema, response_schema, response_status_code,
                    sample_count, confidence, first_seen_at, last_seen_at,
                    created_at, updated_at
             FROM inferred_schemas
             WHERE session_id = $1
             ORDER BY created_at ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, session_id = %session_id, "Failed to list inferred schemas by session");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list inferred schemas for session '{}'", session_id),
            }
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// List inferred schemas for a session grouped by endpoint
    /// Returns a map of (method, path, status_code) -> Vec<InferredSchemaData>
    #[instrument(skip(self), fields(session_id = %session_id), name = "db_list_inferred_schemas_grouped")]
    pub async fn list_by_session_grouped(
        &self,
        session_id: &str,
    ) -> Result<GroupedSchemas>
    {
        let schemas = self.list_by_session_id(session_id).await?;

        let mut grouped = GroupedSchemas::new();

        for schema in schemas {
            let key = (
                schema.http_method.clone(),
                schema.path_pattern.clone(),
                schema.response_status_code,
            );

            grouped.entry(key).or_default().push(schema);
        }

        Ok(grouped)
    }

    /// List inferred schemas for a team
    #[instrument(skip(self), fields(team = %team), name = "db_list_inferred_schemas_by_team")]
    pub async fn list_by_team(&self, team: &str) -> Result<Vec<InferredSchemaData>> {
        let rows = sqlx::query_as::<Sqlite, InferredSchemaRow>(
            "SELECT id, team, session_id, http_method, path_pattern,
                    request_schema, response_schema, response_status_code,
                    sample_count, confidence, first_seen_at, last_seen_at,
                    created_at, updated_at
             FROM inferred_schemas
             WHERE team = $1
             ORDER BY created_at DESC",
        )
        .bind(team)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list inferred schemas by team");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list inferred schemas for team '{}'", team),
            }
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// List inferred schemas for a specific endpoint across all sessions
    #[instrument(skip(self), fields(team = %team, path = %path_pattern, method = %http_method), name = "db_list_inferred_schemas_by_endpoint")]
    pub async fn list_by_endpoint(
        &self,
        team: &str,
        path_pattern: &str,
        http_method: &str,
    ) -> Result<Vec<InferredSchemaData>> {
        let rows = sqlx::query_as::<Sqlite, InferredSchemaRow>(
            "SELECT id, team, session_id, http_method, path_pattern,
                    request_schema, response_schema, response_status_code,
                    sample_count, confidence, first_seen_at, last_seen_at,
                    created_at, updated_at
             FROM inferred_schemas
             WHERE team = $1 AND path_pattern = $2 AND http_method = $3
             ORDER BY created_at DESC",
        )
        .bind(team)
        .bind(path_pattern)
        .bind(http_method)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                team = %team,
                path = %path_pattern,
                method = %http_method,
                "Failed to list inferred schemas by endpoint"
            );
            FlowplaneError::Database {
                source: e,
                context: "Failed to list inferred schemas for endpoint".to_string(),
            }
        })?;

        rows.into_iter().map(|r| r.try_into()).collect()
    }

    /// Count inferred schemas for a session
    #[instrument(skip(self), fields(session_id = %session_id), name = "db_count_inferred_schemas")]
    pub async fn count_by_session(&self, session_id: &str) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM inferred_schemas WHERE session_id = $1")
            .bind(session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, session_id = %session_id, "Failed to count inferred schemas");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to count inferred schemas for session '{}'", session_id),
                }
            })?;

        Ok(row.get("count"))
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

    async fn create_test_session(pool: &DbPool) -> String {
        let session_id = uuid::Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO learning_sessions (
                id, team, route_pattern, status, target_sample_count, current_sample_count
            ) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&session_id)
        .bind("test-team")
        .bind("/test/*")
        .bind("active")
        .bind(100)
        .bind(0)
        .execute(pool)
        .await
        .unwrap();

        session_id
    }

    async fn insert_test_schema(
        pool: &DbPool,
        session_id: &str,
        method: &str,
        path: &str,
        status_code: Option<i64>,
    ) {
        sqlx::query(
            "INSERT INTO inferred_schemas (
                team, session_id, http_method, path_pattern, response_status_code,
                sample_count, confidence, first_seen_at, last_seen_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind("test-team")
        .bind(session_id)
        .bind(method)
        .bind(path)
        .bind(status_code)
        .bind(1)
        .bind(1.0)
        .bind(chrono::Utc::now())
        .bind(chrono::Utc::now())
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_list_by_session_id() {
        let pool = setup_test_db().await;
        let repo = InferredSchemaRepository::new(pool.clone());
        let session_id = create_test_session(&pool).await;

        // Insert test schemas
        insert_test_schema(&pool, &session_id, "GET", "/users/{id}", Some(200)).await;
        insert_test_schema(&pool, &session_id, "GET", "/users/{id}", Some(404)).await;
        insert_test_schema(&pool, &session_id, "POST", "/users", Some(201)).await;

        let schemas = repo.list_by_session_id(&session_id).await.unwrap();

        assert_eq!(schemas.len(), 3);
        assert_eq!(schemas[0].session_id, session_id);
        assert_eq!(schemas[0].team, "test-team");
    }

    #[tokio::test]
    async fn test_list_by_session_grouped() {
        let pool = setup_test_db().await;
        let repo = InferredSchemaRepository::new(pool.clone());
        let session_id = create_test_session(&pool).await;

        // Insert multiple observations of same endpoint
        insert_test_schema(&pool, &session_id, "GET", "/users/{id}", Some(200)).await;
        insert_test_schema(&pool, &session_id, "GET", "/users/{id}", Some(200)).await;
        insert_test_schema(&pool, &session_id, "GET", "/users/{id}", Some(200)).await;
        insert_test_schema(&pool, &session_id, "GET", "/users/{id}", Some(404)).await;
        insert_test_schema(&pool, &session_id, "POST", "/users", Some(201)).await;

        let grouped = repo.list_by_session_grouped(&session_id).await.unwrap();

        // Should have 3 groups: GET /users/{id} 200, GET /users/{id} 404, POST /users 201
        assert_eq!(grouped.len(), 3);

        let get_200_key = ("GET".to_string(), "/users/{id}".to_string(), Some(200));
        let get_200_schemas = grouped.get(&get_200_key).unwrap();
        assert_eq!(get_200_schemas.len(), 3);

        let get_404_key = ("GET".to_string(), "/users/{id}".to_string(), Some(404));
        let get_404_schemas = grouped.get(&get_404_key).unwrap();
        assert_eq!(get_404_schemas.len(), 1);
    }

    #[tokio::test]
    async fn test_count_by_session() {
        let pool = setup_test_db().await;
        let repo = InferredSchemaRepository::new(pool.clone());
        let session_id = create_test_session(&pool).await;

        // Insert test schemas
        insert_test_schema(&pool, &session_id, "GET", "/users/{id}", Some(200)).await;
        insert_test_schema(&pool, &session_id, "POST", "/users", Some(201)).await;

        let count = repo.count_by_session(&session_id).await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_list_by_endpoint() {
        let pool = setup_test_db().await;
        let repo = InferredSchemaRepository::new(pool.clone());

        let session1 = create_test_session(&pool).await;
        let session2 = create_test_session(&pool).await;

        // Insert schemas for same endpoint across different sessions
        insert_test_schema(&pool, &session1, "GET", "/products/{id}", Some(200)).await;
        insert_test_schema(&pool, &session2, "GET", "/products/{id}", Some(200)).await;
        insert_test_schema(&pool, &session1, "POST", "/products", Some(201)).await;

        let schemas = repo.list_by_endpoint("test-team", "/products/{id}", "GET").await.unwrap();

        assert_eq!(schemas.len(), 2);
        for schema in schemas {
            assert_eq!(schema.http_method, "GET");
            assert_eq!(schema.path_pattern, "/products/{id}");
        }
    }
}
