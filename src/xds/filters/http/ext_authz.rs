//! External Authorization (ext_authz) HTTP filter configuration
//!
//! This module provides configuration types for the Envoy ext_authz filter,
//! which delegates authorization decisions to external gRPC or HTTP services.

use crate::xds::filters::{any_from_message, invalid_config};
use envoy_types::pb::envoy::config::core::v3::{
    config_source::ConfigSourceSpecifier, AggregatedConfigSource, ConfigSource,
    GrpcService as GrpcServiceProto, HeaderValue, HttpUri,
};
use envoy_types::pb::envoy::extensions::filters::http::ext_authz::v3::{
    ext_authz::Services, AuthorizationRequest, AuthorizationResponse, BufferSettings,
    ExtAuthz as ExtAuthzProto, ExtAuthzPerRoute as ExtAuthzPerRouteProto, HttpService,
};
use envoy_types::pb::envoy::extensions::transport_sockets::tls::v3::SdsSecretConfig;
use envoy_types::pb::envoy::r#type::v3::HttpStatus;
use envoy_types::pb::google::protobuf::{Any as EnvoyAny, Duration};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Build an SDS secret config that uses ADS for secret discovery
fn build_sds_secret_config(name: &str) -> SdsSecretConfig {
    SdsSecretConfig {
        name: name.to_string(),
        sds_config: Some(ConfigSource {
            config_source_specifier: Some(ConfigSourceSpecifier::Ads(
                AggregatedConfigSource::default(),
            )),
            ..Default::default()
        }),
    }
}

/// Type URLs for ext_authz filter configuration
pub const EXT_AUTHZ_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.ext_authz.v3.ExtAuthz";
pub const EXT_AUTHZ_PER_ROUTE_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.filters.http.ext_authz.v3.ExtAuthzPerRoute";

/// gRPC service configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GrpcServiceConfig {
    /// Target URI for the gRPC service (e.g., "dns:///authz.example.com:50051")
    pub target_uri: String,
    /// Timeout for gRPC calls in milliseconds (default: 200ms)
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Initial metadata to send with gRPC requests
    #[serde(default)]
    pub initial_metadata: Vec<HeaderEntry>,
}

fn default_timeout_ms() -> u64 {
    200
}

/// HTTP service configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HttpServiceConfig {
    /// URI for the HTTP authorization service
    pub server_uri: HttpUriConfig,
    /// Path prefix for authorization requests
    #[serde(default)]
    pub path_prefix: String,
    /// Headers to add to authorization requests
    #[serde(default)]
    pub headers_to_add: Vec<HeaderEntry>,
    /// Authorization request configuration
    #[serde(default)]
    pub authorization_request: Option<AuthorizationRequestConfig>,
    /// Authorization response configuration
    #[serde(default)]
    pub authorization_response: Option<AuthorizationResponseConfig>,
}

/// HTTP URI configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HttpUriConfig {
    /// The URI to use
    pub uri: String,
    /// The cluster to use for the upstream
    pub cluster: String,
    /// Timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

/// Header entry configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HeaderEntry {
    /// Header key
    pub key: String,
    /// Header value
    pub value: String,
}

/// Authorization request configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct AuthorizationRequestConfig {
    /// Headers to include from the original request
    #[serde(default)]
    pub allowed_headers: Vec<String>,
    /// Headers to add to the authorization request
    #[serde(default)]
    pub headers_to_add: Vec<HeaderEntry>,
}

/// Authorization response configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct AuthorizationResponseConfig {
    /// Headers to add to the upstream request on success
    #[serde(default)]
    pub allowed_upstream_headers: Vec<String>,
    /// Headers from authz response to add to the client response on success
    #[serde(default)]
    pub allowed_client_headers: Vec<String>,
    /// Headers from authz response to add to the client response on denial
    #[serde(default)]
    pub allowed_client_headers_on_success: Vec<String>,
}

/// Service type for ext_authz (gRPC or HTTP)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtAuthzService {
    /// gRPC service
    Grpc(GrpcServiceConfig),
    /// HTTP service
    Http(HttpServiceConfig),
}

/// Request body handling configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct WithRequestBodyConfig {
    /// Max request body bytes to buffer
    #[serde(default)]
    pub max_request_bytes: Option<u32>,
    /// Whether to allow partial message
    #[serde(default)]
    pub allow_partial_message: bool,
    /// Pack as bytes
    #[serde(default)]
    pub pack_as_bytes: bool,
}

/// Secret reference for API key/token authentication with the ext_authz service.
/// The secret is delivered via SDS (Secret Discovery Service).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuthSecretConfig {
    /// Name of the SDS secret containing the auth credential (API key, bearer token, etc.)
    pub name: String,
}

/// Secret reference for mTLS certificate to the ext_authz service.
/// The certificate is delivered via SDS (Secret Discovery Service).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TlsSecretConfig {
    /// Name of the SDS secret containing the TLS certificate and private key
    pub name: String,
}

/// External authorization filter configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExtAuthzConfig {
    /// The authorization service to use (gRPC or HTTP)
    pub service: ExtAuthzService,
    /// Whether to allow the request if the authz service fails
    #[serde(default)]
    pub failure_mode_allow: bool,
    /// Buffer settings for request body
    #[serde(default)]
    pub with_request_body: Option<WithRequestBodyConfig>,
    /// Clear route cache on successful authz response
    #[serde(default)]
    pub clear_route_cache: bool,
    /// HTTP status code to return on error
    #[serde(default)]
    pub status_on_error: Option<u32>,
    /// Stat prefix for metrics
    #[serde(default)]
    pub stat_prefix: Option<String>,
    /// Include peer certificate in authz request
    #[serde(default)]
    pub include_peer_certificate: bool,
    /// Optional SDS secret for API key/token authentication with the auth service.
    /// When set, the secret is fetched via SDS and can be used by the credential
    /// injector or transport socket configuration to authenticate with the authz service.
    #[serde(default)]
    pub auth_secret: Option<AuthSecretConfig>,
    /// Optional SDS secret for mTLS certificate to the auth service.
    /// When set, the TLS certificate is fetched via SDS and used to establish
    /// mTLS connections to the ext_authz service's cluster.
    #[serde(default)]
    pub tls_secret: Option<TlsSecretConfig>,
}

impl ExtAuthzConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), crate::Error> {
        match &self.service {
            ExtAuthzService::Grpc(grpc) => {
                if grpc.target_uri.is_empty() {
                    return Err(invalid_config("ext_authz gRPC target_uri is required"));
                }
            }
            ExtAuthzService::Http(http) => {
                if http.server_uri.uri.is_empty() {
                    return Err(invalid_config("ext_authz HTTP server_uri.uri is required"));
                }
                if http.server_uri.cluster.is_empty() {
                    return Err(invalid_config("ext_authz HTTP server_uri.cluster is required"));
                }
            }
        }

        if let Some(auth) = &self.auth_secret {
            if auth.name.is_empty() {
                return Err(invalid_config("ext_authz auth_secret.name cannot be empty"));
            }
        }
        if let Some(tls) = &self.tls_secret {
            if tls.name.is_empty() {
                return Err(invalid_config("ext_authz tls_secret.name cannot be empty"));
            }
        }

        Ok(())
    }

    /// Returns the SDS secret config for the auth credential, if configured.
    /// The cluster builder or credential injector uses this to fetch the
    /// API key/token from the secrets system via ADS.
    pub fn auth_sds_config(&self) -> Option<SdsSecretConfig> {
        self.auth_secret.as_ref().map(|s| build_sds_secret_config(&s.name))
    }

    /// Returns the SDS secret config for the mTLS certificate, if configured.
    /// The cluster builder uses this to configure the transport socket with
    /// SDS-delivered TLS certificates for connecting to the ext_authz service.
    pub fn tls_sds_config(&self) -> Option<SdsSecretConfig> {
        self.tls_secret.as_ref().map(|s| build_sds_secret_config(&s.name))
    }

    /// Convert to Envoy Any protobuf
    #[allow(deprecated)] // allowed_headers is deprecated in envoy_types but still functional
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        self.validate()?;

        let services = match &self.service {
            ExtAuthzService::Grpc(grpc) => {
                let timeout = Duration {
                    seconds: (grpc.timeout_ms / 1000) as i64,
                    nanos: ((grpc.timeout_ms % 1000) * 1_000_000) as i32,
                };

                let initial_metadata: Vec<HeaderValue> = grpc
                    .initial_metadata
                    .iter()
                    .map(|h| HeaderValue {
                        key: h.key.clone(),
                        value: h.value.clone(),
                        raw_value: Vec::new(),
                    })
                    .collect();

                Services::GrpcService(GrpcServiceProto {
                    target_specifier: Some(
                        envoy_types::pb::envoy::config::core::v3::grpc_service::TargetSpecifier::EnvoyGrpc(
                            envoy_types::pb::envoy::config::core::v3::grpc_service::EnvoyGrpc {
                                cluster_name: grpc.target_uri.clone(),
                                authority: String::new(),
                                retry_policy: None,
                                max_receive_message_length: None,
                                skip_envoy_headers: false,
                            },
                        ),
                    ),
                    timeout: Some(timeout),
                    initial_metadata,
                    retry_policy: None,
                })
            }
            ExtAuthzService::Http(http) => {
                let timeout = Duration {
                    seconds: (http.server_uri.timeout_ms / 1000) as i64,
                    nanos: ((http.server_uri.timeout_ms % 1000) * 1_000_000) as i32,
                };

                let authorization_request = http.authorization_request.as_ref().map(|req| {
                    AuthorizationRequest {
                        allowed_headers: Some(
                            envoy_types::pb::envoy::r#type::matcher::v3::ListStringMatcher {
                                patterns: req
                                    .allowed_headers
                                    .iter()
                                    .map(|h| {
                                        envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
                                            match_pattern: Some(
                                                envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                                    h.clone(),
                                                ),
                                            ),
                                            ignore_case: false,
                                        }
                                    })
                                    .collect(),
                            },
                        ),
                        headers_to_add: req
                            .headers_to_add
                            .iter()
                            .map(|h| HeaderValue {
                                key: h.key.clone(),
                                value: h.value.clone(),
                                raw_value: Vec::new(),
                            })
                            .collect(),
                    }
                });

                let authorization_response = http.authorization_response.as_ref().map(|resp| {
                    let to_list_matcher = |headers: &[String]| -> Option<envoy_types::pb::envoy::r#type::matcher::v3::ListStringMatcher> {
                        if headers.is_empty() {
                            None
                        } else {
                            Some(envoy_types::pb::envoy::r#type::matcher::v3::ListStringMatcher {
                                patterns: headers
                                    .iter()
                                    .map(|h| {
                                        envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
                                            match_pattern: Some(
                                                envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                                    h.clone(),
                                                ),
                                            ),
                                            ignore_case: false,
                                        }
                                    })
                                    .collect(),
                            })
                        }
                    };
                    AuthorizationResponse {
                        allowed_upstream_headers: to_list_matcher(&resp.allowed_upstream_headers),
                        allowed_client_headers: to_list_matcher(&resp.allowed_client_headers),
                        ..Default::default()
                    }
                });

                Services::HttpService(HttpService {
                    server_uri: Some(HttpUri {
                        uri: http.server_uri.uri.clone(),
                        http_upstream_type: Some(
                            envoy_types::pb::envoy::config::core::v3::http_uri::HttpUpstreamType::Cluster(
                                http.server_uri.cluster.clone(),
                            ),
                        ),
                        timeout: Some(timeout),
                    }),
                    path_prefix: http.path_prefix.clone(),
                    authorization_request,
                    authorization_response,
                    retry_policy: None,
                })
            }
        };

        let with_request_body = self.with_request_body.as_ref().map(|body| BufferSettings {
            max_request_bytes: body.max_request_bytes.unwrap_or(0),
            allow_partial_message: body.allow_partial_message,
            pack_as_bytes: body.pack_as_bytes,
        });

        let status_on_error = self.status_on_error.map(|code| HttpStatus { code: code as i32 });

        let proto = ExtAuthzProto {
            services: Some(services),
            failure_mode_allow: self.failure_mode_allow,
            with_request_body,
            clear_route_cache: self.clear_route_cache,
            status_on_error,
            stat_prefix: self.stat_prefix.clone().unwrap_or_default(),
            include_peer_certificate: self.include_peer_certificate,
            ..Default::default()
        };

        Ok(any_from_message(EXT_AUTHZ_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &ExtAuthzProto) -> Result<Self, crate::Error> {
        let service = match &proto.services {
            Some(Services::GrpcService(grpc)) => {
                let timeout_ms = grpc
                    .timeout
                    .as_ref()
                    .map(|d| (d.seconds as u64 * 1000) + (d.nanos as u64 / 1_000_000))
                    .unwrap_or(200);

                let target_uri = match &grpc.target_specifier {
                    Some(
                        envoy_types::pb::envoy::config::core::v3::grpc_service::TargetSpecifier::EnvoyGrpc(
                            envoy_grpc,
                        ),
                    ) => envoy_grpc.cluster_name.clone(),
                    _ => String::new(),
                };

                let initial_metadata = grpc
                    .initial_metadata
                    .iter()
                    .map(|h| HeaderEntry { key: h.key.clone(), value: h.value.clone() })
                    .collect();

                ExtAuthzService::Grpc(GrpcServiceConfig {
                    target_uri,
                    timeout_ms,
                    initial_metadata,
                })
            }
            Some(Services::HttpService(http)) => {
                let server_uri = http
                    .server_uri
                    .as_ref()
                    .map(|uri| {
                        let cluster = match &uri.http_upstream_type {
                            Some(
                                envoy_types::pb::envoy::config::core::v3::http_uri::HttpUpstreamType::Cluster(
                                    c,
                                ),
                            ) => c.clone(),
                            _ => String::new(),
                        };
                        let timeout_ms = uri
                            .timeout
                            .as_ref()
                            .map(|d| (d.seconds as u64 * 1000) + (d.nanos as u64 / 1_000_000))
                            .unwrap_or(200);
                        HttpUriConfig {
                            uri: uri.uri.clone(),
                            cluster,
                            timeout_ms,
                        }
                    })
                    .unwrap_or(HttpUriConfig {
                        uri: String::new(),
                        cluster: String::new(),
                        timeout_ms: 200,
                    });

                ExtAuthzService::Http(HttpServiceConfig {
                    server_uri,
                    path_prefix: http.path_prefix.clone(),
                    headers_to_add: vec![],
                    authorization_request: None,
                    authorization_response: None,
                })
            }
            None => {
                return Err(invalid_config(
                    "ext_authz requires either grpc_service or http_service",
                ))
            }
        };

        let with_request_body =
            proto.with_request_body.as_ref().map(|body| WithRequestBodyConfig {
                max_request_bytes: if body.max_request_bytes > 0 {
                    Some(body.max_request_bytes)
                } else {
                    None
                },
                allow_partial_message: body.allow_partial_message,
                pack_as_bytes: body.pack_as_bytes,
            });

        let status_on_error = proto.status_on_error.as_ref().map(|s| s.code as u32);

        Ok(Self {
            service,
            failure_mode_allow: proto.failure_mode_allow,
            with_request_body,
            clear_route_cache: proto.clear_route_cache,
            status_on_error,
            stat_prefix: if proto.stat_prefix.is_empty() {
                None
            } else {
                Some(proto.stat_prefix.clone())
            },
            include_peer_certificate: proto.include_peer_certificate,
            // SDS secret references are not stored in the ext_authz proto;
            // they are applied at the cluster/transport-socket level.
            auth_secret: None,
            tls_secret: None,
        })
    }
}

/// Per-route ext_authz configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum ExtAuthzPerRouteConfig {
    /// Disable ext_authz for this route
    Disabled { disabled: bool },
    /// Check settings override
    CheckSettings {
        /// Context extensions to pass to authz service
        #[serde(default)]
        context_extensions: std::collections::HashMap<String, String>,
        /// Disable request body buffering
        #[serde(default)]
        disable_request_body_buffering: bool,
    },
}

impl Default for ExtAuthzPerRouteConfig {
    fn default() -> Self {
        Self::Disabled { disabled: false }
    }
}

impl ExtAuthzPerRouteConfig {
    /// Convert to Envoy Any protobuf
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        use envoy_types::pb::envoy::extensions::filters::http::ext_authz::v3::ext_authz_per_route::Override;
        use envoy_types::pb::envoy::extensions::filters::http::ext_authz::v3::CheckSettings;

        let override_val = match self {
            Self::Disabled { disabled } => {
                if *disabled {
                    Override::Disabled(true)
                } else {
                    return Ok(any_from_message(
                        EXT_AUTHZ_PER_ROUTE_TYPE_URL,
                        &ExtAuthzPerRouteProto::default(),
                    ));
                }
            }
            Self::CheckSettings { context_extensions, disable_request_body_buffering } => {
                Override::CheckSettings(CheckSettings {
                    context_extensions: context_extensions.clone(),
                    disable_request_body_buffering: *disable_request_body_buffering,
                    with_request_body: None,
                    service_override: None,
                })
            }
        };

        let proto = ExtAuthzPerRouteProto { r#override: Some(override_val) };

        Ok(any_from_message(EXT_AUTHZ_PER_ROUTE_TYPE_URL, &proto))
    }

    /// Build configuration from Envoy proto
    pub fn from_proto(proto: &ExtAuthzPerRouteProto) -> Result<Self, crate::Error> {
        use envoy_types::pb::envoy::extensions::filters::http::ext_authz::v3::ext_authz_per_route::Override;

        match &proto.r#override {
            Some(Override::Disabled(v)) => Ok(Self::Disabled { disabled: *v }),
            Some(Override::CheckSettings(settings)) => Ok(Self::CheckSettings {
                context_extensions: settings.context_extensions.clone(),
                disable_request_body_buffering: settings.disable_request_body_buffering,
            }),
            None => Ok(Self::Disabled { disabled: false }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn test_grpc_service_config_validation() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Grpc(GrpcServiceConfig {
                target_uri: String::new(), // Invalid: empty
                timeout_ms: 200,
                initial_metadata: vec![],
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: None,
            tls_secret: None,
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_valid_grpc_config() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Grpc(GrpcServiceConfig {
                target_uri: "authz-cluster".to_string(),
                timeout_ms: 500,
                initial_metadata: vec![HeaderEntry {
                    key: "x-custom".to_string(),
                    value: "value".to_string(),
                }],
            }),
            failure_mode_allow: true,
            with_request_body: Some(WithRequestBodyConfig {
                max_request_bytes: Some(1024),
                allow_partial_message: true,
                pack_as_bytes: false,
            }),
            clear_route_cache: true,
            status_on_error: Some(503),
            stat_prefix: Some("ext_authz".to_string()),
            include_peer_certificate: true,
            auth_secret: None,
            tls_secret: None,
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed");
        assert_eq!(any.type_url, EXT_AUTHZ_TYPE_URL);
    }

    #[test]
    fn test_valid_http_config() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Http(HttpServiceConfig {
                server_uri: HttpUriConfig {
                    uri: "http://authz.example.com/check".to_string(),
                    cluster: "authz-http-cluster".to_string(),
                    timeout_ms: 300,
                },
                path_prefix: "/authz".to_string(),
                headers_to_add: vec![HeaderEntry {
                    key: "x-api-key".to_string(),
                    value: "secret".to_string(),
                }],
                authorization_request: Some(AuthorizationRequestConfig {
                    allowed_headers: vec!["authorization".to_string()],
                    headers_to_add: vec![],
                }),
                authorization_response: None,
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: None,
            tls_secret: None,
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed");
        assert_eq!(any.type_url, EXT_AUTHZ_TYPE_URL);
    }

    #[test]
    fn test_per_route_disabled() {
        let config = ExtAuthzPerRouteConfig::Disabled { disabled: true };
        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, EXT_AUTHZ_PER_ROUTE_TYPE_URL);
    }

    #[test]
    fn test_per_route_check_settings() {
        let mut context_extensions = std::collections::HashMap::new();
        context_extensions.insert("custom-key".to_string(), "custom-value".to_string());

        let config = ExtAuthzPerRouteConfig::CheckSettings {
            context_extensions,
            disable_request_body_buffering: true,
        };

        let any = config.to_any().expect("to_any");
        assert_eq!(any.type_url, EXT_AUTHZ_PER_ROUTE_TYPE_URL);
    }

    #[test]
    fn test_config_with_auth_secret() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Grpc(GrpcServiceConfig {
                target_uri: "authz-cluster".to_string(),
                timeout_ms: 200,
                initial_metadata: vec![],
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: Some(AuthSecretConfig { name: "ext-authz-api-key".to_string() }),
            tls_secret: None,
        };

        assert!(config.validate().is_ok());
        let any = config.to_any().expect("to_any should succeed");
        assert_eq!(any.type_url, EXT_AUTHZ_TYPE_URL);

        // Verify SDS config is produced
        let sds = config.auth_sds_config().expect("auth_sds_config should be Some");
        assert_eq!(sds.name, "ext-authz-api-key");
        assert!(sds.sds_config.is_some());
        assert!(config.tls_sds_config().is_none());
    }

    #[test]
    fn test_config_with_tls_secret() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Http(HttpServiceConfig {
                server_uri: HttpUriConfig {
                    uri: "https://authz.example.com/check".to_string(),
                    cluster: "authz-cluster".to_string(),
                    timeout_ms: 300,
                },
                path_prefix: String::new(),
                headers_to_add: vec![],
                authorization_request: None,
                authorization_response: None,
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: None,
            tls_secret: Some(TlsSecretConfig { name: "ext-authz-mtls-cert".to_string() }),
        };

        assert!(config.validate().is_ok());

        let sds = config.tls_sds_config().expect("tls_sds_config should be Some");
        assert_eq!(sds.name, "ext-authz-mtls-cert");
        assert!(sds.sds_config.is_some());
        assert!(config.auth_sds_config().is_none());
    }

    #[test]
    fn test_config_with_both_secrets() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Grpc(GrpcServiceConfig {
                target_uri: "authz-cluster".to_string(),
                timeout_ms: 200,
                initial_metadata: vec![],
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: Some(AuthSecretConfig { name: "authz-token".to_string() }),
            tls_secret: Some(TlsSecretConfig { name: "authz-mtls".to_string() }),
        };

        assert!(config.validate().is_ok());

        let auth_sds = config.auth_sds_config().expect("auth SDS");
        assert_eq!(auth_sds.name, "authz-token");

        let tls_sds = config.tls_sds_config().expect("tls SDS");
        assert_eq!(tls_sds.name, "authz-mtls");
    }

    #[test]
    fn test_empty_auth_secret_name_fails_validation() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Grpc(GrpcServiceConfig {
                target_uri: "authz-cluster".to_string(),
                timeout_ms: 200,
                initial_metadata: vec![],
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: Some(AuthSecretConfig { name: String::new() }),
            tls_secret: None,
        };

        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("auth_secret.name"));
    }

    #[test]
    fn test_empty_tls_secret_name_fails_validation() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Grpc(GrpcServiceConfig {
                target_uri: "authz-cluster".to_string(),
                timeout_ms: 200,
                initial_metadata: vec![],
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: None,
            tls_secret: Some(TlsSecretConfig { name: String::new() }),
        };

        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("tls_secret.name"));
    }

    #[test]
    fn test_no_secrets_returns_none_sds_configs() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Grpc(GrpcServiceConfig {
                target_uri: "authz-cluster".to_string(),
                timeout_ms: 200,
                initial_metadata: vec![],
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: None,
            tls_secret: None,
        };

        assert!(config.auth_sds_config().is_none());
        assert!(config.tls_sds_config().is_none());
    }

    #[test]
    fn test_sds_config_uses_ads() {
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Grpc(GrpcServiceConfig {
                target_uri: "authz-cluster".to_string(),
                timeout_ms: 200,
                initial_metadata: vec![],
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: Some(AuthSecretConfig { name: "my-secret".to_string() }),
            tls_secret: None,
        };

        let sds = config.auth_sds_config().expect("should have SDS config");
        let source = sds.sds_config.expect("should have config source");
        assert!(matches!(source.config_source_specifier, Some(ConfigSourceSpecifier::Ads(_))));
    }

    #[test]
    fn test_from_proto_sets_secrets_to_none() {
        // SDS secret references are not stored in the ext_authz proto,
        // so from_proto should always return None for both
        let config = ExtAuthzConfig {
            service: ExtAuthzService::Grpc(GrpcServiceConfig {
                target_uri: "authz-cluster".to_string(),
                timeout_ms: 200,
                initial_metadata: vec![],
            }),
            failure_mode_allow: false,
            with_request_body: None,
            clear_route_cache: false,
            status_on_error: None,
            stat_prefix: None,
            include_peer_certificate: false,
            auth_secret: Some(AuthSecretConfig { name: "will-be-lost".to_string() }),
            tls_secret: Some(TlsSecretConfig { name: "also-lost".to_string() }),
        };

        // Round-trip through proto loses SDS refs (expected)
        let any = config.to_any().expect("to_any");
        let proto = ExtAuthzProto::decode(any.value.as_slice()).expect("decode");
        let restored = ExtAuthzConfig::from_proto(&proto).expect("from_proto");
        assert!(restored.auth_secret.is_none());
        assert!(restored.tls_secret.is_none());
    }

    #[test]
    fn test_secrets_deserialize_from_json() {
        let json = r#"{
            "service": {
                "type": "grpc",
                "target_uri": "authz-cluster",
                "timeout_ms": 200
            },
            "auth_secret": { "name": "my-api-key" },
            "tls_secret": { "name": "my-mtls-cert" }
        }"#;

        let config: ExtAuthzConfig = serde_json::from_str(json).expect("deserialize");
        assert_eq!(config.auth_secret.as_ref().map(|s| s.name.as_str()), Some("my-api-key"));
        assert_eq!(config.tls_secret.as_ref().map(|s| s.name.as_str()), Some("my-mtls-cert"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_secrets_optional_in_json() {
        let json = r#"{
            "service": {
                "type": "grpc",
                "target_uri": "authz-cluster"
            }
        }"#;

        let config: ExtAuthzConfig = serde_json::from_str(json).expect("deserialize");
        assert!(config.auth_secret.is_none());
        assert!(config.tls_secret.is_none());
        assert!(config.validate().is_ok());
    }
}
