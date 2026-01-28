//! Listener validation and conversion utilities

use std::convert::TryFrom;

use serde_json::Value;

use crate::{
    api::error::ApiError,
    errors::Error,
    storage::ListenerData,
    xds::listener::{
        AccessLogConfig, FilterChainConfig, FilterConfig, FilterType, ListenerConfig,
        TlsContextConfig, TracingConfig,
    },
    xds::route::RouteConfig,
};

use super::types::{
    CreateListenerBody, ListenerAccessLogInput, ListenerFilterChainInput, ListenerFilterInput,
    ListenerFilterTypeInput, ListenerResponse, ListenerTlsContextInput, ListenerTracingInput,
    UpdateListenerBody,
};

/// Convert database listener data to API response
pub(super) fn listener_response_from_data(
    data: ListenerData,
) -> Result<ListenerResponse, ApiError> {
    let config_value: serde_json::Value =
        serde_json::from_str(&data.configuration).map_err(|err| {
            ApiError::from(Error::internal(format!(
                "Failed to parse stored listener configuration '{}': {}",
                data.name, err
            )))
        })?;

    // Parse as ListenerConfig
    let config: ListenerConfig = serde_json::from_value(config_value).map_err(|err| {
        ApiError::from(Error::internal(format!(
            "Failed to deserialize listener configuration '{}': {}",
            data.name, err
        )))
    })?;

    Ok(ListenerResponse {
        name: data.name,
        team: data.team.unwrap_or_else(|| "unknown".to_string()),
        address: data.address,
        port: port_from_i64(data.port)?,
        protocol: data.protocol,
        version: data.version,
        import_id: data.import_id,
        dataplane_id: data.dataplane_id,
        config,
    })
}

/// Build listener config from create request
pub(super) fn listener_config_from_create(
    body: &CreateListenerBody,
) -> Result<ListenerConfig, ApiError> {
    listener_config_from_parts(
        body.name.clone(),
        body.address.clone(),
        body.port,
        &body.filter_chains,
    )
}

/// Build listener config from update request
pub(super) fn listener_config_from_update(
    name: String,
    body: &UpdateListenerBody,
) -> Result<ListenerConfig, ApiError> {
    listener_config_from_parts(name, body.address.clone(), body.port, &body.filter_chains)
}

fn listener_config_from_parts(
    name: String,
    address: String,
    port: u16,
    filter_chains: &[ListenerFilterChainInput],
) -> Result<ListenerConfig, ApiError> {
    let chains = filter_chains.iter().map(convert_filter_chain).collect::<Result<Vec<_>, _>>()?;

    Ok(ListenerConfig { name, address, port: port.into(), filter_chains: chains })
}

fn convert_filter_chain(input: &ListenerFilterChainInput) -> Result<FilterChainConfig, ApiError> {
    let filters = input.filters.iter().map(convert_filter).collect::<Result<Vec<_>, _>>()?;

    Ok(FilterChainConfig {
        name: input.name.clone(),
        filters,
        tls_context: input.tls_context.as_ref().map(convert_tls_context),
    })
}

fn convert_filter(input: &ListenerFilterInput) -> Result<FilterConfig, ApiError> {
    Ok(FilterConfig {
        name: input.name.clone(),
        filter_type: convert_filter_type(&input.filter_type)?,
    })
}

pub(super) fn convert_filter_type(input: &ListenerFilterTypeInput) -> Result<FilterType, ApiError> {
    match input {
        ListenerFilterTypeInput::HttpConnectionManager {
            route_config_name,
            inline_route_config,
            access_log,
            tracing,
            http_filters,
        } => {
            let inline_route_config = match inline_route_config {
                Some(value) => Some(parse_route_config(value)?),
                None => None,
            };

            if route_config_name.as_ref().map(|name| name.trim().is_empty()).unwrap_or(true)
                && inline_route_config.is_none()
            {
                return Err(ApiError::from(Error::validation(
                    "HttpConnectionManager requires route_config_name or inline_route_config",
                )));
            }

            Ok(FilterType::HttpConnectionManager {
                route_config_name: route_config_name.as_ref().map(|s| s.trim().to_string()),
                inline_route_config,
                access_log: access_log.as_ref().map(convert_access_log),
                tracing: tracing.as_ref().map(convert_tracing).transpose()?,
                http_filters: http_filters.clone(),
            })
        }
        ListenerFilterTypeInput::TcpProxy { cluster, access_log } => {
            if cluster.trim().is_empty() {
                return Err(ApiError::from(Error::validation(
                    "TCP proxy filter requires a non-empty cluster",
                )));
            }
            Ok(FilterType::TcpProxy {
                cluster: cluster.trim().to_string(),
                access_log: access_log.as_ref().map(convert_access_log),
            })
        }
    }
}

fn convert_tls_context(input: &ListenerTlsContextInput) -> TlsContextConfig {
    TlsContextConfig {
        cert_chain_file: input.cert_chain_file.clone(),
        private_key_file: input.private_key_file.clone(),
        ca_cert_file: input.ca_cert_file.clone(),
        require_client_certificate: input.require_client_certificate,
    }
}

fn convert_access_log(input: &ListenerAccessLogInput) -> AccessLogConfig {
    AccessLogConfig { path: input.path.clone(), format: input.format.clone() }
}

fn convert_tracing(input: &ListenerTracingInput) -> Result<TracingConfig, ApiError> {
    use crate::xds::listener::TracingProvider;

    // Parse the provider configuration from the JSON value
    let provider: TracingProvider = serde_json::from_value(input.provider.clone()).map_err(|err| {
        ApiError::from(Error::validation(format!(
            "Invalid tracing provider configuration: {}. Expected one of: \
             {{ \"type\": \"open_telemetry\", \"service_name\": \"...\", \"grpc_cluster\": \"...\" }}, \
             {{ \"type\": \"zipkin\", \"collector_cluster\": \"...\", \"collector_endpoint\": \"...\" }}, \
             {{ \"type\": \"generic\", \"name\": \"...\", \"config\": {{}} }}",
            err
        )))
    })?;

    Ok(TracingConfig {
        provider,
        random_sampling_percentage: input.random_sampling_percentage,
        spawn_upstream_span: input.spawn_upstream_span,
        custom_tags: input.custom_tags.clone().unwrap_or_default(),
    })
}

fn parse_route_config(value: &Value) -> Result<RouteConfig, ApiError> {
    serde_json::from_value(value.clone()).map_err(|err| {
        ApiError::from(Error::validation(format!("Invalid inline route configuration: {}", err)))
    })
}

/// Validate create listener request
pub(super) fn validate_create_listener_body(body: &CreateListenerBody) -> Result<(), ApiError> {
    if body.team.trim().is_empty() {
        return Err(ApiError::from(Error::validation("Listener team cannot be empty")));
    }
    if body.name.trim().is_empty() {
        return Err(ApiError::from(Error::validation("Listener name cannot be empty")));
    }
    if body.dataplane_id.trim().is_empty() {
        return Err(ApiError::from(Error::validation(
            "dataplane_id is required - create a dataplane first",
        )));
    }
    validate_listener_common(&body.address, body.port, &body.filter_chains)
}

/// Validate update listener request
pub(super) fn validate_update_listener_body(body: &UpdateListenerBody) -> Result<(), ApiError> {
    validate_listener_common(&body.address, body.port, &body.filter_chains)
}

fn validate_listener_common(
    address: &str,
    port: u16,
    filter_chains: &[ListenerFilterChainInput],
) -> Result<(), ApiError> {
    if address.trim().is_empty() {
        return Err(ApiError::from(Error::validation("Listener address cannot be empty")));
    }

    if port < 1024 {
        return Err(ApiError::from(Error::validation("Listener port must be >= 1024")));
    }

    if filter_chains.is_empty() {
        return Err(ApiError::from(Error::validation("At least one filter chain is required")));
    }

    for chain in filter_chains {
        if chain.filters.is_empty() {
            return Err(ApiError::from(Error::validation(
                "Each filter chain must include at least one filter",
            )));
        }

        for filter in &chain.filters {
            if filter.name.trim().is_empty() {
                return Err(ApiError::from(Error::validation("Filter name cannot be empty")));
            }

            if let ListenerFilterTypeInput::HttpConnectionManager {
                route_config_name,
                inline_route_config,
                ..
            } = &filter.filter_type
            {
                if route_config_name.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true)
                    && inline_route_config.is_none()
                {
                    return Err(ApiError::from(Error::validation(
                        "HttpConnectionManager filter requires route_config_name or inline_route_config",
                    )));
                }

                if let Some(value) = inline_route_config {
                    parse_route_config(value)?;
                }
            }
        }
    }

    Ok(())
}

fn port_from_i64(port: Option<i64>) -> Result<Option<u16>, ApiError> {
    match port {
        Some(value) => u16::try_from(value).map(Some).map_err(|_| {
            ApiError::from(Error::internal(format!(
                "Stored listener port value '{}' is out of range",
                value
            )))
        }),
        None => Ok(None),
    }
}
