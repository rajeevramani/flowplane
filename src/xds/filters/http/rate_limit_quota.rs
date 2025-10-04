//! Rate Limit Quota HTTP filter configuration helpers
//!
//! This module provides configuration for Envoy's rate limit quota filter,
//! which integrates with a gRPC-based Rate Limit Quota Service (RLQS).

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::core::v3::{
    grpc_service::{EnvoyGrpc, TargetSpecifier},
    GrpcService,
};
use envoy_types::pb::envoy::extensions::filters::http::rate_limit_quota::v3::{
    RateLimitQuotaFilterConfig as RateLimitQuotaProto, RateLimitQuotaOverride,
};
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const RATE_LIMIT_QUOTA_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.rate_limit_quota.v3.RateLimitQuotaFilterConfig";
const RATE_LIMIT_QUOTA_OVERRIDE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.rate_limit_quota.v3.RateLimitQuotaOverride";

/// Configuration for rate limit quota filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RateLimitQuotaConfig {
    /// Application domain for quota service
    pub domain: String,
    /// gRPC service configuration for RLQS
    pub rlqs_server: RlqsServerConfig,
}

impl RateLimitQuotaConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.domain.trim().is_empty() {
            return Err(invalid_config("RateLimitQuota domain cannot be empty"));
        }

        self.rlqs_server.validate()?;

        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let proto = RateLimitQuotaProto {
            domain: self.domain.clone(),
            rlqs_server: Some(self.rlqs_server.to_proto()?),
            bucket_matchers: None, // Advanced feature - can be added later
            filter_enabled: None,
            filter_enforced: None,
            request_headers_to_add_when_not_enforced: Vec::new(),
        };

        Ok(any_from_message(RATE_LIMIT_QUOTA_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &RateLimitQuotaProto) -> Result<Self, crate::Error> {
        let rlqs_server = proto
            .rlqs_server
            .as_ref()
            .ok_or_else(|| invalid_config("RateLimitQuota requires rlqs_server"))?;

        let config = Self {
            domain: proto.domain.clone(),
            rlqs_server: RlqsServerConfig::from_proto(rlqs_server)?,
        };

        config.validate()?;
        Ok(config)
    }
}

/// RLQS server gRPC configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RlqsServerConfig {
    /// Name of the Envoy cluster for the RLQS service
    pub cluster_name: String,
    /// Authority header to send with gRPC requests
    #[serde(default)]
    pub authority: Option<String>,
}

impl RlqsServerConfig {
    fn validate(&self) -> Result<(), crate::Error> {
        if self.cluster_name.trim().is_empty() {
            return Err(invalid_config("RateLimitQuota server cluster_name cannot be empty"));
        }
        Ok(())
    }

    fn to_proto(&self) -> Result<GrpcService, crate::Error> {
        self.validate()?;

        Ok(GrpcService {
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
        })
    }

    fn from_proto(proto: &GrpcService) -> Result<Self, crate::Error> {
        let (cluster_name, authority) = match &proto.target_specifier {
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
                    "RateLimitQuota server requires envoy_grpc target specifier",
                ))
            }
        };

        Ok(Self { cluster_name, authority })
    }
}

/// Per-route rate limit quota override configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RateLimitQuotaOverrideConfig {
    /// Domain override for this route
    pub domain: String,
}

impl RateLimitQuotaOverrideConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.domain.trim().is_empty() {
            return Err(invalid_config("RateLimitQuotaOverride domain cannot be empty"));
        }
        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let proto = RateLimitQuotaOverride {
            domain: self.domain.clone(),
            bucket_matchers: None, // Advanced feature - can be added later
        };

        Ok(any_from_message(RATE_LIMIT_QUOTA_OVERRIDE_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &RateLimitQuotaOverride) -> Result<Self, crate::Error> {
        let config = Self { domain: proto.domain.clone() };

        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn sample_config() -> RateLimitQuotaConfig {
        RateLimitQuotaConfig {
            domain: "quota-domain".into(),
            rlqs_server: RlqsServerConfig {
                cluster_name: "rlqs_cluster".into(),
                authority: Some("rlqs.svc.cluster.local".into()),
            },
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
        config.rlqs_server.cluster_name = "".into();
        let err = config.validate().expect_err("empty cluster should fail");
        assert!(format!("{err}").contains("cluster_name"));
    }

    #[test]
    fn builds_proto() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, RATE_LIMIT_QUOTA_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn proto_round_trip() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");

        let proto = RateLimitQuotaProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = RateLimitQuotaConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(round_tripped.domain, "quota-domain");
        assert_eq!(round_tripped.rlqs_server.cluster_name, "rlqs_cluster");
        assert_eq!(round_tripped.rlqs_server.authority, Some("rlqs.svc.cluster.local".into()));
    }

    #[test]
    fn handles_missing_authority() {
        let config = RateLimitQuotaConfig {
            domain: "test".into(),
            rlqs_server: RlqsServerConfig { cluster_name: "cluster".into(), authority: None },
        };

        let any = config.to_any().expect("to_any");
        assert!(!any.value.is_empty());
    }

    #[test]
    fn override_builds_proto() {
        let config = RateLimitQuotaOverrideConfig { domain: "override-domain".into() };

        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, RATE_LIMIT_QUOTA_OVERRIDE_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn override_proto_round_trip() {
        let config = RateLimitQuotaOverrideConfig { domain: "my-override-domain".into() };

        let any = config.to_any().expect("to_any");
        let proto = RateLimitQuotaOverride::decode(any.value.as_slice()).expect("decode");
        let round_tripped = RateLimitQuotaOverrideConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(round_tripped.domain, "my-override-domain");
    }

    #[test]
    fn override_validates_empty_domain() {
        let config = RateLimitQuotaOverrideConfig { domain: "".into() };

        let err = config.validate().expect_err("empty domain should fail");
        assert!(format!("{err}").contains("cannot be empty"));
    }
}
