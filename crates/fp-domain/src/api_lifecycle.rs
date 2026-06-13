//! Config-first API lifecycle types (S8/D-017).
//!
//! These rows are the shared spine for imported specs, learned specs, route bindings, and
//! generated tools. Learning traffic may create or update observations later, but API shape
//! always lands here first instead of crossing a v1-style export/import string bridge.

use crate::error::{DomainError, DomainResult};
use crate::id::{
    ApiDefinitionId, ApiRouteBindingId, ApiToolId, ListenerId, RetentionPolicyId, RouteConfigId,
    SpecVersionId, TeamId,
};
use crate::identity::validate_name;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const DEFAULT_RAW_OBSERVATION_TTL_DAYS: i32 = 14;
pub const DEFAULT_MAX_SPEC_VERSIONS: i32 = 50;

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
        if !(self.output_schema.is_null() || self.output_schema.is_object()) {
            return Err(DomainError::validation(
                "api tool output_schema must be null or a JSON object",
            ));
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
