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
use std::collections::{HashMap, HashSet};
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

        // Aggregate response schemas by status code.
        // Always insert the status code key so bodyless endpoints (DELETE 204,
        // GET collections) retain their status code in the aggregated record.
        let mut response_schemas_map = HashMap::new();
        if let Some(status) = response_status_code {
            let response_schema = merge_schemas(&observations, |obs| obs.response_schema.as_ref())?;
            response_schemas_map
                .insert(status.to_string(), response_schema.unwrap_or(serde_json::Value::Null));
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

        // Merge headers across observations: collect unique header names with one example value
        let request_headers = merge_headers(&observations, |obs| obs.request_headers.as_ref());
        let response_headers = merge_headers(&observations, |obs| obs.response_headers.as_ref());

        info!(
            method = %http_method,
            path = %path_pattern,
            sample_count = sample_count,
            confidence = confidence_score,
            has_breaking_changes = has_breaking_changes,
            request_header_count = request_headers.as_ref().map_or(0, |v| v.as_array().map_or(0, |a| a.len())),
            response_header_count = response_headers.as_ref().map_or(0, |v| v.as_array().map_or(0, |a| a.len())),
            "Prepared aggregation request for endpoint"
        );

        // Return the request (actual DB write happens in batch)
        Ok(CreateAggregatedSchemaRequest {
            team: team.clone(),
            path: path_pattern.to_string(),
            http_method: http_method.to_string(),
            request_schema,
            response_schemas,
            request_headers,
            response_headers,
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

/// Merge header observations across multiple samples into a deduplicated list.
///
/// Each observation stores headers as a JSON array of `{"name": "...", "example": "..."}`.
/// This function collects all unique header names across observations, keeping one
/// example value per header name. Returns `None` if no headers found.
fn merge_headers<F>(
    observations: &[InferredSchemaData],
    header_accessor: F,
) -> Option<serde_json::Value>
where
    F: Fn(&InferredSchemaData) -> Option<&serde_json::Value>,
{
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut merged: Vec<serde_json::Value> = Vec::new();

    for obs in observations {
        if let Some(headers_val) = header_accessor(obs) {
            if let Some(arr) = headers_val.as_array() {
                for entry in arr {
                    if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                        let lower_name = name.to_lowercase();
                        if seen_names.insert(lower_name) {
                            merged.push(entry.clone());
                        }
                    }
                }
            }
        }
    }

    if merged.is_empty() {
        None
    } else {
        // Sort by header name for consistent output
        merged.sort_by(|a, b| {
            let name_a = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let name_b = b.get("name").and_then(|n| n.as_str()).unwrap_or("");
            name_a.to_lowercase().cmp(&name_b.to_lowercase())
        });
        Some(serde_json::Value::Array(merged))
    }
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

    // NOTE: Database integration tests require Testcontainers setup
    // They are temporarily commented out until Phase 4 of PostgreSQL migration
    // See: .local/features/plans/postgresql-compatibility.md

    /// Helper to create proper InferredSchema JSON from a serde_json::Value
    /// This uses the actual schema inference engine to ensure correct format
    #[allow(dead_code)]
    fn infer_schema_json(value: &serde_json::Value) -> String {
        let engine = SchemaInferenceEngine::new();
        let schema = engine.infer_from_value(value).unwrap();
        serde_json::to_string(&schema).unwrap()
    }

    // NOTE: Integration tests test_aggregate_single_endpoint and test_aggregate_multiple_endpoints
    // have been moved to tests/schema_aggregator_integration.rs and require
    // PostgreSQL via Testcontainers - Phase 4 of PostgreSQL migration

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

    #[cfg(feature = "postgres_tests")]
    mod integration {
        use crate::schema::SchemaInferenceEngine;
        use crate::services::schema_aggregator::SchemaAggregator;
        use crate::storage::repositories::{AggregatedSchemaRepository, InferredSchemaRepository};
        use crate::storage::test_helpers::{TestDatabase, TEST_TEAM_ID};

        /// Helper: create a learning session and return its ID
        async fn create_session(pool: &crate::storage::DbPool) -> String {
            let session_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO learning_sessions (
                    id, team, route_pattern, status, target_sample_count, current_sample_count
                ) VALUES ($1, $2, '/test/*', 'active', 100, 0)",
            )
            .bind(&session_id)
            .bind(TEST_TEAM_ID)
            .execute(pool)
            .await
            .unwrap();
            session_id
        }

        /// Helper: insert an inferred schema with a response_schema JSON
        async fn insert_inferred_schema(
            pool: &crate::storage::DbPool,
            session_id: &str,
            method: &str,
            path: &str,
            status_code: Option<i64>,
            response_json: &serde_json::Value,
        ) {
            let engine = SchemaInferenceEngine::new();
            let schema = engine.infer_from_value(response_json).unwrap();
            let schema_str = serde_json::to_string(&schema).unwrap();

            sqlx::query(
                "INSERT INTO inferred_schemas (
                    team, session_id, http_method, path_pattern, response_schema,
                    response_status_code, sample_count, confidence,
                    first_seen_at, last_seen_at
                ) VALUES ($1, $2, $3, $4, $5, $6, 1, 1.0, NOW(), NOW())",
            )
            .bind(TEST_TEAM_ID)
            .bind(session_id)
            .bind(method)
            .bind(path)
            .bind(&schema_str)
            .bind(status_code)
            .execute(pool)
            .await
            .unwrap();
        }

        #[tokio::test]
        async fn test_aggregate_session_basic() {
            let test_db = TestDatabase::new("agg_session_basic").await;
            let pool = test_db.pool.clone();
            let session_id = create_session(&pool).await;

            // Insert 5 inferred_schemas: 3 for GET /api/users (200), 2 for POST /api/users (201)
            let get_response = serde_json::json!({"id": 1, "name": "Alice", "email": "a@b.com"});
            for _ in 0..3 {
                insert_inferred_schema(
                    &pool,
                    &session_id,
                    "GET",
                    "/api/users",
                    Some(200),
                    &get_response,
                )
                .await;
            }

            let post_response = serde_json::json!({"id": 2, "created": true});
            for _ in 0..2 {
                insert_inferred_schema(
                    &pool,
                    &session_id,
                    "POST",
                    "/api/users",
                    Some(201),
                    &post_response,
                )
                .await;
            }

            // Create aggregator with real repos
            let inferred_repo = InferredSchemaRepository::new(pool.clone());
            let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
            let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

            // Aggregate
            let ids = aggregator.aggregate_session(&session_id).await.unwrap();
            assert_eq!(ids.len(), 2, "Should produce 2 aggregated schemas (one per endpoint)");

            // Verify aggregated schemas
            let schemas = aggregated_repo.get_by_ids(&ids).await.unwrap();
            assert_eq!(schemas.len(), 2);

            for schema in &schemas {
                assert_eq!(schema.team, TEST_TEAM_ID);
                assert_eq!(schema.path, "/api/users");
                assert!(schema.confidence_score > 0.0, "Confidence should be > 0");
                assert!(schema.sample_count > 0, "Sample count should be > 0");
            }

            // Check per-endpoint sample counts
            let get_schema = schemas.iter().find(|s| s.http_method == "GET").unwrap();
            assert_eq!(get_schema.sample_count, 3);

            let post_schema = schemas.iter().find(|s| s.http_method == "POST").unwrap();
            assert_eq!(post_schema.sample_count, 2);
        }

        #[tokio::test]
        async fn test_aggregate_session_no_schemas() {
            let test_db = TestDatabase::new("agg_session_empty").await;
            let pool = test_db.pool.clone();
            let session_id = create_session(&pool).await;

            // Don't insert any inferred schemas
            let inferred_repo = InferredSchemaRepository::new(pool.clone());
            let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
            let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo);

            let ids = aggregator.aggregate_session(&session_id).await.unwrap();
            assert!(ids.is_empty(), "Empty session should produce no aggregated schemas");
        }

        #[tokio::test]
        async fn test_aggregate_session_version_increment() {
            let test_db = TestDatabase::new("agg_version_inc").await;
            let pool = test_db.pool.clone();

            // Session 1: initial schemas
            let session1 = create_session(&pool).await;
            let response_v1 = serde_json::json!({"id": 1, "name": "Alice"});
            for _ in 0..3 {
                insert_inferred_schema(
                    &pool,
                    &session1,
                    "GET",
                    "/api/users",
                    Some(200),
                    &response_v1,
                )
                .await;
            }

            let inferred_repo = InferredSchemaRepository::new(pool.clone());
            let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
            let aggregator = SchemaAggregator::new(inferred_repo.clone(), aggregated_repo.clone());

            let ids_v1 = aggregator.aggregate_session(&session1).await.unwrap();
            assert_eq!(ids_v1.len(), 1);

            let v1 = aggregated_repo.get_by_id(ids_v1[0]).await.unwrap();
            assert_eq!(v1.version, 1);

            // Session 2: same endpoint, slightly different schema (added field)
            let session2 = create_session(&pool).await;
            let response_v2 = serde_json::json!({"id": 2, "name": "Bob", "email": "bob@test.com"});
            for _ in 0..5 {
                insert_inferred_schema(
                    &pool,
                    &session2,
                    "GET",
                    "/api/users",
                    Some(200),
                    &response_v2,
                )
                .await;
            }

            let aggregator2 = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());
            let ids_v2 = aggregator2.aggregate_session(&session2).await.unwrap();
            assert_eq!(ids_v2.len(), 1);

            let v2 = aggregated_repo.get_by_id(ids_v2[0]).await.unwrap();
            assert_eq!(v2.version, 2, "Second aggregation should produce version 2");
            assert_eq!(v2.previous_version_id, Some(v1.id));
            assert_eq!(v2.sample_count, 5);
        }
    }
}
