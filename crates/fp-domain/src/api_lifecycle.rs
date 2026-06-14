//! Config-first API lifecycle types (S8/D-017).
//!
//! These rows are the shared spine for imported specs, learned specs, route bindings, and
//! generated tools. Learning traffic may create or update observations later, but API shape
//! always lands here first instead of crossing a v1-style export/import string bridge.

use crate::error::{DomainError, DomainResult};
use crate::id::{
    ApiDefinitionId, ApiRouteBindingId, ApiToolId, CaptureSessionId, ListenerId, RawObservationId,
    RetentionPolicyId, RouteConfigId, SpecVersionId, TeamId,
};
use crate::identity::validate_name;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};

pub const DEFAULT_RAW_OBSERVATION_TTL_DAYS: i32 = 14;
pub const DEFAULT_MAX_SPEC_VERSIONS: i32 = 50;
pub const DEFAULT_CAPTURE_TARGET_SAMPLE_COUNT: i32 = 1000;
pub const DEFAULT_CAPTURE_MAX_BYTES: i64 = 10 * 1024 * 1024;
pub const DEFAULT_CAPTURE_MAX_DISTINCT_PATHS: i32 = 500;
pub const MAX_API_SPEC_BYTES: usize = 512 * 1024;
pub const MAX_API_TOOL_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_API_JSON_DEPTH: usize = 64;
const MAX_API_JSON_KEYS: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiDefinition {
    pub id: ApiDefinitionId,
    pub team_id: TeamId,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiDefinitionSpec {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
}

impl ApiDefinitionSpec {
    pub fn validate(&self) -> DomainResult<()> {
        if self.display_name.len() > 160 {
            return Err(DomainError::validation(
                "api display_name must be at most 160 characters",
            ));
        }
        if self.description.len() > 4000 {
            return Err(DomainError::validation(
                "api description must be at most 4000 characters",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiRouteBinding {
    pub id: ApiRouteBindingId,
    pub team_id: TeamId,
    pub api_definition_id: ApiDefinitionId,
    pub route_config_id: RouteConfigId,
    pub listener_id: Option<ListenerId>,
    pub name: String,
    pub virtual_host: Option<String>,
    pub route: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiRouteBindingSpec {
    pub route_config_id: RouteConfigId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_id: Option<ListenerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}

impl ApiRouteBindingSpec {
    pub fn validate(&self) -> DomainResult<()> {
        validate_optional_selector("virtual_host", self.virtual_host.as_deref())?;
        validate_optional_selector("route", self.route.as_deref())?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecSourceKind {
    Imported,
    Learned,
    Manual,
}

impl SpecSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Imported => "imported",
            Self::Learned => "learned",
            Self::Manual => "manual",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        match raw {
            "imported" => Ok(Self::Imported),
            "learned" => Ok(Self::Learned),
            "manual" => Ok(Self::Manual),
            other => Err(DomainError::internal(format!(
                "unknown spec source kind \"{other}\" in database"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecFormat {
    OpenApi3,
}

impl SpecFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenApi3 => "openapi3",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        match raw {
            "openapi3" => Ok(Self::OpenApi3),
            other => Err(DomainError::internal(format!(
                "unknown spec format \"{other}\" in database"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpecVersion {
    pub id: SpecVersionId,
    pub team_id: TeamId,
    pub api_definition_id: ApiDefinitionId,
    pub version: i64,
    pub source_kind: SpecSourceKind,
    pub format: SpecFormat,
    pub spec: serde_json::Value,
    pub spec_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecVersionInput {
    pub source_kind: SpecSourceKind,
    #[serde(default = "default_spec_format")]
    pub format: SpecFormat,
    pub spec: serde_json::Value,
}

fn default_spec_format() -> SpecFormat {
    SpecFormat::OpenApi3
}

impl SpecVersionInput {
    pub fn validate(&self) -> DomainResult<()> {
        if !self.spec.is_object() {
            return Err(DomainError::validation("api spec must be a JSON object"));
        }
        validate_json_bounds(
            "api spec",
            &self.spec,
            MAX_API_SPEC_BYTES,
            MAX_API_JSON_DEPTH,
            MAX_API_JSON_KEYS,
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Options,
    Head,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
            Self::Head => "HEAD",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        match raw {
            "GET" => Ok(Self::Get),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "PATCH" => Ok(Self::Patch),
            "DELETE" => Ok(Self::Delete),
            "OPTIONS" => Ok(Self::Options),
            "HEAD" => Ok(Self::Head),
            other => Err(DomainError::internal(format!(
                "unknown HTTP method \"{other}\" in database"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiTool {
    pub id: ApiToolId,
    pub team_id: TeamId,
    pub api_definition_id: ApiDefinitionId,
    pub spec_version_id: SpecVersionId,
    pub name: String,
    pub operation_id: String,
    pub method: HttpMethod,
    pub path: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiToolSpec {
    pub operation_id: String,
    pub method: HttpMethod,
    pub path: String,
    #[serde(default)]
    pub input_schema: serde_json::Value,
    #[serde(default)]
    pub output_schema: serde_json::Value,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl ApiToolSpec {
    pub fn validate(&self) -> DomainResult<()> {
        validate_selector("operation_id", &self.operation_id)?;
        if !self.path.starts_with('/') || self.path.contains('\0') || self.path.len() > 2048 {
            return Err(DomainError::validation(
                "api tool path must start with / and be at most 2048 characters",
            ));
        }
        if !(self.input_schema.is_null() || self.input_schema.is_object()) {
            return Err(DomainError::validation(
                "api tool input_schema must be null or a JSON object",
            ));
        }
        if !self.input_schema.is_null() {
            validate_json_bounds(
                "api tool input_schema",
                &self.input_schema,
                MAX_API_TOOL_SCHEMA_BYTES,
                MAX_API_JSON_DEPTH,
                MAX_API_JSON_KEYS,
            )?;
        }
        if !(self.output_schema.is_null() || self.output_schema.is_object()) {
            return Err(DomainError::validation(
                "api tool output_schema must be null or a JSON object",
            ));
        }
        if !self.output_schema.is_null() {
            validate_json_bounds(
                "api tool output_schema",
                &self.output_schema,
                MAX_API_TOOL_SCHEMA_BYTES,
                MAX_API_JSON_DEPTH,
                MAX_API_JSON_KEYS,
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub id: RetentionPolicyId,
    pub team_id: TeamId,
    pub api_definition_id: Option<ApiDefinitionId>,
    pub name: String,
    pub raw_observation_ttl_days: i32,
    pub max_spec_versions: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionPolicySpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_definition_id: Option<ApiDefinitionId>,
    #[serde(default = "default_raw_ttl")]
    pub raw_observation_ttl_days: i32,
    #[serde(default = "default_max_versions")]
    pub max_spec_versions: i32,
}

fn default_raw_ttl() -> i32 {
    DEFAULT_RAW_OBSERVATION_TTL_DAYS
}

fn default_max_versions() -> i32 {
    DEFAULT_MAX_SPEC_VERSIONS
}

impl RetentionPolicySpec {
    pub fn validate(&self) -> DomainResult<()> {
        if !(1..=365).contains(&self.raw_observation_ttl_days) {
            return Err(DomainError::validation(
                "raw_observation_ttl_days must be between 1 and 365",
            ));
        }
        if !(1..=500).contains(&self.max_spec_versions) {
            return Err(DomainError::validation(
                "max_spec_versions must be between 1 and 500",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSessionStatus {
    Capturing,
    Completed,
    Cancelled,
    Failed,
}

impl CaptureSessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capturing => "capturing",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> DomainResult<Self> {
        match raw {
            "capturing" => Ok(Self::Capturing),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            other => Err(DomainError::internal(format!(
                "unknown capture session status \"{other}\" in database"
            ))),
        }
    }

    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureSession {
    pub id: CaptureSessionId,
    pub team_id: TeamId,
    pub name: String,
    pub status: CaptureSessionStatus,
    pub api_definition_id: Option<ApiDefinitionId>,
    pub route_config_id: Option<RouteConfigId>,
    pub listener_id: Option<ListenerId>,
    pub virtual_host: Option<String>,
    pub route: Option<String>,
    pub target_sample_count: i32,
    pub max_duration_seconds: Option<i32>,
    pub max_bytes: i64,
    pub max_distinct_paths: i32,
    pub sample_count: i64,
    pub byte_count: i64,
    pub path_count: i64,
    pub drop_count: i64,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSessionSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_definition_id: Option<ApiDefinitionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_config_id: Option<RouteConfigId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_id: Option<ListenerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default = "default_capture_target_sample_count")]
    pub target_sample_count: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<i32>,
    #[serde(default = "default_capture_max_bytes")]
    pub max_bytes: i64,
    #[serde(default = "default_capture_max_distinct_paths")]
    pub max_distinct_paths: i32,
}

fn default_capture_target_sample_count() -> i32 {
    DEFAULT_CAPTURE_TARGET_SAMPLE_COUNT
}

fn default_capture_max_bytes() -> i64 {
    DEFAULT_CAPTURE_MAX_BYTES
}

fn default_capture_max_distinct_paths() -> i32 {
    DEFAULT_CAPTURE_MAX_DISTINCT_PATHS
}

impl CaptureSessionSpec {
    pub fn validate(&self) -> DomainResult<()> {
        let api_scoped = self.api_definition_id.is_some();
        let route_scoped = self.route_config_id.is_some();
        if api_scoped == route_scoped {
            return Err(DomainError::validation(
                "learning session must target exactly one of api_definition_id or route_config_id",
            ));
        }
        if api_scoped
            && (self.listener_id.is_some() || self.virtual_host.is_some() || self.route.is_some())
        {
            return Err(DomainError::validation(
                "listener_id, virtual_host, and route are only valid with route_config_id scope",
            ));
        }
        validate_optional_selector("virtual_host", self.virtual_host.as_deref())?;
        validate_optional_selector("route", self.route.as_deref())?;
        if !(1..=100_000).contains(&self.target_sample_count) {
            return Err(DomainError::validation(
                "target_sample_count must be between 1 and 100000",
            ));
        }
        if let Some(seconds) = self.max_duration_seconds {
            if !(1..=86_400).contains(&seconds) {
                return Err(DomainError::validation(
                    "max_duration_seconds must be between 1 and 86400",
                ));
            }
        }
        if !(1..=1_073_741_824).contains(&self.max_bytes) {
            return Err(DomainError::validation(
                "max_bytes must be between 1 and 1073741824",
            ));
        }
        if !(1..=10_000).contains(&self.max_distinct_paths) {
            return Err(DomainError::validation(
                "max_distinct_paths must be between 1 and 10000",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawObservation {
    pub id: RawObservationId,
    pub team_id: TeamId,
    pub capture_session_id: CaptureSessionId,
    pub request_id: String,
    pub method: String,
    pub path: String,
    pub response_status: Option<i32>,
    pub request_headers: serde_json::Value,
    pub response_headers: serde_json::Value,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub request_body_truncated: bool,
    pub response_body_truncated: bool,
    pub request_body_bytes: i64,
    pub response_body_bytes: i64,
    pub metadata_seen: bool,
    pub body_seen: bool,
    pub observed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationIngest {
    pub request_id: String,
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_status: Option<i32>,
    #[serde(default)]
    pub request_headers: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub response_headers: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,
    #[serde(default)]
    pub request_body_truncated: bool,
    #[serde(default)]
    pub response_body_truncated: bool,
    #[serde(default)]
    pub metadata_seen: bool,
    #[serde(default)]
    pub body_seen: bool,
    pub observed_at: DateTime<Utc>,
}

impl ObservationIngest {
    pub fn validate(&self) -> DomainResult<()> {
        validate_selector("request_id", &self.request_id)?;
        if self.method.is_empty()
            || self.method.len() > 16
            || !self.method.chars().all(|c| c.is_ascii_uppercase())
        {
            return Err(DomainError::validation(
                "observation method must be an uppercase HTTP method",
            ));
        }
        if !self.path.starts_with('/') || self.path.contains('\0') || self.path.len() > 2048 {
            return Err(DomainError::validation(
                "observation path must start with / and be at most 2048 characters",
            ));
        }
        if let Some(status) = self.response_status {
            if !(100..=599).contains(&status) {
                return Err(DomainError::validation(
                    "observation response_status must be between 100 and 599",
                ));
            }
        }
        for (label, headers) in [
            ("request_headers", &self.request_headers),
            ("response_headers", &self.response_headers),
        ] {
            if headers.len() > 64 {
                return Err(DomainError::validation(format!(
                    "{label} must contain at most 64 headers"
                )));
            }
            for (name, value) in headers {
                validate_header_name(name)?;
                if !value.is_string() {
                    return Err(DomainError::validation(format!(
                        "{label}.{name} must be a string"
                    )));
                }
                if value
                    .as_str()
                    .is_some_and(|v| v.len() > 4096 || v.contains('\0'))
                {
                    return Err(DomainError::validation(format!(
                        "{label}.{name} must be at most 4096 chars and contain no NUL"
                    )));
                }
            }
        }
        if self
            .request_body
            .as_ref()
            .is_some_and(|v| v.len() > 64 * 1024)
            || self
                .response_body
                .as_ref()
                .is_some_and(|v| v.len() > 64 * 1024)
        {
            return Err(DomainError::validation(
                "observation bodies must be at most 65536 characters each",
            ));
        }
        if !(self.metadata_seen || self.body_seen) {
            return Err(DomainError::validation(
                "observation must include metadata_seen or body_seen",
            ));
        }
        Ok(())
    }
}

fn validate_header_name(name: &str) -> DomainResult<()> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(DomainError::validation(
            "header names must be 1-128 chars of ASCII alnum, - or _",
        ));
    }
    Ok(())
}

fn validate_json_bounds(
    label: &str,
    value: &serde_json::Value,
    max_bytes: usize,
    max_depth: usize,
    max_keys: usize,
) -> DomainResult<()> {
    let mut stack = vec![(value, 1usize)];
    let mut keys = 0usize;

    while let Some((value, depth)) = stack.pop() {
        if depth > max_depth {
            return Err(DomainError::validation(format!(
                "{label} nesting depth must be at most {max_depth}"
            )));
        }
        match value {
            serde_json::Value::Object(map) => {
                keys = keys.saturating_add(map.len());
                if keys > max_keys {
                    return Err(DomainError::validation(format!(
                        "{label} must contain at most {max_keys} object keys"
                    )));
                }
                stack.extend(map.values().map(|value| (value, depth + 1)));
            }
            serde_json::Value::Array(items) => {
                stack.extend(items.iter().map(|value| (value, depth + 1)));
            }
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {}
        }
    }

    serde_json::to_writer(
        CappedJsonWriter {
            written: 0,
            cap: max_bytes,
        },
        value,
    )
    .map_err(|_| {
        DomainError::validation(format!(
            "{label} JSON encoding must be at most {max_bytes} bytes"
        ))
    })?;
    Ok(())
}

struct CappedJsonWriter {
    written: usize,
    cap: usize,
}

impl Write for CappedJsonWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written = self.written.saturating_add(buf.len());
        if self.written > self.cap {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "json byte limit exceeded",
            ));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn validate_optional_selector(label: &str, value: Option<&str>) -> DomainResult<()> {
    if let Some(value) = value {
        validate_selector(label, value)?;
    }
    Ok(())
}

fn validate_selector(label: &str, value: &str) -> DomainResult<()> {
    if value.is_empty()
        || value.len() > 200
        || value
            .chars()
            .any(|c| c.is_control() || c == '\0' || c == '/')
    {
        return Err(DomainError::validation(format!(
            "{label} must be 1-200 characters with no control characters or /"
        )));
    }
    Ok(())
}

pub fn validate_api_name(name: &str) -> DomainResult<()> {
    validate_name(name)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn spec_versions_require_json_object() {
        let input = SpecVersionInput {
            source_kind: SpecSourceKind::Imported,
            format: SpecFormat::OpenApi3,
            spec: serde_json::json!("not-object"),
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn spec_versions_reject_oversized_json() {
        let input = SpecVersionInput {
            source_kind: SpecSourceKind::Imported,
            format: SpecFormat::OpenApi3,
            spec: serde_json::json!({
                "openapi": "3.1.0",
                "info": { "title": "large", "version": "1" },
                "paths": {},
                "description": "x".repeat(MAX_API_SPEC_BYTES),
            }),
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn spec_versions_reject_excessive_nesting() {
        let mut nested = serde_json::json!({});
        for _ in 0..MAX_API_JSON_DEPTH {
            nested = serde_json::json!({ "next": nested });
        }

        let input = SpecVersionInput {
            source_kind: SpecSourceKind::Imported,
            format: SpecFormat::OpenApi3,
            spec: nested,
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn tool_specs_validate_path_and_schema_shape() {
        let tool = ApiToolSpec {
            operation_id: "listUsers".into(),
            method: HttpMethod::Get,
            path: "/users".into(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::Value::Null,
            enabled: true,
        };
        assert!(tool.validate().is_ok());

        let bad = ApiToolSpec {
            path: "users".into(),
            ..tool
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn tool_specs_reject_oversized_schemas() {
        let tool = ApiToolSpec {
            operation_id: "listUsers".into(),
            method: HttpMethod::Get,
            path: "/users".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "description": "x".repeat(MAX_API_TOOL_SCHEMA_BYTES),
            }),
            output_schema: serde_json::Value::Null,
            enabled: true,
        };
        assert!(tool.validate().is_err());
    }

    #[test]
    fn retention_policy_bounds_are_explicit() {
        let policy = RetentionPolicySpec {
            api_definition_id: None,
            raw_observation_ttl_days: 14,
            max_spec_versions: 50,
        };
        assert!(policy.validate().is_ok());

        let bad = RetentionPolicySpec {
            raw_observation_ttl_days: 0,
            ..policy
        };
        assert!(bad.validate().is_err());
    }
}
