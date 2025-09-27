//! Listener management using envoy-types
//!
//! This module provides functionality for creating and managing Envoy listener configurations
//! using the proper envoy-types protobuf definitions.

use envoy_types::pb::envoy::config::trace::v3::tracing::{self, Http as HttpTracing};
use envoy_types::pb::envoy::config::{
    accesslog::v3::{access_log::ConfigType as AccessLogConfigType, AccessLog},
    core::v3::{
        address::Address as AddressType, transport_socket::ConfigType as TransportSocketConfigType,
        Address, DataSource, SocketAddress, SubstitutionFormatString, TransportSocket,
    },
    listener::v3::{Filter, FilterChain, Listener},
};
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::{
    http_connection_manager::{self, RouteSpecifier},
    HttpConnectionManager,
};
use envoy_types::pb::envoy::extensions::filters::network::tcp_proxy::v3::TcpProxy;
use envoy_types::pb::envoy::extensions::transport_sockets::tls::v3::{
    common_tls_context, CertificateValidationContext, CommonTlsContext, DownstreamTlsContext,
    TlsCertificate,
};
use envoy_types::pb::google::protobuf::{
    Any as EnvoyAny, BoolValue, Struct as ProstStruct, Value as ProstValue,
};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::xds::filters::http::{build_http_filters, HttpFilterConfigEntry};

/// REST API representation of a listener configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerConfig {
    pub name: String,
    pub address: String,
    pub port: u32,
    pub filter_chains: Vec<FilterChainConfig>,
}

/// REST API representation of a filter chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterChainConfig {
    pub name: Option<String>,
    pub filters: Vec<FilterConfig>,
    pub tls_context: Option<TlsContextConfig>,
}

/// REST API representation of a filter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    pub name: String,
    pub filter_type: FilterType,
}

/// REST API representation of filter types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterType {
    HttpConnectionManager {
        route_config_name: Option<String>,
        inline_route_config: Option<crate::xds::route::RouteConfig>,
        access_log: Option<AccessLogConfig>,
        tracing: Option<TracingConfig>,
        #[serde(default)]
        http_filters: Vec<HttpFilterConfigEntry>,
    },
    TcpProxy {
        cluster: String,
        access_log: Option<AccessLogConfig>,
    },
}

/// REST API representation of TLS context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsContextConfig {
    pub cert_chain_file: Option<String>,
    pub private_key_file: Option<String>,
    pub ca_cert_file: Option<String>,
    pub require_client_certificate: Option<bool>,
}

/// REST API representation of access log configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLogConfig {
    pub path: Option<String>,
    pub format: Option<String>,
}

/// REST API representation of tracing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    pub provider: String,
    pub config: HashMap<String, String>,
}

impl ListenerConfig {
    /// Convert REST API ListenerConfig to envoy-types Listener
    pub fn to_envoy_listener(&self) -> Result<Listener, crate::Error> {
        let socket_address = SocketAddress {
            address: self.address.clone(),
            port_specifier: Some(
                envoy_types::pb::envoy::config::core::v3::socket_address::PortSpecifier::PortValue(
                    self.port,
                ),
            ),
            ..Default::default()
        };

        let address = Address { address: Some(AddressType::SocketAddress(socket_address)) };

        let filter_chains: Result<Vec<FilterChain>, crate::Error> =
            self.filter_chains.iter().map(|fc| fc.to_envoy_filter_chain()).collect();

        let listener = Listener {
            name: self.name.clone(),
            address: Some(address),
            filter_chains: filter_chains?,
            ..Default::default()
        };

        Ok(listener)
    }
}

impl FilterChainConfig {
    /// Convert REST API FilterChainConfig to envoy-types FilterChain
    fn to_envoy_filter_chain(&self) -> Result<FilterChain, crate::Error> {
        let filters: Result<Vec<Filter>, crate::Error> =
            self.filters.iter().map(|f| f.to_envoy_filter()).collect();

        let filter_chain = FilterChain {
            filters: filters?,
            transport_socket: match &self.tls_context {
                Some(cfg) => Some(build_transport_socket(cfg)?),
                None => None,
            },
            ..Default::default()
        };

        Ok(filter_chain)
    }
}

impl FilterConfig {
    /// Convert REST API FilterConfig to envoy-types Filter
    fn to_envoy_filter(&self) -> Result<Filter, crate::Error> {
        let typed_config = match &self.filter_type {
            FilterType::HttpConnectionManager {
                route_config_name,
                inline_route_config,
                access_log,
                tracing,
                http_filters,
            } => {
                let route_specifier = if let Some(route_name) = route_config_name {
                    RouteSpecifier::Rds(envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::Rds {
                        route_config_name: route_name.clone(),
                        config_source: Some(envoy_types::pb::envoy::config::core::v3::ConfigSource {
                            config_source_specifier: Some(
                                envoy_types::pb::envoy::config::core::v3::config_source::ConfigSourceSpecifier::Ads(
                                    envoy_types::pb::envoy::config::core::v3::AggregatedConfigSource::default()
                                )
                            ),
                            ..Default::default()
                        })
                    })
                } else if let Some(inline_config) = inline_route_config {
                    RouteSpecifier::RouteConfig(inline_config.to_envoy_route_configuration()?)
                } else {
                    return Err(crate::Error::Config("HttpConnectionManager requires either route_config_name or inline_route_config".to_string()));
                };

                let http_filters = build_http_filters(http_filters.as_slice())?;

                let hcm = HttpConnectionManager {
                    route_specifier: Some(route_specifier),
                    codec_type: envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::CodecType::Auto as i32,
                    stat_prefix: "ingress_http".to_string(),
                    http_filters,
                    access_log: match access_log {
                        Some(cfg) => vec![build_access_log(cfg)?],
                        None => Vec::new(),
                    },
                    tracing: tracing
                        .as_ref()
                        .map(build_tracing)
                        .transpose()?,
                    ..Default::default()
                };

                EnvoyAny {
                    type_url: "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager".to_string(),
                    value: prost::Message::encode_to_vec(&hcm),
                }
            }
            FilterType::TcpProxy { cluster, access_log } => {
                let tcp_proxy = TcpProxy {
                    cluster_specifier: Some(
                        envoy_types::pb::envoy::extensions::filters::network::tcp_proxy::v3::tcp_proxy::ClusterSpecifier::Cluster(cluster.clone())
                    ),
                    stat_prefix: "ingress_tcp".to_string(),
                    access_log: match access_log {
                        Some(cfg) => vec![build_access_log(cfg)?],
                        None => Vec::new(),
                    },
                    ..Default::default()
                };

                EnvoyAny {
                    type_url:
                        "type.googleapis.com/envoy.extensions.filters.network.tcp_proxy.v3.TcpProxy"
                            .to_string(),
                    value: prost::Message::encode_to_vec(&tcp_proxy),
                }
            }
        };

        let filter = Filter {
            name: self.name.clone(),
            config_type: Some(
                envoy_types::pb::envoy::config::listener::v3::filter::ConfigType::TypedConfig(
                    typed_config,
                ),
            ),
        };

        Ok(filter)
    }
}

fn build_transport_socket(cfg: &TlsContextConfig) -> Result<TransportSocket, crate::Error> {
    let cert_chain = cfg
        .cert_chain_file
        .as_ref()
        .ok_or_else(|| crate::Error::config("Listener TLS requires cert_chain_file"))?;
    let private_key = cfg
        .private_key_file
        .as_ref()
        .ok_or_else(|| crate::Error::config("Listener TLS requires private_key_file"))?;

    let validation_context_type = cfg.ca_cert_file.as_ref().map(|ca_file| {
        common_tls_context::ValidationContextType::ValidationContext(CertificateValidationContext {
            trusted_ca: Some(data_source_from_path(ca_file)),
            ..Default::default()
        })
    });

    let common = CommonTlsContext {
        tls_certificates: vec![TlsCertificate {
            certificate_chain: Some(data_source_from_path(cert_chain)),
            private_key: Some(data_source_from_path(private_key)),
            ..Default::default()
        }],
        validation_context_type,
        ..Default::default()
    };

    let mut downstream =
        DownstreamTlsContext { common_tls_context: Some(common), ..Default::default() };

    if let Some(require) = cfg.require_client_certificate {
        downstream.require_client_certificate = Some(BoolValue { value: require });
    }

    let any = EnvoyAny {
        type_url:
            "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.DownstreamTlsContext"
                .to_string(),
        value: downstream.encode_to_vec(),
    };

    Ok(TransportSocket {
        name: "envoy.transport_sockets.tls".to_string(),
        config_type: Some(TransportSocketConfigType::TypedConfig(any)),
    })
}

fn build_access_log(cfg: &AccessLogConfig) -> Result<AccessLog, crate::Error> {
    let path = cfg
        .path
        .as_ref()
        .ok_or_else(|| crate::Error::config("Access log config requires a path"))?
        .to_string();

    let mut file_log =
        envoy_types::pb::envoy::extensions::access_loggers::file::v3::FileAccessLog {
            path,
            access_log_format: None,
        };

    if let Some(format) = &cfg.format {
        let substitution = SubstitutionFormatString {
            omit_empty_values: false,
            content_type: String::new(),
            formatters: Vec::new(),
            json_format_options: None,
            format: Some(
                envoy_types::pb::envoy::config::core::v3::substitution_format_string::Format::TextFormat(
                    format.clone(),
                ),
            ),
        };

        file_log.access_log_format = Some(
            envoy_types::pb::envoy::extensions::access_loggers::file::v3::file_access_log::AccessLogFormat::LogFormat(
                substitution,
            ),
        );
    }

    let access_log_any = EnvoyAny {
        type_url: "type.googleapis.com/envoy.extensions.access_loggers.file.v3.FileAccessLog"
            .to_string(),
        value: file_log.encode_to_vec(),
    };

    Ok(AccessLog {
        name: "envoy.access_loggers.file".to_string(),
        filter: None,
        config_type: Some(AccessLogConfigType::TypedConfig(access_log_any)),
    })
}

fn build_tracing(cfg: &TracingConfig) -> Result<http_connection_manager::Tracing, crate::Error> {
    if cfg.provider.trim().is_empty() {
        return Err(crate::Error::config("Tracing provider name cannot be empty"));
    }

    let provider_struct = ProstStruct {
        fields: cfg
            .config
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    ProstValue {
                        kind: Some(envoy_types::pb::google::protobuf::value::Kind::StringValue(
                            value.clone(),
                        )),
                    },
                )
            })
            .collect(),
    };

    let provider_any = EnvoyAny {
        type_url: "type.googleapis.com/google.protobuf.Struct".to_string(),
        value: provider_struct.encode_to_vec(),
    };

    let http_provider = HttpTracing {
        name: cfg.provider.clone(),
        config_type: Some(tracing::http::ConfigType::TypedConfig(provider_any)),
    };

    Ok(http_connection_manager::Tracing { provider: Some(http_provider), ..Default::default() })
}

fn data_source_from_path(path: &str) -> DataSource {
    DataSource {
        watched_directory: None,
        specifier: Some(
            envoy_types::pb::envoy::config::core::v3::data_source::Specifier::Filename(
                path.to_string(),
            ),
        ),
    }
}

/// Listener manager for handling listener operations
#[derive(Debug)]
pub struct ListenerManager {
    listeners: HashMap<String, Listener>,
}

impl ListenerManager {
    /// Create a new listener manager
    pub fn new() -> Self {
        Self { listeners: HashMap::new() }
    }

    /// Add or update a listener
    pub fn upsert_listener(&mut self, config: ListenerConfig) -> Result<(), crate::Error> {
        let listener = config.to_envoy_listener()?;
        self.listeners.insert(listener.name.clone(), listener);
        Ok(())
    }

    /// Remove a listener
    pub fn remove_listener(&mut self, name: &str) -> Option<Listener> {
        self.listeners.remove(name)
    }

    /// Get a listener by name
    pub fn get_listener(&self, name: &str) -> Option<&Listener> {
        self.listeners.get(name)
    }

    /// Get all listeners
    pub fn get_all_listeners(&self) -> Vec<Listener> {
        self.listeners.values().cloned().collect()
    }

    /// List listener names
    pub fn list_listener_names(&self) -> Vec<String> {
        self.listeners.keys().cloned().collect()
    }

    /// Create a basic HTTP listener configuration
    pub fn create_http_listener(
        name: String,
        address: String,
        port: u32,
        route_config_name: String,
    ) -> ListenerConfig {
        ListenerConfig {
            name,
            address,
            port,
            filter_chains: vec![FilterChainConfig {
                name: Some("default".to_string()),
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: FilterType::HttpConnectionManager {
                        route_config_name: Some(route_config_name),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
        }
    }

    /// Create a basic TCP proxy listener configuration
    pub fn create_tcp_listener(
        name: String,
        address: String,
        port: u32,
        cluster: String,
    ) -> ListenerConfig {
        ListenerConfig {
            name,
            address,
            port,
            filter_chains: vec![FilterChainConfig {
                name: Some("default".to_string()),
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.tcp_proxy".to_string(),
                    filter_type: FilterType::TcpProxy { cluster, access_log: None },
                }],
                tls_context: None,
            }],
        }
    }
}

impl Default for ListenerManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xds::filters::http::local_rate_limit::{LocalRateLimitConfig, TokenBucketConfig};
    use crate::xds::filters::http::{HttpFilterConfigEntry, HttpFilterKind, ROUTER_FILTER_NAME};
    use crate::xds::route::{
        PathMatch, RouteActionConfig, RouteConfig, RouteMatchConfig, RouteRule, VirtualHostConfig,
    };
    use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpConnectionManager as ProtoHttpConnectionManager;
    use prost::Message;
    use std::collections::HashMap;

    #[test]
    fn test_listener_config_conversion() {
        let route_config = RouteConfig {
            name: "test-route".to_string(),
            virtual_hosts: vec![VirtualHostConfig {
                name: "test-vhost".to_string(),
                domains: vec!["example.com".to_string()],
                routes: vec![RouteRule {
                    name: Some("default".to_string()),
                    r#match: RouteMatchConfig {
                        path: PathMatch::Prefix("/".to_string()),
                        headers: None,
                        query_parameters: None,
                    },
                    action: RouteActionConfig::Cluster {
                        name: "backend-cluster".to_string(),
                        timeout: None,
                        prefix_rewrite: None,
                        path_template_rewrite: None,
                    },
                    typed_per_filter_config: HashMap::new(),
                }],
                typed_per_filter_config: HashMap::new(),
            }],
        };

        let config = ListenerConfig {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            filter_chains: vec![FilterChainConfig {
                name: Some("default".to_string()),
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: FilterType::HttpConnectionManager {
                        route_config_name: None,
                        inline_route_config: Some(route_config),
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
        };

        let listener = config.to_envoy_listener().expect("Failed to convert listener config");

        assert_eq!(listener.name, "test-listener");
        assert!(listener.address.is_some());
        assert_eq!(listener.filter_chains.len(), 1);

        let filter_chain = &listener.filter_chains[0];
        assert_eq!(filter_chain.filters.len(), 1);

        let filter = &filter_chain.filters[0];
        assert_eq!(filter.name, "envoy.filters.network.http_connection_manager");
        assert!(filter.config_type.is_some());
    }

    #[test]
    fn test_listener_manager() {
        let mut manager = ListenerManager::new();

        let config = ListenerManager::create_http_listener(
            "http-listener".to_string(),
            "0.0.0.0".to_string(),
            8080,
            "default-route".to_string(),
        );

        manager.upsert_listener(config).expect("Failed to add listener");

        assert!(manager.get_listener("http-listener").is_some());
        assert_eq!(manager.list_listener_names().len(), 1);

        let removed = manager.remove_listener("http-listener");
        assert!(removed.is_some());
        assert_eq!(manager.list_listener_names().len(), 0);
    }

    #[test]
    fn test_tcp_listener_creation() {
        let config = ListenerManager::create_tcp_listener(
            "tcp-listener".to_string(),
            "127.0.0.1".to_string(),
            9090,
            "tcp-cluster".to_string(),
        );

        assert_eq!(config.name, "tcp-listener");
        assert_eq!(config.address, "127.0.0.1");
        assert_eq!(config.port, 9090);
        assert_eq!(config.filter_chains.len(), 1);

        let filter_chain = &config.filter_chains[0];
        assert_eq!(filter_chain.filters.len(), 1);

        let filter = &filter_chain.filters[0];
        assert_eq!(filter.name, "envoy.filters.network.tcp_proxy");
        assert!(matches!(filter.filter_type, FilterType::TcpProxy { .. }));
    }

    #[test]
    fn http_connection_manager_supports_local_rate_limit_filter() {
        let route_config = RouteConfig {
            name: "test-route".to_string(),
            virtual_hosts: vec![VirtualHostConfig {
                name: "vh".to_string(),
                domains: vec!["*".to_string()],
                routes: vec![RouteRule {
                    name: None,
                    r#match: RouteMatchConfig {
                        path: PathMatch::Prefix("/".into()),
                        headers: None,
                        query_parameters: None,
                    },
                    action: RouteActionConfig::Cluster {
                        name: "backend".into(),
                        timeout: None,
                        prefix_rewrite: None,
                        path_template_rewrite: None,
                    },
                    typed_per_filter_config: HashMap::new(),
                }],
                typed_per_filter_config: HashMap::new(),
            }],
        };

        let listener = ListenerConfig {
            name: "test-listener".into(),
            address: "0.0.0.0".into(),
            port: 8080,
            filter_chains: vec![FilterChainConfig {
                name: None,
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".into(),
                    filter_type: FilterType::HttpConnectionManager {
                        route_config_name: None,
                        inline_route_config: Some(route_config),
                        access_log: None,
                        tracing: None,
                        http_filters: vec![HttpFilterConfigEntry {
                            name: None,
                            is_optional: false,
                            disabled: false,
                            filter: HttpFilterKind::LocalRateLimit(LocalRateLimitConfig {
                                stat_prefix: "ingress_http_ratelimit".into(),
                                token_bucket: Some(TokenBucketConfig {
                                    max_tokens: 10,
                                    tokens_per_fill: Some(10),
                                    fill_interval_ms: 1000,
                                }),
                                status_code: Some(429),
                                filter_enabled: None,
                                filter_enforced: None,
                                per_downstream_connection: Some(false),
                                rate_limited_as_resource_exhausted: Some(false),
                                max_dynamic_descriptors: None,
                                always_consume_default_token_bucket: Some(false),
                            }),
                        }],
                    },
                }],
                tls_context: None,
            }],
        };

        let envoy_listener = listener.to_envoy_listener().expect("listener conversion");

        let filter = &envoy_listener.filter_chains[0].filters[0];
        let config = filter.config_type.as_ref().expect("filter missing config type");
        let any = match config {
            envoy_types::pb::envoy::config::listener::v3::filter::ConfigType::TypedConfig(any) => {
                any
            }
            other => panic!("unsupported config type in test: {:?}", other),
        };

        let hcm = ProtoHttpConnectionManager::decode(any.value.as_slice())
            .expect("decode http connection manager");

        assert_eq!(hcm.http_filters.len(), 2);
        assert_eq!(hcm.http_filters[0].name, "envoy.filters.http.local_ratelimit");
        assert_eq!(hcm.http_filters[1].name, ROUTER_FILTER_NAME);
    }
}
