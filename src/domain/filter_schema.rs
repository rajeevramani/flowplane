//! Dynamic filter schema registry for runtime-loaded filter definitions.
//!
//! This module provides a schema-driven approach to filter configuration,
//! allowing new Envoy HTTP filters to be added via YAML/JSON schema files
//! without requiring control plane recompilation.
//!
//! # Architecture
//!
//! Filter definitions live in schema files:
//! ```text
//! ./filter-schemas/
//! ├── built-in/               # Shipped with control plane
//! │   ├── header_mutation.yaml
//! │   ├── jwt_auth.yaml
//! │   └── local_rate_limit.yaml
//! └── custom/                 # User-defined (hot-reloadable)
//!     └── my_custom_filter.yaml
//! ```
//!
//! # Adding a New Filter (Dynamic)
//!
//! 1. Create schema file in `filter-schemas/custom/my_filter.yaml`
//! 2. Call `POST /api/v1/admin/filter-schemas/reload`
//! 3. (Optional) Create custom UI form if auto-generated form is insufficient

use crate::domain::filter::{AttachmentPoint, FilterType, FilterTypeMetadata, PerRouteBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use utoipa::ToSchema;

/// Runtime-loaded filter type definition from a schema file.
///
/// This struct captures all metadata needed to support a filter type
/// without compile-time Rust code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterSchemaDefinition {
    /// Unique identifier for this filter type (e.g., "header_mutation")
    pub name: String,

    /// Human-readable display name (e.g., "Header Mutation")
    pub display_name: String,

    /// Description of what this filter does
    pub description: String,

    /// Schema version for compatibility tracking
    #[serde(default = "default_version")]
    pub version: String,

    /// Envoy filter metadata
    pub envoy: EnvoyFilterMetadata,

    /// Filter capabilities and behavior
    pub capabilities: FilterCapabilities,

    /// JSON Schema for validating filter configuration
    pub config_schema: serde_json::Value,

    /// JSON Schema for per-route configuration (if different from main)
    #[serde(default)]
    pub per_route_config_schema: Option<serde_json::Value>,

    /// Protobuf field mapping for conversion to Envoy Any
    #[serde(default)]
    pub proto_mapping: HashMap<String, String>,

    /// UI hints for form generation
    #[serde(default)]
    pub ui_hints: Option<UiHints>,

    /// Source of this schema definition
    #[serde(default)]
    pub source: SchemaSource,

    /// Whether this filter has full implementation support
    #[serde(default = "default_true")]
    pub is_implemented: bool,
}

fn default_version() -> String {
    "1.0".to_string()
}

fn default_true() -> bool {
    true
}

/// Envoy-specific filter metadata for xDS generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvoyFilterMetadata {
    /// Envoy HTTP filter name (e.g., "envoy.filters.http.header_mutation")
    pub http_filter_name: String,

    /// Full protobuf type URL for listener-level configuration
    pub type_url: String,

    /// Full protobuf type URL for per-route configuration (if supported)
    #[serde(default)]
    pub per_route_type_url: Option<String>,
}

/// Filter capabilities describing attachment points and behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterCapabilities {
    /// Valid attachment points for this filter
    pub attachment_points: Vec<AttachmentPoint>,

    /// Whether this filter requires listener-level configuration
    #[serde(default)]
    pub requires_listener_config: bool,

    /// How this filter handles per-route configuration
    #[serde(default)]
    pub per_route_behavior: PerRouteBehavior,
}

impl Default for FilterCapabilities {
    fn default() -> Self {
        Self {
            attachment_points: vec![AttachmentPoint::Route],
            requires_listener_config: false,
            per_route_behavior: PerRouteBehavior::FullConfig,
        }
    }
}

/// UI hints for automatic form generation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UiHints {
    /// Form layout style
    #[serde(default)]
    pub form_layout: FormLayout,

    /// Form sections for grouped fields
    #[serde(default)]
    pub sections: Vec<FormSection>,

    /// Custom form component name (if using a custom form)
    #[serde(default)]
    pub custom_form_component: Option<String>,
}

/// Form layout style for UI generation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FormLayout {
    /// Flat list of fields
    #[default]
    Flat,
    /// Fields organized into sections
    Sections,
    /// Fields organized into tabs
    Tabs,
}

/// A section in a form layout.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FormSection {
    /// Section name/title
    pub name: String,

    /// Field names included in this section
    pub fields: Vec<String>,

    /// Whether the section is collapsible
    #[serde(default)]
    pub collapsible: bool,

    /// Whether the section is collapsed by default
    #[serde(default)]
    pub collapsed_by_default: bool,
}

/// Source of a schema definition.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchemaSource {
    /// Built-in schema shipped with the control plane
    #[default]
    BuiltIn,
    /// Custom user-defined schema
    Custom,
}

impl FilterSchemaDefinition {
    /// Convert schema definition to FilterTypeMetadata for compatibility.
    ///
    /// This bridges the gap between dynamic schemas and the existing
    /// static metadata system.
    pub fn to_metadata(&self) -> FilterTypeMetadata {
        FilterTypeMetadata {
            filter_type: FilterType::from_str_dynamic(&self.name),
            http_filter_name: Box::leak(self.envoy.http_filter_name.clone().into_boxed_str()),
            type_url: Box::leak(self.envoy.type_url.clone().into_boxed_str()),
            per_route_type_url: self
                .envoy
                .per_route_type_url
                .as_ref()
                .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str),
            attachment_points: Box::leak(
                self.capabilities.attachment_points.clone().into_boxed_slice(),
            ),
            requires_listener_config: self.capabilities.requires_listener_config,
            per_route_behavior: self.capabilities.per_route_behavior,
            is_implemented: self.is_implemented,
            description: Box::leak(self.description.clone().into_boxed_str()),
        }
    }

    /// Validate the schema definition itself.
    pub fn validate(&self) -> Result<(), SchemaValidationError> {
        if self.name.is_empty() {
            return Err(SchemaValidationError::MissingField("name".to_string()));
        }
        if self.envoy.http_filter_name.is_empty() {
            return Err(SchemaValidationError::MissingField("envoy.http_filter_name".to_string()));
        }
        if self.envoy.type_url.is_empty() {
            return Err(SchemaValidationError::MissingField("envoy.type_url".to_string()));
        }
        if self.capabilities.attachment_points.is_empty() {
            return Err(SchemaValidationError::MissingField(
                "capabilities.attachment_points".to_string(),
            ));
        }

        // Validate config_schema is a valid JSON Schema object
        if !self.config_schema.is_object() {
            return Err(SchemaValidationError::InvalidConfigSchema(
                "config_schema must be a JSON object".to_string(),
            ));
        }

        Ok(())
    }

    /// Load a schema definition from a YAML file.
    pub fn from_yaml_file(path: &Path) -> Result<Self, SchemaLoadError> {
        let content = std::fs::read_to_string(path).map_err(|e| SchemaLoadError::IoError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let schema: Self = serde_yaml::from_str(&content).map_err(|e| {
            SchemaLoadError::ParseError { path: path.to_path_buf(), message: e.to_string() }
        })?;

        schema
            .validate()
            .map_err(|e| SchemaLoadError::ValidationError { path: path.to_path_buf(), inner: e })?;

        Ok(schema)
    }
}

/// Registry of all filter schemas (built-in + custom).
///
/// The registry provides thread-safe access to filter schema definitions
/// and supports hot reloading of custom schemas without restart.
#[derive(Debug)]
pub struct FilterSchemaRegistry {
    /// Loaded schema definitions keyed by filter name
    schemas: HashMap<String, FilterSchemaDefinition>,

    /// Base directory for schema files
    schema_dir: PathBuf,
}

impl FilterSchemaRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { schemas: HashMap::new(), schema_dir: PathBuf::new() }
    }

    /// Create a registry with built-in schemas only.
    pub fn with_builtin_schemas() -> Self {
        let mut registry = Self::new();
        registry.load_builtin_schemas();
        registry
    }

    /// Load filter schemas from a directory.
    ///
    /// This loads schemas from both `built-in/` and `custom/` subdirectories.
    pub fn load_from_directory(dir: &Path) -> Result<Self, SchemaLoadError> {
        let mut registry = Self { schemas: HashMap::new(), schema_dir: dir.to_path_buf() };

        // Load built-in schemas first
        registry.load_builtin_schemas();

        // Then load from directory (custom schemas can override built-in)
        let built_in_dir = dir.join("built-in");
        if built_in_dir.exists() {
            registry.load_schemas_from_subdir(&built_in_dir, SchemaSource::BuiltIn)?;
        }

        let custom_dir = dir.join("custom");
        if custom_dir.exists() {
            registry.load_schemas_from_subdir(&custom_dir, SchemaSource::Custom)?;
        }

        Ok(registry)
    }

    /// Embedded YAML schema files for built-in filters.
    /// These are compiled into the binary at build time.
    const BUILTIN_SCHEMAS: &[(&str, &str)] = &[
        ("header_mutation", include_str!("../../filter-schemas/built-in/header_mutation.yaml")),
        ("jwt_auth", include_str!("../../filter-schemas/built-in/jwt_auth.yaml")),
        ("local_rate_limit", include_str!("../../filter-schemas/built-in/local_rate_limit.yaml")),
        ("custom_response", include_str!("../../filter-schemas/built-in/custom_response.yaml")),
        ("mcp", include_str!("../../filter-schemas/built-in/mcp.yaml")),
        ("cors", include_str!("../../filter-schemas/built-in/cors.yaml")),
        ("rate_limit", include_str!("../../filter-schemas/built-in/rate_limit.yaml")),
        ("ext_authz", include_str!("../../filter-schemas/built-in/ext_authz.yaml")),
    ];

    /// Load built-in filter schemas from embedded YAML files.
    ///
    /// Built-in schemas are embedded at compile time using `include_str!`.
    /// This approach:
    /// - Keeps schemas versioned with the codebase
    /// - Removes hardcoded Rust schema definitions
    /// - Makes schema format consistent between built-in and custom filters
    fn load_builtin_schemas(&mut self) {
        for (name, yaml_content) in Self::BUILTIN_SCHEMAS {
            match serde_yaml::from_str::<FilterSchemaDefinition>(yaml_content) {
                Ok(mut schema) => {
                    schema.source = SchemaSource::BuiltIn;
                    tracing::debug!(name = %schema.name, "Loaded built-in filter schema");
                    self.schemas.insert(schema.name.clone(), schema);
                }
                Err(e) => {
                    tracing::error!(
                        name = %name,
                        error = %e,
                        "Failed to parse built-in filter schema YAML"
                    );
                    // This is a compile-time embedded file, so this should never fail
                    // in production. Log the error but continue loading other schemas.
                }
            }
        }
    }

    /// Load schemas from a subdirectory.
    fn load_schemas_from_subdir(
        &mut self,
        dir: &Path,
        source: SchemaSource,
    ) -> Result<(), SchemaLoadError> {
        let entries = std::fs::read_dir(dir).map_err(|e| SchemaLoadError::IoError {
            path: dir.to_path_buf(),
            message: e.to_string(),
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext == "yaml" || ext == "yml" || ext == "json" {
                    match FilterSchemaDefinition::from_yaml_file(&path) {
                        Ok(mut schema) => {
                            schema.source = source;
                            tracing::info!(
                                name = %schema.name,
                                source = ?source,
                                path = %path.display(),
                                "Loaded filter schema"
                            );
                            self.schemas.insert(schema.name.clone(), schema);
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "Failed to load filter schema, skipping"
                            );
                            // Continue loading other schemas
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get a schema by filter type name.
    pub fn get(&self, filter_type: &str) -> Option<&FilterSchemaDefinition> {
        self.schemas.get(filter_type)
    }

    /// List all loaded schemas.
    pub fn list_all(&self) -> Vec<&FilterSchemaDefinition> {
        self.schemas.values().collect()
    }

    /// List only implemented filter schemas.
    pub fn list_implemented(&self) -> Vec<&FilterSchemaDefinition> {
        self.schemas.values().filter(|s| s.is_implemented).collect()
    }

    /// List filter type names.
    pub fn filter_types(&self) -> Vec<&str> {
        self.schemas.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a filter type exists in the registry.
    pub fn contains(&self, filter_type: &str) -> bool {
        self.schemas.contains_key(filter_type)
    }

    /// Get the number of loaded schemas.
    pub fn len(&self) -> usize {
        self.schemas.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
    }

    /// Reload schemas from the configured directory.
    ///
    /// This allows hot-reloading of custom schemas without restarting.
    pub fn reload(&mut self) -> Result<(), SchemaLoadError> {
        if self.schema_dir.as_os_str().is_empty() {
            // No directory configured, just reload built-in schemas
            self.schemas.clear();
            self.load_builtin_schemas();
            return Ok(());
        }

        let new_registry = Self::load_from_directory(&self.schema_dir)?;
        self.schemas = new_registry.schemas;
        Ok(())
    }

    /// Get metadata for a filter type using this registry.
    pub fn get_metadata(&self, filter_type: &str) -> Option<FilterTypeMetadata> {
        self.get(filter_type).map(|s| s.to_metadata())
    }
}

impl Default for FilterSchemaRegistry {
    fn default() -> Self {
        Self::with_builtin_schemas()
    }
}

/// Thread-safe shared filter schema registry.
pub type SharedFilterSchemaRegistry = Arc<RwLock<FilterSchemaRegistry>>;

/// Create a new shared filter schema registry with built-in schemas.
pub fn create_shared_registry() -> SharedFilterSchemaRegistry {
    Arc::new(RwLock::new(FilterSchemaRegistry::with_builtin_schemas()))
}

/// Create a shared registry loading from a directory.
pub fn create_shared_registry_from_dir(
    dir: &Path,
) -> Result<SharedFilterSchemaRegistry, SchemaLoadError> {
    let registry = FilterSchemaRegistry::load_from_directory(dir)?;
    Ok(Arc::new(RwLock::new(registry)))
}

/// Error types for schema operations.
#[derive(Debug, thiserror::Error)]
pub enum SchemaLoadError {
    #[error("IO error reading {path}: {message}")]
    IoError { path: PathBuf, message: String },

    #[error("Parse error in {path}: {message}")]
    ParseError { path: PathBuf, message: String },

    #[error("Validation error in {path}: {inner}")]
    ValidationError {
        path: PathBuf,
        #[source]
        inner: SchemaValidationError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SchemaValidationError {
    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid config schema: {0}")]
    InvalidConfigSchema(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_schemas_load() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        assert!(!registry.is_empty());
        assert!(registry.contains("header_mutation"));
        assert!(registry.contains("jwt_auth"));
        assert!(registry.contains("local_rate_limit"));
        assert!(registry.contains("custom_response"));
        assert!(registry.contains("mcp"));
    }

    #[test]
    fn test_get_schema() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let schema = registry.get("header_mutation").expect("should exist");
        assert_eq!(schema.name, "header_mutation");
        assert_eq!(schema.display_name, "Header Mutation");
        assert_eq!(schema.envoy.http_filter_name, "envoy.filters.http.header_mutation");
    }

    #[test]
    fn test_list_implemented() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let implemented = registry.list_implemented();
        assert!(implemented.iter().any(|s| s.name == "header_mutation"));
        assert!(implemented.iter().any(|s| s.name == "jwt_auth"));
        // CORS is not implemented
        assert!(!implemented.iter().any(|s| s.name == "cors"));
    }

    #[test]
    fn test_schema_to_metadata() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        let schema = registry.get("jwt_auth").unwrap();
        let metadata = schema.to_metadata();

        assert_eq!(metadata.http_filter_name, "envoy.filters.http.jwt_authn");
        assert!(metadata.requires_listener_config);
        assert_eq!(metadata.per_route_behavior, PerRouteBehavior::ReferenceOnly);
    }

    #[test]
    fn test_schema_validation() {
        let valid_schema = FilterSchemaDefinition {
            name: "test".to_string(),
            display_name: "Test".to_string(),
            description: "Test filter".to_string(),
            version: "1.0".to_string(),
            envoy: EnvoyFilterMetadata {
                http_filter_name: "envoy.filters.http.test".to_string(),
                type_url: "type.googleapis.com/test".to_string(),
                per_route_type_url: None,
            },
            capabilities: FilterCapabilities {
                attachment_points: vec![AttachmentPoint::Route],
                requires_listener_config: false,
                per_route_behavior: PerRouteBehavior::FullConfig,
            },
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            proto_mapping: HashMap::new(),
            ui_hints: None,
            source: SchemaSource::Custom,
            is_implemented: true,
        };

        assert!(valid_schema.validate().is_ok());

        // Test invalid schema - empty name
        let mut invalid = valid_schema.clone();
        invalid.name = String::new();
        assert!(invalid.validate().is_err());

        // Test invalid schema - empty attachment points
        let mut invalid = valid_schema.clone();
        invalid.capabilities.attachment_points = vec![];
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_reload() {
        let mut registry = FilterSchemaRegistry::with_builtin_schemas();
        let count = registry.len();

        // Reload should maintain built-in schemas
        registry.reload().expect("reload should succeed");
        assert_eq!(registry.len(), count);
    }

    #[test]
    fn test_config_schema_is_json_object() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        for schema in registry.list_all() {
            assert!(
                schema.config_schema.is_object(),
                "Schema {} config_schema should be an object",
                schema.name
            );
        }
    }

    #[test]
    fn test_all_builtin_schemas_have_envoy_metadata() {
        let registry = FilterSchemaRegistry::with_builtin_schemas();
        for schema in registry.list_all() {
            assert!(
                !schema.envoy.http_filter_name.is_empty(),
                "Schema {} should have http_filter_name",
                schema.name
            );
            assert!(
                !schema.envoy.type_url.is_empty(),
                "Schema {} should have type_url",
                schema.name
            );
        }
    }
}
