//! AI gateway provider resources (S10).

use crate::error::{DomainError, DomainResult};
use crate::id::{AiBudgetId, AiProviderId, AiRouteId, ListenerId, RouteConfigId, SecretId, TeamId};
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
pub const MAX_AI_REQUEST_BODY_BYTES: usize = 1024 * 1024;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiBudget {
    pub id: AiBudgetId,
    pub team_id: TeamId,
    pub name: String,
    pub spec: AiBudgetSpec,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AiBudgetSpec {
    pub mode: AiBudgetMode,
    pub limit_units: u64,
    #[serde(default = "default_budget_window_seconds")]
    pub window_seconds: u32,
    #[schema(value_type = Option<uuid::Uuid>)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<AiProviderId>,
    #[schema(value_type = Option<uuid::Uuid>)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_config_id: Option<RouteConfigId>,
    #[serde(default)]
    pub prompt_token_weight: u32,
    #[serde(default = "default_completion_weight")]
    pub completion_token_weight: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AiBudgetMode {
    Shadow,
    Enforcing,
}

impl AiBudgetMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Enforcing => "enforcing",
        }
    }
}

impl std::str::FromStr for AiBudgetMode {
    type Err = DomainError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "shadow" => Ok(Self::Shadow),
            "enforcing" => Ok(Self::Enforcing),
            _ => Err(DomainError::validation(format!(
                "\"{raw}\" is not a supported AI budget mode"
            ))),
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiChatRequest {
    pub model: String,
    pub body: Vec<u8>,
    pub include_usage_injected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OpenAiTokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AiUsageSummary {
    #[schema(value_type = Option<uuid::Uuid>)]
    pub route_config_id: Option<RouteConfigId>,
    #[schema(value_type = Option<uuid::Uuid>)]
    pub provider_id: Option<AiProviderId>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub event_count: u64,
}

/// One end-to-end trace row per AI data-plane request, keyed by the server-owned
/// `x-request-id`. Hop detail is carried only in `hops` (a JSON array of
/// `{hop, started_at, ended_at, outcome, origin, detail}` entries) — there is no
/// relational hop projection, and by construction no prompt/completion/credential
/// payload field exists anywhere in the row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AiTraceEvent {
    #[schema(value_type = uuid::Uuid)]
    pub id: uuid::Uuid,
    #[schema(value_type = uuid::Uuid)]
    pub team_id: TeamId,
    pub request_id: String,
    pub trace_id: Option<String>,
    #[schema(value_type = uuid::Uuid)]
    pub route_config_id: RouteConfigId,
    #[schema(value_type = Option<uuid::Uuid>)]
    pub listener_id: Option<ListenerId>,
    #[schema(value_type = Option<uuid::Uuid>)]
    pub provider_id: Option<AiProviderId>,
    pub model: Option<String>,
    pub status_code: Option<i32>,
    pub failure_hop: Option<String>,
    #[schema(value_type = Object)]
    pub hops: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Per-team AI trace retention policy: the TTL stamped onto new `ai_trace_events` rows at
/// insert time. At most one row per team; absence means the built-in 30-day default applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiRetentionPolicy {
    pub id: uuid::Uuid,
    pub team_id: TeamId,
    pub trace_ttl_days: i32,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub const MAX_AI_TRACE_TTL_DAYS: i32 = 365;

/// Bounds mirror `raw_observation_ttl_days` (1..=365): a zero/negative TTL would expire rows
/// at insert, and an unbounded one defeats retention entirely.
pub fn validate_trace_ttl_days(days: i32) -> DomainResult<()> {
    if !(1..=MAX_AI_TRACE_TTL_DAYS).contains(&days) {
        return Err(DomainError::validation(format!(
            "trace_ttl_days must be between 1 and {MAX_AI_TRACE_TTL_DAYS}, got {days}"
        )));
    }
    Ok(())
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

fn default_budget_window_seconds() -> u32 {
    30 * 24 * 60 * 60
}

fn default_completion_weight() -> u32 {
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
        let Some(authority) = self.base_url.split_once("://").map(|(_, rest)| rest) else {
            return Err(DomainError::validation(
                "AI provider base_url must start with http:// or https://",
            ));
        };
        if authority.is_empty()
            || authority.contains('?')
            || authority.contains('#')
            || authority
                .find('/')
                .is_some_and(|idx| !authority[idx..].trim_matches('/').is_empty())
        {
            return Err(DomainError::validation(
                "AI provider base_url must not include a path, query, or fragment; use path_prefix for upstream paths",
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

pub fn validate_ai_budget_name(name: &str) -> DomainResult<()> {
    validate_name(name)
}

impl AiBudgetSpec {
    pub fn validate(&self) -> DomainResult<()> {
        if self.limit_units == 0 {
            return Err(DomainError::validation(
                "AI budget limit_units must be greater than zero",
            ));
        }
        if self.window_seconds == 0 {
            return Err(DomainError::validation(
                "AI budget window_seconds must be greater than zero",
            ));
        }
        if self.prompt_token_weight == 0 && self.completion_token_weight == 0 {
            return Err(DomainError::validation(
                "AI budget token weights must not both be zero",
            ));
        }
        Ok(())
    }

    pub fn units_for_usage(&self, usage: OpenAiTokenUsage) -> DomainResult<u64> {
        let prompt = usage
            .prompt_tokens
            .checked_mul(u64::from(self.prompt_token_weight))
            .ok_or_else(|| DomainError::validation("AI budget prompt units overflow"))?;
        let completion = usage
            .completion_tokens
            .checked_mul(u64::from(self.completion_token_weight))
            .ok_or_else(|| DomainError::validation("AI budget completion units overflow"))?;
        prompt
            .checked_add(completion)
            .ok_or_else(|| DomainError::validation("AI budget units overflow"))
    }
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

    pub fn eligible_backend_indexes(&self, model: &str) -> Vec<usize> {
        self.backends
            .iter()
            .enumerate()
            .filter_map(|(idx, backend)| {
                (backend.models.is_empty() || backend.models.iter().any(|m| m == model))
                    .then_some(idx)
            })
            .collect()
    }
}

pub fn prepare_openai_chat_request(body: &[u8]) -> DomainResult<OpenAiChatRequest> {
    if body.len() > MAX_AI_REQUEST_BODY_BYTES {
        return Err(DomainError::validation(format!(
            "AI request body exceeds {} bytes",
            MAX_AI_REQUEST_BODY_BYTES
        )));
    }
    let mut value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| DomainError::validation("AI request body must be JSON"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| DomainError::validation("AI request body must be a JSON object"))?;
    let model = object
        .get("model")
        .and_then(|value| value.as_str())
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| DomainError::validation("AI request body must include model"))?
        .to_string();

    let include_usage_injected = if object.get("stream").and_then(|v| v.as_bool()) == Some(true) {
        force_stream_usage(object)?
    } else {
        false
    };
    let body = if include_usage_injected {
        serde_json::to_vec(&value)
            .map_err(|err| DomainError::internal(format!("serialize AI request body: {err}")))?
    } else {
        body.to_vec()
    };
    Ok(OpenAiChatRequest {
        model,
        body,
        include_usage_injected,
    })
}

pub fn rewrite_openai_chat_request_model(body: &[u8], model: &str) -> DomainResult<Vec<u8>> {
    if body.len() > MAX_AI_REQUEST_BODY_BYTES {
        return Err(DomainError::validation(format!(
            "AI request body exceeds {} bytes",
            MAX_AI_REQUEST_BODY_BYTES
        )));
    }
    let mut value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| DomainError::validation("AI request body must be JSON"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| DomainError::validation("AI request body must be a JSON object"))?;
    object.insert("model".into(), serde_json::Value::String(model.to_string()));
    serde_json::to_vec(&value)
        .map_err(|err| DomainError::internal(format!("serialize AI request body: {err}")))
}

fn force_stream_usage(
    object: &mut serde_json::Map<String, serde_json::Value>,
) -> DomainResult<bool> {
    match object.get_mut("stream_options") {
        Some(serde_json::Value::Object(options)) => {
            if options.get("include_usage").and_then(|v| v.as_bool()) == Some(true) {
                Ok(false)
            } else {
                options.insert("include_usage".into(), serde_json::Value::Bool(true));
                Ok(true)
            }
        }
        Some(_) => Err(DomainError::validation(
            "AI request stream_options must be an object",
        )),
        None => {
            object.insert(
                "stream_options".into(),
                serde_json::json!({"include_usage": true}),
            );
            Ok(true)
        }
    }
}

pub fn openai_usage_from_json(value: &serde_json::Value) -> Option<OpenAiTokenUsage> {
    let usage = value.get("usage")?;
    Some(OpenAiTokenUsage {
        prompt_tokens: usage.get("prompt_tokens")?.as_u64()?,
        completion_tokens: usage.get("completion_tokens")?.as_u64()?,
        total_tokens: usage.get("total_tokens")?.as_u64()?,
    })
}

pub fn strip_synthetic_openai_usage_sse(
    body: &str,
    include_usage_injected: bool,
) -> (String, Option<OpenAiTokenUsage>) {
    let mut usage = None;
    if !include_usage_injected {
        for event in body.split("\n\n") {
            if let Some(parsed) = usage_from_sse_event(event) {
                usage = Some(parsed);
            }
        }
        return (body.to_string(), usage);
    }

    let mut kept = Vec::new();
    for event in body.split("\n\n") {
        if event.is_empty() {
            continue;
        }
        if let Some(parsed) = usage_from_sse_event(event) {
            usage = Some(parsed);
        } else {
            kept.push(event);
        }
    }
    let stripped = if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", kept.join("\n\n"))
    };
    (stripped, usage)
}

fn usage_from_sse_event(event: &str) -> Option<OpenAiTokenUsage> {
    let data = event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != "[DONE]")?;
    let value = serde_json::from_str::<serde_json::Value>(data).ok()?;
    openai_usage_from_json(&value)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn trace_ttl_days_bounds_are_explicit() {
        assert!(validate_trace_ttl_days(1).is_ok());
        assert!(validate_trace_ttl_days(30).is_ok());
        assert!(validate_trace_ttl_days(MAX_AI_TRACE_TTL_DAYS).is_ok());
        assert!(validate_trace_ttl_days(0).is_err());
        assert!(validate_trace_ttl_days(-7).is_err());
        assert!(validate_trace_ttl_days(MAX_AI_TRACE_TTL_DAYS + 1).is_err());
    }

    #[test]
    fn provider_spec_rejects_unsupported_kind_and_bad_url() {
        assert!("anthropic".parse::<AiProviderKind>().is_err());
        let mut spec = AiProviderSpec {
            kind: AiProviderKind::OpenaiCompatible,
            base_url: "file:///tmp/key".into(),
            path_prefix: None,
            credential_secret_id: SecretId::generate(),
            models: vec!["gpt-5".into()],
            auth_header: "authorization".into(),
        };
        assert!(spec.validate().is_err());

        spec.base_url = "https://api.openai.com/v1".into();
        assert!(spec.validate().is_err());

        spec.base_url = "https://api.openai.com".into();
        spec.validate().expect("origin-only base_url");
    }

    #[test]
    fn openai_request_extracts_model_and_forces_stream_usage() {
        let request = prepare_openai_chat_request(
            br#"{"model":"gpt-5","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
        )
        .expect("request");

        assert_eq!(request.model, "gpt-5");
        assert!(request.include_usage_injected);
        let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json");
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn openai_request_rejects_oversized_or_missing_model() {
        let err = prepare_openai_chat_request(&vec![b' '; MAX_AI_REQUEST_BODY_BYTES + 1])
            .expect_err("oversized");
        assert_eq!(err.code, crate::ErrorCode::ValidationFailed);

        let err = prepare_openai_chat_request(br#"{"messages":[]}"#).expect_err("missing model");
        assert_eq!(err.code, crate::ErrorCode::ValidationFailed);
    }

    #[test]
    fn ai_route_selects_model_specific_and_catch_all_backends() {
        let provider_id = AiProviderId::generate();
        let spec = AiRouteSpec {
            listener_port: 19000,
            path: default_chat_path(),
            backends: vec![
                AiRouteBackend {
                    provider_id,
                    models: vec!["gpt-5".into()],
                    model_override: None,
                    weight: 1,
                    priority: 0,
                },
                AiRouteBackend {
                    provider_id,
                    models: Vec::new(),
                    model_override: None,
                    weight: 1,
                    priority: 0,
                },
            ],
        };

        assert_eq!(spec.eligible_backend_indexes("gpt-5"), vec![0, 1]);
        assert_eq!(spec.eligible_backend_indexes("other"), vec![1]);
    }

    #[test]
    fn ai_route_accepts_model_override_and_rewrites_body_model() {
        let spec = AiRouteSpec {
            listener_port: 19000,
            path: default_chat_path(),
            backends: vec![AiRouteBackend {
                provider_id: AiProviderId::generate(),
                models: Vec::new(),
                model_override: Some("upstream-model".into()),
                weight: 1,
                priority: 0,
            }],
        };

        spec.validate().expect("model override supported");
        let body = rewrite_openai_chat_request_model(
            br#"{"model":"client-model","messages":[]}"#,
            "upstream-model",
        )
        .expect("rewrite");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["model"], "upstream-model");
    }

    #[test]
    fn strips_synthetic_stream_usage_chunk_but_keeps_usage_for_accounting() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );

        let (stripped, usage) = strip_synthetic_openai_usage_sse(body, true);

        assert!(stripped.contains("\"content\":\"hi\""));
        assert!(!stripped.contains("\"usage\""));
        assert_eq!(
            usage,
            Some(OpenAiTokenUsage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            })
        );
    }
}
