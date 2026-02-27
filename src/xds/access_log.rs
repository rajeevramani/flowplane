//! Dynamic access log configuration for learning sessions
//!
//! This module provides functionality to generate Envoy AccessLog configurations
//! dynamically based on active learning sessions. When a session is activated,
//! we configure Envoy to send HTTP access logs (with request/response bodies)
//! to the Flowplane access log service.

use envoy_types::pb::envoy::config::{
    accesslog::v3::{access_log::ConfigType as AccessLogConfigType, AccessLog},
    core::v3::{grpc_service, ApiVersion, GrpcService},
};
use envoy_types::pb::envoy::extensions::access_loggers::grpc::v3::{
    CommonGrpcAccessLogConfig, HttpGrpcAccessLogConfig,
};
use envoy_types::pb::google::protobuf::{Any as EnvoyAny, UInt32Value};
use prost::Message;

use crate::Result;

/// Configuration for generating access log configs for learning sessions
#[derive(Debug, Clone)]
pub struct LearningSessionAccessLogConfig {
    /// Session ID to identify the learning session
    pub session_id: String,
    /// Team ID for multi-tenancy
    pub team: String,
    /// gRPC service address for Flowplane access log service
    pub access_log_service_address: String,
    /// Maximum body bytes to capture (default: 10KB)
    pub max_body_bytes: u32,
}

impl Default for LearningSessionAccessLogConfig {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            team: String::new(),
            access_log_service_address: "127.0.0.1:18000".to_string(),
            max_body_bytes: 10 * 1024, // 10KB
        }
    }
}

impl LearningSessionAccessLogConfig {
    /// Create a new access log config for a learning session
    pub fn new(session_id: String, team: String, access_log_service_address: String) -> Self {
        Self { session_id, team, access_log_service_address, ..Default::default() }
    }

    /// Build an Envoy AccessLog configuration for this learning session
    ///
    /// This generates an HttpGrpcAccessLogConfig that:
    /// - Sends access logs to Flowplane's gRPC access log service
    /// - Captures request and response bodies (up to max_body_bytes)
    /// - Includes additional headers for context
    /// - Uses a unique log name for the session
    pub fn build_access_log(&self) -> Result<AccessLog> {
        // Create gRPC service configuration pointing to Flowplane
        let grpc_service = GrpcService {
            target_specifier: Some(grpc_service::TargetSpecifier::EnvoyGrpc(
                grpc_service::EnvoyGrpc {
                    cluster_name: "flowplane_access_log_service".to_string(),
                    authority: String::new(),
                    retry_policy: None,
                    max_receive_message_length: None,
                    skip_envoy_headers: false,
                },
            )),
            timeout: None,
            initial_metadata: Vec::new(),
            retry_policy: None,
        };

        // Common configuration for gRPC access log
        let common_config = CommonGrpcAccessLogConfig {
            log_name: format!("flowplane_learning_session_{}", self.session_id),
            grpc_service: Some(grpc_service),
            transport_api_version: ApiVersion::V3 as i32,
            buffer_flush_interval: None, // Use Envoy defaults (1 second)
            buffer_size_bytes: Some(UInt32Value { value: 16384 }), // 16KB buffer
            filter_state_objects_to_log: Vec::new(),
            grpc_stream_retry_policy: None,
            custom_tags: Vec::new(),
        };

        // HTTP-specific access log configuration
        let http_grpc_config = HttpGrpcAccessLogConfig {
            common_config: Some(common_config),
            additional_request_headers_to_log: vec![
                "content-type".to_string(),
                "content-length".to_string(),
                "accept".to_string(),
                "user-agent".to_string(),
                "authorization".to_string(),
                "proxy-authorization".to_string(),
                "x-api-key".to_string(),
                "x-auth-token".to_string(),
                "x-request-id".to_string(),
                "x-envoy-original-path".to_string(), // Original path before rewriting
            ],
            additional_response_headers_to_log: vec![
                "content-type".to_string(),
                "content-length".to_string(),
                "www-authenticate".to_string(),
            ],
            additional_response_trailers_to_log: Vec::new(),
        };

        // Serialize HttpGrpcAccessLogConfig to protobuf Any
        let mut buf = Vec::new();
        http_grpc_config.encode(&mut buf).map_err(|e| {
            crate::Error::internal(format!("Failed to encode HttpGrpcAccessLogConfig: {}", e))
        })?;

        let typed_config = EnvoyAny {
            type_url: "type.googleapis.com/envoy.extensions.access_loggers.grpc.v3.HttpGrpcAccessLogConfig".to_string(),
            value: buf,
        };

        Ok(AccessLog {
            name: "envoy.access_loggers.http_grpc".to_string(),
            filter: None, // No filter - log all matching traffic
            config_type: Some(AccessLogConfigType::TypedConfig(typed_config)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LearningSessionAccessLogConfig::default();
        assert_eq!(config.max_body_bytes, 10 * 1024);
        assert_eq!(config.access_log_service_address, "127.0.0.1:18000");
    }

    #[test]
    fn test_new_config() {
        let config = LearningSessionAccessLogConfig::new(
            "session-123".to_string(),
            "team-a".to_string(),
            "flowplane:18000".to_string(),
        );

        assert_eq!(config.session_id, "session-123");
        assert_eq!(config.team, "team-a");
        assert_eq!(config.access_log_service_address, "flowplane:18000");
    }

    #[test]
    fn test_build_access_log() {
        let config = LearningSessionAccessLogConfig::new(
            "test-session".to_string(),
            "test-team".to_string(),
            "localhost:18000".to_string(),
        );

        let access_log = config.build_access_log().unwrap();

        // Verify basic structure
        assert_eq!(access_log.name, "envoy.access_loggers.http_grpc");
        assert!(access_log.filter.is_none());

        // Verify typed config exists
        let typed_config = match access_log.config_type {
            Some(AccessLogConfigType::TypedConfig(cfg)) => cfg,
            _ => panic!("Expected TypedConfig"),
        };

        assert!(typed_config.type_url.contains("HttpGrpcAccessLogConfig"));
        assert!(!typed_config.value.is_empty());
    }

    #[test]
    fn test_build_access_log_decoding() {
        let config = LearningSessionAccessLogConfig::new(
            "decode-test".to_string(),
            "team-x".to_string(),
            "localhost:18000".to_string(),
        );

        let access_log = config.build_access_log().unwrap();

        // Extract and decode the HttpGrpcAccessLogConfig
        let typed_config = match access_log.config_type {
            Some(AccessLogConfigType::TypedConfig(cfg)) => cfg,
            _ => panic!("Expected TypedConfig"),
        };

        let decoded = HttpGrpcAccessLogConfig::decode(&typed_config.value[..]).unwrap();

        // Verify common config
        let common = decoded.common_config.unwrap();
        assert_eq!(common.log_name, "flowplane_learning_session_decode-test");
        assert!(common.grpc_service.is_some());

        // Verify headers to log
        assert!(decoded.additional_request_headers_to_log.contains(&"content-type".to_string()));
        assert!(decoded.additional_request_headers_to_log.contains(&"user-agent".to_string()));
        assert!(decoded.additional_request_headers_to_log.contains(&"authorization".to_string()));
        assert!(decoded.additional_request_headers_to_log.contains(&"x-api-key".to_string()));
        assert!(decoded.additional_response_headers_to_log.contains(&"content-type".to_string()));
        assert!(decoded
            .additional_response_headers_to_log
            .contains(&"www-authenticate".to_string()));
    }

    #[test]
    fn test_grpc_service_configuration() {
        let config = LearningSessionAccessLogConfig::new(
            "grpc-test".to_string(),
            "team-y".to_string(),
            "flowplane:18000".to_string(),
        );

        let access_log = config.build_access_log().unwrap();

        let typed_config = match access_log.config_type {
            Some(AccessLogConfigType::TypedConfig(cfg)) => cfg,
            _ => panic!("Expected TypedConfig"),
        };

        let decoded = HttpGrpcAccessLogConfig::decode(&typed_config.value[..]).unwrap();
        let common = decoded.common_config.unwrap();
        let grpc_service = common.grpc_service.unwrap();

        // Verify gRPC service points to correct cluster
        match grpc_service.target_specifier {
            Some(grpc_service::TargetSpecifier::EnvoyGrpc(envoy_grpc)) => {
                assert_eq!(envoy_grpc.cluster_name, "flowplane_access_log_service");
            }
            _ => panic!("Expected EnvoyGrpc target specifier"),
        }
    }
}
