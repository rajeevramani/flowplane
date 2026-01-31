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
use crate::schema::InferredSchema;
use crate::services::schema_diff::detect_breaking_changes;
use crate::storage::repositories::{
    AggregatedSchemaRepository, CreateAggregatedSchemaRequest, InferredSchemaData,
    InferredSchemaRepository,
};
use std::collections::HashMap;
use tracing::{info, instrument, warn};

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
    /// The aggregation is atomic: either all endpoints are aggregated or none are.
    /// This is achieved by collecting all aggregation requests first, then batch
    /// creating them in a single transaction.
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

        // Step 2: For each group (endpoint), build the aggregation request
        // This phase is read-only - all writes happen in the batch create below
        let mut create_requests = Vec::new();

        for ((http_method, path_pattern, response_status_code), observations) in grouped_schemas {
            info!(
                method = %http_method,
                path = %path_pattern,
                status_code = ?response_status_code,
                observation_count = observations.len(),
                "Preparing endpoint aggregation"
            );

            let request = self
                .prepare_aggregation(
                    &http_method,
                    &path_pattern,
                    response_status_code,
                    observations,
                )
                .await?;

            create_requests.push(request);
        }

        // Step 3: Batch create all aggregated schemas in a single transaction
        // If any insert fails, the entire transaction is rolled back
        let aggregated_ids = self.aggregated_repo.create_batch(create_requests).await?;

        info!(
            session_id = %session_id,
            aggregated_count = aggregated_ids.len(),
            "Completed schema aggregation for session (atomic batch)"
        );

        Ok(aggregated_ids)
    }

    /// Prepare aggregation for a single endpoint
    ///
    /// This function performs all the read operations and schema merging,
    /// then returns a CreateAggregatedSchemaRequest that can be batch-inserted.
    ///
    /// **Implementation:**
    /// - Task 6.2: Field presence tracking
    /// - Task 6.3: Type conflict resolution
    /// - Task 6.4: Confidence scoring
    /// - Task 6.5: Breaking change detection
    #[instrument(skip(self, observations), fields(method = %http_method, path = %path_pattern), name = "prepare_aggregation")]
    async fn prepare_aggregation(
        &self,
        http_method: &str,
        path_pattern: &str,
        response_status_code: Option<i64>,
        observations: Vec<InferredSchemaData>,
    ) -> Result<CreateAggregatedSchemaRequest> {
        if observations.is_empty() {
            return Err(FlowplaneError::validation("Cannot aggregate empty observation set"));
        }

        // Extract team from first observation (all should have same team)
        let team = &observations[0].team;

        // Task 6.2: Merge schemas and track field presence
        // Use InferredSchema::merge() to properly combine observations

        // Aggregate request schemas by merging all observations
        let request_schema = merge_schemas(&observations, |obs| obs.request_schema.as_ref())?;

        // Aggregate response schemas by status code
        let mut response_schemas_map = HashMap::new();
        if let Some(status) = response_status_code {
            // Merge all response schemas for this status code
            let response_schema = merge_schemas(&observations, |obs| obs.response_schema.as_ref())?;

            if let Some(schema) = response_schema {
                response_schemas_map.insert(status.to_string(), schema);
            }
        }

        // Calculate sample count
        let sample_count = observations.len() as i64;

        // Calculate time range - guard verified by observations.is_empty() check above
        let first_observed =
            observations.iter().map(|obs| obs.first_seen_at).min().ok_or_else(|| {
                FlowplaneError::internal("Cannot compute min on empty observations")
            })?;

        let last_observed =
            observations.iter().map(|obs| obs.last_seen_at).max().ok_or_else(|| {
                FlowplaneError::internal("Cannot compute max on empty observations")
            })?;

        // Task 6.4: Calculate comprehensive confidence score (before serializing response_schemas_map)
        let confidence_score =
            calculate_confidence_score(sample_count, &request_schema, &response_schemas_map);

        // Serialize response schemas (this consumes response_schemas_map)
        let response_schemas = if response_schemas_map.is_empty() {
            None
        } else {
            Some(serde_json::to_value(response_schemas_map).map_err(|e| {
                FlowplaneError::validation(format!("Failed to serialize response_schemas: {}", e))
            })?)
        };

        // Task 6.5: Detect breaking changes from previous version
        let previous_version =
            self.aggregated_repo.get_latest(team, path_pattern, http_method).await?;

        let previous_version_id = previous_version.as_ref().map(|v| v.id);

        // Detect breaking changes if there's a previous version
        let breaking_changes = if let Some(ref prev) = previous_version {
            detect_schema_breaking_changes(
                &prev.request_schema,
                &request_schema,
                &prev.response_schemas,
                &response_schemas,
            )
        } else {
            None
        };

        let has_breaking_changes = breaking_changes.is_some();

        info!(
            method = %http_method,
            path = %path_pattern,
            sample_count = sample_count,
            confidence = confidence_score,
            has_breaking_changes = has_breaking_changes,
            "Prepared aggregation request for endpoint"
        );

        // Return the request (actual DB write happens in batch)
        Ok(CreateAggregatedSchemaRequest {
            team: team.clone(),
            path: path_pattern.to_string(),
            http_method: http_method.to_string(),
            request_schema,
            response_schemas,
            sample_count,
            confidence_score,
            breaking_changes,
            first_observed,
            last_observed,
            previous_version_id,
        })
    }
}

/// Merge multiple schema observations into a single aggregated schema
///
/// This function implements Task 6.2's field presence tracking by:
/// 1. Extracting schemas from observations using the provided accessor
/// 2. Merging them using InferredSchema::merge() which tracks field presence
/// 3. Counting actual field presence across observations
/// 4. Calculating required fields based on 100% presence threshold
///
/// After merging, the schema's `required` field is populated with fields
/// that appear in 100% of observations.
fn merge_schemas<F>(
    observations: &[InferredSchemaData],
    schema_accessor: F,
) -> Result<Option<serde_json::Value>>
where
    F: Fn(&InferredSchemaData) -> Option<&serde_json::Value> + Copy,
{
    // Collect all non-null schemas
    let schemas: Vec<_> = observations.iter().filter_map(&schema_accessor).collect();

    if schemas.is_empty() {
        return Ok(None);
    }

    // Parse first schema as InferredSchema
    let first_json = schemas[0];
    let mut merged: InferredSchema = serde_json::from_value(first_json.clone()).map_err(|e| {
        warn!(error = %e, "Failed to parse first schema as InferredSchema, using raw JSON");
        // If parsing fails, just return the first schema as-is
        FlowplaneError::validation(format!("Failed to parse schema: {}", e))
    })?;

    // Track total observations for presence calculation
    let total_observations = observations.len();

    // Merge remaining schemas
    for schema_json in schemas.iter().skip(1) {
        let other: InferredSchema =
            serde_json::from_value((*schema_json).clone()).map_err(|e| {
                FlowplaneError::validation(format!("Failed to parse schema for merging: {}", e))
            })?;

        merged.merge(&other);
    }

    // Fix field-level stats: Count actual field presence across all observations
    fix_field_stats_with_observations(&mut merged, observations, schema_accessor);

    // Calculate required fields based on 100% presence
    // For object schemas, determine which fields are required
    calculate_required_fields(&mut merged, total_observations);

    // Convert merged schema to JSON value
    let result = serde_json::to_value(&merged).map_err(|e| {
        FlowplaneError::validation(format!("Failed to serialize merged schema: {}", e))
    })?;

    Ok(Some(result))
}

/// Fix field-level stats after merging
///
/// The SchemaInferenceEngine doesn't set field-level stats, and the merge operation
/// doesn't properly track field presence. We need to count how many times each field
/// appears by looking at all the original observations.
///
/// This function properly traverses nested objects and counts actual field presence
/// at each level of nesting.
fn fix_field_stats_with_observations<F>(
    schema: &mut InferredSchema,
    observations: &[InferredSchemaData],
    schema_accessor: F,
) where
    F: Fn(&InferredSchemaData) -> Option<&serde_json::Value> + Copy,
{
    let total_observations = observations.len();

    // Extract JSON values from observations for recursive processing
    let obs_json_values: Vec<serde_json::Value> =
        observations.iter().filter_map(|obs| schema_accessor(obs).cloned()).collect();

    fix_field_stats_recursive(schema, &obs_json_values, total_observations);
}

/// Recursively fix stats for all fields including nested objects
///
/// This properly counts presence for nested fields by traversing into
/// the nested structure of each observation.
fn fix_field_stats_recursive(
    schema: &mut InferredSchema,
    obs_schemas: &[serde_json::Value],
    total_observations: usize,
) {
    if let Some(ref mut properties) = schema.properties {
        for (field_name, field_schema) in properties.iter_mut() {
            // Count how many observations have this field at the current level
            let mut presence_count = 0u64;
            let mut nested_obs_values: Vec<serde_json::Value> = Vec::new();

            for obs_json in obs_schemas {
                // Try to find this field in the observation
                if let Some(obs_props) = obs_json.get("properties").and_then(|p| p.as_object()) {
                    if let Some(obs_field) = obs_props.get(field_name) {
                        presence_count += 1;

                        // If this is a nested object, collect the nested schema for recursive processing
                        if field_schema.properties.is_some() {
                            nested_obs_values.push(obs_field.clone());
                        }
                    }
                }
            }

            // Update field stats
            field_schema.stats.sample_count = total_observations as u64;
            field_schema.stats.presence_count = presence_count;
            field_schema.stats.confidence = if total_observations > 0 {
                presence_count as f64 / total_observations as f64
            } else {
                0.0
            };

            // Recursively fix nested objects using only observations that have this field
            if field_schema.properties.is_some() && !nested_obs_values.is_empty() {
                // For nested objects, the total observations count is how many times
                // the parent field was present (not the overall total)
                fix_field_stats_recursive(
                    field_schema,
                    &nested_obs_values,
                    presence_count as usize,
                );
            }
        }
    }
}

/// Calculate required fields based on field presence across observations
///
/// Task 6.2: A field is marked as required if it appears in 100% of observations.
/// This function recursively processes object schemas and their nested properties.
///
/// The threshold is set to 1.0 (100%) - fields must be present in ALL samples
/// to be considered required.
///
/// NOTE: We use the object-level sample_count (which represents the total number
/// of observations) and compare each field's presence_count against it. The field-level
/// sample_count may not accurately reflect the total observations when fields are optional.
fn calculate_required_fields(schema: &mut InferredSchema, total_observations: usize) {
    const REQUIRED_THRESHOLD: f64 = 1.0; // 100% presence

    // Only process object schemas
    if let Some(ref mut properties) = schema.properties {
        let mut required_fields = Vec::new();

        for (field_name, field_schema) in properties.iter_mut() {
            // Recursively process nested objects, passing the total_observations
            // For nested objects, we use the parent's sample count
            let nested_total = if field_schema.properties.is_some() {
                field_schema.stats.sample_count as usize
            } else {
                total_observations
            };
            calculate_required_fields(field_schema, nested_total);

            // Check if this field is required (100% presence)
            // A field is required if presence_count == total_observations
            let field_presence_ratio =
                field_schema.stats.presence_count as f64 / total_observations as f64;

            if field_presence_ratio >= REQUIRED_THRESHOLD {
                required_fields.push(field_name.clone());
            }
        }

        // Sort required fields for consistency
        required_fields.sort();

        // Set required fields (or None if empty)
        schema.required = if required_fields.is_empty() { None } else { Some(required_fields) };
    }

    // Process array item schemas recursively
    if let Some(ref mut items) = schema.items {
        calculate_required_fields(items, total_observations);
    }
}

/// Calculate comprehensive confidence score for aggregated schema
///
/// Task 6.4: Confidence score based on multiple factors:
/// 1. Sample size (40% weight) - More samples = higher confidence
/// 2. Field consistency (40% weight) - All fields present in all samples = higher confidence
/// 3. Type stability (20% weight) - No type conflicts = higher confidence
///
/// Returns a score between 0.0 and 1.0
fn calculate_confidence_score(
    sample_count: i64,
    request_schema: &Option<serde_json::Value>,
    response_schemas: &HashMap<String, serde_json::Value>,
) -> f64 {
    // Component 1: Sample size score (40% weight)
    let sample_score = calculate_sample_size_score(sample_count);

    // Component 2: Field consistency score (40% weight)
    let field_score = calculate_field_consistency_score(request_schema, response_schemas);

    // Component 3: Type stability score (20% weight)
    let type_score = calculate_type_stability_score(request_schema, response_schemas);

    // Weighted average
    let confidence = (sample_score * 0.4) + (field_score * 0.4) + (type_score * 0.2);

    // Clamp to [0.0, 1.0]
    confidence.clamp(0.0, 1.0)
}

/// Calculate score based on sample size
/// Uses logarithmic scale to reward more samples with diminishing returns
fn calculate_sample_size_score(sample_count: i64) -> f64 {
    if sample_count <= 0 {
        return 0.0;
    }

    // Logarithmic scaling: ln(n) / ln(100)
    // 1 sample: 0.0
    // 5 samples: ~0.35
    // 10 samples: 0.5
    // 20 samples: ~0.65
    // 50 samples: ~0.85
    // 100 samples: 1.0
    let log_score = (sample_count as f64).ln() / (100.0_f64).ln();
    log_score.clamp(0.0, 1.0)
}

/// Calculate score based on field consistency
/// Checks what percentage of fields are required (100% presence)
fn calculate_field_consistency_score(
    request_schema: &Option<serde_json::Value>,
    response_schemas: &HashMap<String, serde_json::Value>,
) -> f64 {
    let mut total_fields = 0;
    let mut required_fields = 0;

    // Check request schema
    if let Some(schema) = request_schema {
        count_field_consistency(schema, &mut total_fields, &mut required_fields);
    }

    // Check all response schemas
    for schema in response_schemas.values() {
        count_field_consistency(schema, &mut total_fields, &mut required_fields);
    }

    if total_fields == 0 {
        return 1.0; // No fields means perfect consistency
    }

    required_fields as f64 / total_fields as f64
}

/// Helper to count total fields and required fields in a schema
fn count_field_consistency(schema: &serde_json::Value, total: &mut usize, required: &mut usize) {
    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
        for (field_name, field_schema) in properties {
            *total += 1;

            // Check if field is in required array
            if let Some(req_array) = schema.get("required").and_then(|r| r.as_array()) {
                if req_array.iter().any(|r| r.as_str() == Some(field_name)) {
                    *required += 1;
                }
            }

            // Recursively check nested objects
            if field_schema.get("type").and_then(|t| t.as_str()) == Some("object") {
                count_field_consistency(field_schema, total, required);
            }
        }
    }
}

/// Calculate score based on type stability (inverse of type conflicts)
/// Checks how many fields have oneOf types (type conflicts)
fn calculate_type_stability_score(
    request_schema: &Option<serde_json::Value>,
    response_schemas: &HashMap<String, serde_json::Value>,
) -> f64 {
    let mut total_fields = 0;
    let mut stable_fields = 0;

    // Check request schema
    if let Some(schema) = request_schema {
        count_type_stability(schema, &mut total_fields, &mut stable_fields);
    }

    // Check all response schemas
    for schema in response_schemas.values() {
        count_type_stability(schema, &mut total_fields, &mut stable_fields);
    }

    if total_fields == 0 {
        return 1.0; // No fields means perfect stability
    }

    stable_fields as f64 / total_fields as f64
}

/// Helper to count total fields and stable-type fields
fn count_type_stability(schema: &serde_json::Value, total: &mut usize, stable: &mut usize) {
    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
        for (_field_name, field_schema) in properties {
            *total += 1;

            // Check if type is stable (not a oneOf)
            if let Some(type_val) = field_schema.get("type") {
                let has_conflict = type_val.is_object() && type_val.get("oneof").is_some();

                if !has_conflict {
                    *stable += 1;
                }
            } else {
                // No type field means stable
                *stable += 1;
            }

            // Recursively check nested objects
            if let Some(type_str) = field_schema.get("type").and_then(|t| t.as_str()) {
                if type_str == "object" {
                    count_type_stability(field_schema, total, stable);
                }
            }
        }
    }
}

/// Legacy simple confidence calculation (kept for backward compatibility in tests)
#[allow(dead_code)]
fn calculate_simple_confidence(sample_count: i64) -> f64 {
    match sample_count {
        1..=5 => 0.5,
        6..=20 => 0.7,
        21..=50 => 0.85,
        _ => 0.95,
    }
}

/// Detect breaking changes between old and new schemas (both request and response)
///
/// Task 6.5: Compare request schemas and all response schemas to detect breaking changes.
/// Returns a Vec of BreakingChange objects if any breaking changes are found, or None if schemas are compatible.
fn detect_schema_breaking_changes(
    old_request: &Option<serde_json::Value>,
    new_request: &Option<serde_json::Value>,
    old_responses: &Option<serde_json::Value>,
    new_responses: &Option<serde_json::Value>,
) -> Option<Vec<serde_json::Value>> {
    let mut all_changes = Vec::new();

    // Compare request schemas
    if let (Some(old_req), Some(new_req)) = (old_request, new_request) {
        let diff = detect_breaking_changes(old_req, new_req);
        for change in diff.breaking_changes {
            // Prefix path with "request" to indicate it's in the request schema
            match serde_json::to_value(&change) {
                Ok(mut change_json) => {
                    if let Some(path) = change_json.get_mut("path") {
                        if let Some(path_str) = path.as_str() {
                            *path = serde_json::Value::String(format!("request{}", path_str));
                        }
                    }
                    all_changes.push(change_json);
                }
                Err(e) => {
                    warn!(error = %e, "Failed to serialize breaking change for request schema");
                }
            }
        }
    } else if old_request.is_some() && new_request.is_none() {
        // Request body was removed - this could be breaking
        warn!("Request body was removed from schema");
    } else if old_request.is_none() && new_request.is_some() {
        // Request body was added - non-breaking
        info!("Request body was added to schema");
    }

    // Compare response schemas by status code
    if let (Some(old_resp), Some(new_resp)) = (old_responses, new_responses) {
        if let (Some(old_map), Some(new_map)) = (old_resp.as_object(), new_resp.as_object()) {
            // Check each status code in old responses
            for (status_code, old_schema) in old_map {
                if let Some(new_schema) = new_map.get(status_code) {
                    // Status code exists in both - compare schemas
                    let diff = detect_breaking_changes(old_schema, new_schema);
                    for change in diff.breaking_changes {
                        // Prefix path with "response[status]" to indicate location
                        match serde_json::to_value(&change) {
                            Ok(mut change_json) => {
                                if let Some(path) = change_json.get_mut("path") {
                                    if let Some(path_str) = path.as_str() {
                                        *path = serde_json::Value::String(format!(
                                            "response[{}]{}",
                                            status_code, path_str
                                        ));
                                    }
                                }
                                all_changes.push(change_json);
                            }
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    status_code = %status_code,
                                    "Failed to serialize breaking change for response schema"
                                );
                            }
                        }
                    }
                } else {
                    // Status code was removed - potentially breaking
                    warn!(status_code = %status_code, "Response status code removed from schema");
                }
            }

            // Check for new status codes (non-breaking)
            for status_code in new_map.keys() {
                if !old_map.contains_key(status_code) {
                    info!(status_code = %status_code, "New response status code added to schema");
                }
            }
        }
    }

    if all_changes.is_empty() {
        None
    } else {
        info!(breaking_change_count = all_changes.len(), "Detected breaking changes in schema");
        Some(all_changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaInferenceEngine;
    use crate::storage::repositories::{AggregatedSchemaRepository, InferredSchemaRepository};
    use sqlx::SqlitePool;

    async fn setup_test_db() -> sqlx::Pool<sqlx::Sqlite> {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        pool
    }

    /// Helper to create a test team in the database (idempotent)
    async fn create_test_team(pool: &sqlx::Pool<sqlx::Sqlite>, name: &str) {
        sqlx::query(
            "INSERT OR IGNORE INTO teams (id, name, display_name, status) VALUES (?, ?, ?, 'active')"
        )
        .bind(format!("team-{}", uuid::Uuid::new_v4()))
        .bind(name)
        .bind(format!("Test {}", name))
        .execute(pool)
        .await
        .unwrap();
    }

    async fn create_test_session(pool: &sqlx::Pool<sqlx::Sqlite>) -> String {
        let session_id = uuid::Uuid::new_v4().to_string();

        // Create the team first
        create_test_team(pool, "test-team").await;

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

    /// Helper to create proper InferredSchema JSON from a serde_json::Value
    /// This uses the actual schema inference engine to ensure correct format
    fn infer_schema_json(value: &serde_json::Value) -> String {
        let engine = SchemaInferenceEngine::new();
        let schema = engine.infer_from_value(value).unwrap();
        serde_json::to_string(&schema).unwrap()
    }

    #[tokio::test]
    async fn test_aggregate_single_endpoint() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Insert multiple observations of same endpoint
        let response_value = serde_json::json!({"id": 1, "name": "Test"});
        let response_schema = infer_schema_json(&response_value);

        for _ in 0..3 {
            insert_test_observation(
                &pool,
                &session_id,
                "GET",
                "/users/{id}",
                Some(200),
                None,
                Some(&response_schema),
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
        // Comprehensive confidence: sample=0.239, field=1.0, type=1.0
        // (0.239 * 0.4) + (1.0 * 0.4) + (1.0 * 0.2) = 0.6954
        assert!((aggregated.confidence_score - 0.695).abs() < 0.01);
        assert_eq!(aggregated.version, 1);
        assert!(aggregated.response_schemas.is_some());
    }

    #[tokio::test]
    async fn test_aggregate_multiple_endpoints() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Insert observations for different endpoints
        let schema_get = infer_schema_json(&serde_json::json!({"id": 1}));
        let schema_error = infer_schema_json(&serde_json::json!({"error": "Not found"}));
        let schema_post = infer_schema_json(&serde_json::json!({"name": "New User"}));

        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/users/{id}",
            Some(200),
            None,
            Some(&schema_get),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/users/{id}",
            Some(404),
            None,
            Some(&schema_error),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "POST",
            "/users",
            Some(201),
            Some(&schema_post),
            Some(&schema_get),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();

        // Should create 3 aggregated schemas: GET /users/{id} 200, GET /users/{id} 404, POST /users 201
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn test_sample_size_scoring() {
        // Test logarithmic scaling: ln(n) / ln(100)
        assert_eq!(calculate_sample_size_score(1), 0.0); // ln(1) = 0
        assert!((calculate_sample_size_score(5) - 0.35).abs() < 0.01); // ~0.35 for 5 samples
        assert_eq!(calculate_sample_size_score(10), 0.5); // ln(10) / ln(100) = 0.5
        assert!((calculate_sample_size_score(20) - 0.65).abs() < 0.01); // ~0.65 for 20 samples
        assert!((calculate_sample_size_score(50) - 0.85).abs() < 0.01); // ~0.85 for 50 samples
        assert_eq!(calculate_sample_size_score(100), 1.0); // ln(100) / ln(100) = 1.0
        assert_eq!(calculate_sample_size_score(0), 0.0); // 0 for no samples
    }

    #[test]
    fn test_field_consistency_perfect() {
        // Schema with all fields required (100% presence)
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            },
            "required": ["id", "name"]
        });

        let mut response_map = HashMap::new();
        response_map.insert("200".to_string(), schema);

        let score = calculate_field_consistency_score(&None, &response_map);
        assert_eq!(score, 1.0); // Perfect consistency
    }

    #[test]
    fn test_field_consistency_partial() {
        // Schema with some fields required, some optional
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"},
                "email": {"type": "string"}
            },
            "required": ["id"] // Only 1 out of 3 required
        });

        let mut response_map = HashMap::new();
        response_map.insert("200".to_string(), schema);

        let score = calculate_field_consistency_score(&None, &response_map);
        assert!((score - 0.333).abs() < 0.01); // ~33% required
    }

    #[test]
    fn test_type_stability_perfect() {
        // Schema with no type conflicts (all stable types)
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            }
        });

        let mut response_map = HashMap::new();
        response_map.insert("200".to_string(), schema);

        let score = calculate_type_stability_score(&None, &response_map);
        assert_eq!(score, 1.0); // Perfect stability
    }

    #[test]
    fn test_type_stability_with_conflicts() {
        // Schema with type conflicts (oneOf)
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "value": {
                    "type": {
                        "oneof": ["string", "integer"]
                    }
                }
            }
        });

        let mut response_map = HashMap::new();
        response_map.insert("200".to_string(), schema);

        let score = calculate_type_stability_score(&None, &response_map);
        assert_eq!(score, 0.5); // 1 stable out of 2 fields = 50%
    }

    #[test]
    fn test_comprehensive_confidence_high() {
        // Perfect scenario: many samples, all fields required, no conflicts
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            },
            "required": ["id", "name"]
        });

        let mut response_map = HashMap::new();
        response_map.insert("200".to_string(), schema);

        let score = calculate_confidence_score(100, &None, &response_map);
        // Should be very high: sample_score(100)=~0.92, field=1.0, type=1.0
        // (0.92 * 0.4) + (1.0 * 0.4) + (1.0 * 0.2) = 0.368 + 0.4 + 0.2 = 0.968
        assert!(score > 0.95);
    }

    #[test]
    fn test_comprehensive_confidence_low() {
        // Poor scenario: few samples, few required fields, type conflicts
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "value": {
                    "type": {
                        "oneof": ["string", "integer", "boolean"]
                    }
                }
            },
            "required": []
        });

        let mut response_map = HashMap::new();
        response_map.insert("200".to_string(), schema);

        let score = calculate_confidence_score(1, &None, &response_map);
        // Should be low: sample_score(1)=~0.0, field=0.0, type=0.0
        assert!(score < 0.1);
    }

    #[test]
    fn test_comprehensive_confidence_medium() {
        // Medium scenario: moderate samples, some required fields, no conflicts
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"},
                "email": {"type": "string"}
            },
            "required": ["id"] // 1 out of 3
        });

        let mut response_map = HashMap::new();
        response_map.insert("200".to_string(), schema);

        let score = calculate_confidence_score(10, &None, &response_map);
        // sample_score(10)=0.5, field=0.33, type=1.0
        // (0.5 * 0.4) + (0.33 * 0.4) + (1.0 * 0.2) = 0.2 + 0.132 + 0.2 = 0.532
        assert!(score > 0.50 && score < 0.56);
    }

    #[tokio::test]
    async fn test_version_tracking() {
        let pool = setup_test_db().await;

        let schema = infer_schema_json(&serde_json::json!({"id": 1, "name": "Product"}));

        // Create first session and aggregate
        let session1 = create_test_session(&pool).await;
        insert_test_observation(
            &pool,
            &session1,
            "GET",
            "/products",
            Some(200),
            None,
            Some(&schema),
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
            Some(&schema),
        )
        .await;

        let ids2 = aggregator.aggregate_session(&session2).await.unwrap();
        let v2 = aggregated_repo.get_by_id(ids2[0]).await.unwrap();

        assert_eq!(v2.version, 2);
        assert_eq!(v2.previous_version_id, Some(v1.id));
    }

    // Task 6.2 Tests: Field Presence Tracking and Required Fields

    #[tokio::test]
    async fn test_field_presence_all_required() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Insert 3 observations where all have the same fields (id, name)
        let schema = infer_schema_json(&serde_json::json!({"id": 1, "name": "Alice"}));

        for _ in 1..=3 {
            insert_test_observation(
                &pool,
                &session_id,
                "GET",
                "/users",
                Some(200),
                None,
                Some(&schema),
            )
            .await;
        }

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        // Verify response schema has required fields
        let response_schemas = aggregated.response_schemas.unwrap();
        let schema_200 = response_schemas.get("200").unwrap();
        let required = schema_200.get("required");

        assert!(required.is_some(), "Should have required fields");
        let required_fields: Vec<String> =
            serde_json::from_value(required.unwrap().clone()).unwrap();

        // Both id and name should be required (100% presence)
        assert_eq!(required_fields.len(), 2);
        assert!(required_fields.contains(&"id".to_string()));
        assert!(required_fields.contains(&"name".to_string()));
    }

    #[tokio::test]
    async fn test_field_presence_optional_fields() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Observation 1: has id, name, email
        let schema1 = infer_schema_json(
            &serde_json::json!({"id": 1, "name": "Alice", "email": "alice@test.com"}),
        );

        // Observation 2 & 3: has id, name (no email)
        let schema2 = infer_schema_json(&serde_json::json!({"id": 2, "name": "Bob"}));

        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/users",
            Some(200),
            None,
            Some(&schema1),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/users",
            Some(200),
            None,
            Some(&schema2),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/users",
            Some(200),
            None,
            Some(&schema2),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        let response_schemas = aggregated.response_schemas.unwrap();
        let schema_200 = response_schemas.get("200").unwrap();

        // Check properties
        let properties = schema_200.get("properties").unwrap();
        assert!(properties.get("id").is_some());
        assert!(properties.get("name").is_some());
        assert!(properties.get("email").is_some()); // Should exist but optional

        // Check required fields - only id and name (100% presence)
        let required = schema_200.get("required");
        assert!(required.is_some());
        let required_fields: Vec<String> =
            serde_json::from_value(required.unwrap().clone()).unwrap();

        assert_eq!(required_fields.len(), 2);
        assert!(required_fields.contains(&"id".to_string()));
        assert!(required_fields.contains(&"name".to_string()));
        assert!(!required_fields.contains(&"email".to_string())); // email is optional (33% presence)
    }

    #[tokio::test]
    async fn test_nested_object_required_fields() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Nested object schema with profile.bio always present
        let schema1 = infer_schema_json(&serde_json::json!({
            "id": 1,
            "profile": {
                "age": 30,
                "bio": "Test bio 1"
            }
        }));

        let schema2 = infer_schema_json(&serde_json::json!({
            "id": 2,
            "profile": {
                "age": 25,
                "bio": "Test bio 2"
            }
        }));

        insert_test_observation(
            &pool,
            &session_id,
            "POST",
            "/users",
            Some(201),
            None,
            Some(&schema1),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "POST",
            "/users",
            Some(201),
            None,
            Some(&schema2),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        let response_schemas = aggregated.response_schemas.unwrap();
        let schema_201 = response_schemas.get("201").unwrap();

        // Top level required fields
        let required = schema_201.get("required");
        assert!(required.is_some());
        let required_fields: Vec<String> =
            serde_json::from_value(required.unwrap().clone()).unwrap();
        assert!(required_fields.contains(&"id".to_string()));
        assert!(required_fields.contains(&"profile".to_string()));

        // Nested profile required fields
        let profile = schema_201.get("properties").unwrap().get("profile").unwrap();
        let profile_required = profile.get("required");
        assert!(profile_required.is_some());
        let profile_required_fields: Vec<String> =
            serde_json::from_value(profile_required.unwrap().clone()).unwrap();
        assert!(profile_required_fields.contains(&"age".to_string()));
        assert!(profile_required_fields.contains(&"bio".to_string()));
    }

    #[tokio::test]
    async fn test_no_required_fields() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Each observation has different fields - none are 100% present
        let schema1 = infer_schema_json(&serde_json::json!({"field_a": "value_a"}));
        let schema2 = infer_schema_json(&serde_json::json!({"field_b": "value_b"}));
        let schema3 = infer_schema_json(&serde_json::json!({"field_c": "value_c"}));

        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/dynamic",
            Some(200),
            None,
            Some(&schema1),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/dynamic",
            Some(200),
            None,
            Some(&schema2),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/dynamic",
            Some(200),
            None,
            Some(&schema3),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        let response_schemas = aggregated.response_schemas.unwrap();
        let schema_200 = response_schemas.get("200").unwrap();

        // Should have all three fields in properties
        let properties = schema_200.get("properties").unwrap();
        assert!(properties.get("field_a").is_some());
        assert!(properties.get("field_b").is_some());
        assert!(properties.get("field_c").is_some());

        // But NO required fields (each only 33% present)
        let required = schema_200.get("required");
        // required should either be None or an empty array
        if let Some(req) = required {
            let required_fields: Vec<String> =
                serde_json::from_value(req.clone()).unwrap_or_default();
            assert_eq!(required_fields.len(), 0, "Should have no required fields");
        }
    }

    #[tokio::test]
    async fn test_type_conflict_resolution() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Observation 1: "value" field is a string
        let schema1 = infer_schema_json(&serde_json::json!({"id": 1, "value": "text"}));

        // Observation 2: "value" field is a number
        let schema2 = infer_schema_json(&serde_json::json!({"id": 2, "value": 42}));

        // Observation 3: "value" field is a boolean
        let schema3 = infer_schema_json(&serde_json::json!({"id": 3, "value": true}));

        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/dynamic",
            Some(200),
            None,
            Some(&schema1),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/dynamic",
            Some(200),
            None,
            Some(&schema2),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/dynamic",
            Some(200),
            None,
            Some(&schema3),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        let response_schemas = aggregated.response_schemas.unwrap();
        let schema_200 = response_schemas.get("200").unwrap();
        let properties = schema_200.get("properties").unwrap();

        // "id" should be integer (consistent across all observations)
        let id_field = properties.get("id").unwrap();
        assert_eq!(id_field.get("type").unwrap().as_str().unwrap(), "integer");

        // "value" should have oneOf with multiple types
        let value_field = properties.get("value").unwrap();

        // The type field should be an object with "oneof" key containing array of types
        let type_val = value_field.get("type").unwrap();
        assert!(type_val.is_object(), "Type should be an object for oneOf");

        let type_obj = type_val.as_object().unwrap();
        let oneof_types = type_obj.get("oneof").expect("Should have 'oneof' key");
        let types_array = oneof_types.as_array().unwrap();

        // Should have 3 different types: boolean, integer, string
        assert_eq!(types_array.len(), 3, "Should have 3 different types");

        // Verify all three types are present
        let type_names: Vec<String> =
            types_array.iter().map(|t| t.as_str().unwrap().to_string()).collect();

        assert!(type_names.contains(&"boolean".to_string()));
        assert!(type_names.contains(&"integer".to_string()));
        assert!(type_names.contains(&"string".to_string()));
    }

    #[tokio::test]
    async fn test_partial_type_conflicts() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Observation 1 & 2: "status" field is a string
        let schema1 = infer_schema_json(&serde_json::json!({"id": 1, "status": "active"}));
        let schema2 = infer_schema_json(&serde_json::json!({"id": 2, "status": "inactive"}));

        // Observation 3: "status" field is a number (conflict!)
        let schema3 = infer_schema_json(&serde_json::json!({"id": 3, "status": 1}));

        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/items",
            Some(200),
            None,
            Some(&schema1),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/items",
            Some(200),
            None,
            Some(&schema2),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/items",
            Some(200),
            None,
            Some(&schema3),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        let response_schemas = aggregated.response_schemas.unwrap();
        let schema_200 = response_schemas.get("200").unwrap();
        let properties = schema_200.get("properties").unwrap();

        // "id" should still be integer (no conflict)
        let id_field = properties.get("id").unwrap();
        assert_eq!(id_field.get("type").unwrap().as_str().unwrap(), "integer");

        // "status" should have oneOf with string and integer
        let status_field = properties.get("status").unwrap();
        let type_val = status_field.get("type").unwrap();
        let type_obj = type_val.as_object().unwrap();
        let oneof_types = type_obj.get("oneof").unwrap();
        let types_array = oneof_types.as_array().unwrap();

        // Should have 2 types: integer and string
        assert_eq!(types_array.len(), 2);

        let type_names: Vec<String> =
            types_array.iter().map(|t| t.as_str().unwrap().to_string()).collect();

        assert!(type_names.contains(&"integer".to_string()));
        assert!(type_names.contains(&"string".to_string()));

        // Verify field presence is correct (3/3)
        assert_eq!(status_field.get("sample_count").unwrap().as_u64().unwrap(), 3);
        assert_eq!(status_field.get("presence_count").unwrap().as_u64().unwrap(), 3);
    }

    #[tokio::test]
    async fn test_nested_type_conflicts() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Observation 1: nested "age" is integer
        let schema1 = infer_schema_json(&serde_json::json!({
            "user": {
                "name": "Alice",
                "age": 30
            }
        }));

        // Observation 2: nested "age" is string (conflict in nested field!)
        let schema2 = infer_schema_json(&serde_json::json!({
            "user": {
                "name": "Bob",
                "age": "25"
            }
        }));

        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/profile",
            Some(200),
            None,
            Some(&schema1),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/profile",
            Some(200),
            None,
            Some(&schema2),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        let response_schemas = aggregated.response_schemas.unwrap();
        let schema_200 = response_schemas.get("200").unwrap();
        let properties = schema_200.get("properties").unwrap();

        // Navigate to nested "user" object
        let user_field = properties.get("user").unwrap();
        assert_eq!(user_field.get("type").unwrap().as_str().unwrap(), "object");

        let user_props = user_field.get("properties").unwrap();

        // "name" should be string (no conflict)
        let name_field = user_props.get("name").unwrap();
        assert_eq!(name_field.get("type").unwrap().as_str().unwrap(), "string");

        // "age" should have oneOf with integer and string
        let age_field = user_props.get("age").unwrap();
        let type_val = age_field.get("type").unwrap();
        let type_obj = type_val.as_object().unwrap();
        let oneof_types = type_obj.get("oneof").unwrap();
        let types_array = oneof_types.as_array().unwrap();

        assert_eq!(types_array.len(), 2);

        let type_names: Vec<String> =
            types_array.iter().map(|t| t.as_str().unwrap().to_string()).collect();

        assert!(type_names.contains(&"integer".to_string()));
        assert!(type_names.contains(&"string".to_string()));
    }

    // Task 6.5 Tests: Breaking Change Detection

    #[tokio::test]
    async fn test_no_breaking_changes_on_first_version() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        let schema = infer_schema_json(&serde_json::json!({"id": 1, "name": "Test"}));

        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/items",
            Some(200),
            None,
            Some(&schema),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        // First version should have no breaking changes
        assert_eq!(aggregated.version, 1);
        assert!(aggregated.breaking_changes.is_none());
        assert!(aggregated.previous_version_id.is_none());
    }

    #[tokio::test]
    async fn test_detect_required_field_removed() {
        let pool = setup_test_db().await;

        // Version 1: Schema with id and name (both required)
        let session1 = create_test_session(&pool).await;
        let schema1 = infer_schema_json(&serde_json::json!({"id": 1, "name": "Alice"}));

        for _ in 0..3 {
            insert_test_observation(
                &pool,
                &session1,
                "GET",
                "/users",
                Some(200),
                None,
                Some(&schema1),
            )
            .await;
        }

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo.clone(), aggregated_repo.clone());

        let ids1 = aggregator.aggregate_session(&session1).await.unwrap();
        let v1 = aggregated_repo.get_by_id(ids1[0]).await.unwrap();

        assert_eq!(v1.version, 1);
        assert!(v1.breaking_changes.is_none());

        // Version 2: Schema with only id (name removed - BREAKING!)
        let session2 = create_test_session(&pool).await;
        let schema2 = infer_schema_json(&serde_json::json!({"id": 2}));

        for _ in 0..3 {
            insert_test_observation(
                &pool,
                &session2,
                "GET",
                "/users",
                Some(200),
                None,
                Some(&schema2),
            )
            .await;
        }

        let ids2 = aggregator.aggregate_session(&session2).await.unwrap();
        let v2 = aggregated_repo.get_by_id(ids2[0]).await.unwrap();

        assert_eq!(v2.version, 2);
        assert_eq!(v2.previous_version_id, Some(v1.id));
        assert!(v2.breaking_changes.is_some());

        let breaking_changes = v2.breaking_changes.unwrap();
        assert!(!breaking_changes.is_empty());

        // Verify breaking change details
        let change = &breaking_changes[0];
        assert_eq!(change["type"].as_str().unwrap(), "required_field_removed");
        assert!(change["path"].as_str().unwrap().contains("name"));
    }

    #[tokio::test]
    async fn test_detect_type_change() {
        let pool = setup_test_db().await;

        // Version 1: age is integer
        let session1 = create_test_session(&pool).await;
        let schema1 = infer_schema_json(&serde_json::json!({"id": 1, "age": 30}));

        for _ in 0..3 {
            insert_test_observation(
                &pool,
                &session1,
                "POST",
                "/profiles",
                Some(201),
                None,
                Some(&schema1),
            )
            .await;
        }

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo.clone(), aggregated_repo.clone());

        let ids1 = aggregator.aggregate_session(&session1).await.unwrap();
        let _v1 = aggregated_repo.get_by_id(ids1[0]).await.unwrap();

        // Version 2: age is string (type change - BREAKING!)
        let session2 = create_test_session(&pool).await;
        let schema2 = infer_schema_json(&serde_json::json!({"id": 2, "age": "30"}));

        for _ in 0..3 {
            insert_test_observation(
                &pool,
                &session2,
                "POST",
                "/profiles",
                Some(201),
                None,
                Some(&schema2),
            )
            .await;
        }

        let ids2 = aggregator.aggregate_session(&session2).await.unwrap();
        let v2 = aggregated_repo.get_by_id(ids2[0]).await.unwrap();

        assert_eq!(v2.version, 2);
        assert!(v2.breaking_changes.is_some());

        let breaking_changes = v2.breaking_changes.unwrap();
        assert!(!breaking_changes.is_empty());

        // Verify it's a type change
        let change = &breaking_changes
            .iter()
            .find(|c| c["type"].as_str() == Some("incompatible_type_change"));
        assert!(change.is_some());
    }

    #[tokio::test]
    async fn test_no_breaking_changes_on_compatible_change() {
        let pool = setup_test_db().await;

        // Version 1: Just id
        let session1 = create_test_session(&pool).await;
        let schema1 = infer_schema_json(&serde_json::json!({"id": 1}));

        for _ in 0..3 {
            insert_test_observation(
                &pool,
                &session1,
                "GET",
                "/products",
                Some(200),
                None,
                Some(&schema1),
            )
            .await;
        }

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo.clone(), aggregated_repo.clone());

        aggregator.aggregate_session(&session1).await.unwrap();

        // Version 2: Added optional field (non-breaking)
        let session2 = create_test_session(&pool).await;
        let schema2_a = infer_schema_json(&serde_json::json!({"id": 2}));
        let schema2_b = infer_schema_json(&serde_json::json!({"id": 3, "name": "Product"}));

        // Mix of with and without name - name is optional
        insert_test_observation(
            &pool,
            &session2,
            "GET",
            "/products",
            Some(200),
            None,
            Some(&schema2_a),
        )
        .await;
        insert_test_observation(
            &pool,
            &session2,
            "GET",
            "/products",
            Some(200),
            None,
            Some(&schema2_b),
        )
        .await;

        let ids2 = aggregator.aggregate_session(&session2).await.unwrap();
        let v2 = aggregated_repo.get_by_id(ids2[0]).await.unwrap();

        assert_eq!(v2.version, 2);
        // Adding optional field is non-breaking
        assert!(v2.breaking_changes.is_none() || v2.breaking_changes.as_ref().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_detect_field_became_required() {
        let pool = setup_test_db().await;

        // Version 1: email is optional (appears in some observations)
        let session1 = create_test_session(&pool).await;
        let schema1_a = infer_schema_json(&serde_json::json!({"id": 1, "email": "a@test.com"}));
        let schema1_b = infer_schema_json(&serde_json::json!({"id": 2}));

        insert_test_observation(
            &pool,
            &session1,
            "POST",
            "/users",
            Some(201),
            None,
            Some(&schema1_a),
        )
        .await;
        insert_test_observation(
            &pool,
            &session1,
            "POST",
            "/users",
            Some(201),
            None,
            Some(&schema1_b),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo.clone(), aggregated_repo.clone());

        aggregator.aggregate_session(&session1).await.unwrap();

        // Version 2: email is required (appears in all observations)
        let session2 = create_test_session(&pool).await;
        let schema2 = infer_schema_json(&serde_json::json!({"id": 3, "email": "b@test.com"}));

        for _ in 0..3 {
            insert_test_observation(
                &pool,
                &session2,
                "POST",
                "/users",
                Some(201),
                None,
                Some(&schema2),
            )
            .await;
        }

        let ids2 = aggregator.aggregate_session(&session2).await.unwrap();
        let v2 = aggregated_repo.get_by_id(ids2[0]).await.unwrap();

        assert_eq!(v2.version, 2);
        assert!(v2.breaking_changes.is_some());

        let breaking_changes = v2.breaking_changes.unwrap();
        assert!(!breaking_changes.is_empty());

        // Verify it's a field-became-required change
        let has_field_became_required =
            breaking_changes.iter().any(|c| c["type"].as_str() == Some("field_became_required"));
        assert!(has_field_became_required);
    }

    // Issue #4: Nested Object Field Stats Bug Test
    // Tests that nested fields have correct presence counts, not assumed 100%

    #[tokio::test]
    async fn test_nested_field_optional_presence() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Observation 1: user has name AND email
        let schema1 = infer_schema_json(&serde_json::json!({
            "user": {
                "name": "Alice",
                "email": "alice@test.com"
            }
        }));

        // Observation 2: user has only name (no email)
        let schema2 = infer_schema_json(&serde_json::json!({
            "user": {
                "name": "Bob"
            }
        }));

        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/profiles",
            Some(200),
            None,
            Some(&schema1),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/profiles",
            Some(200),
            None,
            Some(&schema2),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        let response_schemas = aggregated.response_schemas.unwrap();
        let schema_200 = response_schemas.get("200").unwrap();

        // Navigate to nested "user" object
        let user_field = schema_200.get("properties").unwrap().get("user").unwrap();
        let user_props = user_field.get("properties").unwrap();

        // "name" should have 100% presence (2/2)
        let name_field = user_props.get("name").unwrap();
        assert_eq!(
            name_field.get("presence_count").unwrap().as_u64().unwrap(),
            2,
            "name should be present in 2/2 observations"
        );
        assert!(
            (name_field.get("confidence").unwrap().as_f64().unwrap() - 1.0).abs() < 0.01,
            "name confidence should be 1.0 (100%)"
        );

        // "email" should have 50% presence (1/2) - THIS IS THE BUG!
        let email_field = user_props.get("email").unwrap();
        assert_eq!(
            email_field.get("presence_count").unwrap().as_u64().unwrap(),
            1,
            "email should be present in 1/2 observations"
        );
        assert!(
            (email_field.get("confidence").unwrap().as_f64().unwrap() - 0.5).abs() < 0.01,
            "email confidence should be 0.5 (50%)"
        );

        // Check required fields: only "name" should be required for nested user object
        let user_required = user_field.get("required");
        assert!(user_required.is_some(), "user should have required array");
        let user_required_fields: Vec<String> =
            serde_json::from_value(user_required.unwrap().clone()).unwrap();

        assert!(
            user_required_fields.contains(&"name".to_string()),
            "name should be required (100% presence)"
        );
        assert!(
            !user_required_fields.contains(&"email".to_string()),
            "email should NOT be required (50% presence)"
        );
    }

    #[tokio::test]
    async fn test_deeply_nested_field_optional_presence() {
        let pool = setup_test_db().await;
        let session_id = create_test_session(&pool).await;

        // Observation 1: deeply nested with all fields
        let schema1 = infer_schema_json(&serde_json::json!({
            "data": {
                "user": {
                    "name": "Alice",
                    "profile": {
                        "bio": "Hello",
                        "avatar": "url1"
                    }
                }
            }
        }));

        // Observation 2: deeply nested with some fields missing
        let schema2 = infer_schema_json(&serde_json::json!({
            "data": {
                "user": {
                    "name": "Bob",
                    "profile": {
                        "bio": "World"
                        // avatar missing
                    }
                }
            }
        }));

        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/deep",
            Some(200),
            None,
            Some(&schema1),
        )
        .await;
        insert_test_observation(
            &pool,
            &session_id,
            "GET",
            "/deep",
            Some(200),
            None,
            Some(&schema2),
        )
        .await;

        let inferred_repo = InferredSchemaRepository::new(pool.clone());
        let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
        let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

        let ids = aggregator.aggregate_session(&session_id).await.unwrap();
        let aggregated = aggregated_repo.get_by_id(ids[0]).await.unwrap();

        let response_schemas = aggregated.response_schemas.unwrap();
        let schema_200 = response_schemas.get("200").unwrap();

        // Navigate to deeply nested profile
        let data = schema_200.get("properties").unwrap().get("data").unwrap();
        let user = data.get("properties").unwrap().get("user").unwrap();
        let profile = user.get("properties").unwrap().get("profile").unwrap();
        let profile_props = profile.get("properties").unwrap();

        // "bio" should have 100% presence
        let bio_field = profile_props.get("bio").unwrap();
        assert_eq!(
            bio_field.get("presence_count").unwrap().as_u64().unwrap(),
            2,
            "bio should be present in 2/2 observations"
        );

        // "avatar" should have 50% presence
        let avatar_field = profile_props.get("avatar").unwrap();
        assert_eq!(
            avatar_field.get("presence_count").unwrap().as_u64().unwrap(),
            1,
            "avatar should be present in 1/2 observations"
        );
        assert!(
            (avatar_field.get("confidence").unwrap().as_f64().unwrap() - 0.5).abs() < 0.01,
            "avatar confidence should be 0.5 (50%)"
        );

        // Check required fields for profile: only "bio" should be required
        let profile_required = profile.get("required");
        assert!(profile_required.is_some());
        let profile_required_fields: Vec<String> =
            serde_json::from_value(profile_required.unwrap().clone()).unwrap();

        assert!(profile_required_fields.contains(&"bio".to_string()));
        assert!(!profile_required_fields.contains(&"avatar".to_string()));
    }
}
