//! Custom WASM Filter domain types
//!
//! This module contains domain entities for user-uploaded custom WASM filters
//! that can be registered and used as filter types in the system.
//!
//! ## Overview
//!
//! Custom WASM filters allow teams to upload their own WebAssembly filter
//! binaries with configuration schemas. These are stored in the database
//! and injected as `inline_bytes` during xDS generation.

use crate::domain::filter::AttachmentPoint;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

/// WASM runtime options supported by Envoy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum WasmRuntime {
    /// V8 JavaScript engine (recommended, most compatible)
    #[default]
    #[serde(rename = "envoy.wasm.runtime.v8")]
    V8,
    /// Wasmtime runtime
    #[serde(rename = "envoy.wasm.runtime.wasmtime")]
    Wasmtime,
    /// WAMR (WebAssembly Micro Runtime)
    #[serde(rename = "envoy.wasm.runtime.wamr")]
    Wamr,
    /// Null runtime (for testing)
    #[serde(rename = "envoy.wasm.runtime.null")]
    Null,
}

impl WasmRuntime {
    /// Get the Envoy runtime string
    pub fn as_envoy_runtime(&self) -> &'static str {
        match self {
            Self::V8 => "envoy.wasm.runtime.v8",
            Self::Wasmtime => "envoy.wasm.runtime.wasmtime",
            Self::Wamr => "envoy.wasm.runtime.wamr",
            Self::Null => "envoy.wasm.runtime.null",
        }
    }
}

impl FromStr for WasmRuntime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "envoy.wasm.runtime.v8" | "v8" => Ok(Self::V8),
            "envoy.wasm.runtime.wasmtime" | "wasmtime" => Ok(Self::Wasmtime),
            "envoy.wasm.runtime.wamr" | "wamr" => Ok(Self::Wamr),
            "envoy.wasm.runtime.null" | "null" => Ok(Self::Null),
            _ => Err(format!("Unknown WASM runtime: {}", s)),
        }
    }
}

impl fmt::Display for WasmRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_envoy_runtime())
    }
}

/// WASM failure policy - what happens when the filter fails
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WasmFailurePolicy {
    /// Fail closed - reject requests if filter fails (default, safer)
    #[default]
    FailClosed,
    /// Fail open - allow requests to pass if filter fails
    FailOpen,
}

impl WasmFailurePolicy {
    /// Get the string representation for Envoy
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FailClosed => "FAIL_CLOSED",
            Self::FailOpen => "FAIL_OPEN",
        }
    }
}

impl FromStr for WasmFailurePolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "FAIL_CLOSED" | "FAILCLOSED" => Ok(Self::FailClosed),
            "FAIL_OPEN" | "FAILOPEN" => Ok(Self::FailOpen),
            _ => Err(format!("Unknown failure policy: {}", s)),
        }
    }
}

impl fmt::Display for WasmFailurePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// WASM binary magic bytes - all valid WASM binaries start with these 4 bytes
pub const WASM_MAGIC_BYTES: [u8; 4] = [0x00, 0x61, 0x73, 0x6d]; // \0asm

/// Maximum allowed WASM binary size (10MB)
pub const MAX_WASM_BINARY_SIZE: usize = 10 * 1024 * 1024;

/// Validation error for custom WASM filters
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomWasmFilterValidationError {
    /// Name is empty or invalid
    InvalidName(String),
    /// Display name is empty
    EmptyDisplayName,
    /// WASM binary is empty
    EmptyWasmBinary,
    /// WASM binary exceeds size limit
    WasmBinaryTooLarge { size: usize, max_size: usize },
    /// WASM binary has invalid magic bytes
    InvalidWasmMagic,
    /// Config schema is not valid JSON
    InvalidConfigSchema(String),
    /// Config schema is not a valid JSON Schema
    InvalidJsonSchema(String),
    /// Attachment points list is empty
    EmptyAttachmentPoints,
    /// Invalid attachment point
    InvalidAttachmentPoint(String),
}

impl fmt::Display for CustomWasmFilterValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(msg) => write!(f, "Invalid name: {}", msg),
            Self::EmptyDisplayName => write!(f, "Display name cannot be empty"),
            Self::EmptyWasmBinary => write!(f, "WASM binary cannot be empty"),
            Self::WasmBinaryTooLarge { size, max_size } => {
                write!(f, "WASM binary size {} bytes exceeds maximum {} bytes", size, max_size)
            }
            Self::InvalidWasmMagic => {
                write!(f, "Invalid WASM binary: missing magic bytes (\\0asm)")
            }
            Self::InvalidConfigSchema(msg) => write!(f, "Invalid config schema: {}", msg),
            Self::InvalidJsonSchema(msg) => write!(f, "Invalid JSON Schema: {}", msg),
            Self::EmptyAttachmentPoints => write!(f, "At least one attachment point required"),
            Self::InvalidAttachmentPoint(point) => {
                write!(f, "Invalid attachment point: {}", point)
            }
        }
    }
}

impl std::error::Error for CustomWasmFilterValidationError {}

/// Specification for creating a custom WASM filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CustomWasmFilterSpec {
    /// Unique name for the filter (used as filter type identifier)
    pub name: String,
    /// Human-readable display name
    pub display_name: String,
    /// Optional description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for validating filter configuration
    pub config_schema: serde_json::Value,
    /// Optional per-route configuration schema
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_route_config_schema: Option<serde_json::Value>,
    /// UI hints for form generation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_hints: Option<serde_json::Value>,
    /// Valid attachment points for this filter
    #[serde(default = "default_attachment_points")]
    pub attachment_points: Vec<AttachmentPoint>,
    /// WASM runtime to use
    #[serde(default)]
    pub runtime: WasmRuntime,
    /// Failure policy
    #[serde(default)]
    pub failure_policy: WasmFailurePolicy,
}

fn default_attachment_points() -> Vec<AttachmentPoint> {
    vec![AttachmentPoint::Listener, AttachmentPoint::Route]
}

impl CustomWasmFilterSpec {
    /// Validate the specification (excluding binary validation)
    pub fn validate(&self) -> Result<(), CustomWasmFilterValidationError> {
        // Validate name
        if self.name.is_empty() {
            return Err(CustomWasmFilterValidationError::InvalidName(
                "Name cannot be empty".to_string(),
            ));
        }
        if !self.name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            return Err(CustomWasmFilterValidationError::InvalidName(
                "Name must contain only alphanumeric characters, underscores, or hyphens"
                    .to_string(),
            ));
        }

        // Validate display name
        if self.display_name.trim().is_empty() {
            return Err(CustomWasmFilterValidationError::EmptyDisplayName);
        }

        // Validate config_schema is a JSON object
        if !self.config_schema.is_object() {
            return Err(CustomWasmFilterValidationError::InvalidConfigSchema(
                "Config schema must be a JSON object".to_string(),
            ));
        }

        // Validate attachment points
        if self.attachment_points.is_empty() {
            return Err(CustomWasmFilterValidationError::EmptyAttachmentPoints);
        }

        Ok(())
    }
}

/// Validate a WASM binary
pub fn validate_wasm_binary(binary: &[u8]) -> Result<(), CustomWasmFilterValidationError> {
    // Check not empty
    if binary.is_empty() {
        return Err(CustomWasmFilterValidationError::EmptyWasmBinary);
    }

    // Check size limit
    if binary.len() > MAX_WASM_BINARY_SIZE {
        return Err(CustomWasmFilterValidationError::WasmBinaryTooLarge {
            size: binary.len(),
            max_size: MAX_WASM_BINARY_SIZE,
        });
    }

    // Check magic bytes
    if binary.len() < 4 || binary[..4] != WASM_MAGIC_BYTES {
        return Err(CustomWasmFilterValidationError::InvalidWasmMagic);
    }

    Ok(())
}

/// Compute SHA256 hash of binary data
pub fn compute_sha256(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex::encode(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_runtime_from_str() {
        assert_eq!(WasmRuntime::from_str("envoy.wasm.runtime.v8").unwrap(), WasmRuntime::V8);
        assert_eq!(WasmRuntime::from_str("v8").unwrap(), WasmRuntime::V8);
        assert_eq!(WasmRuntime::from_str("wasmtime").unwrap(), WasmRuntime::Wasmtime);
        assert!(WasmRuntime::from_str("unknown").is_err());
    }

    #[test]
    fn test_wasm_runtime_display() {
        assert_eq!(WasmRuntime::V8.to_string(), "envoy.wasm.runtime.v8");
        assert_eq!(WasmRuntime::Wasmtime.to_string(), "envoy.wasm.runtime.wasmtime");
    }

    #[test]
    fn test_failure_policy_from_str() {
        assert_eq!(
            WasmFailurePolicy::from_str("FAIL_CLOSED").unwrap(),
            WasmFailurePolicy::FailClosed
        );
        assert_eq!(WasmFailurePolicy::from_str("fail_open").unwrap(), WasmFailurePolicy::FailOpen);
    }

    #[test]
    fn test_validate_wasm_binary_empty() {
        let result = validate_wasm_binary(&[]);
        assert!(matches!(result, Err(CustomWasmFilterValidationError::EmptyWasmBinary)));
    }

    #[test]
    fn test_validate_wasm_binary_too_large() {
        let large_binary = vec![0u8; MAX_WASM_BINARY_SIZE + 1];
        let result = validate_wasm_binary(&large_binary);
        assert!(matches!(result, Err(CustomWasmFilterValidationError::WasmBinaryTooLarge { .. })));
    }

    #[test]
    fn test_validate_wasm_binary_invalid_magic() {
        let invalid_binary = vec![0x00, 0x00, 0x00, 0x00, 0x01, 0x02];
        let result = validate_wasm_binary(&invalid_binary);
        assert!(matches!(result, Err(CustomWasmFilterValidationError::InvalidWasmMagic)));
    }

    #[test]
    fn test_validate_wasm_binary_valid() {
        // Valid WASM magic bytes followed by some content
        let valid_binary = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let result = validate_wasm_binary(&valid_binary);
        assert!(result.is_ok());
    }

    #[test]
    fn test_compute_sha256() {
        let data = b"hello world";
        let hash = compute_sha256(data);
        // Known SHA256 hash of "hello world"
        assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    #[test]
    fn test_spec_validation_empty_name() {
        let spec = CustomWasmFilterSpec {
            name: "".to_string(),
            display_name: "Test Filter".to_string(),
            description: None,
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec![AttachmentPoint::Listener],
            runtime: WasmRuntime::V8,
            failure_policy: WasmFailurePolicy::FailClosed,
        };
        assert!(matches!(spec.validate(), Err(CustomWasmFilterValidationError::InvalidName(_))));
    }

    #[test]
    fn test_spec_validation_invalid_name_chars() {
        let spec = CustomWasmFilterSpec {
            name: "test filter!".to_string(),
            display_name: "Test Filter".to_string(),
            description: None,
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec![AttachmentPoint::Listener],
            runtime: WasmRuntime::V8,
            failure_policy: WasmFailurePolicy::FailClosed,
        };
        assert!(matches!(spec.validate(), Err(CustomWasmFilterValidationError::InvalidName(_))));
    }

    #[test]
    fn test_spec_validation_valid() {
        let spec = CustomWasmFilterSpec {
            name: "add-header".to_string(),
            display_name: "Add Header".to_string(),
            description: Some("Adds headers to requests".to_string()),
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "headers": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "value": {"type": "string"}
                            }
                        }
                    }
                }
            }),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec![AttachmentPoint::Listener, AttachmentPoint::Route],
            runtime: WasmRuntime::V8,
            failure_policy: WasmFailurePolicy::FailClosed,
        };
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_spec_serialization() {
        let spec = CustomWasmFilterSpec {
            name: "test-filter".to_string(),
            display_name: "Test Filter".to_string(),
            description: None,
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec![AttachmentPoint::Listener],
            runtime: WasmRuntime::V8,
            failure_policy: WasmFailurePolicy::FailClosed,
        };

        let json = serde_json::to_string(&spec).expect("Failed to serialize");
        let parsed: CustomWasmFilterSpec =
            serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(parsed.name, spec.name);
        assert_eq!(parsed.display_name, spec.display_name);
    }
}
