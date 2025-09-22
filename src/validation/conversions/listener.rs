use crate::validation::requests::{
    ValidatedAccessLogRequest, ValidatedCreateListenerRequest, ValidatedFilterChainRequest,
    ValidatedFilterRequest, ValidatedFilterType, ValidatedTlsContextRequest,
    ValidatedTracingRequest, ValidatedUpdateListenerRequest,
};
use crate::xds::listener;

impl From<ValidatedCreateListenerRequest> for listener::ListenerConfig {
    fn from(validated: ValidatedCreateListenerRequest) -> Self {
        Self {
            name: validated.name,
            address: validated.address,
            port: validated.port,
            filter_chains: validated.filter_chains.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ValidatedUpdateListenerRequest> for listener::ListenerConfig {
    fn from(validated: ValidatedUpdateListenerRequest) -> Self {
        Self {
            name: String::new(),
            address: validated.address,
            port: validated.port,
            filter_chains: validated.filter_chains.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ValidatedFilterChainRequest> for listener::FilterChainConfig {
    fn from(validated: ValidatedFilterChainRequest) -> Self {
        Self {
            name: validated.name,
            filters: validated.filters.into_iter().map(Into::into).collect(),
            tls_context: validated.tls_context.map(Into::into),
        }
    }
}

impl From<ValidatedFilterRequest> for listener::FilterConfig {
    fn from(validated: ValidatedFilterRequest) -> Self {
        Self {
            name: validated.name,
            filter_type: validated.filter_type.into(),
        }
    }
}

impl From<ValidatedFilterType> for listener::FilterType {
    fn from(validated: ValidatedFilterType) -> Self {
        match validated {
            ValidatedFilterType::HttpConnectionManager {
                route_config_name,
                inline_route_config,
                access_log,
                tracing,
            } => listener::FilterType::HttpConnectionManager {
                route_config_name,
                inline_route_config: inline_route_config.map(Into::into),
                access_log: access_log.map(Into::into),
                tracing: tracing.map(Into::into),
            },
            ValidatedFilterType::TcpProxy { cluster, access_log } => listener::FilterType::TcpProxy {
                cluster,
                access_log: access_log.map(Into::into),
            },
        }
    }
}

impl From<ValidatedTlsContextRequest> for listener::TlsContextConfig {
    fn from(validated: ValidatedTlsContextRequest) -> Self {
        Self {
            cert_chain_file: validated.cert_chain_file,
            private_key_file: validated.private_key_file,
            ca_cert_file: validated.ca_cert_file,
            require_client_certificate: validated.require_client_certificate,
        }
    }
}

impl From<ValidatedAccessLogRequest> for listener::AccessLogConfig {
    fn from(validated: ValidatedAccessLogRequest) -> Self {
        Self {
            path: validated.path,
            format: validated.format,
        }
    }
}

impl From<ValidatedTracingRequest> for listener::TracingConfig {
    fn from(validated: ValidatedTracingRequest) -> Self {
        Self {
            provider: validated.provider,
            config: validated.config,
        }
    }
}

impl From<listener::ListenerConfig> for ValidatedCreateListenerRequest {
    fn from(listener_config: listener::ListenerConfig) -> Self {
        Self {
            name: listener_config.name,
            address: listener_config.address,
            port: listener_config.port,
            filter_chains: listener_config.filter_chains.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<listener::FilterChainConfig> for ValidatedFilterChainRequest {
    fn from(filter_chain_config: listener::FilterChainConfig) -> Self {
        Self {
            name: filter_chain_config.name,
            filters: filter_chain_config.filters.into_iter().map(Into::into).collect(),
            tls_context: filter_chain_config.tls_context.map(Into::into),
        }
    }
}

impl From<listener::FilterConfig> for ValidatedFilterRequest {
    fn from(filter_config: listener::FilterConfig) -> Self {
        Self {
            name: filter_config.name,
            filter_type: filter_config.filter_type.into(),
        }
    }
}

impl From<listener::FilterType> for ValidatedFilterType {
    fn from(filter_type: listener::FilterType) -> Self {
        match filter_type {
            listener::FilterType::HttpConnectionManager {
                route_config_name,
                inline_route_config,
                access_log,
                tracing,
            } => ValidatedFilterType::HttpConnectionManager {
                route_config_name,
                inline_route_config: inline_route_config.map(Into::into),
                access_log: access_log.map(Into::into),
                tracing: tracing.map(Into::into),
            },
            listener::FilterType::TcpProxy { cluster, access_log } => ValidatedFilterType::TcpProxy {
                cluster,
                access_log: access_log.map(Into::into),
            },
        }
    }
}

impl From<listener::TlsContextConfig> for ValidatedTlsContextRequest {
    fn from(tls_context_config: listener::TlsContextConfig) -> Self {
        Self {
            cert_chain_file: tls_context_config.cert_chain_file,
            private_key_file: tls_context_config.private_key_file,
            ca_cert_file: tls_context_config.ca_cert_file,
            require_client_certificate: tls_context_config.require_client_certificate,
        }
    }
}

impl From<listener::AccessLogConfig> for ValidatedAccessLogRequest {
    fn from(access_log_config: listener::AccessLogConfig) -> Self {
        Self {
            path: access_log_config.path,
            format: access_log_config.format,
        }
    }
}

impl From<listener::TracingConfig> for ValidatedTracingRequest {
    fn from(tracing_config: listener::TracingConfig) -> Self {
        Self {
            provider: tracing_config.provider,
            config: tracing_config.config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_listener_conversion() {
        let validated_request = ValidatedCreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            filter_chains: vec![ValidatedFilterChainRequest {
                name: Some("default".to_string()),
                filters: vec![ValidatedFilterRequest {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ValidatedFilterType::HttpConnectionManager {
                        route_config_name: Some("default-route".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                    },
                }],
                tls_context: None,
            }],
        };

        let listener_config: listener::ListenerConfig = validated_request.into();

        assert_eq!(listener_config.name, "test-listener");
        assert_eq!(listener_config.address, "0.0.0.0");
        assert_eq!(listener_config.port, 8080);
        assert_eq!(listener_config.filter_chains.len(), 1);
    }
}
