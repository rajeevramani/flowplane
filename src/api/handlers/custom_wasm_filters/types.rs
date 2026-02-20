//! Request and response types for custom WASM filter API endpoints

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::storage::CustomWasmFilterData;

/// Path parameters for custom filter by ID
#[derive(Debug, Deserialize)]
pub struct CustomFilterPath {
    pub team: String,
    pub id: String,
}

/// Request to create a custom WASM filter (JSON API)
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateCustomWasmFilterRequest {
    /// Unique name for the filter (alphanumeric, underscores, hyphens)
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    /// Human-readable display name
    #[validate(length(min = 1, max = 200))]
    pub display_name: String,

    /// Optional description
    #[schema(nullable)]
    pub description: Option<String>,

    /// Base64-encoded WASM binary
    pub wasm_binary_base64: String,

    /// JSON Schema for validating filter configuration
    pub config_schema: serde_json::Value,

    /// Optional per-route configuration schema
    #[schema(nullable)]
    pub per_route_config_schema: Option<serde_json::Value>,

    /// UI hints for form generation
    #[schema(nullable)]
    pub ui_hints: Option<serde_json::Value>,

    /// Valid attachment points (listener, route, cluster)
    #[schema(default = json!(["listener", "route"]))]
    pub attachment_points: Option<Vec<String>>,

    /// WASM runtime (default: envoy.wasm.runtime.v8)
    #[schema(default = "envoy.wasm.runtime.v8")]
    pub runtime: Option<String>,

    /// Failure policy: FAIL_CLOSED or FAIL_OPEN (default: FAIL_CLOSED)
    #[schema(default = "FAIL_CLOSED")]
    pub failure_policy: Option<String>,
}

/// Request to update a custom WASM filter (metadata only)
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateCustomWasmFilterRequest {
    /// Human-readable display name
    #[validate(length(min = 1, max = 200))]
    #[schema(nullable)]
    pub display_name: Option<String>,

    /// Optional description (use null to clear)
    #[schema(nullable)]
    pub description: Option<Option<String>>,

    /// JSON Schema for validating filter configuration
    #[schema(nullable)]
    pub config_schema: Option<serde_json::Value>,

    /// Per-route configuration schema (use null to clear)
    #[schema(nullable)]
    pub per_route_config_schema: Option<Option<serde_json::Value>>,

    /// UI hints for form generation (use null to clear)
    #[schema(nullable)]
    pub ui_hints: Option<Option<serde_json::Value>>,

    /// Valid attachment points
    #[schema(nullable)]
    pub attachment_points: Option<Vec<String>>,
}

/// Response for a custom WASM filter
#[derive(Debug, Serialize, ToSchema)]
pub struct CustomWasmFilterResponse {
    /// Unique identifier
    pub id: String,
    /// Filter name (used as type identifier)
    pub name: String,
    /// Human-readable display name
    pub display_name: String,
    /// Optional description
    #[schema(nullable)]
    pub description: Option<String>,
    /// SHA256 hash of the WASM binary
    pub wasm_sha256: String,
    /// Size of WASM binary in bytes
    pub wasm_size_bytes: i64,
    /// JSON Schema for filter configuration
    pub config_schema: serde_json::Value,
    /// Per-route configuration schema
    #[schema(nullable)]
    pub per_route_config_schema: Option<serde_json::Value>,
    /// UI hints for form generation
    #[schema(nullable)]
    pub ui_hints: Option<serde_json::Value>,
    /// Valid attachment points
    pub attachment_points: Vec<String>,
    /// WASM runtime
    pub runtime: String,
    /// Failure policy
    pub failure_policy: String,
    /// Version number (for optimistic locking)
    pub version: i64,
    /// Team that owns this filter
    pub team: String,
    /// User who created this filter
    #[schema(nullable)]
    pub created_by: Option<String>,
    /// Creation timestamp
    pub created_at: String,
    /// Last update timestamp
    pub updated_at: String,
    /// Filter type to use when creating filter instances
    pub filter_type: String,
}

impl CustomWasmFilterResponse {
    /// Create response from database data
    pub fn from_data(data: &CustomWasmFilterData) -> Self {
        Self {
            id: data.id.to_string(),
            name: data.name.clone(),
            display_name: data.display_name.clone(),
            description: data.description.clone(),
            wasm_sha256: data.wasm_sha256.clone(),
            wasm_size_bytes: data.wasm_size_bytes,
            config_schema: data.config_schema.clone(),
            per_route_config_schema: data.per_route_config_schema.clone(),
            ui_hints: data.ui_hints.clone(),
            attachment_points: data.attachment_points.clone(),
            runtime: data.runtime.clone(),
            failure_policy: data.failure_policy.clone(),
            version: data.version,
            team: data.team.clone(),
            created_by: data.created_by.clone(),
            created_at: data.created_at.to_rfc3339(),
            updated_at: data.updated_at.to_rfc3339(),
            filter_type: format!("custom_wasm_{}", data.id),
        }
    }
}
