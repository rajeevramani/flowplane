//! AI gateway provider resources (S10).

use crate::error::{DomainError, DomainResult};
use crate::id::{AiProviderId, SecretId, TeamId};
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
