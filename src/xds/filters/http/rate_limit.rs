//! Distributed Rate Limit HTTP filter configuration helpers
//!
//! This module provides configuration structures for Envoy's distributed rate limit filter,
//! which integrates with an external rate limit service (e.g., Lyft's ratelimit service).

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::core::v3::{
    grpc_service::{EnvoyGrpc, TargetSpecifier},
    GrpcService,
};
use envoy_types::pb::envoy::config::ratelimit::v3::RateLimitServiceConfig;
use envoy_types::pb::envoy::extensions::filters::http::ratelimit::v3::{
    rate_limit::XRateLimitHeadersRfcVersion, RateLimit as RateLimitProto,
    RateLimitPerRoute as RateLimitPerRouteProto,
};
use envoy_types::pb::google::protobuf::{Any as EnvoyAny, Duration as ProtoDuration};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const RATE_LIMIT_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.ratelimit.v3.RateLimit";
const RATE_LIMIT_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.ratelimit.v3.RateLimitPerRoute";

/// Configuration for distributed rate limiting filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RateLimitConfig {
    /// Domain name to use when calling rate limit service
    pub domain: String,
    /// Rate limit service configuration
    pub rate_limit_service: RateLimitServiceGrpcConfig,
    /// Timeout for rate limit service calls in milliseconds
    #[serde(default = "RateLimitConfig::default_timeout_ms")]
    pub timeout_ms: u64,
    /// Whether to fail open (allow traffic) when rate limit service is unavailable
    #[serde(default)]
    pub failure_mode_deny: bool,
    /// Whether to enable rate limiting based on x-ratelimit headers
    #[serde(default)]
    pub enable_x_ratelimit_headers: Option<XRateLimitHeadersRfcVersionConfig>,
    /// Whether to disable sending x-envoy-ratelimited header on rate limited responses
    #[serde(default)]
    pub disable_x_envoy_ratelimited_header: bool,
    /// Custom rate limit status override (default: 429)
    #[serde(default)]
    pub rate_limited_status: Option<u32>,
    /// Optional statistics prefix
    #[serde(default)]
    pub stat_prefix: Option<String>,
}

impl RateLimitConfig {
    const fn default_timeout_ms() -> u64 {
        20
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.domain.trim().is_empty() {
            return Err(invalid_config("RateLimit domain cannot be empty"));
        }

        self.rate_limit_service.validate()?;

        if let Some(status) = self.rate_limited_status {
            if !(400..=599).contains(&status) {
                return Err(invalid_config(
                    "RateLimit rate_limited_status must be between 400 and 599",
                ));
            }
        }

        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let seconds = (self.timeout_ms / 1000) as i64;
        let nanos = ((self.timeout_ms % 1000) * 1_000_000) as i32;

        let mut proto = RateLimitProto {
            domain: self.domain.clone(),
            timeout: Some(ProtoDuration { seconds, nanos }),
            failure_mode_deny: self.failure_mode_deny,
            rate_limit_service: Some(self.rate_limit_service.to_proto()?),
            disable_x_envoy_ratelimited_header: self.disable_x_envoy_ratelimited_header,
            stat_prefix: self.stat_prefix.clone().unwrap_or_default(),
            ..Default::default()
        };

        if let Some(ref mode) = self.enable_x_ratelimit_headers {
            proto.enable_x_ratelimit_headers = mode.to_proto_value();
        }

        if let Some(status) = self.rate_limited_status {
            proto.rate_limited_status =
                Some(envoy_types::pb::envoy::r#type::v3::HttpStatus { code: status as i32 });
        }

        Ok(any_from_message(RATE_LIMIT_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &RateLimitProto) -> Result<Self, crate::Error> {
        let rate_limit_service = proto
            .rate_limit_service
            .as_ref()
            .ok_or_else(|| invalid_config("RateLimit requires rate_limit_service"))?;

        let timeout =
            proto.timeout.as_ref().ok_or_else(|| invalid_config("RateLimit requires timeout"))?;

        let timeout_ms = (timeout.seconds as u64) * 1000 + (timeout.nanos as u64) / 1_000_000;

        let config = Self {
            domain: proto.domain.clone(),
            rate_limit_service: RateLimitServiceGrpcConfig::from_proto(rate_limit_service)?,
            timeout_ms,
            failure_mode_deny: proto.failure_mode_deny,
            enable_x_ratelimit_headers: XRateLimitHeadersRfcVersionConfig::from_proto_value(
                proto.enable_x_ratelimit_headers,
            ),
            disable_x_envoy_ratelimited_header: proto.disable_x_envoy_ratelimited_header,
            rate_limited_status: proto.rate_limited_status.as_ref().map(|s| s.code as u32),
            stat_prefix: if proto.stat_prefix.is_empty() {
                None
            } else {
                Some(proto.stat_prefix.clone())
            },
        };

        config.validate()?;
        Ok(config)
    }
}

/// X-RateLimit header RFC version configuration
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum XRateLimitHeadersRfcVersionConfig {
    /// Do not send x-ratelimit headers
    Off,
    /// Send draft RFC Version 03 headers
    DraftVersion03,
}

impl XRateLimitHeadersRfcVersionConfig {
    fn to_proto_value(self) -> i32 {
        match self {
            Self::Off => XRateLimitHeadersRfcVersion::Off as i32,
            Self::DraftVersion03 => XRateLimitHeadersRfcVersion::DraftVersion03 as i32,
        }
    }

    fn from_proto_value(value: i32) -> Option<Self> {
        match value {
            v if v == XRateLimitHeadersRfcVersion::Off as i32 => Some(Self::Off),
            v if v == XRateLimitHeadersRfcVersion::DraftVersion03 as i32 => {
                Some(Self::DraftVersion03)
            }
            _ => None,
        }
    }
}

/// Rate limit service gRPC configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RateLimitServiceGrpcConfig {
    /// Name of the Envoy cluster for the rate limit service
    pub cluster_name: String,
    /// Authority header to send with gRPC requests
    #[serde(default)]
    pub authority: Option<String>,
}

impl RateLimitServiceGrpcConfig {
    fn validate(&self) -> Result<(), crate::Error> {
        if self.cluster_name.trim().is_empty() {
            return Err(invalid_config("RateLimit service cluster_name cannot be empty"));
        }
        Ok(())
    }

    fn to_proto(&self) -> Result<RateLimitServiceConfig, crate::Error> {
        self.validate()?;

        let grpc_service = GrpcService {
            timeout: None,
            target_specifier: Some(TargetSpecifier::EnvoyGrpc(EnvoyGrpc {
                cluster_name: self.cluster_name.clone(),
                authority: self.authority.clone().unwrap_or_default(),
                retry_policy: None,
                max_receive_message_length: None,
                skip_envoy_headers: false,
            })),
            initial_metadata: Vec::new(),
            retry_policy: None,
        };

        Ok(RateLimitServiceConfig {
            grpc_service: Some(grpc_service),
            transport_api_version: envoy_types::pb::envoy::config::core::v3::ApiVersion::V3 as i32,
        })
    }

    fn from_proto(proto: &RateLimitServiceConfig) -> Result<Self, crate::Error> {
        let grpc_service = proto
            .grpc_service
            .as_ref()
            .ok_or_else(|| invalid_config("RateLimit service requires grpc_service"))?;

        let (cluster_name, authority) = match &grpc_service.target_specifier {
            Some(TargetSpecifier::EnvoyGrpc(envoy_grpc)) => {
                let auth = if envoy_grpc.authority.is_empty() {
                    None
                } else {
                    Some(envoy_grpc.authority.clone())
                };
                (envoy_grpc.cluster_name.clone(), auth)
            }
            _ => {
                return Err(invalid_config(
                    "RateLimit service requires envoy_grpc target specifier",
                ))
            }
        };

        Ok(Self { cluster_name, authority })
    }
}

/// Per-route rate limit configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RateLimitPerRouteConfig {
    /// Optional domain override for this route
    #[serde(default)]
    pub domain: Option<String>,
    /// Whether to include virtual host rate limits (default: true)
    #[serde(default = "RateLimitPerRouteConfig::default_vh_rate_limits")]
    pub include_vh_rate_limits: bool,
}

impl RateLimitPerRouteConfig {
    const fn default_vh_rate_limits() -> bool {
        true
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if let Some(ref domain) = self.domain {
            if domain.trim().is_empty() {
                return Err(invalid_config(
                    "RateLimitPerRoute domain cannot be empty when specified",
                ));
            }
        }
        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let proto = RateLimitPerRouteProto {
            vh_rate_limits: if self.include_vh_rate_limits {
                1 // INCLUDE
            } else {
                2 // IGNORE
            },
            override_option: 0,      // DEFAULT (not implemented)
            rate_limits: Vec::new(), // Advanced feature - can be added later
            domain: self.domain.clone().unwrap_or_default(),
        };

        Ok(any_from_message(RATE_LIMIT_PER_ROUTE_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &RateLimitPerRouteProto) -> Result<Self, crate::Error> {
        let domain = if proto.domain.is_empty() { None } else { Some(proto.domain.clone()) };

        let config = Self { domain, include_vh_rate_limits: proto.vh_rate_limits == 1 };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn sample_config() -> RateLimitConfig {
        RateLimitConfig {
            domain: "my-domain".into(),
            rate_limit_service: RateLimitServiceGrpcConfig {
                cluster_name: "rate_limit_cluster".into(),
                authority: Some("ratelimit.svc.cluster.local".into()),
            },
            timeout_ms: 100,
            failure_mode_deny: false,
            enable_x_ratelimit_headers: Some(XRateLimitHeadersRfcVersionConfig::DraftVersion03),
            disable_x_envoy_ratelimited_header: false,
            rate_limited_status: Some(429),
            stat_prefix: Some("http_rate_limit".into()),
        }
    }

    #[test]
    fn validates_domain() {
        let mut config = sample_config();
        config.domain = "".into();
        let err = config.validate().expect_err("empty domain should fail");
        assert!(format!("{err}").contains("domain"));
    }

    #[test]
    fn validates_cluster_name() {
        let mut config = sample_config();
        config.rate_limit_service.cluster_name = "".into();
        let err = config.validate().expect_err("empty cluster should fail");
        assert!(format!("{err}").contains("cluster_name"));
    }

    #[test]
    fn validates_status_code() {
        let mut config = sample_config();
        config.rate_limited_status = Some(200);
        let err = config.validate().expect_err("invalid status should fail");
        assert!(format!("{err}").contains("400 and 599"));
    }

    #[test]
    fn builds_proto() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, RATE_LIMIT_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn proto_round_trip() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");

        let proto = RateLimitProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = RateLimitConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(round_tripped.domain, "my-domain");
        assert_eq!(round_tripped.rate_limit_service.cluster_name, "rate_limit_cluster");
        assert_eq!(round_tripped.timeout_ms, 100);
        assert!(!round_tripped.failure_mode_deny);
        assert_eq!(round_tripped.rate_limited_status, Some(429));
    }

    #[test]
    fn x_ratelimit_header_modes() {
        assert_eq!(
            XRateLimitHeadersRfcVersionConfig::Off.to_proto_value(),
            XRateLimitHeadersRfcVersion::Off as i32
        );
        assert_eq!(
            XRateLimitHeadersRfcVersionConfig::DraftVersion03.to_proto_value(),
            XRateLimitHeadersRfcVersion::DraftVersion03 as i32
        );
        assert!(matches!(
            XRateLimitHeadersRfcVersionConfig::from_proto_value(
                XRateLimitHeadersRfcVersion::Off as i32
            ),
            Some(XRateLimitHeadersRfcVersionConfig::Off)
        ));
    }

    #[test]
    fn per_route_builds_proto() {
        let config = RateLimitPerRouteConfig {
            domain: Some("route-domain".into()),
            include_vh_rate_limits: false,
        };

        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, RATE_LIMIT_PER_ROUTE_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn per_route_proto_round_trip() {
        let config = RateLimitPerRouteConfig {
            domain: Some("my-route-domain".into()),
            include_vh_rate_limits: true,
        };

        let any = config.to_any().expect("to_any");
        let proto = RateLimitPerRouteProto::decode(any.value.as_slice()).expect("decode");
        let round_tripped = RateLimitPerRouteConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(round_tripped.domain, Some("my-route-domain".into()));
        assert!(round_tripped.include_vh_rate_limits);
    }

    #[test]
    fn per_route_with_no_domain() {
        let config = RateLimitPerRouteConfig { domain: None, include_vh_rate_limits: false };

        let any = config.to_any().expect("to_any");
        let proto = RateLimitPerRouteProto::decode(any.value.as_slice()).expect("decode");
        let round_tripped = RateLimitPerRouteConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(round_tripped.domain, None);
        assert!(!round_tripped.include_vh_rate_limits);
    }

    #[test]
    fn per_route_validates_empty_domain() {
        let config =
            RateLimitPerRouteConfig { domain: Some("".into()), include_vh_rate_limits: true };

        let err = config.validate().expect_err("empty domain should fail");
        assert!(format!("{err}").contains("cannot be empty"));
    }

    #[test]
    fn per_route_default_includes_vh_limits() {
        let config = RateLimitPerRouteConfig {
            domain: None,
            include_vh_rate_limits: RateLimitPerRouteConfig::default_vh_rate_limits(),
        };

        assert!(config.include_vh_rate_limits);
    }
}
