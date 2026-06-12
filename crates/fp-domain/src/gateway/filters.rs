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
    Cors(CorsConfig),
    LocalRateLimit(LocalRateLimitConfig),
    HeaderMutation(HeaderMutationConfig),
}

impl HttpFilterSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Cors(_) => "cors",
            Self::LocalRateLimit(_) => "local_rate_limit",
            Self::HeaderMutation(_) => "header_mutation",
        }
    }

    pub fn validate(&self) -> DomainResult<()> {
        match self {
            Self::Cors(c) => c.validate(),
            Self::LocalRateLimit(c) => c.validate(),
            Self::HeaderMutation(c) => c.validate(),
        }
    }
}

/// Per-route/vhost override: disable a chain filter on this scope. Full per-route config
/// overrides arrive with the rest of the catalog (S5.8c); disable is universal and safe
/// for every type shipped so far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterOverride {
    /// `kind()` string of the chain filter this override targets.
    pub filter_type: String,
    /// Currently only `true` is meaningful: skip this filter for the scope.
    pub disabled: bool,
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
