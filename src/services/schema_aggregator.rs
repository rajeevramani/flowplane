//! Schema aggregation service for combining multiple schema observations
//!
//! This module implements the core aggregation logic that combines multiple individual
//! schema observations (from `inferred_schemas`) into consensus schemas (stored in
//! `aggregated_api_schemas`).
//!
//! ## Aggregation Pipeline
//!
//! 1. **Subtask 6.1**: Group observations by endpoint (method, path, status_code)
//! 2. **Subtask 6.2**: Track field presence and determine required fields
//! 3. **Subtask 6.3**: Resolve type conflicts using union types
//! 4. **Subtask 6.4**: Calculate confidence scores
//! 5. **Subtask 6.5**: Detect breaking changes from previous versions

use crate::errors::{FlowplaneError, Result};
use crate::storage::repositories::{
    AggregatedSchemaRepository, CreateAggregatedSchemaRequest, InferredSchemaData,
    InferredSchemaRepository,
};
use std::collections::HashMap;
use tracing::{info, instrument};

/// Schema aggregation service
pub struct SchemaAggregator {
    inferred_repo: InferredSchemaRepository,
    aggregated_repo: AggregatedSchemaRepository,
}

impl SchemaAggregator {
    /// Create a new schema aggregator
    pub fn new(
        inferred_repo: InferredSchemaRepository,
        aggregated_repo: AggregatedSchemaRepository,
    ) -> Self {
        Self { inferred_repo, aggregated_repo }
    }

    /// Aggregate all schemas for a learning session
    ///
    /// This is the main entry point for Task 6.1 - it groups observations by endpoint
    /// and creates aggregated schemas.
    ///
    /// Later subtasks will enhance the aggregation logic with field presence tracking,
    /// type conflict resolution, confidence scoring, and breaking change detection.
    #[instrument(skip(self), fields(session_id = %session_id), name = "aggregate_session_schemas")]
    pub async fn aggregate_session(&self, session_id: &str) -> Result<Vec<i64>> {
        info!(session_id = %session_id, "Starting schema aggregation for session");

        // Step 1: Fetch all inferred schemas for this session, grouped by endpoint
        let grouped_schemas = self.inferred_repo.list_by_session_grouped(session_id).await?;

        info!(
            session_id = %session_id,
            endpoint_count = grouped_schemas.len(),
            "Grouped schemas by endpoint"
        );

        let mut aggregated_ids = Vec::new();

        // Step 2: For each group (endpoint), aggregate the observations
        for ((http_method, path_pattern, response_status_code), observations) in grouped_schemas {
            info!(
                method = %http_method,
                path = %path_pattern,
                status_code = ?response_status_code,
                observation_count = observations.len(),
                "Aggregating endpoint"
            );

            let aggregated_id = self
                .aggregate_endpoint(&http_method, &path_pattern, response_status_code, observations)
                .await?;

            aggregated_ids.push(aggregated_id);
        }

        info!(
            session_id = %session_id,
            aggregated_count = aggregated_ids.len(),
            "Completed schema aggregation for session"
        );

        Ok(aggregated_ids)
    }

    /// Aggregate observations for a single endpoint
    ///
    /// **Current implementation (Task 6.1):** Basic aggregation
    /// - Combine observations by endpoint
    /// - Count total samples
    /// - Use first non-null schema as representative
    ///
    /// **Future enhancements:**
    /// - Task 6.2: Field presence tracking
    /// - Task 6.3: Type conflict resolution
    /// - Task 6.4: Confidence scoring
    /// - Task 6.5: Breaking change detection
    #[instrument(skip(self, observations), fields(method = %http_method, path = %path_pattern), name = "aggregate_endpoint")]
    async fn aggregate_endpoint(
        &self,
        http_method: &str,
        path_pattern: &str,
        response_status_code: Option<i64>,
        observations: Vec<InferredSchemaData>,
    ) -> Result<i64> {
        if observations.is_empty() {
            return Err(FlowplaneError::validation("Cannot aggregate empty observation set"));
        }

        // Extract team from first observation (all should have same team)
        let team = &observations[0].team;

        // Task 6.1: Basic aggregation logic
        // Just combine observations and count them

        // Aggregate request schemas (take first non-null)
        let request_schema = observations.iter().find_map(|obs| obs.request_schema.clone());

        // Aggregate response schemas by status code
        // For now, create a simple map of {status_code: schema}
        let mut response_schemas_map = HashMap::new();
        if let Some(status) = response_status_code {
            // Find first non-null response schema
            if let Some(obs) = observations.iter().find(|o| o.response_schema.is_some()) {
                response_schemas_map
                    .insert(status.to_string(), obs.response_schema.clone().unwrap());
            }
        }

        let response_schemas = if response_schemas_map.is_empty() {
            None
        } else {
            Some(serde_json::to_value(response_schemas_map).map_err(|e| {
                FlowplaneError::validation(format!("Failed to serialize response_schemas: {}", e))
            })?)
        };

        // Calculate sample count
        let sample_count = observations.len() as i64;

        // Calculate time range
        let first_observed = observations.iter().map(|obs| obs.first_seen_at).min().unwrap(); // Safe because we checked observations is not empty

        let last_observed = observations.iter().map(|obs| obs.last_seen_at).max().unwrap();

        // Task 6.4 will implement proper confidence scoring
        // For now, use a simple heuristic: more samples = higher confidence
        let confidence_score = calculate_simple_confidence(sample_count);

        // Task 6.5 will implement breaking change detection
        // For now, check if there's a previous version
        let previous_version =
            self.aggregated_repo.get_latest(team, path_pattern, http_method).await?;

        let previous_version_id = previous_version.as_ref().map(|v| v.id);

        // Create aggregated schema
        let request = CreateAggregatedSchemaRequest {
            team: team.clone(),
            path: path_pattern.to_string(),
            http_method: http_method.to_string(),
            request_schema,
            response_schemas,
            sample_count,
            confidence_score,
            breaking_changes: None, // Task 6.5 will implement this
            first_observed,
            last_observed,
            previous_version_id,
        };

        let aggregated = self.aggregated_repo.create(request).await?;

        info!(
            aggregated_id = aggregated.id,
            method = %http_method,
            path = %path_pattern,
            version = aggregated.version,
            sample_count = sample_count,
            confidence = confidence_score,
            "Created aggregated schema"
        );

        Ok(aggregated.id)
    }
}

/// Simple confidence calculation based on sample size
///
/// This is a placeholder for Task 6.4's comprehensive confidence scoring.
/// Current logic:
/// - 1-5 samples: 0.5 confidence
/// - 6-20 samples: 0.7 confidence
/// - 21-50 samples: 0.85 confidence
/// - 51+ samples: 0.95 confidence
fn calculate_simple_confidence(sample_count: i64) -> f64 {
    match sample_count {
        1..=5 => 0.5,
        6..=20 => 0.7,
        21..=50 => 0.85,
        _ => 0.95,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::repositories::{AggregatedSchemaRepository, InferredSchemaRepository};
    use sqlx::SqlitePool;

    async fn setup_test_db() -> sqlx::Pool<sqlx::Sqlite> {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        pool
    }

    async fn create_test_session(pool: &sqlx::Pool<sqlx::Sqlite>) -> String {
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

    async fn insert_test_observation(
        pool: &sqlx::Pool<sqlx::Sqlite>,
        session_id: &str,
        method: &str,
        path: &str,
        status_code: Option<i64>,
        request_schema: Option<&str>,
        response_schema: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO inferred_schemas (
                team, session_id, http_method, path_pattern, response_status_code,
                request_schema, response_schema,
                sample_count, confidence, first_seen_at, last_seen_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind("test-team")
        .bind(session_id)
        .bind(method)
        .bind(path)
        .bind(status_code)
        .bind(request_schema)
        .bind(response_schema)
        .bind(1)
        .bind(1.0)
        .bind(chrono::Utc::now())
        .bind(chrono::Utc::now())
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_aggregate_single_endpoint() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Insert multiple observations of same endpoint
        let response_schema = r#"{"type": "object", "properties": {"id": {"type": "integer"}}}"#;

        for _ in 0..3 {
            insert_test_observation(
                &pool,
                &session_id,
                "GET",
                "/users/{id}",
                Some(200),
                None,
                Some(response_schema),
            )
            .await;
        }

        // Create aggregator and run aggregation
        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();

        assert_eq!(ids.len(), 1, "Should create 1 aggregated schema");

        // Verify aggregated schema
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        assert_eq!(aggregated.team, "test-team");
        assert_eq!(aggregated.http_method, "GET");
        assert_eq!(aggregated.path, "/users/{id}");
        assert_eq!(aggregated.sample_count, 3);
        assert_eq!(aggregated.confidence_score, 0.5); // 3 samples = 0.5 confidence
        assert_eq!(aggregated.version, 1);
        assert!(aggregated.response_schemas.is_some());
    }

    #[tokio::test]
    async fn test_aggregate_multiple_endpoints() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Insert observations for different endpoints
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/users/{id}",
            Some(200),
            None,
            Some(r#"{"type": "object"}"#),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/users/{id}",
            Some(404),
            None,
            Some(r#"{"type": "object"}"#),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "POST",
            "/users",
            Some(201),
            Some(r#"{"type": "object"}"#),
            Some(r#"{"type": "object"}"#),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();

        // Should create 3 aggregated schemas: GET /users/{id} 200, GET /users/{id} 404, POST /users 201
        assert_eq!(ids.len(), 3);
    }

    #[tokio::test]
    async fn test_confidence_scoring() {
        assert_eq!(calculate_simple_confidence(1), 0.5);
        assert_eq!(calculate_simple_confidence(5), 0.5);
        assert_eq!(calculate_simple_confidence(10), 0.7);
        assert_eq!(calculate_simple_confidence(30), 0.85);
        assert_eq!(calculate_simple_confidence(100), 0.95);
    }

    #[tokio::test]
    async fn test_version_tracking() {
        let pool = setup_test_db().await;

        // Create first session and aggregate
        let session1 = create_test_session(&pool).await;
        insert_test_observation(
            &pool,
            &session1,
            "GET",
            "/products",
            Some(200),
            None,
            Some(r#"{"type": "object"}"#),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo.clone(), aggregated_repo.clone());

        let ids1 = aggregator.aggregate_session(&session1).await.unwrap();
        let v1 = aggregated_repo.get_by_id(ids1[0]).await.unwrap();

        assert_eq!(v1.version, 1);
        assert!(v1.previous_version_id.is_none());

        // Create second session and aggregate - should create version 2
        let session2 = create_test_session(&pool).await;
        insert_test_observation(
            &pool,
            &session2,
            "GET",
            "/products",
            Some(200),
            None,
            Some(r#"{"type": "object"}"#),
        )
        .await;

        let ids2 = aggregator.aggregate_session(&session2).await.unwrap();
        let v2 = aggregated_repo.get_by_id(ids2[0]).await.unwrap();

        assert_eq!(v2.version, 2);
        assert_eq!(v2.previous_version_id, Some(v1.id));
    }
}
