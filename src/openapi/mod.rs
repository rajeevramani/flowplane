use std::collections::HashSet;

use openapiv3::{OpenAPI, ReferenceOr, Server};
use serde::{Deserialize, Serialize};
use serde_json::{self, Value as JsonValue};
use utoipa::ToSchema;

use crate::{
    errors::Error,
    storage::{CreateClusterRequest, CreateListenerRequest, CreateRouteRepositoryRequest},
    utils::VALID_NAME_REGEX,
    xds::{
        filters::http::HttpFilterConfigEntry,
        listener::{FilterChainConfig, FilterConfig, FilterType, ListenerConfig},
        route::{
            PathMatch, RouteActionConfig, RouteConfig as XdsRouteConfig, RouteMatchConfig,
            RouteRule, VirtualHostConfig,
        },
    },
};

const EXTENSION_GLOBAL_FILTERS: &str = "x-flowplane-filters";

pub mod defaults;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayOptions {
    pub name: String,
    pub bind_address: String,
    pub port: u16,
    pub protocol: String,
    pub shared_listener: bool,
    pub listener_name: String,
}

#[derive(Debug, Clone)]
pub struct GatewayPlan {
    pub cluster_requests: Vec<CreateClusterRequest>,
    pub route_request: Option<CreateRouteRepositoryRequest>,
    pub listener_request: Option<CreateListenerRequest>,
    pub default_virtual_host: Option<VirtualHostConfig>,
    pub summary: GatewaySummary,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GatewaySummary {
    pub gateway: String,
    pub route_config: String,
    pub listener: String,
    pub clusters: Vec<String>,
    pub shared_listener: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("Invalid gateway name '{0}'")]
    InvalidGatewayName(String),
    #[error("OpenAPI document has no servers defined")]
    MissingServers,
    #[error("Failed to parse OpenAPI document: {0}")]
    InvalidSpec(String),
    #[error("Unsupported server URL '{0}'")]
    UnsupportedServer(String),
    #[error("No routes could be generated from the OpenAPI document")]
    NoRoutes,
    #[error("Failed to parse Flowplane filters: {0}")]
    InvalidFilters(String),
}

pub fn build_gateway_plan(
    openapi: OpenAPI,
    options: GatewayOptions,
) -> Result<GatewayPlan, GatewayError> {
    if !VALID_NAME_REGEX.is_match(&options.name) {
        return Err(GatewayError::InvalidGatewayName(options.name));
    }

    let route_name = if options.shared_listener {
        defaults::DEFAULT_GATEWAY_ROUTES.to_string()
    } else {
        format!("{}-routes", &options.name)
    };

    let listener_name = if options.shared_listener {
        defaults::DEFAULT_GATEWAY_LISTENER.to_string()
    } else {
        options.listener_name.clone()
    };

    let virtual_host_name = format!("{}-vh", &options.name);

    let servers = if openapi.servers.is_empty() { Vec::new() } else { openapi.servers.clone() };

    if servers.is_empty() {
        return Err(GatewayError::MissingServers);
    }

    let primary_server = &servers[0];
    let cluster_info = cluster_from_server(primary_server, &options.name, &options.name)?;
    let cluster_requests = vec![cluster_info.request.clone()];
    let mut domains: HashSet<String> = HashSet::new();
    if !options.shared_listener {
        domains.insert("*".to_string());
    }
    if !cluster_info.domain.is_empty() {
        domains.insert(cluster_info.domain.clone());
    }

    let global_filters = parse_global_filters(&openapi)?;

    let mut route_rules: Vec<RouteRule> = Vec::new();

    for (path_template, item) in openapi.paths.paths.iter() {
        let _path_item = match item {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { reference } => {
                return Err(GatewayError::UnsupportedServer(reference.clone()))
            }
        };

        let effective_path = combine_base_path(primary_server, path_template);
        let route_name = route_name_for_path(&options.name, &effective_path);

        let route_rule = RouteRule {
            name: Some(route_name),
            r#match: RouteMatchConfig {
                path: PathMatch::Template(effective_path),
                headers: None,
                query_parameters: None,
            },
            action: RouteActionConfig::Cluster {
                name: cluster_info.request.name.clone(),
                timeout: None,
                prefix_rewrite: None,
                path_template_rewrite: None,
            },
            typed_per_filter_config: Default::default(),
        };

        route_rules.push(route_rule);
    }

    if route_rules.is_empty() {
        return Err(GatewayError::NoRoutes);
    }

    let mut domains_vec: Vec<String> =
        if domains.is_empty() { vec!["*".to_string()] } else { domains.into_iter().collect() };

    if options.shared_listener {
        domains_vec.retain(|domain| domain != "*");
        if domains_vec.is_empty() {
            domains_vec.push(cluster_info.domain.clone());
        }
    }

    let virtual_host = VirtualHostConfig {
        name: virtual_host_name,
        domains: domains_vec,
        routes: route_rules,
        typed_per_filter_config: Default::default(),
    };

    let summary = GatewaySummary {
        gateway: options.name.clone(),
        listener: listener_name.clone(),
        route_config: route_name.clone(),
        clusters: cluster_requests.iter().map(|request| request.name.clone()).collect(),
        shared_listener: options.shared_listener,
    };

    let default_cluster_name =
        summary.clusters.first().cloned().unwrap_or_else(|| cluster_info.request.name.clone());

    if options.shared_listener {
        Ok(GatewayPlan {
            cluster_requests,
            route_request: None,
            listener_request: None,
            default_virtual_host: Some(virtual_host),
            summary,
        })
    } else {
        let route_config =
            XdsRouteConfig { name: route_name.clone(), virtual_hosts: vec![virtual_host] };

        let mut route_config_value = serde_json::to_value(&route_config)
            .map_err(|err| GatewayError::InvalidSpec(err.to_string()))?;
        attach_gateway_tag(&mut route_config_value, &options.name);

        let route_request = CreateRouteRepositoryRequest {
            name: route_name,
            path_prefix: "/".to_string(),
            cluster_name: default_cluster_name,
            configuration: route_config_value,
            team: None, // OpenAPI Gateway routes are not team-scoped by default
        };

        let listener_config = ListenerConfig {
            name: listener_name.clone(),
            address: options.bind_address.clone(),
            port: options.port as u32,
            filter_chains: vec![FilterChainConfig {
                name: Some(format!("{}-chain", options.name)),
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: FilterType::HttpConnectionManager {
                        route_config_name: Some(route_request.name.clone()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: global_filters,
                    },
                }],
                tls_context: None,
            }],
        };

        let mut listener_config_value = serde_json::to_value(&listener_config)
            .map_err(|err| GatewayError::InvalidSpec(err.to_string()))?;
        attach_gateway_tag(&mut listener_config_value, &options.name);

        let listener_request = CreateListenerRequest {
            name: listener_name,
            address: options.bind_address,
            port: Some(options.port as i64),
            protocol: Some(options.protocol),
            configuration: listener_config_value,
            team: None, // OpenAPI Gateway listeners are not team-scoped by default
        };

        Ok(GatewayPlan {
            cluster_requests,
            route_request: Some(route_request),
            listener_request: Some(listener_request),
            default_virtual_host: None,
            summary,
        })
    }
}

pub fn parse_global_filters(openapi: &OpenAPI) -> Result<Vec<HttpFilterConfigEntry>, GatewayError> {
    if let Some(value) = openapi.extensions.get(EXTENSION_GLOBAL_FILTERS) {
        serde_json::from_value::<Vec<HttpFilterConfigEntry>>(value.clone()).map_err(|err| {
            GatewayError::InvalidFilters(format!(
                "Failed to parse {}: {}",
                EXTENSION_GLOBAL_FILTERS, err
            ))
        })
    } else {
        Ok(Vec::new())
    }
}

struct ClusterInfo {
    request: CreateClusterRequest,
    domain: String,
}

fn cluster_from_server(
    server: &Server,
    prefix: &str,
    gateway_tag: &str,
) -> Result<ClusterInfo, GatewayError> {
    let url = url::Url::parse(&server.url)
        .map_err(|err| GatewayError::UnsupportedServer(format!("{} ({})", server.url, err)))?;

    let host = url.host_str().ok_or_else(|| {
        GatewayError::UnsupportedServer(format!(
            "Server URL '{}' does not contain a host",
            server.url
        ))
    })?;

    let port = url.port_or_known_default().ok_or_else(|| {
        GatewayError::UnsupportedServer(format!(
            "Server URL '{}' does not include a usable port",
            server.url
        ))
    })?;

    let use_tls = matches!(url.scheme(), "https" | "grpcs");

    let cluster_name = sanitize_name(&format!("{}-{}", prefix, host));

    let spec = crate::xds::ClusterSpec {
        connect_timeout_seconds: Some(5),
        endpoints: vec![crate::xds::EndpointSpec::Address { host: host.to_string(), port }],
        use_tls: Some(use_tls),
        tls_server_name: if use_tls { Some(host.to_string()) } else { None },
        dns_lookup_family: None,
        lb_policy: None,
        least_request: None,
        ring_hash: None,
        maglev: None,
        circuit_breakers: None,
        health_checks: Vec::new(),
        outlier_detection: None,
    };

    let mut configuration =
        spec.to_value().map_err(|err| GatewayError::InvalidSpec(err.to_string()))?;
    attach_gateway_tag(&mut configuration, gateway_tag);

    let request = CreateClusterRequest {
        name: cluster_name.clone(),
        service_name: cluster_name.clone(),
        configuration,
        team: None, // OpenAPI Gateway clusters are not team-scoped by default
    };

    let domain = host.to_string();

    Ok(ClusterInfo { request, domain })
}

fn combine_base_path(server: &Server, template: &str) -> String {
    let base_path = url::Url::parse(&server.url)
        .map(|url| url.path().trim_matches('/').to_string())
        .unwrap_or_else(|_| String::new());

    let template_path = template.trim_matches('/');

    let mut segments: Vec<String> = Vec::new();

    if !base_path.is_empty() {
        segments.extend(
            base_path
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.to_string()),
        );
    }

    if !template_path.is_empty() {
        segments.extend(
            template_path
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.to_string()),
        );
    }

    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

fn route_name_for_path(prefix: &str, path: &str) -> String {
    let slug = path.trim_matches('/').replace('/', "_").replace(['{', '}'], "");
    let slug = if slug.is_empty() { "root".to_string() } else { slug };
    sanitize_name(&format!("{}-{}", prefix, slug))
}

fn sanitize_name(raw: &str) -> String {
    let mut name: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();

    if name.is_empty() {
        name.push_str("resource");
    }

    // Ensure first character is alphabetic or underscore
    if let Some(first_char) = name.chars().next() {
        if !first_char.is_ascii_alphabetic() && first_char != '_' {
            name.insert(0, '_');
        }
    } else {
        // Defensive: should not happen since we handle empty strings above
        name.push_str("resource");
    }

    if name.len() > 48 {
        name.truncate(48);
    }

    name
}

impl From<GatewayError> for Error {
    fn from(value: GatewayError) -> Self {
        Error::validation(value.to_string())
    }
}

pub(crate) fn attach_gateway_tag(value: &mut JsonValue, gateway: &str) {
    match value {
        JsonValue::Object(map) => {
            if !is_enum_wrapper(map) {
                map.entry("flowplaneGateway".to_string())
                    .or_insert_with(|| JsonValue::String(gateway.to_string()));
            }

            for (key, child) in map.iter_mut() {
                if key == "flowplaneGateway" || key == "typed_per_filter_config" {
                    continue;
                }
                attach_gateway_tag(child, gateway);
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                attach_gateway_tag(item, gateway);
            }
        }
        _ => {}
    }
}

pub(crate) fn strip_gateway_tags(value: &mut JsonValue) {
    match value {
        JsonValue::Object(map) => {
            map.remove("flowplaneGateway");
            for child in map.values_mut() {
                strip_gateway_tags(child);
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                strip_gateway_tags(item);
            }
        }
        _ => {}
    }
}

fn is_enum_wrapper(map: &serde_json::Map<String, JsonValue>) -> bool {
    if map.len() != 1 {
        return false;
    }

    map.keys().all(|key| key.chars().next().map(|c| c.is_uppercase()).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_gateway_plan_from_basic_openapi() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/users": {
                        "get": {
                            "responses": {
                                "200": {"description": "OK"}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let options = GatewayOptions {
            name: "example".to_string(),
            bind_address: "0.0.0.0".to_string(),
            port: 10000,
            protocol: "HTTP".to_string(),
            shared_listener: false,
            listener_name: "example-listener".to_string(),
        };

        let plan = build_gateway_plan(doc, options).expect("plan");

        assert_eq!(plan.summary.gateway, "example");
        assert_eq!(plan.summary.route_config, "example-routes");
        assert_eq!(plan.summary.listener, "example-listener");
        assert!(!plan.summary.shared_listener);
        assert_eq!(plan.summary.clusters.len(), 1);
        assert_eq!(plan.cluster_requests.len(), 1);
        let route_request = plan.route_request.as_ref().expect("route request should exist");
        assert_eq!(route_request.name, "example-routes");
        let listener_request =
            plan.listener_request.as_ref().expect("listener request should exist");
        assert_eq!(listener_request.name, "example-listener");
        assert!(plan.default_virtual_host.is_none());

        fn expect_tag(value: &JsonValue, gateway: &str) {
            if let Some(map) = value.as_object() {
                if let Some(tag) = map.get("flowplaneGateway").and_then(|value| value.as_str()) {
                    assert_eq!(tag, gateway);
                    return;
                }

                if super::is_enum_wrapper(map) {
                    if let Some(child) = map.values().next() {
                        expect_tag(child, gateway);
                        return;
                    }
                }
            }

            panic!(
                "Expected flowplaneGateway tag '{}' but it was missing in value: {}",
                gateway, value
            );
        }

        let cluster_config = &plan.cluster_requests[0].configuration;
        expect_tag(cluster_config, "example");
        if let Some(endpoint) = cluster_config
            .get("endpoints")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
        {
            expect_tag(endpoint, "example");
        }

        let route_config = &route_request.configuration;
        expect_tag(route_config, "example");
        if let Some(virtual_host) = route_config
            .get("virtual_hosts")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
        {
            expect_tag(virtual_host, "example");
            if let Some(route) = virtual_host
                .get("routes")
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
            {
                expect_tag(route, "example");
            }
        }

        let listener_config = &listener_request.configuration;
        expect_tag(listener_config, "example");
        if let Some(filter_chain) = listener_config
            .get("filter_chains")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
        {
            expect_tag(filter_chain, "example");
            if let Some(filter) = filter_chain
                .get("filters")
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
            {
                expect_tag(filter, "example");
                if let Some(filter_type) = filter.get("filter_type") {
                    expect_tag(filter_type, "example");
                }
            }
        }
    }

    #[test]
    fn builds_shared_gateway_plan_from_basic_openapi() {
        let doc: OpenAPI = serde_json::from_str(
            r#"{
                "openapi": "3.0.0",
                "info": {"title": "Example", "version": "1.0.0"},
                "servers": [{"url": "https://api.example.com"}],
                "paths": {
                    "/users": {
                        "get": {
                            "responses": {
                                "200": {"description": "OK"}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("parse openapi");

        let options = GatewayOptions {
            name: "example".to_string(),
            bind_address: defaults::DEFAULT_GATEWAY_ADDRESS.to_string(),
            port: defaults::DEFAULT_GATEWAY_PORT,
            protocol: "HTTP".to_string(),
            shared_listener: true,
            listener_name: defaults::DEFAULT_GATEWAY_LISTENER.to_string(),
        };

        let plan = build_gateway_plan(doc, options).expect("plan");

        assert!(plan.route_request.is_none());
        assert!(plan.listener_request.is_none());
        assert!(plan.default_virtual_host.is_some());
        assert!(plan.summary.shared_listener);
        assert_eq!(plan.summary.listener, defaults::DEFAULT_GATEWAY_LISTENER);
        assert_eq!(plan.summary.route_config, defaults::DEFAULT_GATEWAY_ROUTES);
    }
}
