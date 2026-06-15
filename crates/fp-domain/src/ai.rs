//! AI gateway provider resources (S10).

use crate::error::{DomainError, DomainResult};
use crate::id::{AiProviderId, AiRouteId, SecretId, TeamId};
use crate::validate_name;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiProvider {
    pub id: AiProviderId,
    pub team_id: TeamId,
    pub name: String,
    pub spec: AiProviderSpec,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub const AI_MODEL_HEADER: &str = "x-flowplane-ai-model";
pub const DEFAULT_AI_ROUTE_TIMEOUT_SECS: u32 = 120;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiRoute {
    pub id: AiRouteId,
    pub team_id: TeamId,
    pub name: String,
    pub spec: AiRouteSpec,
    pub status: AiRouteStatus,
    pub materialized: AiRouteMaterializedResources,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AiRouteSpec {
    pub listener_port: u16,
    #[serde(default = "default_chat_path")]
    pub path: String,
    pub backends: Vec<AiRouteBackend>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AiRouteBackend {
    #[schema(value_type = uuid::Uuid)]
    pub provider_id: AiProviderId,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(default = "default_backend_weight")]
    pub weight: u32,
    #[serde(default)]
    pub priority: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AiRouteStatus {
    Active,
    Stale,
}

impl AiRouteStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Stale => "stale",
        }
    }
}

impl std::str::FromStr for AiRouteStatus {
    type Err = DomainError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "active" => Ok(Self::Active),
            "stale" => Ok(Self::Stale),
            _ => Err(DomainError::validation(format!(
                "\"{raw}\" is not a supported AI route status"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AiRouteMaterializedResources {
    pub cluster_names: Vec<String>,
    pub route_config_name: String,
    pub listener_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AiProviderSpec {
    pub kind: AiProviderKind,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    #[schema(value_type = uuid::Uuid)]
    pub credential_secret_id: SecretId,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default = "default_auth_header")]
    pub auth_header: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AiProviderKind {
    Openai,
    OpenaiCompatible,
}

impl AiProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::OpenaiCompatible => "openai-compatible",
        }
    }
}

impl std::str::FromStr for AiProviderKind {
    type Err = DomainError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "openai" => Ok(Self::Openai),
            "openai-compatible" => Ok(Self::OpenaiCompatible),
            _ => Err(DomainError::validation(format!(
                "\"{raw}\" is not a supported AI provider kind"
            ))
            .with_hint("S10 v1.0 supports openai and openai-compatible")),
        }
    }
}

fn default_auth_header() -> String {
    "authorization".into()
}

fn default_chat_path() -> String {
    "/v1/chat/completions".into()
}

fn default_backend_weight() -> u32 {
    1
}

impl AiProviderSpec {
    pub fn validate(&self) -> DomainResult<()> {
        if !matches!(self.base_url.as_str(), url if url.starts_with("https://") || url.starts_with("http://"))
        {
            return Err(DomainError::validation(
                "AI provider base_url must start with http:// or https://",
            ));
        }
        if self.auth_header.trim().is_empty()
            || self.auth_header.bytes().any(|b| b <= b' ' || b == b':')
        {
            return Err(DomainError::validation(
                "AI provider auth_header must be a non-empty HTTP header name",
            ));
        }
        if let Some(prefix) = &self.path_prefix {
            if !prefix.starts_with('/') {
                return Err(DomainError::validation(
                    "AI provider path_prefix must start with /",
                ));
            }
        }
        for model in &self.models {
            if model.trim().is_empty() {
                return Err(DomainError::validation(
                    "AI provider models must not contain empty values",
                ));
            }
        }
        Ok(())
    }
}

pub fn validate_ai_provider_name(name: &str) -> DomainResult<()> {
    validate_name(name)
}

pub fn validate_ai_route_name(name: &str) -> DomainResult<()> {
    validate_name(name)
}

impl AiRouteSpec {
    pub fn validate(&self) -> DomainResult<()> {
        if self.path != "/v1/chat/completions" {
            return Err(DomainError::validation(
                "S10 AI routes only support /v1/chat/completions",
            ));
        }
        if self.backends.is_empty() || self.backends.len() > 32 {
            return Err(DomainError::validation(
                "AI route must include 1-32 backends",
            ));
        }
        for backend in &self.backends {
            if backend.weight == 0 || backend.weight > 1000 {
                return Err(DomainError::validation(
                    "AI route backend weight must be 1-1000",
                ));
            }
            if backend.models.iter().any(|model| model.trim().is_empty()) {
                return Err(DomainError::validation(
                    "AI route backend models must not contain empty values",
                ));
            }
            if backend
                .model_override
                .as_deref()
                .is_some_and(|model| model.trim().is_empty())
            {
                return Err(DomainError::validation(
                    "AI route backend model_override must not be empty",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn provider_spec_rejects_unsupported_kind_and_bad_url() {
        assert!("anthropic".parse::<AiProviderKind>().is_err());
        let spec = AiProviderSpec {
            kind: AiProviderKind::OpenaiCompatible,
            base_url: "file:///tmp/key".into(),
            path_prefix: None,
            credential_secret_id: SecretId::generate(),
            models: vec!["gpt-5".into()],
            auth_header: "authorization".into(),
        };
        assert!(spec.validate().is_err());
    }
}
