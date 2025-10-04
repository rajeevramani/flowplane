//! Custom Response HTTP filter configuration helpers
//!
//! This module provides configuration for Envoy's custom response filter,
//! which allows defining custom response policies based on matcher trees.

use crate::xds::filters::{any_from_message, Base64Bytes, TypedConfig};
use envoy_types::pb::envoy::extensions::filters::http::custom_response::v3::CustomResponse as CustomResponseProto;
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const CUSTOM_RESPONSE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.custom_response.v3.CustomResponse";

/// Configuration for custom response filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct CustomResponseConfig {
    /// Custom response matcher configuration
    #[serde(default)]
    pub custom_response_matcher: Option<MatcherConfig>,
}

/// Matcher configuration for custom responses
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MatcherConfig {
    /// Typed matcher configuration
    #[serde(flatten)]
    pub config: TypedConfig,
}

impl CustomResponseConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        // No specific validation needed - matcher can be optional
        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        use prost::Message;

        self.validate()?;

        let custom_response_matcher = self.custom_response_matcher.as_ref().map(|m| {
            let any = m.config.to_any();
            envoy_types::pb::xds::r#type::matcher::v3::Matcher::decode(any.value.as_slice())
                .unwrap_or_default()
        });

        let proto = CustomResponseProto { custom_response_matcher };

        Ok(any_from_message(CUSTOM_RESPONSE_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &CustomResponseProto) -> Result<Self, crate::Error> {
        use prost::Message;

        let custom_response_matcher = proto.custom_response_matcher.as_ref().map(|matcher| {
            let mut buf = Vec::new();
            matcher.encode(&mut buf).ok();

            MatcherConfig {
                config: TypedConfig {
                    type_url: "type.googleapis.com/xds.type.matcher.v3.Matcher".into(),
                    value: Base64Bytes(buf),
                },
            }
        });

        let config = Self { custom_response_matcher };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn builds_proto_with_no_matcher() {
        let config = CustomResponseConfig::default();
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, CUSTOM_RESPONSE_TYPE_URL);
        // Empty CustomResponse proto is valid and may serialize to empty bytes
    }

    #[test]
    fn builds_proto_with_matcher() {
        let config = CustomResponseConfig {
            custom_response_matcher: Some(MatcherConfig {
                config: TypedConfig {
                    type_url: "type.googleapis.com/xds.type.matcher.v3.Matcher".into(),
                    value: Base64Bytes(vec![1, 2, 3, 4]),
                },
            }),
        };

        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, CUSTOM_RESPONSE_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn proto_round_trip_no_matcher() {
        let config = CustomResponseConfig::default();
        let any = config.to_any().expect("to_any");

        let proto = CustomResponseProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = CustomResponseConfig::from_proto(&proto).expect("from_proto");

        assert!(round_tripped.custom_response_matcher.is_none());
    }

    #[test]
    fn proto_round_trip_with_matcher() {
        let config = CustomResponseConfig {
            custom_response_matcher: Some(MatcherConfig {
                config: TypedConfig {
                    type_url: "type.googleapis.com/xds.type.matcher.v3.Matcher".into(),
                    value: Base64Bytes(vec![10, 20, 30]),
                },
            }),
        };

        let any = config.to_any().expect("to_any");
        let proto = CustomResponseProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = CustomResponseConfig::from_proto(&proto).expect("from_proto");

        assert!(round_tripped.custom_response_matcher.is_some());
        let matcher = round_tripped.custom_response_matcher.unwrap();
        assert_eq!(matcher.config.type_url, "type.googleapis.com/xds.type.matcher.v3.Matcher");
    }

    #[test]
    fn validates_successfully() {
        let config = CustomResponseConfig::default();
        assert!(config.validate().is_ok());
    }
}
