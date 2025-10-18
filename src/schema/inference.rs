//! Schema inference engine for JSON payloads
//!
//! This module processes JSON payloads and infers their schema structure
//! WITHOUT storing the actual data. Only metadata about types, formats,
//! and constraints are retained.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use tracing::debug;

use crate::errors::{Error, Result};

/// Inferred schema type
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    String,
    Number,
    Integer,
    Boolean,
    Null,
    Object,
    Array,
    /// Multiple possible types (e.g., "string | null")
    OneOf(Vec<SchemaType>),
}

impl SchemaType {
    /// Check if this type can merge with another type
    pub fn can_merge(&self, other: &SchemaType) -> bool {
        self == other
            || matches!(self, SchemaType::OneOf(_))
            || matches!(other, SchemaType::OneOf(_))
    }

    /// Merge this type with another, creating a OneOf if types differ
    pub fn merge(self, other: SchemaType) -> SchemaType {
        if self == other {
            return self;
        }

        // Extract all types from both sides
        let mut types = HashSet::new();

        match self {
            SchemaType::OneOf(ref inner) => {
                for t in inner {
                    types.insert(t.clone());
                }
            }
            _ => {
                types.insert(self);
            }
        }

        match other {
            SchemaType::OneOf(inner) => {
                for t in inner {
                    types.insert(t);
                }
            }
            _ => {
                types.insert(other);
            }
        }

        if types.len() == 1 {
            types.into_iter().next().unwrap()
        } else {
            let mut sorted_types: Vec<_> = types.into_iter().collect();
            sorted_types.sort_by(|a, b| format!("{:?}", a).cmp(&format!("{:?}", b)));
            SchemaType::OneOf(sorted_types)
        }
    }
}

/// Detected format for string values
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StringFormat {
    Email,
    Uri,
    Uuid,
    DateTime,
    Date,
    Time,
    Ipv4,
    Ipv6,
    // No specific format detected
    None,
}

/// Numeric constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumericConstraints {
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub multiple_of: Option<f64>,
}

/// Array constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrayConstraints {
    pub min_items: Option<usize>,
    pub max_items: Option<usize>,
    pub unique_items: Option<bool>,
}

/// Anonymization mode for field names
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnonymizationMode {
    /// No anonymization - use original field names
    None,
    /// Hash field names using SHA-256 (truncated to 8 chars)
    Hash,
    /// Use sequential field names (field_1, field_2, etc.)
    Sequential,
}

/// Configuration for field name anonymization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnonymizationConfig {
    /// Anonymization mode
    pub mode: AnonymizationMode,
    /// Prefix for anonymized field names (e.g., "field_" for sequential mode)
    pub prefix: String,
    /// Store mapping for reversibility (original -> anonymized)
    pub store_mapping: bool,
}

impl Default for AnonymizationConfig {
    fn default() -> Self {
        Self { mode: AnonymizationMode::None, prefix: "field_".to_string(), store_mapping: false }
    }
}

impl AnonymizationConfig {
    /// Create config with hash mode
    pub fn hash() -> Self {
        Self { mode: AnonymizationMode::Hash, prefix: "field_".to_string(), store_mapping: true }
    }

    /// Create config with sequential mode
    pub fn sequential() -> Self {
        Self {
            mode: AnonymizationMode::Sequential,
            prefix: "field_".to_string(),
            store_mapping: true,
        }
    }

    /// Anonymize a field name according to the configuration
    pub fn anonymize_field_name(&self, original: &str, counter: &mut usize) -> String {
        match self.mode {
            AnonymizationMode::None => original.to_string(),
            AnonymizationMode::Hash => {
                let mut hasher = Sha256::new();
                hasher.update(original.as_bytes());
                let hash = hasher.finalize();
                // Take first 8 characters of hex hash
                format!(
                    "{}{:x}",
                    self.prefix,
                    &hash[0..4].iter().fold(0u32, |acc, &b| (acc << 8) | b as u32)
                )
            }
            AnonymizationMode::Sequential => {
                *counter += 1;
                format!("{}{}", self.prefix, counter)
            }
        }
    }
}

/// Field statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldStats {
    /// Number of times this field was observed
    pub sample_count: u64,
    /// Number of times this field was present
    pub presence_count: u64,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f64,
}

impl FieldStats {
    pub fn new() -> Self {
        Self { sample_count: 0, presence_count: 0, confidence: 0.0 }
    }

    pub fn record_sample(&mut self, present: bool) {
        self.sample_count += 1;
        if present {
            self.presence_count += 1;
        }
        self.update_confidence();
    }

    fn update_confidence(&mut self) {
        if self.sample_count == 0 {
            self.confidence = 0.0;
        } else {
            self.confidence = self.presence_count as f64 / self.sample_count as f64;
        }
    }

    pub fn is_required(&self, threshold: f64) -> bool {
        self.confidence >= threshold
    }
}

impl Default for FieldStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Inferred schema for a JSON value
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferredSchema {
    /// Type of the value
    #[serde(rename = "type")]
    pub schema_type: SchemaType,

    /// String format (if type is string)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<StringFormat>,

    /// Numeric constraints (if type is number/integer)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numeric_constraints: Option<NumericConstraints>,

    /// Array constraints (if type is array)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub array_constraints: Option<ArrayConstraints>,

    /// Items schema (if type is array)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<InferredSchema>>,

    /// Properties schemas (if type is object)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, InferredSchema>>,

    /// Required fields (if type is object)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,

    /// Field name anonymization mapping (anonymized -> original)
    /// Only populated if anonymization is enabled and store_mapping is true
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_mapping: Option<HashMap<String, String>>,

    /// Field statistics
    #[serde(flatten)]
    pub stats: FieldStats,
}

impl InferredSchema {
    /// Create a new schema for a given type
    pub fn new(schema_type: SchemaType) -> Self {
        Self {
            schema_type,
            format: None,
            field_mapping: None,
            numeric_constraints: None,
            array_constraints: None,
            items: None,
            properties: None,
            required: None,
            stats: FieldStats::new(),
        }
    }

    /// Merge this schema with another observation
    pub fn merge(&mut self, other: &InferredSchema) {
        // Merge types
        let merged_type = self.schema_type.clone().merge(other.schema_type.clone());
        self.schema_type = merged_type;

        // Merge numeric constraints
        if let (Some(ref mut nc), Some(ref other_nc)) =
            (&mut self.numeric_constraints, &other.numeric_constraints)
        {
            if let Some(other_min) = other_nc.minimum {
                nc.minimum = Some(nc.minimum.map_or(other_min, |m| m.min(other_min)));
            }
            if let Some(other_max) = other_nc.maximum {
                nc.maximum = Some(nc.maximum.map_or(other_max, |m| m.max(other_max)));
            }
        }

        // Merge array constraints
        if let (Some(ref mut ac), Some(ref other_ac)) =
            (&mut self.array_constraints, &other.array_constraints)
        {
            if let Some(other_min) = other_ac.min_items {
                ac.min_items = Some(ac.min_items.map_or(other_min, |m| m.min(other_min)));
            }
            if let Some(other_max) = other_ac.max_items {
                ac.max_items = Some(ac.max_items.map_or(other_max, |m| m.max(other_max)));
            }
        }

        // Merge object properties
        if let (Some(ref mut props), Some(ref other_props)) =
            (&mut self.properties, &other.properties)
        {
            for (key, other_schema) in other_props {
                props
                    .entry(key.clone())
                    .and_modify(|s| s.merge(other_schema))
                    .or_insert_with(|| other_schema.clone());
            }
        }

        // Merge array items
        if let (Some(ref mut items), Some(ref other_items)) = (&mut self.items, &other.items) {
            items.merge(other_items);
        }

        // Update stats
        self.stats.sample_count += other.stats.sample_count;
        self.stats.presence_count += other.stats.presence_count;
        self.stats.update_confidence();
    }
}

/// Schema inference engine
#[derive(Debug)]
pub struct SchemaInferenceEngine {
    /// Threshold for considering a field required (0.0 to 1.0)
    required_threshold: f64,
    /// Anonymization configuration for field names
    anonymization: AnonymizationConfig,
}

impl SchemaInferenceEngine {
    /// Create a new schema inference engine with default settings
    pub fn new() -> Self {
        Self {
            required_threshold: 0.95, // 95% presence = required
            anonymization: AnonymizationConfig::default(),
        }
    }

    /// Create a new schema inference engine with custom anonymization
    pub fn with_anonymization(anonymization: AnonymizationConfig) -> Self {
        Self { required_threshold: 0.95, anonymization }
    }

    /// Set the required field threshold
    pub fn with_required_threshold(mut self, threshold: f64) -> Self {
        self.required_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Infer schema from a JSON value
    ///
    /// IMPORTANT: This function does NOT store the payload data.
    /// Only structural metadata is extracted and the original value
    /// is immediately discarded after processing.
    pub fn infer_from_value(&self, value: &Value) -> Result<InferredSchema> {
        let mut schema = self.infer_value_schema(value)?;
        schema.stats.record_sample(true);
        Ok(schema)
    }

    /// Infer schema from a JSON string
    ///
    /// IMPORTANT: The raw JSON string is parsed and immediately discarded.
    /// Only schema metadata is retained.
    pub fn infer_from_json(&self, json_str: &str) -> Result<InferredSchema> {
        // Parse JSON (error if malformed)
        let value: Value = serde_json::from_str(json_str)
            .map_err(|e| Error::validation(format!("Invalid JSON payload: {}", e)))?;

        // Infer schema from parsed value
        let schema = self.infer_from_value(&value)?;

        // Note: `value` is dropped here, ensuring no payload data is retained
        debug!("Inferred schema from JSON payload (payload discarded)");

        Ok(schema)
    }

    /// Infer schema from a serde_json::Value
    fn infer_value_schema(&self, value: &Value) -> Result<InferredSchema> {
        match value {
            Value::Null => Ok(InferredSchema::new(SchemaType::Null)),

            Value::Bool(_) => Ok(InferredSchema::new(SchemaType::Boolean)),

            Value::Number(n) => {
                let schema_type =
                    if n.is_i64() || n.is_u64() { SchemaType::Integer } else { SchemaType::Number };

                let numeric_value = n.as_f64().unwrap_or(0.0);

                let mut schema = InferredSchema::new(schema_type);
                schema.numeric_constraints = Some(NumericConstraints {
                    minimum: Some(numeric_value),
                    maximum: Some(numeric_value),
                    multiple_of: None,
                });

                Ok(schema)
            }

            Value::String(s) => {
                let mut schema = InferredSchema::new(SchemaType::String);
                schema.format = Some(self.detect_string_format(s));
                Ok(schema)
            }

            Value::Array(arr) => {
                let mut schema = InferredSchema::new(SchemaType::Array);

                schema.array_constraints = Some(ArrayConstraints {
                    min_items: Some(arr.len()),
                    max_items: Some(arr.len()),
                    unique_items: None,
                });

                // Infer items schema from array elements
                if !arr.is_empty() {
                    let mut items_schema = self.infer_value_schema(&arr[0])?;

                    // Merge with other array items to get unified schema
                    for item in arr.iter().skip(1) {
                        let item_schema = self.infer_value_schema(item)?;
                        items_schema.merge(&item_schema);
                    }

                    schema.items = Some(Box::new(items_schema));
                }

                Ok(schema)
            }

            Value::Object(obj) => {
                let mut schema = InferredSchema::new(SchemaType::Object);
                let mut properties = HashMap::new();
                let mut field_mapping = HashMap::new();
                let mut counter = 0;

                for (key, val) in obj {
                    let prop_schema = self.infer_value_schema(val)?;

                    // Apply anonymization to field name
                    let anonymized_key = self.anonymization.anonymize_field_name(key, &mut counter);

                    // Store mapping if requested
                    if self.anonymization.store_mapping && anonymized_key != *key {
                        field_mapping.insert(anonymized_key.clone(), key.clone());
                    }

                    properties.insert(anonymized_key, prop_schema);
                }

                schema.properties = Some(properties);

                // Only include mapping if non-empty
                if !field_mapping.is_empty() {
                    schema.field_mapping = Some(field_mapping);
                }

                Ok(schema)
            }
        }
    }

    /// Detect string format using regex patterns
    fn detect_string_format(&self, s: &str) -> StringFormat {
        // Email: simple pattern
        if s.contains('@') && s.contains('.') {
            let parts: Vec<&str> = s.split('@').collect();
            if parts.len() == 2 && !parts[0].is_empty() && parts[1].contains('.') {
                return StringFormat::Email;
            }
        }

        // UUID: 8-4-4-4-12 hex pattern
        if s.len() == 36 && s.chars().filter(|&c| c == '-').count() == 4 {
            let uuid_pattern = regex::Regex::new(
                r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
            )
            .unwrap();
            if uuid_pattern.is_match(s) {
                return StringFormat::Uuid;
            }
        }

        // URI: starts with http:// or https://
        if s.starts_with("http://") || s.starts_with("https://") {
            return StringFormat::Uri;
        }

        // ISO 8601 DateTime
        if s.contains('T') && (s.contains('Z') || s.contains('+') || s.contains('-')) {
            let datetime_pattern =
                regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}").unwrap();
            if datetime_pattern.is_match(s) {
                return StringFormat::DateTime;
            }
        }

        // ISO 8601 Date
        if s.len() == 10 {
            let date_pattern = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
            if date_pattern.is_match(s) {
                return StringFormat::Date;
            }
        }

        // IPv4
        let ipv4_pattern = regex::Regex::new(r"^(\d{1,3}\.){3}\d{1,3}$").unwrap();
        if ipv4_pattern.is_match(s) {
            return StringFormat::Ipv4;
        }

        StringFormat::None
    }
}

impl Default for SchemaInferenceEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_null() {
        let engine = SchemaInferenceEngine::new();
        let schema = engine.infer_from_value(&Value::Null).unwrap();
        assert_eq!(schema.schema_type, SchemaType::Null);
    }

    #[test]
    fn test_infer_boolean() {
        let engine = SchemaInferenceEngine::new();
        let schema = engine.infer_from_value(&Value::Bool(true)).unwrap();
        assert_eq!(schema.schema_type, SchemaType::Boolean);
    }

    #[test]
    fn test_infer_integer() {
        let engine = SchemaInferenceEngine::new();
        let schema = engine.infer_from_value(&serde_json::json!(42)).unwrap();
        assert_eq!(schema.schema_type, SchemaType::Integer);
        assert!(schema.numeric_constraints.is_some());
    }

    #[test]
    fn test_infer_number() {
        let engine = SchemaInferenceEngine::new();
        let schema = engine.infer_from_value(&serde_json::json!(3.75)).unwrap();
        assert_eq!(schema.schema_type, SchemaType::Number);
        assert!(schema.numeric_constraints.is_some());
    }

    #[test]
    fn test_infer_string() {
        let engine = SchemaInferenceEngine::new();
        let schema = engine.infer_from_value(&Value::String("hello".to_string())).unwrap();
        assert_eq!(schema.schema_type, SchemaType::String);
        assert_eq!(schema.format, Some(StringFormat::None));
    }

    #[test]
    fn test_detect_email_format() {
        let engine = SchemaInferenceEngine::new();
        let schema =
            engine.infer_from_value(&Value::String("user@example.com".to_string())).unwrap();
        assert_eq!(schema.format, Some(StringFormat::Email));
    }

    #[test]
    fn test_detect_uuid_format() {
        let engine = SchemaInferenceEngine::new();
        let schema = engine
            .infer_from_value(&Value::String("550e8400-e29b-41d4-a716-446655440000".to_string()))
            .unwrap();
        assert_eq!(schema.format, Some(StringFormat::Uuid));
    }

    #[test]
    fn test_detect_uri_format() {
        let engine = SchemaInferenceEngine::new();
        let schema = engine
            .infer_from_value(&Value::String("https://example.com/path".to_string()))
            .unwrap();
        assert_eq!(schema.format, Some(StringFormat::Uri));
    }

    #[test]
    fn test_detect_datetime_format() {
        let engine = SchemaInferenceEngine::new();
        let schema =
            engine.infer_from_value(&Value::String("2023-10-18T12:00:00Z".to_string())).unwrap();
        assert_eq!(schema.format, Some(StringFormat::DateTime));
    }

    #[test]
    fn test_infer_array() {
        let engine = SchemaInferenceEngine::new();
        let value = serde_json::json!([1, 2, 3]);
        let schema = engine.infer_from_value(&value).unwrap();
        assert_eq!(schema.schema_type, SchemaType::Array);
        assert!(schema.items.is_some());
        assert_eq!(schema.items.unwrap().schema_type, SchemaType::Integer);
    }

    #[test]
    fn test_infer_object() {
        let engine = SchemaInferenceEngine::new();
        let value = serde_json::json!({
            "name": "John",
            "age": 30,
            "active": true
        });
        let schema = engine.infer_from_value(&value).unwrap();
        assert_eq!(schema.schema_type, SchemaType::Object);
        assert!(schema.properties.is_some());

        let props = schema.properties.unwrap();
        assert_eq!(props.len(), 3);
        assert_eq!(props.get("name").unwrap().schema_type, SchemaType::String);
        assert_eq!(props.get("age").unwrap().schema_type, SchemaType::Integer);
        assert_eq!(props.get("active").unwrap().schema_type, SchemaType::Boolean);
    }

    #[test]
    fn test_infer_nested_object() {
        let engine = SchemaInferenceEngine::new();
        let value = serde_json::json!({
            "user": {
                "name": "John",
                "email": "john@example.com"
            },
            "metadata": {
                "created": "2023-10-18T12:00:00Z"
            }
        });
        let schema = engine.infer_from_value(&value).unwrap();
        assert_eq!(schema.schema_type, SchemaType::Object);

        let props = schema.properties.unwrap();
        let user_schema = props.get("user").unwrap();
        assert_eq!(user_schema.schema_type, SchemaType::Object);

        let user_props = user_schema.properties.as_ref().unwrap();
        assert_eq!(user_props.get("email").unwrap().format, Some(StringFormat::Email));
    }

    #[test]
    fn test_infer_from_json_string() {
        let engine = SchemaInferenceEngine::new();
        let json_str = r#"{"name": "John", "age": 30}"#;
        let schema = engine.infer_from_json(json_str).unwrap();
        assert_eq!(schema.schema_type, SchemaType::Object);
    }

    #[test]
    fn test_infer_from_malformed_json() {
        let engine = SchemaInferenceEngine::new();
        let json_str = r#"{"name": "John", "age": 30"#; // Missing closing brace
        let result = engine.infer_from_json(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_schema_type_merge() {
        let type1 = SchemaType::String;
        let type2 = SchemaType::Null;
        let merged = type1.merge(type2);
        assert!(matches!(merged, SchemaType::OneOf(_)));
    }

    #[test]
    fn test_schema_merge() {
        let engine = SchemaInferenceEngine::new();
        let mut schema1 = engine.infer_from_value(&serde_json::json!(10)).unwrap();
        let schema2 = engine.infer_from_value(&serde_json::json!(20)).unwrap();

        schema1.merge(&schema2);

        let constraints = schema1.numeric_constraints.unwrap();
        assert_eq!(constraints.minimum, Some(10.0));
        assert_eq!(constraints.maximum, Some(20.0));
    }

    #[test]
    fn test_field_stats() {
        let mut stats = FieldStats::new();
        stats.record_sample(true);
        stats.record_sample(true);
        stats.record_sample(false);

        assert_eq!(stats.sample_count, 3);
        assert_eq!(stats.presence_count, 2);
        assert!((stats.confidence - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_anonymization_mode_none() {
        let config = AnonymizationConfig::default();
        assert_eq!(config.mode, AnonymizationMode::None);

        let mut counter = 0;
        let anonymized = config.anonymize_field_name("user_email", &mut counter);
        assert_eq!(anonymized, "user_email"); // No change
        assert_eq!(counter, 0); // Counter not used
    }

    #[test]
    fn test_anonymization_mode_sequential() {
        let config = AnonymizationConfig::sequential();
        assert_eq!(config.mode, AnonymizationMode::Sequential);
        assert!(config.store_mapping);

        let mut counter = 0;
        let anon1 = config.anonymize_field_name("user_email", &mut counter);
        let anon2 = config.anonymize_field_name("user_name", &mut counter);
        let anon3 = config.anonymize_field_name("user_id", &mut counter);

        assert_eq!(anon1, "field_1");
        assert_eq!(anon2, "field_2");
        assert_eq!(anon3, "field_3");
        assert_eq!(counter, 3);
    }

    #[test]
    fn test_anonymization_mode_hash() {
        let config = AnonymizationConfig::hash();
        assert_eq!(config.mode, AnonymizationMode::Hash);
        assert!(config.store_mapping);

        let mut counter = 0;
        let anon1 = config.anonymize_field_name("user_email", &mut counter);
        let anon2 = config.anonymize_field_name("user_name", &mut counter);

        // Hash should be deterministic
        assert!(anon1.starts_with("field_"));
        assert!(anon2.starts_with("field_"));
        assert_ne!(anon1, anon2); // Different fields get different hashes
        assert_eq!(counter, 0); // Counter not used in hash mode

        // Same input produces same hash
        let mut counter2 = 0;
        let anon1_again = config.anonymize_field_name("user_email", &mut counter2);
        assert_eq!(anon1, anon1_again);
    }

    #[test]
    fn test_anonymization_with_object() {
        let engine = SchemaInferenceEngine::with_anonymization(AnonymizationConfig::sequential());

        let json = serde_json::json!({
            "user_email": "test@example.com",
            "user_name": "John Doe",
            "user_age": 30
        });

        let schema = engine.infer_from_value(&json).unwrap();

        assert_eq!(schema.schema_type, SchemaType::Object);

        let properties = schema.properties.unwrap();
        // Check that all fields are anonymized (field_1, field_2, field_3)
        let mut keys: Vec<_> = properties.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["field_1", "field_2", "field_3"]);
        assert!(!properties.contains_key("user_email")); // Original keys not present

        // Check mapping exists and contains all original fields
        let mapping = schema.field_mapping.unwrap();
        assert_eq!(mapping.len(), 3);
        let mut orig_values: Vec<_> = mapping.values().cloned().collect();
        orig_values.sort();
        assert_eq!(
            orig_values,
            vec!["user_age".to_string(), "user_email".to_string(), "user_name".to_string()]
        );
    }

    #[test]
    fn test_anonymization_with_hash_mode() {
        let engine = SchemaInferenceEngine::with_anonymization(AnonymizationConfig::hash());

        let json = serde_json::json!({
            "sensitive_field": "secret data",
            "public_field": "public data"
        });

        let schema = engine.infer_from_value(&json).unwrap();

        let properties = schema.properties.unwrap();
        assert!(!properties.contains_key("sensitive_field")); // Original not present
        assert!(!properties.contains_key("public_field")); // Original not present

        // All keys should be hashed
        for key in properties.keys() {
            assert!(key.starts_with("field_"));
        }

        // Mapping should exist
        let mapping = schema.field_mapping.unwrap();
        assert_eq!(mapping.len(), 2);
    }

    #[test]
    fn test_no_anonymization_no_mapping() {
        let engine = SchemaInferenceEngine::new(); // Default: no anonymization

        let json = serde_json::json!({
            "user_email": "test@example.com",
            "user_name": "John Doe"
        });

        let schema = engine.infer_from_value(&json).unwrap();

        let properties = schema.properties.unwrap();
        assert!(properties.contains_key("user_email")); // Original keys present
        assert!(properties.contains_key("user_name"));

        // No mapping when anonymization is disabled
        assert!(schema.field_mapping.is_none());
    }

    #[test]
    fn test_anonymization_nested_objects() {
        let engine = SchemaInferenceEngine::with_anonymization(AnonymizationConfig::sequential());

        let json = serde_json::json!({
            "user": {
                "email": "test@example.com",
                "profile": {
                    "name": "John",
                    "age": 30
                }
            },
            "timestamp": "2023-10-18T12:00:00Z"
        });

        let schema = engine.infer_from_value(&json).unwrap();

        // Top level should be anonymized
        let properties = schema.properties.unwrap();
        let mut top_keys: Vec<_> = properties.keys().cloned().collect();
        top_keys.sort();
        assert_eq!(top_keys, vec!["field_1", "field_2"]); // timestamp and user (order varies)

        // Check that original field names are in the mapping
        let mapping = schema.field_mapping.as_ref().unwrap();
        assert_eq!(mapping.len(), 2);
        let orig_values: Vec<_> = mapping.values().cloned().collect();
        assert!(orig_values.contains(&"user".to_string()));
        assert!(orig_values.contains(&"timestamp".to_string()));

        // Find the user field (either field_1 or field_2)
        let user_schema = properties
            .values()
            .find(|s| s.schema_type == SchemaType::Object && s.properties.is_some())
            .unwrap();

        // Check nested object also has anonymization
        let user_props = user_schema.properties.as_ref().unwrap();
        let mut nested_keys: Vec<_> = user_props.keys().cloned().collect();
        nested_keys.sort();
        assert_eq!(nested_keys, vec!["field_1", "field_2"]); // email and profile
    }

    #[test]
    fn test_anonymization_config_custom_prefix() {
        let config = AnonymizationConfig {
            mode: AnonymizationMode::Sequential,
            prefix: "prop_".to_string(),
            store_mapping: true,
        };

        let mut counter = 0;
        let anon = config.anonymize_field_name("test", &mut counter);
        assert_eq!(anon, "prop_1");
    }
}
