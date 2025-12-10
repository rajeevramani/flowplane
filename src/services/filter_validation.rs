//! Schema-based filter configuration validation service.
//!
//! This module provides JSON Schema validation for filter configurations,
//! replacing hardcoded match statements with dynamic validation.
//!
//! # Architecture
//!
//! The validator uses the `jsonschema` crate to compile JSON schemas and
//! validate filter configurations at runtime. Compiled schemas are cached
//! using `DashMap` for efficient concurrent access.

use crate::domain::filter_schema::SharedFilterSchemaRegistry;
use dashmap::DashMap;
use jsonschema::{Draft, Validator};
use serde_json::Value;
use std::sync::Arc;

/// Filter configuration validator using JSON Schema.
///
/// This validator provides schema-based validation for filter configurations,
/// supporting both built-in and dynamically loaded filter types.
#[derive(Debug, Clone)]
pub struct FilterConfigValidator {
    /// Reference to the schema registry
    registry: SharedFilterSchemaRegistry,

    /// Cache of compiled JSON schemas
    compiled_schemas: Arc<DashMap<String, Arc<Validator>>>,
}

impl FilterConfigValidator {
    /// Create a new validator with the given schema registry.
    pub fn new(registry: SharedFilterSchemaRegistry) -> Self {
        Self { registry, compiled_schemas: Arc::new(DashMap::new()) }
    }

    /// Validate a filter configuration against its schema.
    ///
    /// # Arguments
    ///
    /// * `filter_type` - The filter type name (e.g., "header_mutation")
    /// * `config` - The configuration JSON value to validate
    ///
    /// # Returns
    ///
    /// - `Ok(())` if validation passes
    /// - `Err(ValidationErrors)` if validation fails
    pub async fn validate(
        &self,
        filter_type: &str,
        config: &Value,
    ) -> Result<(), FilterValidationError> {
        // Get schema from registry
        let registry = self.registry.read().await;
        let schema_def = registry
            .get(filter_type)
            .ok_or_else(|| FilterValidationError::UnknownFilterType(filter_type.to_string()))?;

        // Get or compile the schema
        let validator = self.get_or_compile_validator(filter_type, &schema_def.config_schema)?;

        // Validate the configuration using iter_errors for all errors
        let errors: Vec<ValidationErrorDetail> = validator
            .iter_errors(config)
            .map(|e| ValidationErrorDetail {
                path: e.instance_path.to_string(),
                message: e.to_string(),
            })
            .collect();

        if !errors.is_empty() {
            return Err(FilterValidationError::ValidationFailed {
                filter_type: filter_type.to_string(),
                errors,
            });
        }

        Ok(())
    }

    /// Validate a per-route configuration against its schema.
    pub async fn validate_per_route(
        &self,
        filter_type: &str,
        config: &Value,
    ) -> Result<(), FilterValidationError> {
        let registry = self.registry.read().await;
        let schema_def = registry
            .get(filter_type)
            .ok_or_else(|| FilterValidationError::UnknownFilterType(filter_type.to_string()))?;

        // Use per-route schema if available, otherwise use main schema
        let schema =
            schema_def.per_route_config_schema.as_ref().unwrap_or(&schema_def.config_schema);

        let cache_key = format!("{}_per_route", filter_type);
        let validator = self.get_or_compile_validator(&cache_key, schema)?;

        let errors: Vec<ValidationErrorDetail> = validator
            .iter_errors(config)
            .map(|e| ValidationErrorDetail {
                path: e.instance_path.to_string(),
                message: e.to_string(),
            })
            .collect();

        if !errors.is_empty() {
            return Err(FilterValidationError::ValidationFailed {
                filter_type: filter_type.to_string(),
                errors,
            });
        }

        Ok(())
    }

    /// Check if a filter type exists and is implemented.
    pub async fn is_filter_type_valid(&self, filter_type: &str) -> bool {
        let registry = self.registry.read().await;
        registry.get(filter_type).map(|s| s.is_implemented).unwrap_or(false)
    }

    /// Check if a filter type is known (exists in registry).
    pub async fn is_filter_type_known(&self, filter_type: &str) -> bool {
        let registry = self.registry.read().await;
        registry.contains(filter_type)
    }

    /// Get the JSON schema for a filter type.
    pub async fn get_config_schema(&self, filter_type: &str) -> Option<Value> {
        let registry = self.registry.read().await;
        registry.get(filter_type).map(|s| s.config_schema.clone())
    }

    /// Clear the compiled schema cache.
    ///
    /// This should be called after reloading schemas to ensure
    /// new schemas are compiled.
    pub fn clear_cache(&self) {
        self.compiled_schemas.clear();
    }

    /// Get or compile a validator for the given schema.
    fn get_or_compile_validator(
        &self,
        cache_key: &str,
        schema: &Value,
    ) -> Result<Arc<Validator>, FilterValidationError> {
        // Check cache first
        if let Some(validator) = self.compiled_schemas.get(cache_key) {
            return Ok(Arc::clone(&validator));
        }

        // Compile the schema
        let validator =
            Validator::options().with_draft(Draft::Draft7).build(schema).map_err(|e| {
                FilterValidationError::SchemaCompilationError {
                    filter_type: cache_key.to_string(),
                    message: e.to_string(),
                }
            })?;

        let validator = Arc::new(validator);
        self.compiled_schemas.insert(cache_key.to_string(), Arc::clone(&validator));

        Ok(validator)
    }
}

/// Detailed validation error for a specific field.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationErrorDetail {
    /// JSON path to the invalid field (e.g., "/providers/0/issuer")
    pub path: String,
    /// Human-readable error message
    pub message: String,
}

/// Filter validation error types.
#[derive(Debug, thiserror::Error)]
pub enum FilterValidationError {
    #[error("Unknown filter type: {0}")]
    UnknownFilterType(String),

    #[error("Filter type '{filter_type}' validation failed")]
    ValidationFailed { filter_type: String, errors: Vec<ValidationErrorDetail> },

    #[error("Schema compilation error for '{filter_type}': {message}")]
    SchemaCompilationError { filter_type: String, message: String },
}

impl FilterValidationError {
    /// Get validation error details if available.
    pub fn errors(&self) -> Option<&[ValidationErrorDetail]> {
        match self {
            FilterValidationError::ValidationFailed { errors, .. } => Some(errors),
            _ => None,
        }
    }

    /// Convert to a user-friendly error message.
    pub fn to_user_message(&self) -> String {
        match self {
            FilterValidationError::UnknownFilterType(t) => {
                format!("Unknown filter type: '{}'. Please check available filter types.", t)
            }
            FilterValidationError::ValidationFailed { filter_type, errors } => {
                let error_list: Vec<String> = errors
                    .iter()
                    .map(|e| {
                        if e.path.is_empty() || e.path == "/" {
                            e.message.clone()
                        } else {
                            format!("{}: {}", e.path, e.message)
                        }
                    })
                    .collect();
                format!(
                    "Filter '{}' configuration validation failed:\n  - {}",
                    filter_type,
                    error_list.join("\n  - ")
                )
            }
            FilterValidationError::SchemaCompilationError { filter_type, message } => {
                format!(
                    "Internal error: Failed to compile schema for '{}': {}",
                    filter_type, message
                )
            }
        }
    }
}

/// Create a shared filter validator with the given registry.
pub fn create_filter_validator(registry: SharedFilterSchemaRegistry) -> FilterConfigValidator {
    FilterConfigValidator::new(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::filter_schema::create_shared_registry;

    #[tokio::test]
    async fn test_validate_header_mutation_valid() {
        let registry = create_shared_registry();
        let validator = FilterConfigValidator::new(registry);

        let config = serde_json::json!({
            "request_headers_to_add": [
                { "key": "X-Test", "value": "test-value", "append": false }
            ],
            "request_headers_to_remove": ["X-Remove"]
        });

        let result = validator.validate("header_mutation", &config).await;
        assert!(result.is_ok(), "Valid config should pass: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_validate_header_mutation_invalid_key() {
        let registry = create_shared_registry();
        let validator = FilterConfigValidator::new(registry);

        let config = serde_json::json!({
            "request_headers_to_add": [
                { "key": "", "value": "test-value" }  // Empty key should fail minLength
            ]
        });

        let result = validator.validate("header_mutation", &config).await;
        assert!(result.is_err(), "Empty key should fail validation");
    }

    #[tokio::test]
    async fn test_validate_local_rate_limit_valid() {
        let registry = create_shared_registry();
        let validator = FilterConfigValidator::new(registry);

        let config = serde_json::json!({
            "stat_prefix": "test",
            "token_bucket": {
                "max_tokens": 100,
                "tokens_per_fill": 50,
                "fill_interval_ms": 1000
            }
        });

        let result = validator.validate("local_rate_limit", &config).await;
        assert!(result.is_ok(), "Valid config should pass: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_validate_local_rate_limit_missing_required() {
        let registry = create_shared_registry();
        let validator = FilterConfigValidator::new(registry);

        let config = serde_json::json!({
            "stat_prefix": "test"
            // Missing required token_bucket
        });

        let result = validator.validate("local_rate_limit", &config).await;
        assert!(result.is_err(), "Missing required field should fail");
    }

    #[tokio::test]
    async fn test_validate_unknown_filter_type() {
        let registry = create_shared_registry();
        let validator = FilterConfigValidator::new(registry);

        let config = serde_json::json!({});
        let result = validator.validate("unknown_filter", &config).await;

        assert!(matches!(result, Err(FilterValidationError::UnknownFilterType(_))));
    }

    #[tokio::test]
    async fn test_is_filter_type_valid() {
        let registry = create_shared_registry();
        let validator = FilterConfigValidator::new(registry);

        assert!(validator.is_filter_type_valid("header_mutation").await);
        assert!(validator.is_filter_type_valid("jwt_auth").await);
        assert!(!validator.is_filter_type_valid("cors").await); // Not implemented
        assert!(!validator.is_filter_type_valid("unknown").await);
    }

    #[tokio::test]
    async fn test_is_filter_type_known() {
        let registry = create_shared_registry();
        let validator = FilterConfigValidator::new(registry);

        assert!(validator.is_filter_type_known("header_mutation").await);
        assert!(validator.is_filter_type_known("cors").await); // Known but not implemented
        assert!(!validator.is_filter_type_known("unknown").await);
    }

    #[tokio::test]
    async fn test_get_config_schema() {
        let registry = create_shared_registry();
        let validator = FilterConfigValidator::new(registry);

        let schema = validator.get_config_schema("header_mutation").await;
        assert!(schema.is_some());

        let schema = schema.unwrap();
        assert!(schema.is_object());
        assert_eq!(schema.get("type"), Some(&serde_json::json!("object")));
    }

    #[tokio::test]
    async fn test_validation_error_message() {
        let error = FilterValidationError::ValidationFailed {
            filter_type: "test".to_string(),
            errors: vec![
                ValidationErrorDetail {
                    path: "/field1".to_string(),
                    message: "is required".to_string(),
                },
                ValidationErrorDetail {
                    path: "/field2".to_string(),
                    message: "must be a string".to_string(),
                },
            ],
        };

        let message = error.to_user_message();
        assert!(message.contains("test"));
        assert!(message.contains("/field1"));
        assert!(message.contains("/field2"));
    }

    #[tokio::test]
    async fn test_cache_behavior() {
        let registry = create_shared_registry();
        let validator = FilterConfigValidator::new(registry);

        // First validation compiles the schema
        let config = serde_json::json!({"request_headers_to_add": []});
        let _ = validator.validate("header_mutation", &config).await;

        // Schema should be cached now
        assert!(!validator.compiled_schemas.is_empty());

        // Clear cache
        validator.clear_cache();
        assert!(validator.compiled_schemas.is_empty());
    }
}
