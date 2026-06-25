//! HTTP filter IR (S5.8, spec/04 §4). A closed, validated vocabulary: filters are part of
//! the listener spec (chain) and of vhost/route specs (per-route overrides), translated to
//! Envoy protos at snapshot build time — never injected by post-hoc protobuf surgery
//! (kills v1's §4.4 injection pipeline).
//!
//! Catalog grows by addition; this file starts with the dependency-free set
//! {cors, local_rate_limit, header_mutation}. Validation rules mirror spec/04 §4.1 so a
//! bad config is a 400 with a hint, not an Envoy NACK.

use crate::error::{DomainError, DomainResult};
use serde::{Deserialize, Serialize};

const MAX_CORS_ORIGINS: usize = 64;
const MAX_CORS_LIST_VALUES: usize = 128;
const MAX_CORS_ORIGIN_VALUE_LEN: usize = 2048;
const MAX_CORS_TOKEN_VALUE_LEN: usize = 256;
const MAX_HEADER_MUTATIONS_PER_DIRECTION: usize = 128;
const MAX_HEADER_NAME_LEN: usize = 256;
const MAX_HEADER_VALUE_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpFilterKind {
    Cors,
    LocalRateLimit,
    HeaderMutation,
    HealthCheck,
    Compressor,
    JwtAuth,
    ExtAuthz,
    Rbac,
    GlobalRateLimit,
}

impl HttpFilterKind {
    const ALL: [Self; 9] = [
        Self::Cors,
        Self::LocalRateLimit,
        Self::HeaderMutation,
        Self::HealthCheck,
        Self::Compressor,
        Self::JwtAuth,
        Self::ExtAuthz,
        Self::Rbac,
        Self::GlobalRateLimit,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::Cors => "cors",
            Self::LocalRateLimit => "local_rate_limit",
            Self::HeaderMutation => "header_mutation",
            Self::HealthCheck => "health_check",
            Self::Compressor => "compressor",
            Self::JwtAuth => "jwt_auth",
            Self::ExtAuthz => "ext_authz",
            Self::Rbac => "rbac",
            Self::GlobalRateLimit => "global_rate_limit",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == raw)
    }

    fn is_disablable(self) -> bool {
        !matches!(self, Self::HealthCheck)
    }

    fn disablable_hint() -> String {
        Self::ALL
            .iter()
            .copied()
            .filter(|kind| kind.is_disablable())
            .map(Self::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One entry in a listener's HTTP filter chain. Order is semantic (chain order); the
/// router filter is appended automatically at translation and may not appear here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpFilterEntry {
    pub filter: HttpFilterSpec,
    /// Disabled filters stay in the chain but Envoy skips them (toggle without re-order).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

/// The closed filter vocabulary (spec/04 §4.1). Tagged by `type` in JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HttpFilterSpec {
    /// Chain marker only — the policy lives in per-scope `filter_overrides` (Envoy reads
    /// CORS policy exclusively from per-route config).
    Cors(CorsConfig),
    LocalRateLimit(LocalRateLimitConfig),
    HeaderMutation(HeaderMutationConfig),
    HealthCheck(HealthCheckConfig),
    Compressor(CompressorConfig),
    JwtAuth(JwtAuthConfig),
    ExtAuthz(ExtAuthzConfig),
    Rbac(RbacConfig),
    GlobalRateLimit(GlobalRateLimitConfig),
}

impl HttpFilterSpec {
    pub fn kind(&self) -> &'static str {
        self.kind_value().as_str()
    }

    fn kind_value(&self) -> HttpFilterKind {
        match self {
            Self::Cors(_) => HttpFilterKind::Cors,
            Self::LocalRateLimit(_) => HttpFilterKind::LocalRateLimit,
            Self::HeaderMutation(_) => HttpFilterKind::HeaderMutation,
            Self::HealthCheck(_) => HttpFilterKind::HealthCheck,
            Self::Compressor(_) => HttpFilterKind::Compressor,
            Self::JwtAuth(_) => HttpFilterKind::JwtAuth,
            Self::ExtAuthz(_) => HttpFilterKind::ExtAuthz,
            Self::Rbac(_) => HttpFilterKind::Rbac,
            Self::GlobalRateLimit(_) => HttpFilterKind::GlobalRateLimit,
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        match self {
            Self::Cors(c) => c.validate(),
            Self::LocalRateLimit(c) => c.validate(),
            Self::HeaderMutation(c) => c.validate(),
            Self::HealthCheck(c) => c.validate(),
            Self::Compressor(c) => c.validate(),
            Self::JwtAuth(c) => c.validate(),
            Self::ExtAuthz(c) => c.validate(),
            Self::Rbac(c) => c.validate(),
            Self::GlobalRateLimit(c) => c.validate(),
        }
    }
}

/// Per-vhost/per-route filter behavior (spec/04 §4.1 per-route column). Tagged by `type`;
/// the variants encode exactly what each filter supports — unsupported combinations
/// (oauth2 per-route, health_check per-route) cannot be expressed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilterOverride {
    /// Skip a chain filter on this scope (universal; `filter_type` is a `kind()` string).
    Disable { filter_type: String },
    /// CORS policy for this scope (requires the cors marker in the listener chain).
    Cors(CorsConfig),
    /// Replace the local rate limit on this scope.
    LocalRateLimit(LocalRateLimitConfig),
    /// JWT requirement for this scope, by name from the chain config's `requirement_map`
    /// (reference-only per spec/04 §4.1; disabling goes through `Disable`).
    JwtAuth { requirement_name: String },
}

impl FilterOverride {
    /// The chain filter type this override targets.
    pub fn target_kind(&self) -> DomainResult<&str> {
        match self {
            Self::Disable { filter_type } => {
                // health_check is listener-only (spec/04 §4.1): no per-route control at all.
                let Some(kind) = HttpFilterKind::parse(filter_type) else {
                    return Err(DomainError::validation(format!(
                        "filter type \"{filter_type}\" cannot be disabled per-route",
                    ))
                    .with_hint(format!(
                        "disablable types: {}",
                        HttpFilterKind::disablable_hint()
                    )));
                };
                if !kind.is_disablable() {
                    return Err(DomainError::validation(format!(
                        "filter type \"{filter_type}\" cannot be disabled per-route",
                    ))
                    .with_hint(format!(
                        "disablable types: {}",
                        HttpFilterKind::disablable_hint()
                    )));
                }
                Ok(kind.as_str())
            }
            Self::Cors(_) => Ok("cors"),
            Self::LocalRateLimit(_) => Ok("local_rate_limit"),
            Self::JwtAuth { .. } => Ok("jwt_auth"),
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        match self {
            Self::Disable { .. } => self.target_kind().map(|_| ()),
            Self::Cors(c) => c.validate(),
            Self::LocalRateLimit(c) => c.validate(),
            Self::JwtAuth { requirement_name } => {
                if requirement_name.is_empty() || requirement_name.len() > 128 {
                    return Err(DomainError::validation(
                        "jwt_auth override: requirement_name must be 1..=128 characters",
                    ));
                }
                Ok(())
            }
        }
    }
}

/// Validate a scope's override list: each override valid, at most one per filter type.
pub fn validate_filter_overrides(overrides: &[FilterOverride]) -> DomainResult<()> {
    let mut seen = std::collections::HashSet::new();
    for ov in overrides {
        ov.validate()?;
        let kind = ov.target_kind()?.to_string();
        if !seen.insert(kind.clone()) {
            return Err(DomainError::validation(format!(
                "multiple overrides target filter type \"{kind}\" in the same scope"
            )));
        }
    }
    Ok(())
}

// ---------------- cors ----------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "match", rename_all = "snake_case", deny_unknown_fields)]
pub enum OriginMatcher {
    Exact { value: String },
    Prefix { value: String },
    Suffix { value: String },
    Contains { value: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CorsConfig {
    /// Required, non-empty (spec/04 §4.1).
    pub allow_origin: Vec<OriginMatcher>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_methods: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_headers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expose_headers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_credentials: bool,
}

impl CorsConfig {
    const MAX_AGE_CAP: u64 = 315_576_000_000;

    pub fn validate(&self) -> DomainResult<()> {
        if self.allow_origin.is_empty() {
            return Err(DomainError::validation(
                "cors: allow_origin must list at least one origin matcher",
            ));
        }
        if self.allow_origin.len() > MAX_CORS_ORIGINS {
            return Err(DomainError::validation(format!(
                "cors: allow_origin may contain at most {MAX_CORS_ORIGINS} matchers",
            )));
        }
        for origin in &self.allow_origin {
            validate_origin_matcher(origin)?;
        }
        validate_bounded_string_list(
            "cors: allow_methods",
            &self.allow_methods,
            MAX_CORS_LIST_VALUES,
            MAX_CORS_TOKEN_VALUE_LEN,
        )?;
        validate_bounded_string_list(
            "cors: allow_headers",
            &self.allow_headers,
            MAX_CORS_LIST_VALUES,
            MAX_CORS_TOKEN_VALUE_LEN,
        )?;
        validate_bounded_string_list(
            "cors: expose_headers",
            &self.expose_headers,
            MAX_CORS_LIST_VALUES,
            MAX_CORS_TOKEN_VALUE_LEN,
        )?;
        let wildcard = self.allow_origin.iter().any(|m| {
            matches!(m, OriginMatcher::Exact { value } | OriginMatcher::Prefix { value }
                if value == "*")
        });
        if wildcard && self.allow_credentials {
            return Err(DomainError::validation(
                "cors: allow_credentials cannot be combined with a wildcard origin",
            )
            .with_hint("list explicit origins, or drop allow_credentials"));
        }
        if self.max_age_seconds.is_some_and(|v| v > Self::MAX_AGE_CAP) {
            return Err(DomainError::validation(format!(
                "cors: max_age_seconds exceeds the protocol cap ({})",
                Self::MAX_AGE_CAP
            )));
        }
        Ok(())
    }
}

// ---------------- local_rate_limit ----------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenBucket {
    pub max_tokens: u32,
    /// Defaults to `max_tokens` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_per_fill: Option<u32>,
    pub fill_interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalRateLimitConfig {
    pub stat_prefix: String,
    pub token_bucket: TokenBucket,
    /// 400–599; Envoy default 429 when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
}

impl LocalRateLimitConfig {
    pub fn validate(&self) -> DomainResult<()> {
        if self.stat_prefix.is_empty() {
            return Err(DomainError::validation(
                "local_rate_limit: stat_prefix is required",
            ));
        }
        if self.token_bucket.max_tokens == 0 {
            return Err(DomainError::validation(
                "local_rate_limit: token_bucket.max_tokens must be >= 1",
            ));
        }
        if self.token_bucket.fill_interval_ms == 0 {
            return Err(DomainError::validation(
                "local_rate_limit: token_bucket.fill_interval_ms must be > 0",
            ));
        }
        if let Some(code) = self.status_code {
            if !(400..=599).contains(&code) {
                return Err(DomainError::validation(
                    "local_rate_limit: status_code must be in 400..=599",
                ));
            }
        }
        Ok(())
    }
}

// ---------------- global_rate_limit ----------------

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitRequestType {
    #[default]
    Both,
    Internal,
    External,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GlobalRateLimitConfig {
    pub domain: String,
    #[serde(default = "default_service_cluster")]
    pub service_cluster: String,
    #[serde(default = "default_global_rate_limit_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub failure_mode_deny: bool,
    #[serde(default)]
    pub stage: u32,
    #[serde(default, skip_serializing_if = "is_default_rate_limit_request_type")]
    pub request_type: RateLimitRequestType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stat_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub enable_x_ratelimit_headers: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_x_envoy_ratelimited_header: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limited_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_on_error: Option<u16>,
}

fn default_global_rate_limit_timeout_ms() -> u64 {
    20
}

/// Default `service_cluster` = the CP-synthesized built-in rate-limit cluster (S6). When set, the
/// operator's filter resolves to the first-party RLS out of the box without naming a cluster.
fn default_service_cluster() -> String {
    crate::gateway::cluster::RESERVED_RATE_LIMIT_CLUSTER.to_string()
}

/// Upper bound on the Envoy filter `domain`. The CP composes it as
/// `{org_uuid}|{team_uuid}|{user_domain}` (S5/S7): two 36-char UUIDs + two `|` separators + the
/// user domain (<= 253, spec/02:329) = 327. The cap admits that composed value; the user-facing
/// domain itself stays bounded at 253 by `rate_limit::validate_rate_limit_domain_name`.
const MAX_GLOBAL_RATE_LIMIT_DOMAIN_LEN: usize = 253 + 36 + 36 + 2;

fn is_default_rate_limit_request_type(value: &RateLimitRequestType) -> bool {
    *value == RateLimitRequestType::Both
}

impl GlobalRateLimitConfig {
    pub fn validate(&self) -> DomainResult<()> {
        if self.domain.is_empty()
            || self.domain.len() > MAX_GLOBAL_RATE_LIMIT_DOMAIN_LEN
            || self.domain.contains('\0')
        {
            return Err(DomainError::validation(format!(
                "global_rate_limit: domain must be 1..={MAX_GLOBAL_RATE_LIMIT_DOMAIN_LEN} \
                 characters and contain no NUL",
            )));
        }
        // The CP-owned built-in cluster name (spec/04) contains underscores and is exempt from
        // the user-facing slug rule; any other service_cluster must be a valid cluster name.
        if self.service_cluster != crate::gateway::cluster::RESERVED_RATE_LIMIT_CLUSTER {
            crate::identity::validate_name(&self.service_cluster)?;
        }
        if self.timeout_ms > 60_000 {
            return Err(DomainError::validation(
                "global_rate_limit: timeout_ms must be <= 60000",
            ));
        }
        if self.stage > 10 {
            return Err(DomainError::validation(
                "global_rate_limit: stage must be in 0..=10",
            ));
        }
        if self
            .stat_prefix
            .as_ref()
            .is_some_and(|v| v.is_empty() || v.len() > 128 || v.contains('\0'))
        {
            return Err(DomainError::validation(
                "global_rate_limit: stat_prefix must be 1..=128 characters and contain no NUL",
            ));
        }
        for (field, status) in [
            ("rate_limited_status", self.rate_limited_status),
            ("status_on_error", self.status_on_error),
        ] {
            if status.is_some_and(|code| !(400..=599).contains(&code)) {
                return Err(DomainError::validation(format!(
                    "global_rate_limit: {field} must be in 400..=599",
                )));
            }
        }
        Ok(())
    }
}

// ---------------- header_mutation ----------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HeaderValue {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub append: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HeaderMutationConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_headers_to_add: Vec<HeaderValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_headers_to_remove: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers_to_add: Vec<HeaderValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers_to_remove: Vec<String>,
}

impl HeaderMutationConfig {
    pub fn validate(&self) -> DomainResult<()> {
        validate_header_values(
            "header_mutation: request_headers_to_add",
            &self.request_headers_to_add,
        )?;
        validate_header_names(
            "header_mutation: request_headers_to_remove",
            &self.request_headers_to_remove,
        )?;
        validate_header_values(
            "header_mutation: response_headers_to_add",
            &self.response_headers_to_add,
        )?;
        validate_header_names(
            "header_mutation: response_headers_to_remove",
            &self.response_headers_to_remove,
        )?;
        for hv in self
            .request_headers_to_add
            .iter()
            .chain(&self.response_headers_to_add)
        {
            if hv.key.is_empty() {
                return Err(DomainError::validation(
                    "header_mutation: header key must be non-empty",
                ));
            }
        }
        Ok(())
    }
}

fn validate_origin_matcher(matcher: &OriginMatcher) -> DomainResult<()> {
    let value = match matcher {
        OriginMatcher::Exact { value }
        | OriginMatcher::Prefix { value }
        | OriginMatcher::Suffix { value }
        | OriginMatcher::Contains { value } => value,
    };
    validate_bounded_string(
        "cors: allow_origin matcher value",
        value,
        MAX_CORS_ORIGIN_VALUE_LEN,
    )
}

fn validate_bounded_string_list(
    label: &str,
    values: &[String],
    max_items: usize,
    max_len: usize,
) -> DomainResult<()> {
    if values.len() > max_items {
        return Err(DomainError::validation(format!(
            "{label} may contain at most {max_items} values",
        )));
    }
    for value in values {
        validate_bounded_string(label, value, max_len)?;
    }
    Ok(())
}

fn validate_bounded_string(label: &str, value: &str, max_len: usize) -> DomainResult<()> {
    if value.is_empty() || value.len() > max_len || value.chars().any(char::is_control) {
        return Err(DomainError::validation(format!(
            "{label} values must be 1..={max_len} printable characters",
        )));
    }
    Ok(())
}

fn validate_header_values(label: &str, values: &[HeaderValue]) -> DomainResult<()> {
    if values.len() > MAX_HEADER_MUTATIONS_PER_DIRECTION {
        return Err(DomainError::validation(format!(
            "{label} may contain at most {MAX_HEADER_MUTATIONS_PER_DIRECTION} headers",
        )));
    }
    for hv in values {
        validate_bounded_string("header_mutation: header key", &hv.key, MAX_HEADER_NAME_LEN)?;
        validate_bounded_string(
            "header_mutation: header value",
            &hv.value,
            MAX_HEADER_VALUE_LEN,
        )?;
    }
    Ok(())
}

fn validate_header_names(label: &str, values: &[String]) -> DomainResult<()> {
    validate_bounded_string_list(
        label,
        values,
        MAX_HEADER_MUTATIONS_PER_DIRECTION,
        MAX_HEADER_NAME_LEN,
    )
}

// ---------------- health_check ----------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthCheckConfig {
    /// Path answered by the proxy itself (exact match), e.g. `/healthz`.
    pub endpoint_path: String,
    /// Pass health checks to the upstream instead of answering locally.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pass_through_mode: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_time_ms: Option<u64>,
}

impl HealthCheckConfig {
    pub fn validate(&self) -> DomainResult<()> {
        if !self.endpoint_path.starts_with('/') || self.endpoint_path.len() > 500 {
            return Err(DomainError::validation(
                "health_check: endpoint_path must start with '/' and be <= 500 chars",
            ));
        }
        Ok(())
    }
}

// ---------------- compressor (gzip) ----------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompressionLevel {
    BestSpeed,
    DefaultCompression,
    BestCompression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CompressorConfig {
    /// zlib memory level, 1-9 (Envoy default 5 when omitted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_level: Option<u32>,
    /// zlib window bits, 9-15 (Envoy default 12 when omitted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_bits: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression_level: Option<CompressionLevel>,
}

impl CompressorConfig {
    pub fn validate(&self) -> DomainResult<()> {
        if self.memory_level.is_some_and(|v| !(1..=9).contains(&v)) {
            return Err(DomainError::validation(
                "compressor: memory_level must be 1-9",
            ));
        }
        if self.window_bits.is_some_and(|v| !(9..=15).contains(&v)) {
            return Err(DomainError::validation(
                "compressor: window_bits must be 9-15",
            ));
        }
        Ok(())
    }
}

// ---------------- jwt_auth ----------------

/// Where a provider's JWKS comes from. Remote sources name a same-team cluster the proxy
/// fetches through (no implicit cluster synthesis — explicit beats magic; the cluster is
/// validated like any other reference and Envoy NACKs honestly if it is missing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum JwksSource {
    Remote {
        /// Full JWKS URI, e.g. `https://issuer.example/.well-known/jwks.json`.
        uri: String,
        /// Cluster (same team) used to reach the JWKS host.
        cluster: String,
        #[serde(default = "default_jwks_timeout_ms")]
        timeout_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_duration_secs: Option<u64>,
    },
    Inline {
        /// JWKS JSON, inline (for static keys / tests).
        jwks: String,
    },
}

fn default_jwks_timeout_ms() -> u64 {
    5000
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JwtProvider {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audiences: Vec<String>,
    pub jwks: JwksSource,
    /// Seconds of clock skew tolerated when validating exp/nbf (default 60, as v1).
    #[serde(default = "default_clock_skew")]
    pub clock_skew_seconds: u32,
    /// Keep the token on the forwarded request (default: stripped).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forward: bool,
}

fn default_clock_skew() -> u32 {
    60
}

/// What a rule (or a per-route override, by name) demands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JwtRequirement {
    /// A specific provider must validate.
    Provider { provider_name: String },
    /// Any of the named providers validates.
    AnyOf { provider_names: Vec<String> },
    /// Token optional; if present it must validate.
    AllowMissing,
    /// Token optional and failures tolerated (audit-only mode).
    AllowMissingOrFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JwtRule {
    #[serde(rename = "match")]
    pub matcher: crate::gateway::route_config::PathMatch,
    /// Name into `requirement_map`.
    pub requirement_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JwtAuthConfig {
    /// Provider name → provider. BTreeMap: deterministic encoding (spec/10 §5).
    pub providers: std::collections::BTreeMap<String, JwtProvider>,
    /// Named requirements, referenced by rules and per-route overrides.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub requirement_map: std::collections::BTreeMap<String, JwtRequirement>,
    /// Path rules, first match wins. Empty → every path requires any provider (v1 rule).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<JwtRule>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypass_cors_preflight: bool,
}

impl JwtAuthConfig {
    fn requirement_providers_exist(&self, req: &JwtRequirement) -> DomainResult<()> {
        let missing: Vec<&str> = match req {
            JwtRequirement::Provider { provider_name } => {
                if self.providers.contains_key(provider_name) {
                    vec![]
                } else {
                    vec![provider_name.as_str()]
                }
            }
            JwtRequirement::AnyOf { provider_names } => provider_names
                .iter()
                .filter(|n| !self.providers.contains_key(*n))
                .map(String::as_str)
                .collect(),
            _ => vec![],
        };
        if missing.is_empty() {
            Ok(())
        } else {
            Err(DomainError::validation(format!(
                "jwt_auth: requirement references unknown providers: {}",
                missing.join(", ")
            )))
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.providers.is_empty() {
            return Err(DomainError::validation(
                "jwt_auth: at least one provider is required",
            ));
        }
        for (name, provider) in &self.providers {
            crate::identity::validate_name(name)?;
            match &provider.jwks {
                JwksSource::Remote {
                    uri,
                    cluster,
                    timeout_ms,
                    ..
                } => {
                    if !uri.starts_with("https://") && !uri.starts_with("http://") {
                        return Err(DomainError::validation(format!(
                            "jwt_auth provider \"{name}\": jwks uri must be http(s)"
                        )));
                    }
                    crate::identity::validate_name(cluster)?;
                    if *timeout_ms == 0 || *timeout_ms > 60_000 {
                        return Err(DomainError::validation(format!(
                            "jwt_auth provider \"{name}\": timeout_ms must be 1..=60000"
                        )));
                    }
                }
                JwksSource::Inline { jwks } => {
                    if jwks.is_empty() || jwks.len() > 65_536 {
                        return Err(DomainError::validation(format!(
                            "jwt_auth provider \"{name}\": inline jwks must be 1..=65536 bytes"
                        )));
                    }
                }
            }
        }
        for (name, req) in &self.requirement_map {
            crate::identity::validate_name(name)?;
            if let JwtRequirement::AnyOf { provider_names } = req {
                if provider_names.is_empty() {
                    return Err(DomainError::validation(format!(
                        "jwt_auth requirement \"{name}\": any_of needs at least one provider"
                    )));
                }
            }
            self.requirement_providers_exist(req)?;
        }
        for rule in &self.rules {
            if !self.requirement_map.contains_key(&rule.requirement_name) {
                return Err(DomainError::validation(format!(
                    "jwt_auth rule references unknown requirement \"{}\"",
                    rule.requirement_name
                ))
                .with_hint("declare it in requirement_map"));
            }
        }
        Ok(())
    }
}

// ---------------- ext_authz ----------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtAuthzConfig {
    /// gRPC authorization service, by same-team cluster name.
    pub cluster: String,
    #[serde(default = "default_ext_authz_timeout_ms")]
    pub timeout_ms: u64,
    /// Allow traffic when the authz service is unreachable (default false: fail closed).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failure_mode_allow: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_peer_certificate: bool,
}

fn default_ext_authz_timeout_ms() -> u64 {
    200
}

impl ExtAuthzConfig {
    pub fn validate(&self) -> DomainResult<()> {
        crate::identity::validate_name(&self.cluster)?;
        if self.timeout_ms == 0 || self.timeout_ms > 60_000 {
            return Err(DomainError::validation(
                "ext_authz: timeout_ms must be 1..=60000",
            ));
        }
        Ok(())
    }
}

// ---------------- rbac ----------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RbacAction {
    /// Matching requests are allowed; everything else denied.
    Allow,
    /// Matching requests are denied; everything else allowed.
    Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RbacPermission {
    Any,
    Header {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exact: Option<String>,
    },
    UrlPath {
        prefix: String,
    },
    DestinationPort {
        port: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RbacPrincipal {
    Any,
    /// Direct peer address in CIDR form, e.g. `10.0.0.0/8`.
    SourceCidr {
        cidr: String,
    },
    Header {
        name: String,
        exact: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RbacPolicy {
    pub permissions: Vec<RbacPermission>,
    pub principals: Vec<RbacPrincipal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RbacConfig {
    pub action: RbacAction,
    /// Policy name → policy. BTreeMap: deterministic encoding.
    pub policies: std::collections::BTreeMap<String, RbacPolicy>,
}

impl RbacConfig {
    pub fn validate(&self) -> DomainResult<()> {
        if self.policies.is_empty() {
            return Err(DomainError::validation(
                "rbac: at least one policy is required",
            ));
        }
        for (name, policy) in &self.policies {
            crate::identity::validate_name(name)?;
            if policy.permissions.is_empty() || policy.principals.is_empty() {
                return Err(DomainError::validation(format!(
                    "rbac policy \"{name}\": permissions and principals must be non-empty"
                )));
            }
            for permission in &policy.permissions {
                match permission {
                    RbacPermission::Header { name: header, .. } => {
                        if header.is_empty() {
                            return Err(DomainError::validation(
                                "rbac: header permission needs a header name",
                            ));
                        }
                    }
                    RbacPermission::UrlPath { prefix } => {
                        if !prefix.starts_with('/') {
                            return Err(DomainError::validation(
                                "rbac: url_path prefix must start with '/'",
                            ));
                        }
                    }
                    RbacPermission::DestinationPort { port } => {
                        if *port == 0 {
                            return Err(DomainError::validation(
                                "rbac: destination_port must be >= 1",
                            ));
                        }
                    }
                    RbacPermission::Any => {}
                }
            }
            for principal in &policy.principals {
                match principal {
                    RbacPrincipal::SourceCidr { cidr } => {
                        if !valid_cidr(cidr) {
                            return Err(DomainError::validation(format!(
                                "rbac: \"{cidr}\" is not a valid CIDR"
                            )));
                        }
                    }
                    RbacPrincipal::Header { name: header, .. } => {
                        if header.is_empty() {
                            return Err(DomainError::validation(
                                "rbac: header principal needs a header name",
                            ));
                        }
                    }
                    RbacPrincipal::Any => {}
                }
            }
        }
        Ok(())
    }
}

fn valid_cidr(cidr: &str) -> bool {
    let Some((ip, len)) = cidr.split_once('/') else {
        return false;
    };
    let Ok(len) = len.parse::<u8>() else {
        return false;
    };
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(_)) => len <= 32,
        Ok(std::net::IpAddr::V6(_)) => len <= 128,
        Err(_) => false,
    }
}

/// Validate a whole chain: per-filter rules plus chain-level invariants (one filter of
/// each type — duplicates make per-scope overrides ambiguous).
pub fn validate_filter_chain(entries: &[HttpFilterEntry]) -> DomainResult<()> {
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        entry.filter.validate()?;
        if !seen.insert(entry.filter.kind()) {
            return Err(DomainError::validation(format!(
                "duplicate filter type \"{}\" in the chain",
                entry.filter.kind()
            ))
            .with_hint("each filter type may appear at most once per listener"));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn cors() -> HttpFilterSpec {
        HttpFilterSpec::Cors(CorsConfig {
            allow_origin: vec![OriginMatcher::Exact {
                value: "https://app.example".into(),
            }],
            allow_methods: vec!["GET".into()],
            allow_headers: vec![],
            expose_headers: vec![],
            max_age_seconds: Some(600),
            allow_credentials: false,
        })
    }

    #[test]
    fn serde_round_trip_with_stable_tag() {
        let entry = HttpFilterEntry {
            filter: cors(),
            disabled: false,
        };
        let json = serde_json::to_value(&entry).expect("serialize");
        assert_eq!(json["filter"]["type"], "cors");
        let back: HttpFilterEntry = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.filter.kind(), "cors");
    }

    #[test]
    fn adversarial_configs_rejected() {
        // wildcard + credentials
        let bad = CorsConfig {
            allow_origin: vec![OriginMatcher::Exact { value: "*".into() }],
            allow_methods: vec![],
            allow_headers: vec![],
            expose_headers: vec![],
            max_age_seconds: None,
            allow_credentials: true,
        };
        assert!(bad.validate().is_err());

        let bad = LocalRateLimitConfig {
            stat_prefix: "x".into(),
            token_bucket: TokenBucket {
                max_tokens: 10,
                tokens_per_fill: None,
                fill_interval_ms: 0,
            },
            status_code: None,
        };
        assert!(bad.validate().is_err());

        let bad = LocalRateLimitConfig {
            stat_prefix: "x".into(),
            token_bucket: TokenBucket {
                max_tokens: 10,
                tokens_per_fill: None,
                fill_interval_ms: 1000,
            },
            status_code: Some(200),
        };
        assert!(bad.validate().is_err(), "2xx rate-limit status");

        let bad = GlobalRateLimitConfig {
            domain: "edge".into(),
            service_cluster: "rls".into(),
            timeout_ms: 20,
            failure_mode_deny: false,
            stage: 11,
            request_type: RateLimitRequestType::Both,
            stat_prefix: None,
            enable_x_ratelimit_headers: false,
            disable_x_envoy_ratelimited_header: false,
            rate_limited_status: None,
            status_on_error: None,
        };
        assert!(
            bad.validate().is_err(),
            "stage outside Envoy's 0..=10 range"
        );

        let bad = GlobalRateLimitConfig {
            stage: 0,
            rate_limited_status: Some(200),
            ..bad
        };
        assert!(bad.validate().is_err(), "2xx global rate-limit status");

        // unknown filter type fails loud
        assert!(serde_json::from_value::<HttpFilterSpec>(
            serde_json::json!({"type": "telnet_proxy"})
        )
        .is_err());
    }

    #[test]
    fn cors_rejects_unbounded_lists_and_values() {
        let mut bad = CorsConfig {
            allow_origin: (0..=MAX_CORS_ORIGINS)
                .map(|i| OriginMatcher::Exact {
                    value: format!("https://app{i}.example"),
                })
                .collect(),
            allow_methods: vec!["GET".into()],
            allow_headers: vec![],
            expose_headers: vec![],
            max_age_seconds: None,
            allow_credentials: false,
        };
        assert!(bad.validate().is_err(), "too many origins");

        bad.allow_origin = vec![OriginMatcher::Exact {
            value: "https://app.example".into(),
        }];
        bad.allow_headers = vec!["x\nbad".into()];
        assert!(bad.validate().is_err(), "control chars rejected");

        bad.allow_headers = (0..=MAX_CORS_LIST_VALUES)
            .map(|i| format!("x-header-{i}"))
            .collect();
        assert!(bad.validate().is_err(), "too many header entries");
    }

    #[test]
    fn header_mutation_rejects_unbounded_lists_and_values() {
        let too_many = HeaderMutationConfig {
            request_headers_to_add: (0..=MAX_HEADER_MUTATIONS_PER_DIRECTION)
                .map(|i| HeaderValue {
                    key: format!("x-header-{i}"),
                    value: "ok".into(),
                    append: false,
                })
                .collect(),
            request_headers_to_remove: vec![],
            response_headers_to_add: vec![],
            response_headers_to_remove: vec![],
        };
        assert!(too_many.validate().is_err(), "too many add mutations");

        let bad_value = HeaderMutationConfig {
            request_headers_to_add: vec![HeaderValue {
                key: "x-test".into(),
                value: "bad\nvalue".into(),
                append: false,
            }],
            request_headers_to_remove: vec![],
            response_headers_to_add: vec![],
            response_headers_to_remove: vec![],
        };
        assert!(bad_value.validate().is_err(), "control chars rejected");

        let bad_remove = HeaderMutationConfig {
            request_headers_to_add: vec![],
            request_headers_to_remove: vec!["".into()],
            response_headers_to_add: vec![],
            response_headers_to_remove: vec![],
        };
        assert!(
            bad_remove.validate().is_err(),
            "empty remove header rejected"
        );
    }

    #[test]
    fn duplicate_filter_types_rejected() {
        let chain = vec![
            HttpFilterEntry {
                filter: cors(),
                disabled: false,
            },
            HttpFilterEntry {
                filter: cors(),
                disabled: true,
            },
        ];
        assert!(validate_filter_chain(&chain).is_err());
    }

    #[test]
    fn jwt_auth_validates_providers_requirements_and_rules() {
        use std::collections::BTreeMap;
        let mut providers = BTreeMap::new();
        providers.insert(
            "auth0".to_string(),
            JwtProvider {
                issuer: Some("https://issuer.example".into()),
                audiences: vec!["api".into()],
                jwks: JwksSource::Remote {
                    uri: "https://issuer.example/jwks".into(),
                    cluster: "jwks-cluster".into(),
                    timeout_ms: 5000,
                    cache_duration_secs: None,
                },
                clock_skew_seconds: 60,
                forward: false,
            },
        );
        let mut requirement_map = BTreeMap::new();
        requirement_map.insert(
            "default".to_string(),
            JwtRequirement::Provider {
                provider_name: "auth0".into(),
            },
        );
        let good = JwtAuthConfig {
            providers: providers.clone(),
            requirement_map: requirement_map.clone(),
            rules: vec![JwtRule {
                matcher: crate::gateway::route_config::PathMatch::Prefix { prefix: "/".into() },
                requirement_name: "default".into(),
            }],
            bypass_cors_preflight: true,
        };
        assert!(good.validate().is_ok());

        // Requirement naming an unknown provider is rejected.
        let mut bad_reqs = BTreeMap::new();
        bad_reqs.insert(
            "default".to_string(),
            JwtRequirement::Provider {
                provider_name: "ghost".into(),
            },
        );
        let bad = JwtAuthConfig {
            providers: providers.clone(),
            requirement_map: bad_reqs,
            rules: vec![],
            bypass_cors_preflight: false,
        };
        assert!(bad.validate().is_err(), "unknown provider in requirement");

        // Rule referencing an undeclared requirement is rejected.
        let bad = JwtAuthConfig {
            providers,
            requirement_map,
            rules: vec![JwtRule {
                matcher: crate::gateway::route_config::PathMatch::Prefix { prefix: "/".into() },
                requirement_name: "nope".into(),
            }],
            bypass_cors_preflight: false,
        };
        assert!(bad.validate().is_err(), "rule names unknown requirement");

        // No providers at all is rejected.
        assert!(JwtAuthConfig {
            providers: BTreeMap::new(),
            requirement_map: BTreeMap::new(),
            rules: vec![],
            bypass_cors_preflight: false,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn ext_authz_and_rbac_validation() {
        assert!(ExtAuthzConfig {
            cluster: "authz".into(),
            timeout_ms: 200,
            failure_mode_allow: false,
            include_peer_certificate: true,
        }
        .validate()
        .is_ok());
        assert!(ExtAuthzConfig {
            cluster: "authz".into(),
            timeout_ms: 0,
            failure_mode_allow: false,
            include_peer_certificate: false,
        }
        .validate()
        .is_err());

        let mut policies = std::collections::BTreeMap::new();
        policies.insert(
            "admins".to_string(),
            RbacPolicy {
                permissions: vec![RbacPermission::UrlPath {
                    prefix: "/admin".into(),
                }],
                principals: vec![RbacPrincipal::SourceCidr {
                    cidr: "10.0.0.0/8".into(),
                }],
            },
        );
        assert!(RbacConfig {
            action: RbacAction::Allow,
            policies: policies.clone(),
        }
        .validate()
        .is_ok());

        // Bad CIDR rejected.
        let mut bad_policies = std::collections::BTreeMap::new();
        bad_policies.insert(
            "x".to_string(),
            RbacPolicy {
                permissions: vec![RbacPermission::Any],
                principals: vec![RbacPrincipal::SourceCidr {
                    cidr: "not-a-cidr".into(),
                }],
            },
        );
        assert!(RbacConfig {
            action: RbacAction::Deny,
            policies: bad_policies,
        }
        .validate()
        .is_err());

        for cidr in ["10.0.0.0/64", "2001:db8::/129"] {
            let mut policies = std::collections::BTreeMap::new();
            policies.insert(
                "x".to_string(),
                RbacPolicy {
                    permissions: vec![RbacPermission::Any],
                    principals: vec![RbacPrincipal::SourceCidr { cidr: cidr.into() }],
                },
            );
            assert!(
                RbacConfig {
                    action: RbacAction::Deny,
                    policies,
                }
                .validate()
                .is_err(),
                "{cidr} must be rejected"
            );
        }

        let mut ipv6_policies = std::collections::BTreeMap::new();
        ipv6_policies.insert(
            "ipv6".to_string(),
            RbacPolicy {
                permissions: vec![RbacPermission::Any],
                principals: vec![RbacPrincipal::SourceCidr {
                    cidr: "2001:db8::/128".into(),
                }],
            },
        );
        assert!(RbacConfig {
            action: RbacAction::Allow,
            policies: ipv6_policies,
        }
        .validate()
        .is_ok());

        // Empty policies rejected.
        assert!(RbacConfig {
            action: RbacAction::Allow,
            policies: std::collections::BTreeMap::new(),
        }
        .validate()
        .is_err());
    }

    #[test]
    fn jwt_per_route_override_target_and_disable_set() {
        // jwt_auth disable is now allowed per-route.
        assert!(FilterOverride::Disable {
            filter_type: "jwt_auth".into(),
        }
        .validate()
        .is_ok());
        // reference-only jwt override targets jwt_auth.
        let ov = FilterOverride::JwtAuth {
            requirement_name: "admin".into(),
        };
        assert_eq!(ov.target_kind().ok(), Some("jwt_auth"));
        assert!(ov.validate().is_ok());
        assert!(FilterOverride::JwtAuth {
            requirement_name: String::new(),
        }
        .validate()
        .is_err());
    }

    #[test]
    fn per_route_disable_uses_filter_kind_disablability() {
        for kind in HttpFilterKind::ALL {
            let result = FilterOverride::Disable {
                filter_type: kind.as_str().into(),
            }
            .validate();
            assert_eq!(
                result.is_ok(),
                kind.is_disablable(),
                "{} disablability must come from HttpFilterKind",
                kind.as_str()
            );
        }
        assert!(FilterOverride::Disable {
            filter_type: "not_a_filter".into(),
        }
        .validate()
        .is_err());
    }

    // ---------------- S6 global_rate_limit (separate author) ----------------

    use crate::gateway::cluster::RESERVED_RATE_LIMIT_CLUSTER;

    /// A fully-specified, otherwise-valid config with overridable domain/service_cluster.
    fn grl(domain: String, service_cluster: String) -> GlobalRateLimitConfig {
        GlobalRateLimitConfig {
            domain,
            service_cluster,
            timeout_ms: 20,
            failure_mode_deny: false,
            stage: 0,
            request_type: RateLimitRequestType::Both,
            stat_prefix: None,
            enable_x_ratelimit_headers: false,
            disable_x_envoy_ratelimited_header: false,
            rate_limited_status: None,
            status_on_error: None,
        }
    }

    #[test]
    fn cp_composed_domain_at_the_cap_passes() {
        // CP composes `{org_uuid}|{team_uuid}|{user_domain}` (S5/S7). Two 36-char UUIDs + two
        // '|' separators + a 253-char user domain = 327 chars == the cap. It must validate.
        let uuid_a = uuid::Uuid::now_v7().to_string();
        let uuid_b = uuid::Uuid::now_v7().to_string();
        assert_eq!(uuid_a.len(), 36, "UUID hyphenated form is 36 chars");
        let domain = format!("{}|{}|{}", uuid_a, uuid_b, "x".repeat(253));
        assert_eq!(domain.len(), 327, "composed length is exactly the cap");
        let cfg = grl(domain, "rls-cluster".into());
        assert!(
            cfg.validate().is_ok(),
            "the CP-composed value at the cap must pass: {:?}",
            cfg.validate()
        );
    }

    #[test]
    fn domain_one_over_cap_fails() {
        let domain = "d".repeat(328);
        assert_eq!(domain.len(), 328);
        let cfg = grl(domain, "rls-cluster".into());
        assert!(
            cfg.validate().is_err(),
            "328 chars (cap+1) must be rejected"
        );
    }

    #[test]
    fn empty_domain_fails() {
        let cfg = grl(String::new(), "rls-cluster".into());
        assert!(cfg.validate().is_err(), "empty domain must be rejected");
    }

    #[test]
    fn service_cluster_defaults_to_reserved_when_omitted() {
        // Omitting service_cluster in JSON must default to the reserved built-in cluster name.
        let cfg: GlobalRateLimitConfig = serde_json::from_value(serde_json::json!({
            "domain": "edge",
        }))
        .expect("deserialize without service_cluster");
        assert_eq!(cfg.service_cluster, "rate_limit_cluster");
        assert_eq!(cfg.service_cluster, RESERVED_RATE_LIMIT_CLUSTER);
        // And the defaulted value must itself validate (exempt from the slug rule).
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn reserved_service_cluster_is_exempt_from_slug_rule_but_others_are_not() {
        // The reserved underscore-bearing name is accepted even though validate_name rejects '_'.
        let reserved = grl("edge".into(), RESERVED_RATE_LIMIT_CLUSTER.into());
        assert!(
            reserved.validate().is_ok(),
            "rate_limit_cluster must be exempt from validate_name"
        );

        // A name with a space/uppercase is not a valid cluster name → must fail.
        let spaced = grl("edge".into(), "Bad Cluster".into());
        assert!(
            spaced.validate().is_err(),
            "service_cluster with a space/uppercase must be rejected"
        );

        // Another underscore-bearing-but-non-reserved name is NOT exempt: validate_name rejects
        // underscores, so this must fail (the exemption is exact-match only).
        let underscored = grl("edge".into(), "rate_limit_other".into());
        assert!(
            underscored.validate().is_err(),
            "only the exact reserved name is exempt; rate_limit_other must fail validate_name"
        );

        // Sanity: an ordinary valid slug cluster name passes.
        let ok = grl("edge".into(), "team-rls".into());
        assert!(ok.validate().is_ok());
    }
}
