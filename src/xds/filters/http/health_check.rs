//! Health Check HTTP filter configuration helpers
//!
//! This module provides configuration for Envoy's health check filter,
//! which allows configuring health check endpoints that return 200 OK.

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::extensions::filters::http::health_check::v3::HealthCheck as HealthCheckProto;
use envoy_types::pb::google::protobuf::{Any as EnvoyAny, BoolValue, Duration as ProtoDuration};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const HEALTH_CHECK_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.health_check.v3.HealthCheck";

/// Configuration for health check filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthCheckConfig {
    /// Whether to operate in pass-through mode
    #[serde(default)]
    pub pass_through_mode: bool,
    /// Cache time in milliseconds for pass-through mode
    #[serde(default)]
    pub cache_time_ms: Option<u64>,
    /// Health check endpoint path
    pub endpoint_path: String,
}

impl HealthCheckConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.endpoint_path.trim().is_empty() {
            return Err(invalid_config("HealthCheck endpoint_path cannot be empty"));
        }

        if !self.endpoint_path.starts_with('/') {
            return Err(invalid_config("HealthCheck endpoint_path must start with '/'"));
        }

        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let mut proto = HealthCheckProto {
            pass_through_mode: Some(BoolValue {
                value: self.pass_through_mode,
            }),
            headers: vec![
                // Match the health check path using :path pseudo-header
                envoy_types::pb::envoy::config::route::v3::HeaderMatcher {
                    name: ":path".to_string(),
                    header_match_specifier: Some(
                        envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::StringMatch(
                            envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
                                match_pattern: Some(
                                    envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                        self.endpoint_path.clone(),
                                    ),
                                ),
                                ignore_case: false,
                            },
                        ),
                    ),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        if let Some(cache_ms) = self.cache_time_ms {
            let seconds = (cache_ms / 1000) as i64;
            let nanos = ((cache_ms % 1000) * 1_000_000) as i32;
            proto.cache_time = Some(ProtoDuration { seconds, nanos });
        }

        Ok(any_from_message(HEALTH_CHECK_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &HealthCheckProto) -> Result<Self, crate::Error> {
        let pass_through_mode = proto.pass_through_mode.as_ref().map(|v| v.value).unwrap_or(false);

        let cache_time_ms = proto
            .cache_time
            .as_ref()
            .map(|duration| (duration.seconds as u64) * 1000 + (duration.nanos as u64) / 1_000_000);

        // Extract endpoint path from headers
        let endpoint_path = proto
            .headers
            .iter()
            .find(|h| h.name == ":path")
            .and_then(|h| match &h.header_match_specifier {
                Some(envoy_types::pb::envoy::config::route::v3::header_matcher::HeaderMatchSpecifier::StringMatch(sm)) => {
                    match &sm.match_pattern {
                        Some(envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(path)) => {
                            Some(path.clone())
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .unwrap_or_else(|| "/health".to_string());

        let config = Self { pass_through_mode, cache_time_ms, endpoint_path };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn sample_config() -> HealthCheckConfig {
        HealthCheckConfig {
            pass_through_mode: true,
            cache_time_ms: Some(5000),
            endpoint_path: "/healthz".into(),
        }
    }

    #[test]
    fn validates_endpoint_path() {
        let mut config = sample_config();
        config.endpoint_path = "".into();
        let err = config.validate().expect_err("empty path should fail");
        assert!(format!("{err}").contains("endpoint_path"));
    }

    #[test]
    fn validates_path_starts_with_slash() {
        let mut config = sample_config();
        config.endpoint_path = "health".into();
        let err = config.validate().expect_err("path without slash should fail");
        assert!(format!("{err}").contains("start with"));
    }

    #[test]
    fn builds_proto() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, HEALTH_CHECK_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn proto_round_trip() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");

        let proto = HealthCheckProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = HealthCheckConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(round_tripped.endpoint_path, "/healthz");
        assert!(round_tripped.pass_through_mode);
        assert_eq!(round_tripped.cache_time_ms, Some(5000));
    }

    #[test]
    fn default_passthrough_false() {
        let config = HealthCheckConfig {
            pass_through_mode: false,
            cache_time_ms: None,
            endpoint_path: "/health".into(),
        };

        let any = config.to_any().expect("to_any");
        assert!(!any.value.is_empty());
    }
}
