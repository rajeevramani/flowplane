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
}

impl HttpFilterSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Cors(_) => "cors",
            Self::LocalRateLimit(_) => "local_rate_limit",
            Self::HeaderMutation(_) => "header_mutation",
            Self::HealthCheck(_) => "health_check",
            Self::Compressor(_) => "compressor",
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        match self {
            Self::Cors(c) => c.validate(),
            Self::LocalRateLimit(c) => c.validate(),
            Self::HeaderMutation(c) => c.validate(),
            Self::HealthCheck(c) => c.validate(),
            Self::Compressor(c) => c.validate(),
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
}

impl FilterOverride {
    /// The chain filter type this override targets.
    pub fn target_kind(&self) -> DomainResult<&str> {
        match self {
            Self::Disable { filter_type } => {
                // health_check is listener-only (spec/04 §4.1): no per-route control at all.
                const DISABLABLE: &[&str] =
                    &["cors", "local_rate_limit", "header_mutation", "compressor"];
                if !DISABLABLE.contains(&filter_type.as_str()) {
                    return Err(DomainError::validation(format!(
                        "filter type \"{filter_type}\" cannot be disabled per-route",
                    ))
                    .with_hint(format!("disablable types: {}", DISABLABLE.join(", "))));
                }
                Ok(filter_type)
            }
            Self::Cors(_) => Ok("cors"),
            Self::LocalRateLimit(_) => Ok("local_rate_limit"),
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        match self {
            Self::Disable { .. } => self.target_kind().map(|_| ()),
            Self::Cors(c) => c.validate(),
            Self::LocalRateLimit(c) => c.validate(),
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

        // unknown filter type fails loud
        assert!(serde_json::from_value::<HttpFilterSpec>(
            serde_json::json!({"type": "telnet_proxy"})
        )
        .is_err());
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
}
