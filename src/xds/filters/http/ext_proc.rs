//! External Processor HTTP filter configuration helpers
//!
//! This module provides configuration for Envoy's External Processor (ext_proc) filter,
//! which enables real-time request/response processing through external gRPC services.

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::core::v3::GrpcService;
use envoy_types::pb::envoy::extensions::filters::http::ext_proc::v3::{
    ExternalProcessor as ExternalProcessorProto, ProcessingMode as ProcessingModeProto,
};
use envoy_types::pb::google::protobuf::{Any as EnvoyAny, Duration as ProtoDuration};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const EXT_PROC_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.ext_proc.v3.ExternalProcessor";

/// Configuration for External Processor filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExtProcConfig {
    /// gRPC service configuration for the external processor
    pub grpc_service: GrpcServiceConfig,
    /// Whether to allow request processing to continue on failure
    #[serde(default)]
    pub failure_mode_allow: bool,
    /// Processing mode configuration
    #[serde(default)]
    pub processing_mode: Option<ProcessingMode>,
    /// Timeout for each individual message (milliseconds)
    #[serde(default)]
    pub message_timeout_ms: Option<u64>,
    /// Request attributes to send to the external processor
    #[serde(default)]
    pub request_attributes: Vec<String>,
    /// Response attributes to send to the external processor
    #[serde(default)]
    pub response_attributes: Vec<String>,
}

/// gRPC service configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GrpcServiceConfig {
    /// Target URI for the gRPC service (e.g., "ext-proc:9000")
    pub target_uri: String,
    /// Timeout in seconds for the gRPC connection
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

fn default_timeout() -> u32 {
    20
}

/// Processing mode configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProcessingMode {
    /// How to handle request headers (SEND, SKIP, DEFAULT)
    #[serde(default)]
    pub request_header_mode: Option<String>,
    /// How to handle response headers (SEND, SKIP, DEFAULT)
    #[serde(default)]
    pub response_header_mode: Option<String>,
    /// How to handle request body (NONE, STREAMED, BUFFERED, BUFFERED_PARTIAL, FULL_DUPLEX_STREAMED)
    #[serde(default)]
    pub request_body_mode: Option<String>,
    /// How to handle response body (NONE, STREAMED, BUFFERED, BUFFERED_PARTIAL, FULL_DUPLEX_STREAMED)
    #[serde(default)]
    pub response_body_mode: Option<String>,
    /// How to handle request trailers (SEND, SKIP, DEFAULT)
    #[serde(default)]
    pub request_trailer_mode: Option<String>,
    /// How to handle response trailers (SEND, SKIP, DEFAULT)
    #[serde(default)]
    pub response_trailer_mode: Option<String>,
}

impl ExtProcConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.grpc_service.target_uri.trim().is_empty() {
            return Err(invalid_config("ExtProc grpc_service.target_uri cannot be empty"));
        }

        if self.grpc_service.timeout_seconds == 0 {
            return Err(invalid_config("ExtProc grpc_service.timeout_seconds must be > 0"));
        }

        Ok(())
    }

    /// Convert to Envoy Any payload
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        // Build gRPC service configuration
        let grpc_service = GrpcService {
            target_specifier: Some(
                envoy_types::pb::envoy::config::core::v3::grpc_service::TargetSpecifier::EnvoyGrpc(
                    envoy_types::pb::envoy::config::core::v3::grpc_service::EnvoyGrpc {
                        cluster_name: self.grpc_service.target_uri.clone(),
                        authority: String::new(),
                        retry_policy: None,
                        max_receive_message_length: None,
                        skip_envoy_headers: false,
                    },
                ),
            ),
            timeout: Some(ProtoDuration {
                seconds: self.grpc_service.timeout_seconds as i64,
                nanos: 0,
            }),
            initial_metadata: Vec::new(),
            retry_policy: None,
        };

        // Build processing mode if provided
        let processing_mode = self.processing_mode.as_ref().map(|pm| ProcessingModeProto {
            request_header_mode: parse_header_send_mode(pm.request_header_mode.as_deref())
                .unwrap_or(1), // Default to SEND
            response_header_mode: parse_header_send_mode(pm.response_header_mode.as_deref())
                .unwrap_or(1), // Default to SEND
            request_body_mode: parse_body_send_mode(pm.request_body_mode.as_deref()).unwrap_or(0), // Default to NONE
            response_body_mode: parse_body_send_mode(pm.response_body_mode.as_deref()).unwrap_or(0), // Default to NONE
            request_trailer_mode: parse_header_send_mode(pm.request_trailer_mode.as_deref())
                .unwrap_or(2), // Default to SKIP
            response_trailer_mode: parse_header_send_mode(pm.response_trailer_mode.as_deref())
                .unwrap_or(2), // Default to SKIP
        });

        // Build message timeout if provided
        let message_timeout = self.message_timeout_ms.map(|ms| {
            let seconds = (ms / 1000) as i64;
            let nanos = ((ms % 1000) * 1_000_000) as i32;
            ProtoDuration { seconds, nanos }
        });

        let proto = ExternalProcessorProto {
            grpc_service: Some(grpc_service),
            http_service: None,
            failure_mode_allow: self.failure_mode_allow,
            processing_mode,
            request_attributes: self.request_attributes.clone(),
            response_attributes: self.response_attributes.clone(),
            message_timeout,
            stat_prefix: String::new(),
            mutation_rules: None,
            max_message_timeout: None,
            disable_clear_route_cache: false,
            forward_rules: None,
            filter_metadata: Default::default(),
            allow_mode_override: false,
            disable_immediate_response: false,
            metadata_options: None,
            observability_mode: false,
            route_cache_action: 0,
            deferred_close_timeout: None,
            send_body_without_waiting_for_header_response: false,
            allowed_override_modes: Vec::new(),
            on_processing_response: None,
            processing_request_modifier: None,
            status_on_error: None,
        };

        Ok(any_from_message(EXT_PROC_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &ExternalProcessorProto) -> Result<Self, crate::Error> {
        // Extract gRPC service configuration
        let grpc_service = proto
            .grpc_service
            .as_ref()
            .ok_or_else(|| invalid_config("ExtProc requires grpc_service configuration"))?;

        let target_uri = match &grpc_service.target_specifier {
            Some(
                envoy_types::pb::envoy::config::core::v3::grpc_service::TargetSpecifier::EnvoyGrpc(
                    envoy_grpc,
                ),
            ) => envoy_grpc.cluster_name.clone(),
            _ => return Err(invalid_config("ExtProc only supports EnvoyGrpc target specifier")),
        };

        let timeout_seconds =
            grpc_service.timeout.as_ref().map(|d| d.seconds as u32).unwrap_or(default_timeout());

        // Extract processing mode
        let processing_mode = proto.processing_mode.as_ref().map(|pm| ProcessingMode {
            request_header_mode: header_send_mode_to_string(pm.request_header_mode),
            response_header_mode: header_send_mode_to_string(pm.response_header_mode),
            request_body_mode: body_send_mode_to_string(pm.request_body_mode),
            response_body_mode: body_send_mode_to_string(pm.response_body_mode),
            request_trailer_mode: header_send_mode_to_string(pm.request_trailer_mode),
            response_trailer_mode: header_send_mode_to_string(pm.response_trailer_mode),
        });

        // Extract message timeout
        let message_timeout_ms = proto
            .message_timeout
            .as_ref()
            .map(|duration| (duration.seconds as u64) * 1000 + (duration.nanos as u64) / 1_000_000);

        let config = Self {
            grpc_service: GrpcServiceConfig { target_uri, timeout_seconds },
            failure_mode_allow: proto.failure_mode_allow,
            processing_mode,
            message_timeout_ms,
            request_attributes: proto.request_attributes.clone(),
            response_attributes: proto.response_attributes.clone(),
        };

        config.validate()?;
        Ok(config)
    }
}

// Helper functions to parse mode strings
fn parse_header_send_mode(mode: Option<&str>) -> Option<i32> {
    match mode? {
        "DEFAULT" => Some(0),
        "SEND" => Some(1),
        "SKIP" => Some(2),
        _ => None,
    }
}

fn parse_body_send_mode(mode: Option<&str>) -> Option<i32> {
    match mode? {
        "NONE" => Some(0),
        "STREAMED" => Some(1),
        "BUFFERED" => Some(2),
        "BUFFERED_PARTIAL" => Some(3),
        "FULL_DUPLEX_STREAMED" => Some(4),
        _ => None,
    }
}

fn header_send_mode_to_string(mode: i32) -> Option<String> {
    match mode {
        0 => Some("DEFAULT".to_string()),
        1 => Some("SEND".to_string()),
        2 => Some("SKIP".to_string()),
        _ => None,
    }
}

fn body_send_mode_to_string(mode: i32) -> Option<String> {
    match mode {
        0 => Some("NONE".to_string()),
        1 => Some("STREAMED".to_string()),
        2 => Some("BUFFERED".to_string()),
        3 => Some("BUFFERED_PARTIAL".to_string()),
        4 => Some("FULL_DUPLEX_STREAMED".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn sample_config() -> ExtProcConfig {
        ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target_uri: "ext-proc-service".to_string(),
                timeout_seconds: 10,
            },
            failure_mode_allow: true,
            processing_mode: Some(ProcessingMode {
                request_header_mode: Some("SEND".to_string()),
                response_header_mode: Some("SEND".to_string()),
                request_body_mode: Some("BUFFERED".to_string()),
                response_body_mode: Some("NONE".to_string()),
                request_trailer_mode: Some("SKIP".to_string()),
                response_trailer_mode: Some("SKIP".to_string()),
            }),
            message_timeout_ms: Some(5000),
            request_attributes: vec!["request.time".to_string()],
            response_attributes: vec!["response.code".to_string()],
        }
    }

    #[test]
    fn validates_target_uri() {
        let mut config = sample_config();
        config.grpc_service.target_uri = "".into();
        let err = config.validate().expect_err("empty target_uri should fail");
        assert!(format!("{err}").contains("target_uri"));
    }

    #[test]
    fn validates_timeout() {
        let mut config = sample_config();
        config.grpc_service.timeout_seconds = 0;
        let err = config.validate().expect_err("zero timeout should fail");
        assert!(format!("{err}").contains("timeout"));
    }

    #[test]
    fn builds_proto() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, EXT_PROC_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn proto_round_trip() {
        let config = sample_config();
        let any = config.to_any().expect("to_any");

        let proto = ExternalProcessorProto::decode(any.value.as_slice()).expect("decode proto");
        let round_tripped = ExtProcConfig::from_proto(&proto).expect("from_proto");

        assert_eq!(round_tripped.grpc_service.target_uri, "ext-proc-service");
        assert_eq!(round_tripped.grpc_service.timeout_seconds, 10);
        assert!(round_tripped.failure_mode_allow);
        assert_eq!(round_tripped.message_timeout_ms, Some(5000));
        assert_eq!(round_tripped.request_attributes, vec!["request.time"]);
        assert_eq!(round_tripped.response_attributes, vec!["response.code"]);
    }

    #[test]
    fn default_failure_mode() {
        let config = ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target_uri: "ext-proc".to_string(),
                timeout_seconds: 20,
            },
            failure_mode_allow: false,
            processing_mode: None,
            message_timeout_ms: None,
            request_attributes: Vec::new(),
            response_attributes: Vec::new(),
        };

        let any = config.to_any().expect("to_any");
        assert!(!any.value.is_empty());
    }

    #[test]
    fn processing_mode_defaults() {
        let config = ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target_uri: "ext-proc".to_string(),
                timeout_seconds: 20,
            },
            failure_mode_allow: false,
            processing_mode: Some(ProcessingMode {
                request_header_mode: None,
                response_header_mode: None,
                request_body_mode: None,
                response_body_mode: None,
                request_trailer_mode: None,
                response_trailer_mode: None,
            }),
            message_timeout_ms: None,
            request_attributes: Vec::new(),
            response_attributes: Vec::new(),
        };

        let any = config.to_any().expect("to_any");
        let proto = ExternalProcessorProto::decode(any.value.as_slice()).expect("decode proto");
        let mode = proto.processing_mode.expect("processing mode present");

        // Verify defaults
        assert_eq!(mode.request_header_mode, 1); // SEND
        assert_eq!(mode.response_header_mode, 1); // SEND
        assert_eq!(mode.request_body_mode, 0); // NONE
        assert_eq!(mode.response_body_mode, 0); // NONE
        assert_eq!(mode.request_trailer_mode, 2); // SKIP
        assert_eq!(mode.response_trailer_mode, 2); // SKIP
    }
}
