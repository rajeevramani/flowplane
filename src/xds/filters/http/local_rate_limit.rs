//! Local Rate Limit HTTP filter configuration helpers

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::core::v3::RuntimeFractionalPercent;
use envoy_types::pb::envoy::extensions::filters::http::local_ratelimit::v3::LocalRateLimit;
use envoy_types::pb::envoy::r#type::v3::{fractional_percent, FractionalPercent, TokenBucket};
use envoy_types::pb::google::protobuf::{
    Any as EnvoyAny, BoolValue, Duration as ProtoDuration, UInt32Value,
};
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use utoipa::ToSchema;

/// Lightweight representation of Envoy's TokenBucket message
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TokenBucketConfig {
    /// Maximum tokens available in the bucket
    pub max_tokens: u32,
    /// Tokens to add during each refill. Defaults to `max_tokens` if omitted.
    #[serde(default)]
    pub tokens_per_fill: Option<u32>,
    /// Fill interval in milliseconds
    pub fill_interval_ms: u64,
}

impl TokenBucketConfig {
    fn to_proto(&self) -> Result<TokenBucket, crate::Error> {
        if self.fill_interval_ms == 0 {
            return Err(invalid_config(
                "LocalRateLimit token bucket fill_interval_ms must be greater than 0",
            ));
        }

        let seconds = (self.fill_interval_ms / 1000) as i64;
        let nanos = ((self.fill_interval_ms % 1000) * 1_000_000) as i32;

        Ok(TokenBucket {
            max_tokens: self.max_tokens,
            tokens_per_fill: Some(UInt32Value {
                value: self.tokens_per_fill.unwrap_or(self.max_tokens),
            }),
            fill_interval: Some(ProtoDuration { seconds, nanos }),
        })
    }

    fn from_proto(proto: &TokenBucket) -> Result<Self, crate::Error> {
        let fill_interval = proto
            .fill_interval
            .as_ref()
            .ok_or_else(|| invalid_config("LocalRateLimit token bucket requires fill_interval"))?;

        let fill_interval_ms = duration_to_millis(fill_interval)?;

        Ok(Self {
            max_tokens: proto.max_tokens,
            tokens_per_fill: proto.tokens_per_fill.as_ref().map(|value| value.value),
            fill_interval_ms,
        })
    }
}

/// Representation of Envoy's FractionalPercent + runtime key wrapper
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RuntimeFractionalPercentConfig {
    /// Optional runtime key for dynamic percentage overrides
    #[serde(default)]
    pub runtime_key: Option<String>,
    /// Numerator value for the fraction
    pub numerator: u32,
    /// Denominator specifying the unit of the numerator
    #[serde(default)]
    pub denominator: FractionalPercentDenominator,
}

impl RuntimeFractionalPercentConfig {
    fn to_proto(&self) -> RuntimeFractionalPercent {
        RuntimeFractionalPercent {
            runtime_key: self.runtime_key.clone().unwrap_or_default(),
            default_value: Some(FractionalPercent {
                numerator: self.numerator,
                denominator: self.denominator.to_proto_value(),
            }),
        }
    }

    fn from_proto(proto: &RuntimeFractionalPercent) -> Result<Self, crate::Error> {
        let default_value = proto
            .default_value
            .as_ref()
            .ok_or_else(|| invalid_config("RuntimeFractionalPercent missing default_value"))?;

        Ok(Self {
            runtime_key: if proto.runtime_key.is_empty() {
                None
            } else {
                Some(proto.runtime_key.clone())
            },
            numerator: default_value.numerator,
            denominator: FractionalPercentDenominator::from_proto_value(default_value.denominator)?,
        })
    }
}

/// Denominator options mirroring Envoy enum
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FractionalPercentDenominator {
    /// Out of 100 (percentage)
    #[default]
    Hundred,
    /// Out of 10,000 (basis points)
    TenThousand,
    /// Out of 1,000,000
    Million,
}

impl FractionalPercentDenominator {
    fn to_proto_value(self) -> i32 {
        match self {
            Self::Hundred => fractional_percent::DenominatorType::Hundred as i32,
            Self::TenThousand => fractional_percent::DenominatorType::TenThousand as i32,
            Self::Million => fractional_percent::DenominatorType::Million as i32,
        }
    }

    fn from_proto_value(value: i32) -> Result<Self, crate::Error> {
        match fractional_percent::DenominatorType::try_from(value) {
            Ok(fractional_percent::DenominatorType::Hundred) => Ok(Self::Hundred),
            Ok(fractional_percent::DenominatorType::TenThousand) => Ok(Self::TenThousand),
            Ok(fractional_percent::DenominatorType::Million) => Ok(Self::Million),
            Err(_) => Err(invalid_config(format!(
                "Unsupported fractional percent denominator: {}",
                value
            ))),
        }
    }
}

/// Simplified Local Rate Limit filter configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LocalRateLimitConfig {
    /// Prefix for statistics emitted by the filter
    pub stat_prefix: String,
    /// Token bucket used by the filter. Required at route level.
    #[serde(default)]
    pub token_bucket: Option<TokenBucketConfig>,
    /// Optional HTTP status returned when the request is rate limited
    #[serde(default)]
    pub status_code: Option<u16>,
    /// Fraction of requests where the filter is enabled
    #[serde(default)]
    pub filter_enabled: Option<RuntimeFractionalPercentConfig>,
    /// Fraction of enabled requests where the rate limit is enforced
    #[serde(default)]
    pub filter_enforced: Option<RuntimeFractionalPercentConfig>,
    /// Apply rate limit on a per-connection basis instead of global tokens
    #[serde(default)]
    pub per_downstream_connection: Option<bool>,
    /// Enable gRPC RESOURCE_EXHAUSTED mapping instead of UNAVAILABLE
    #[serde(default)]
    pub rate_limited_as_resource_exhausted: Option<bool>,
    /// Maximum dynamic descriptors cached for wildcard descriptors
    #[serde(default)]
    pub max_dynamic_descriptors: Option<u32>,
    /// Whether to always consume the default token bucket
    #[serde(default)]
    pub always_consume_default_token_bucket: Option<bool>,
}

impl LocalRateLimitConfig {
    /// Create a default 100% fractional percent config for filter_enabled/filter_enforced.
    /// This is needed because Envoy requires these fields to be set for the rate limit to work.
    fn default_100_percent() -> RuntimeFractionalPercent {
        RuntimeFractionalPercent {
            runtime_key: String::new(),
            default_value: Some(FractionalPercent {
                numerator: 100,
                denominator: fractional_percent::DenominatorType::Hundred as i32,
            }),
        }
    }

    /// Convert into Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        let mut proto =
            LocalRateLimit { stat_prefix: self.stat_prefix.clone(), ..Default::default() };

        if let Some(bucket) = &self.token_bucket {
            proto.token_bucket = Some(bucket.to_proto()?);
        }

        if let Some(code) = self.status_code {
            proto.status = Some(envoy_types::pb::envoy::r#type::v3::HttpStatus {
                code: (code as i32).clamp(400, 599),
            });
        }

        // IMPORTANT: filter_enabled and filter_enforced MUST be set for the rate limit to work.
        // If not explicitly provided, default to 100% enabled/enforced.
        // This is critical for per-route configs that override the listener config.
        proto.filter_enabled = Some(
            self.filter_enabled
                .as_ref()
                .map(|e| e.to_proto())
                .unwrap_or_else(Self::default_100_percent),
        );

        proto.filter_enforced = Some(
            self.filter_enforced
                .as_ref()
                .map(|e| e.to_proto())
                .unwrap_or_else(Self::default_100_percent),
        );

        if let Some(per_conn) = self.per_downstream_connection {
            proto.local_rate_limit_per_downstream_connection = per_conn;
        }

        if let Some(as_resource_exhausted) = self.rate_limited_as_resource_exhausted {
            proto.rate_limited_as_resource_exhausted = as_resource_exhausted;
        }

        if let Some(max_dynamic) = self.max_dynamic_descriptors {
            proto.max_dynamic_descriptors = Some(UInt32Value { value: max_dynamic });
        }

        if let Some(always_consume) = self.always_consume_default_token_bucket {
            proto.always_consume_default_token_bucket = Some(BoolValue { value: always_consume });
        }

        if self.token_bucket.is_none() {
            return Err(invalid_config(
                "LocalRateLimit configuration requires token_bucket to be specified",
            ));
        }

        Ok(any_from_message(
            "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit",
            &proto,
        ))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &LocalRateLimit) -> Result<Self, crate::Error> {
        let token_bucket =
            proto.token_bucket.as_ref().map(TokenBucketConfig::from_proto).transpose()?;

        let status_code =
            proto.status.as_ref().map(|status| status.code as u16).filter(|code| *code >= 100);

        let filter_enabled = proto
            .filter_enabled
            .as_ref()
            .map(RuntimeFractionalPercentConfig::from_proto)
            .transpose()?;

        let filter_enforced = proto
            .filter_enforced
            .as_ref()
            .map(RuntimeFractionalPercentConfig::from_proto)
            .transpose()?;

        let max_dynamic_descriptors =
            proto.max_dynamic_descriptors.as_ref().map(|value| value.value);

        let always_consume_default_token_bucket =
            proto.always_consume_default_token_bucket.as_ref().map(|value| value.value);

        Ok(Self {
            stat_prefix: proto.stat_prefix.clone(),
            token_bucket,
            status_code,
            filter_enabled,
            filter_enforced,
            per_downstream_connection: Some(proto.local_rate_limit_per_downstream_connection),
            rate_limited_as_resource_exhausted: Some(proto.rate_limited_as_resource_exhausted),
            max_dynamic_descriptors,
            always_consume_default_token_bucket,
        })
    }
}

fn duration_to_millis(duration: &ProtoDuration) -> Result<u64, crate::Error> {
    if duration.seconds < 0 || duration.nanos < 0 {
        return Err(invalid_config("LocalRateLimit token bucket fill_interval must be positive"));
    }

    let millis_from_secs = (duration.seconds as u64)
        .checked_mul(1000)
        .ok_or_else(|| invalid_config("LocalRateLimit fill_interval is too large"))?;
    let millis_from_nanos = (duration.nanos as u64) / 1_000_000;

    Ok(millis_from_secs + millis_from_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_basic_proto() {
        let cfg = LocalRateLimitConfig {
            stat_prefix: "ingress_http".into(),
            token_bucket: Some(TokenBucketConfig {
                max_tokens: 100,
                tokens_per_fill: Some(50),
                fill_interval_ms: 1000,
            }),
            status_code: Some(429),
            filter_enabled: None,
            filter_enforced: None,
            per_downstream_connection: Some(false),
            rate_limited_as_resource_exhausted: Some(true),
            max_dynamic_descriptors: Some(10),
            always_consume_default_token_bucket: None,
        };

        let any = cfg.to_any().expect("to_any");
        assert_eq!(
            any.type_url,
            "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit"
        );
        assert!(!any.value.is_empty());
    }

    #[test]
    fn missing_bucket_errors() {
        let cfg = LocalRateLimitConfig {
            stat_prefix: "missing".into(),
            token_bucket: None,
            status_code: None,
            filter_enabled: None,
            filter_enforced: None,
            per_downstream_connection: None,
            rate_limited_as_resource_exhausted: None,
            max_dynamic_descriptors: None,
            always_consume_default_token_bucket: None,
        };

        let err = cfg.to_any().expect_err("missing bucket should fail");
        assert!(matches!(err, crate::Error::Config { .. }));
    }

    #[test]
    fn rejects_zero_interval() {
        let cfg = LocalRateLimitConfig {
            stat_prefix: "bad".into(),
            token_bucket: Some(TokenBucketConfig {
                max_tokens: 1,
                tokens_per_fill: None,
                fill_interval_ms: 0,
            }),
            status_code: None,
            filter_enabled: None,
            filter_enforced: None,
            per_downstream_connection: None,
            rate_limited_as_resource_exhausted: None,
            max_dynamic_descriptors: None,
            always_consume_default_token_bucket: None,
        };

        let err = cfg.to_any().expect_err("zero interval should fail");
        assert!(matches!(err, crate::Error::Config { .. }));
    }

    #[test]
    fn fractional_percent_defaults() {
        let percent = RuntimeFractionalPercentConfig {
            runtime_key: None,
            numerator: 5,
            denominator: FractionalPercentDenominator::TenThousand,
        };

        let proto = percent.to_proto();
        assert_eq!(proto.default_value.unwrap().numerator, 5);
        assert_eq!(proto.runtime_key, "");
    }
}
