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

pub fn ai_error_envelope(code: &str, message: &str) -> String {
    serde_json::json!({
        "code": code,
        "message": message,
    })
    .to_string()
}

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
    /// RFC 7235 auth-scheme token prepended to the decoded credential at injection
    /// time (`<auth_scheme> <secret>`). `None` = the decoded secret is injected
    /// verbatim (the secret then carries any scheme itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_scheme: Option<String>,
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

/// RFC 7235 `auth-scheme` is an RFC 7230 `token`: 1+ tchar.
fn is_auth_scheme_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
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

/// Canonical origin derived from a provider's `base_url`. This is the **single**
/// derivation feeding the materialized cluster endpoint + SNI (`provider_cluster_spec`)
/// and the ExtProc `:authority` rewrite, so the TLS and HTTP layers can never disagree
/// on the provider host (fpv2-ti2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProviderOrigin {
    /// `Url::host_str()` — canonical: IDNA/punycode, percent-decoded, bracketed IPv6.
    pub host: String,
    /// Explicit port, or the scheme default.
    pub port: u16,
    /// `true` iff the scheme is `https`.
    pub use_tls: bool,
    /// `host` when the port is the scheme default, else `host:port`.
    pub authority: String,
}

impl AiProviderSpec {
    /// Parse `base_url` into its canonical origin: `url::Url` parsing plus the
    /// origin-only rules (no userinfo, no path/query/fragment, http(s) only).
    pub fn origin(&self) -> DomainResult<AiProviderOrigin> {
        let url = url::Url::parse(&self.base_url).map_err(|_| {
            DomainError::validation("AI provider base_url must be a valid http:// or https:// URL")
        })?;
        let use_tls = match url.scheme() {
            "https" => true,
            "http" => false,
            _ => {
                return Err(DomainError::validation(
                    "AI provider base_url must start with http:// or https://",
                ))
            }
        };
        if !url.username().is_empty() || url.password().is_some() {
            return Err(DomainError::validation(
                "AI provider base_url must not include userinfo; provider credentials belong in the credential secret",
            ));
        }
        let host = url
            .host_str()
            .filter(|host| !host.is_empty())
            .ok_or_else(|| DomainError::validation("AI provider base_url must include a host"))?
            .to_string();
        if !url.path().chars().all(|c| c == '/')
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(DomainError::validation(
                "AI provider base_url must not include a path, query, or fragment; use path_prefix for upstream paths",
            ));
        }
        let port = url
            .port_or_known_default()
            .ok_or_else(|| DomainError::validation("AI provider base_url must include a port"))?;
        // `Url::port()` is `None` when the port is the scheme default, even if the
        // input spelled it out (`https://host:443`).
        let authority = match url.port() {
            Some(explicit) => format!("{host}:{explicit}"),
            None => host.clone(),
        };
        Ok(AiProviderOrigin {
            host,
            port,
            use_tls,
            authority,
        })
    }

    pub fn validate(&self) -> DomainResult<()> {
        self.origin()?;
        if self.auth_header.trim().is_empty()
            || self.auth_header.bytes().any(|b| b <= b' ' || b == b':')
        {
            return Err(DomainError::validation(
                "AI provider auth_header must be a non-empty HTTP header name",
            ));
        }
        if let Some(scheme) = &self.auth_scheme {
            if scheme.is_empty() || !scheme.bytes().all(is_auth_scheme_token_byte) {
                return Err(DomainError::validation(
                    "AI provider auth_scheme must be a non-empty RFC 7235 token (letters, digits, or !#$%&'*+.^_`|~-; no whitespace)",
                ));
            }
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

/// Byte length of the SSE line terminator starting at `bytes[i]` (`\r\n`, `\n`, or lone
/// `\r`), or 0 when `bytes[i]` does not start one. All terminators are ASCII, so scanning
/// for them can never bisect a multi-byte UTF-8 sequence.
fn sse_terminator_len(bytes: &[u8], i: usize) -> usize {
    match bytes.get(i) {
        Some(b'\r') if bytes.get(i + 1) == Some(&b'\n') => 2,
        Some(b'\r') | Some(b'\n') => 1,
        _ => 0,
    }
}

/// End index (exclusive, including the delimiter) of the first SSE event that completes at
/// or after `from`: an event ends at two consecutive line terminators (the SSE blank line).
fn next_sse_event_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        let first = sse_terminator_len(bytes, i);
        if first == 0 {
            i += 1;
            continue;
        }
        let second = sse_terminator_len(bytes, i + first);
        if second == 0 {
            i += first;
            continue;
        }
        return Some(i + first + second);
    }
    None
}

/// End index (exclusive) of the complete-SSE-event prefix of `buffer`. Bytes past the
/// returned index belong to an event whose blank-line delimiter has not arrived yet and
/// must stay buffered. A trailing lone `\r` is deferred even when it would close a
/// delimiter: a following `\n` could still extend it to `\r\n`, and splitting early would
/// misattribute that `\n` to the next event. `end_of_stream` flushes everything.
pub fn complete_sse_events_end(buffer: &[u8], end_of_stream: bool) -> usize {
    if end_of_stream {
        return buffer.len();
    }
    let mut last = 0;
    let mut i = 0;
    while let Some(end) = next_sse_event_end(buffer, i) {
        if end == buffer.len() && buffer[end - 1] == b'\r' {
            break;
        }
        last = end;
        i = end;
    }
    last
}

/// Split `complete` (whole SSE events, as returned by [`complete_sse_events_end`], plus —
/// at end of stream — a possibly delimiter-less final event) into the bytes to forward and
/// the token usage it carried. Kept events are forwarded **byte-identical**, original
/// delimiters included. When `include_usage_injected` is true (Flowplane itself forced
/// `stream_options.include_usage`), events that parse as usage are dropped — delimiter and
/// all — so the synthetic event never reaches a client that did not ask for usage;
/// otherwise every byte passes through and usage is only observed.
pub fn strip_synthetic_openai_usage_sse(
    complete: &[u8],
    include_usage_injected: bool,
) -> (Vec<u8>, Option<OpenAiTokenUsage>) {
    let mut usage = None;
    let mut kept = Vec::with_capacity(complete.len());
    let mut start = 0;
    while start < complete.len() {
        let end = next_sse_event_end(complete, start).unwrap_or(complete.len());
        let event = &complete[start..end];
        let event_usage = std::str::from_utf8(event)
            .ok()
            .and_then(usage_from_sse_event);
        if let Some(parsed) = event_usage {
            usage = Some(parsed);
        }
        if !(include_usage_injected && event_usage.is_some()) {
            kept.extend_from_slice(event);
        }
        start = end;
    }
    (kept, usage)
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
    fn ai_error_envelope_round_trips_embedded_quotes() {
        let message = r#"AI budget "hard-stop" exceeded for model "gpt-5""#;
        let envelope = ai_error_envelope("flowplane_ai_budget_exceeded", message);

        let parsed: serde_json::Value = serde_json::from_str(&envelope).expect("json envelope");
        assert_eq!(parsed["code"], "flowplane_ai_budget_exceeded");
        assert_eq!(parsed["message"], message);
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
            auth_scheme: None,
        };
        assert!(spec.validate().is_err());

        spec.base_url = "https://api.openai.com/v1".into();
        assert!(spec.validate().is_err());

        spec.base_url = "https://api.openai.com".into();
        spec.validate().expect("origin-only base_url");
    }

    #[test]
    fn provider_spec_auth_scheme_token_rules() {
        let mut spec = origin_spec("https://api.openai.com");

        // Absent scheme is valid (verbatim injection).
        spec.auth_scheme = None;
        spec.validate().expect("no scheme");

        // Valid RFC 7235 tokens.
        for ok in [
            "Bearer",
            "bearer",
            "Token",
            "DPoP",
            "X-Api.Key_1!#$%&'*+^`|~-",
        ] {
            spec.auth_scheme = Some(ok.into());
            assert!(spec.validate().is_ok(), "{ok} must be accepted");
        }

        // Invalid: empty, whitespace anywhere, non-token characters.
        for bad in [
            "", " ", "Bearer ", " Bearer", "Bea rer", "Bearer\t", "Bearer\n", "Bear:er", "Bear;er",
            "Bear\"er", "Bearér", "Bear(er)", "Bear[er]", "Bear{er}", "Bear=er", "Bear/er",
            "Bear\\er", "Bear,er", "Bear?er", "Bear@er", "Bear<er>",
        ] {
            spec.auth_scheme = Some(bad.into());
            assert!(spec.validate().is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn provider_spec_auth_scheme_serde_shape() {
        // Absent in JSON -> None; None -> absent in JSON (lossless, no empty-string leak).
        let json = r#"{"kind":"openai-compatible","base_url":"https://api.openai.com",
            "credential_secret_id":"018f4e6e-0000-7000-8000-000000000000"}"#;
        let spec: AiProviderSpec = serde_json::from_str(json).expect("spec without scheme");
        assert_eq!(spec.auth_scheme, None);
        let out = serde_json::to_value(&spec).expect("serialize");
        assert!(out.get("auth_scheme").is_none(), "None must not serialize");

        let json = r#"{"kind":"openai-compatible","base_url":"https://api.openai.com",
            "credential_secret_id":"018f4e6e-0000-7000-8000-000000000000",
            "auth_scheme":"Bearer"}"#;
        let spec: AiProviderSpec = serde_json::from_str(json).expect("spec with scheme");
        assert_eq!(spec.auth_scheme.as_deref(), Some("Bearer"));
        let out = serde_json::to_value(&spec).expect("serialize");
        assert_eq!(out["auth_scheme"], "Bearer");
    }

    fn origin_spec(base_url: &str) -> AiProviderSpec {
        AiProviderSpec {
            kind: AiProviderKind::OpenaiCompatible,
            base_url: base_url.into(),
            path_prefix: None,
            credential_secret_id: SecretId::generate(),
            models: Vec::new(),
            auth_header: "authorization".into(),
            auth_scheme: None,
        }
    }

    #[test]
    fn provider_origin_canonicalizes_accepted_inputs() {
        let cases = [
            // (base_url, host, port, use_tls, authority)
            (
                "https://openrouter.ai",
                "openrouter.ai",
                443,
                true,
                "openrouter.ai",
            ),
            (
                "https://openrouter.ai/",
                "openrouter.ai",
                443,
                true,
                "openrouter.ai",
            ),
            ("https://host:443", "host", 443, true, "host"),
            ("http://host:80", "host", 80, false, "host"),
            ("https://host:8443", "host", 8443, true, "host:8443"),
            (
                "http://10.0.0.4:8080",
                "10.0.0.4",
                8080,
                false,
                "10.0.0.4:8080",
            ),
            (
                "https://[fd00::7]:8443",
                "[fd00::7]",
                8443,
                true,
                "[fd00::7]:8443",
            ),
            ("https://[fd00::7]:443", "[fd00::7]", 443, true, "[fd00::7]"),
            // percent-encoded host canonicalizes exactly as the cluster/SNI derivation does
            (
                "https://%65xample.com",
                "example.com",
                443,
                true,
                "example.com",
            ),
            // Unicode domain -> punycode, same as SNI
            (
                "https://bücher.example",
                "xn--bcher-kva.example",
                443,
                true,
                "xn--bcher-kva.example",
            ),
            // non-canonical IPv4 spelling normalizes, same as SNI
            (
                "https://127.0.000.001:8443",
                "127.0.0.1",
                8443,
                true,
                "127.0.0.1:8443",
            ),
        ];
        for (base_url, host, port, use_tls, authority) in cases {
            let origin = origin_spec(base_url).origin().expect(base_url);
            assert_eq!(origin.host, host, "host for {base_url}");
            assert_eq!(origin.port, port, "port for {base_url}");
            assert_eq!(origin.use_tls, use_tls, "use_tls for {base_url}");
            assert_eq!(origin.authority, authority, "authority for {base_url}");
        }
    }

    #[test]
    fn provider_origin_rejects_non_origin_inputs() {
        let rejected = [
            "https://user:pw@provider.example", // userinfo must never reach :authority/trace
            "https://user@provider.example",
            "https://host:abc",
            "https://fd00::7", // unbracketed IPv6
            "ftp://host",
            "https://",
            "https://host/api/v1",
            "https://host?x=1",
            "https://host#frag",
            "host:50051",
        ];
        for base_url in rejected {
            let spec = origin_spec(base_url);
            assert!(spec.origin().is_err(), "origin() must reject {base_url}");
            assert!(
                spec.validate().is_err(),
                "validate() must reject {base_url}"
            );
        }
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

        let (stripped, usage) = strip_synthetic_openai_usage_sse(body.as_bytes(), true);

        let expected = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
        assert_eq!(
            stripped,
            expected.as_bytes(),
            "kept events stay byte-identical"
        );
        assert_eq!(
            usage,
            Some(OpenAiTokenUsage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            })
        );
    }

    #[test]
    fn strip_without_injection_is_byte_identical_passthrough() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hé✓\"}}]}\r\n\r\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}\n\n",
        );

        let (kept, usage) = strip_synthetic_openai_usage_sse(body.as_bytes(), false);

        assert_eq!(
            kept,
            body.as_bytes(),
            "observing mode must never rewrite bytes"
        );
        assert_eq!(usage.expect("usage observed").total_tokens, 5);
    }

    #[test]
    fn strip_handles_crlf_framed_events() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\r\n\r\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\r\n\r\n",
            "data: [DONE]\r\n\r\n",
        );

        let (stripped, usage) = strip_synthetic_openai_usage_sse(body.as_bytes(), true);

        let expected =
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        assert_eq!(
            stripped,
            expected.as_bytes(),
            "CRLF delimiters preserved verbatim"
        );
        assert_eq!(usage.expect("usage").total_tokens, 2);
    }

    #[test]
    fn strip_keeps_invalid_utf8_events_verbatim() {
        let mut body = b"data: \xff\xfe garbage".to_vec();
        body.extend_from_slice(b"\n\n");

        let (kept, usage) = strip_synthetic_openai_usage_sse(&body, true);

        assert_eq!(kept, body, "undecodable events are never dropped");
        assert!(usage.is_none());
    }

    #[test]
    fn complete_sse_events_end_recognizes_all_terminator_framings() {
        // (buffer, expected complete-prefix length without end_of_stream)
        let cases: &[(&[u8], usize)] = &[
            (b"data: a\n\ndata: b", 9),       // LF: one complete event
            (b"data: a\r\n\r\ndata: b", 11),  // CRLF
            (b"data: a\r\rdata: b", 9),       // lone-CR terminators
            (b"data: a\n\r\ndata: b", 10),    // mixed LF + CRLF blank line
            (b"data: a\ndata: b", 0),         // no blank line yet
            (b"data: a\n\n", 9),              // complete event, empty remainder
            (b"data: a\r\n\r", 0),            // trailing bare CR: deferred (may become CRLF)
            (b"data: a\n\ndata: b\r\n\r", 9), // second event's delimiter still ambiguous
            (b"", 0),
        ];
        for (buffer, expected) in cases {
            assert_eq!(
                complete_sse_events_end(buffer, false),
                *expected,
                "buffer {:?}",
                String::from_utf8_lossy(buffer)
            );
        }
        // end_of_stream flushes everything, delimiter or not.
        assert_eq!(complete_sse_events_end(b"data: a\r\n\r", true), 10);
        assert_eq!(
            complete_sse_events_end(b"data: tail-no-delimiter", true),
            23
        );
    }

    #[test]
    fn sse_stream_reassembles_identically_across_every_chunk_split() {
        // Chunk-boundary fuzz at the domain layer: for every split point (including inside
        // multi-byte UTF-8 content and inside delimiters), accumulating into a remainder,
        // taking the complete prefix, stripping, and flushing at end-of-stream must yield
        // the same bytes as stripping the whole stream at once.
        for framing in ["\n\n", "\r\n\r\n"] {
            let stream = format!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"héllo✓\"}}}}]}}{framing}\
                 data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}}}{framing}\
                 data: [DONE]{framing}"
            );
            let bytes = stream.as_bytes();
            let (expected, expected_usage) = strip_synthetic_openai_usage_sse(bytes, true);
            for split in 0..=bytes.len() {
                let mut remainder: Vec<u8> = Vec::new();
                let mut out: Vec<u8> = Vec::new();
                let mut usage = None;
                for (chunk, eos) in [(&bytes[..split], false), (&bytes[split..], true)] {
                    remainder.extend_from_slice(chunk);
                    let end = complete_sse_events_end(&remainder, eos);
                    let (kept, chunk_usage) =
                        strip_synthetic_openai_usage_sse(&remainder[..end], true);
                    out.extend_from_slice(&kept);
                    if let Some(parsed) = chunk_usage {
                        assert!(usage.is_none(), "usage must be captured exactly once");
                        usage = Some(parsed);
                    }
                    remainder.drain(..end);
                }
                assert!(remainder.is_empty(), "split {split} framing {framing:?}");
                assert_eq!(out, expected, "split {split} framing {framing:?}");
                assert_eq!(usage, expected_usage, "split {split} framing {framing:?}");
            }
        }
    }
}
