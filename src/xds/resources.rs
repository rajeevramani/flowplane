use std::{collections::HashMap, net::IpAddr};

use crate::platform_api::filter_overrides::typed_per_filter_config;
use crate::xds::{
    CircuitBreakerThresholdsSpec, CircuitBreakersSpec, ClusterSpec, HealthCheckSpec,
    OutlierDetectionSpec,
};
use crate::{
    config::SimpleXdsConfig,
    storage::{ApiDefinitionData, ApiRouteData, ClusterData, ListenerData, RouteData},
    Error, Result,
};
use envoy_types::pb::envoy::config::cluster::v3::circuit_breakers::Thresholds as CircuitThresholds;
use envoy_types::pb::envoy::config::cluster::v3::cluster::{
    self, ring_hash_lb_config::HashFunction as RingHashFunction, ClusterDiscoveryType,
    DiscoveryType, DnsLookupFamily, LbPolicy, LeastRequestLbConfig, MaglevLbConfig,
    RingHashLbConfig,
};
use envoy_types::pb::envoy::config::cluster::v3::{CircuitBreakers, Cluster, OutlierDetection};
use envoy_types::pb::envoy::config::core::v3::transport_socket::ConfigType as TransportSocketConfigType;
use envoy_types::pb::envoy::config::core::v3::{
    health_check::{self, HttpHealthCheck, TcpHealthCheck},
    socket_address::{self, Protocol},
    Address, HealthCheck, Http2ProtocolOptions, RequestMethod, RoutingPriority, SocketAddress,
    TransportSocket,
};
use envoy_types::pb::envoy::extensions::upstreams::http::v3::{
    http_protocol_options::{ExplicitHttpConfig, UpstreamProtocolOptions},
    HttpProtocolOptions as UpstreamHttpProtocolOptionsV3,
};
use envoy_types::pb::envoy::extensions::upstreams::http::v3::http_protocol_options::explicit_http_config::ProtocolConfig;
use envoy_types::pb::envoy::config::endpoint::v3::{
    lb_endpoint, ClusterLoadAssignment, Endpoint, LbEndpoint, LocalityLbEndpoints,
};
use envoy_types::pb::envoy::config::listener::v3::Listener;
use envoy_types::pb::envoy::config::route::v3::RouteConfiguration;
use envoy_types::pb::envoy::extensions::transport_sockets::tls::v3::{
    CommonTlsContext, UpstreamTlsContext,
};
use envoy_types::pb::envoy::r#type::v3::Int64Range;
use envoy_types::pb::google::protobuf::{Any, Duration, UInt32Value, UInt64Value};
use prost::Message;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::xds::{filters::http::build_http_filters, listener::ListenerConfig, route::RouteConfig};

pub const CLUSTER_TYPE_URL: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
pub const ROUTE_TYPE_URL: &str = "type.googleapis.com/envoy.config.route.v3.RouteConfiguration";
pub const LISTENER_TYPE_URL: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";
pub const HTTP_PROTOCOL_OPTIONS_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions";
pub const PLATFORM_ROUTE_PREFIX: &str = "platform-api";

fn strip_gateway_tags(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("flowplaneGateway");
            for child in map.values_mut() {
                strip_gateway_tags(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_gateway_tags(item);
            }
        }
        _ => {}
    }
}

/// Wrapper for a built Envoy resource along with its name.
#[derive(Clone, Debug)]
pub struct BuiltResource {
    pub name: String,
    pub resource: Any,
}

impl BuiltResource {
    pub fn into_any(self) -> Any {
        self.resource
    }

    pub fn type_url(&self) -> &str {
        &self.resource.type_url
    }
}

/// Build cluster resources using the static configuration
pub fn clusters_from_config(config: &SimpleXdsConfig) -> Result<Vec<BuiltResource>> {
    let resources = &config.resources;
    let cluster = Cluster {
        name: resources.cluster_name.clone(),
        connect_timeout: Some(Duration {
            seconds: 5,
            nanos: 0,
        }),
        load_assignment: Some(ClusterLoadAssignment {
            cluster_name: resources.cluster_name.clone(),
            endpoints: vec![LocalityLbEndpoints {
                lb_endpoints: vec![LbEndpoint {
                    host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                        address: Some(Address {
                            address: Some(
                                envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                    SocketAddress {
                                        address: resources.backend_address.clone(),
                                        port_specifier: Some(socket_address::PortSpecifier::PortValue(
                                            resources.backend_port.into(),
                                        )),
                                        protocol: 0,
                                        ..Default::default()
                                    },
                                ),
                            ),
                        }),
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };

    let encoded = cluster.encode_to_vec();
    info!(
        resource = %cluster.name,
        bytes = encoded.len(),
        "Created cluster resource"
    );

    Ok(vec![BuiltResource {
        name: cluster.name.clone(),
        resource: Any { type_url: CLUSTER_TYPE_URL.to_string(), value: encoded },
    }])
}

/// Build resources derived from Platform API definitions and their routes.
pub fn resources_from_api_definitions(
    definitions: Vec<ApiDefinitionData>,
    routes: Vec<ApiRouteData>,
) -> Result<Vec<BuiltResource>> {
    let mut routes_by_definition: HashMap<String, Vec<ApiRouteData>> = HashMap::new();
    for route in routes {
        routes_by_definition.entry(route.api_definition_id.to_string()).or_default().push(route);
    }

    let mut built_resources = Vec::new();
    let mut cluster_resources = Vec::new();

    for definition in definitions {
        let definition_routes = match routes_by_definition.remove(&definition.id.to_string()) {
            Some(mut list) => {
                list.sort_by_key(|r| r.route_order);
                list
            }
            None => continue,
        };

        if definition_routes.is_empty() {
            continue;
        }

        // Skip route config generation for listenerIsolation=false (routes are merged into shared listeners)
        if !definition.listener_isolation {
            // Still need to generate clusters for these routes
            for route in definition_routes {
                let targets = parse_upstream_targets(&route)?;
                if targets.is_empty() {
                    continue;
                }

                let cluster_name = build_cluster_name(definition.id.as_str(), route.id.as_str());
                let cluster_resource = build_platform_cluster(&cluster_name, &targets)?;
                cluster_resources.push(cluster_resource);
            }
            continue;
        }

        let mut virtual_host = crate::xds::route::VirtualHostConfig {
            name: format!("{}-vhost", short_id(definition.id.as_str())),
            domains: vec![definition.domain.clone()],
            routes: Vec::with_capacity(definition_routes.len()),
            typed_per_filter_config: HashMap::new(),
        };

        for route in definition_routes {
            let targets = parse_upstream_targets(&route)?;
            if targets.is_empty() {
                continue;
            }

            let cluster_name = build_cluster_name(definition.id.as_str(), route.id.as_str());
            let cluster_resource = build_platform_cluster(&cluster_name, &targets)?;
            cluster_resources.push(cluster_resource);

            let action = crate::xds::route::RouteActionConfig::Cluster {
                name: cluster_name.clone(),
                timeout: route.timeout_seconds.map(|t| t as u64),
                prefix_rewrite: route.rewrite_prefix.clone(),
                path_template_rewrite: route.rewrite_regex.clone(),
            };

            let path_match = match route.match_type.to_lowercase().as_str() {
                "prefix" => crate::xds::route::PathMatch::Prefix(route.match_value.clone()),
                "path" | "exact" => crate::xds::route::PathMatch::Exact(route.match_value.clone()),
                "template" => crate::xds::route::PathMatch::Template(route.match_value.clone()),
                other => {
                    return Err(Error::validation(format!(
                        "Unsupported route match type '{}'",
                        other
                    )));
                }
            };

            if route.rewrite_regex.is_some() || route.rewrite_substitution.is_some() {
                warn!(
                    route = %route.id,
                    "Regex-based rewrites are not yet supported for Platform API routes; ignoring configuration"
                );
            }

            virtual_host.routes.push(crate::xds::route::RouteRule {
                name: Some(format!("{}-{}", PLATFORM_ROUTE_PREFIX, short_id(route.id.as_str()))),
                r#match: crate::xds::route::RouteMatchConfig {
                    path: path_match,
                    headers: None,
                    query_parameters: None,
                },
                action,
                typed_per_filter_config: typed_per_filter_config(&route.override_config)?,
            });
        }

        if virtual_host.routes.is_empty() {
            continue;
        }

        let route_config_name =
            format!("{}-{}", PLATFORM_ROUTE_PREFIX, short_id(definition.id.as_str()));
        tracing::debug!(
            route_config_name = %route_config_name,
            definition_id = %definition.id,
            num_routes = virtual_host.routes.len(),
            "resources_from_api_definitions: Creating Platform API route config"
        );
        let route_config = crate::xds::route::RouteConfig {
            name: route_config_name.clone(),
            virtual_hosts: vec![virtual_host],
        };

        let envoy_route = route_config.to_envoy_route_configuration()?;
        built_resources.push(BuiltResource {
            name: route_config_name.clone(),
            resource: Any {
                type_url: ROUTE_TYPE_URL.to_string(),
                value: envoy_route.encode_to_vec(),
            },
        });

        // NOTE: Listeners for Platform API definitions are created in the database by
        // materialize_isolated_listener() and loaded via refresh_listeners_from_repository().
        // We don't create listeners here to avoid duplicates.

        // NOTE: Clusters for Platform API definitions are created in the database by
        // materialize_native_resources() and loaded via refresh_clusters_from_repository().
        // We include them here for dynamic generation, but refresh_platform_api_resources()
        // will NOT send them to Envoy to avoid duplicates (they're sent by refresh_clusters_from_repository()).
    }

    built_resources.extend(cluster_resources);
    Ok(built_resources)
}

#[derive(Debug)]
struct ParsedTarget {
    host: String,
    port: u16,
    weight: Option<u32>,
}

fn parse_upstream_targets(route: &ApiRouteData) -> Result<Vec<ParsedTarget>> {
    #[derive(Deserialize)]
    struct StoredTargets {
        targets: Vec<StoredTarget>,
    }

    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct StoredTarget {
        name: Option<String>,
        endpoint: String,
        weight: Option<u32>,
    }

    let stored: StoredTargets =
        serde_json::from_value(route.upstream_targets.clone()).map_err(|err| {
            Error::internal(format!(
                "Failed to parse stored upstream targets for route {}: {}",
                route.id, err
            ))
        })?;

    if stored.targets.is_empty() {
        return Err(Error::internal(format!(
            "Route '{}' does not contain any upstream targets",
            route.id
        )));
    }

    let mut parsed = Vec::with_capacity(stored.targets.len());
    for target in stored.targets {
        let (host, port) = split_host_port(&target.endpoint)?;
        parsed.push(ParsedTarget { host, port, weight: target.weight });
    }

    Ok(parsed)
}

fn split_host_port(endpoint: &str) -> Result<(String, u16)> {
    let (host, port_str) = endpoint.rsplit_once(':').ok_or_else(|| {
        Error::internal(format!("Invalid upstream endpoint '{}': expected host:port", endpoint))
    })?;
    let port: u16 = port_str.parse().map_err(|_| {
        Error::internal(format!("Invalid upstream endpoint '{}': port must be numeric", endpoint))
    })?;
    Ok((host.to_string(), port))
}

fn build_cluster_name(definition_id: &str, route_id: &str) -> String {
    format!("platform-{}-{}", short_id(definition_id), short_id(route_id))
}

fn short_id(id: &str) -> String {
    // Preserve a slightly longer slice to keep route suffixes distinctive while remaining readable.
    let candidate: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
    if candidate.is_empty() {
        "platform".into()
    } else {
        candidate.to_lowercase()
    }
}

fn build_platform_cluster(cluster_name: &str, targets: &[ParsedTarget]) -> Result<BuiltResource> {
    let mut has_hostname = false;
    let mut first_hostname: Option<String> = None;
    let mut tls_candidate_host: Option<String> = None;
    let mut has_tls_port = false;

    let lb_endpoints: Vec<LbEndpoint> = targets
        .iter()
        .map(|target| {
            // Track TLS and hostname hints
            if target.host.parse::<std::net::IpAddr>().is_err() {
                has_hostname = true;
                if first_hostname.is_none() {
                    first_hostname = Some(target.host.clone());
                }
            }
            if target.port == 443 {
                has_tls_port = true;
                if tls_candidate_host.is_none() && target.host.parse::<std::net::IpAddr>().is_err()
                {
                    tls_candidate_host = Some(target.host.clone());
                }
            }

            let socket = SocketAddress {
                address: target.host.clone(),
                port_specifier: Some(socket_address::PortSpecifier::PortValue(target.port as u32)),
                protocol: Protocol::Tcp as i32,
                ..Default::default()
            };
            let address = Address {
                address: Some(
                    envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                        socket,
                    ),
                ),
            };
            let endpoint = Endpoint { address: Some(address), ..Default::default() };
            let mut lb_endpoint = LbEndpoint {
                host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(endpoint)),
                ..Default::default()
            };
            if let Some(weight) = target.weight {
                lb_endpoint.load_balancing_weight = Some(UInt32Value { value: weight });
            }
            Ok::<LbEndpoint, Error>(lb_endpoint)
        })
        .collect::<Result<Vec<_>>>()?;

    let locality = LocalityLbEndpoints { lb_endpoints, ..Default::default() };
    let mut cluster = Cluster {
        name: cluster_name.to_string(),
        connect_timeout: Some(Duration { seconds: 5, nanos: 0 }),
        cluster_discovery_type: Some(ClusterDiscoveryType::Type(DiscoveryType::StrictDns as i32)),
        load_assignment: Some(ClusterLoadAssignment {
            cluster_name: cluster_name.to_string(),
            endpoints: vec![locality],
            ..Default::default()
        }),
        ..Default::default()
    };

    // Infer TLS for HTTPS upstreams (hostname + port 443)
    if has_tls_port {
        let mut tls_context = UpstreamTlsContext {
            common_tls_context: Some(CommonTlsContext::default()),
            ..Default::default()
        };

        // Determine SNI: prefer hostname on 443, then first hostname if any
        let sni = tls_candidate_host.or_else(|| first_hostname.clone());
        if let Some(server_name) = sni {
            tls_context.sni = server_name;
        } else if has_hostname {
            // Hostname present but none chosen; warn at build time (best-effort)
            warn!(cluster = %cluster_name, "TLS inferred but no SNI hostname resolved; upstream verification may fail");
        }

        let tls_any = Any {
            type_url:
                "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext"
                    .to_string(),
            value: tls_context.encode_to_vec(),
        };

        cluster.transport_socket = Some(TransportSocket {
            name: "envoy.transport_sockets.tls".to_string(),
            config_type: Some(TransportSocketConfigType::TypedConfig(tls_any)),
        });
    }

    let encoded = cluster.encode_to_vec();
    Ok(BuiltResource {
        name: cluster.name.clone(),
        resource: Any { type_url: CLUSTER_TYPE_URL.to_string(), value: encoded },
    })
}

/// Build route configuration resources from the static configuration
pub fn routes_from_config(config: &SimpleXdsConfig) -> Result<Vec<BuiltResource>> {
    let resources = &config.resources;
    let route_config = RouteConfiguration {
        name: resources.route_name.clone(),
        virtual_hosts: vec![envoy_types::pb::envoy::config::route::v3::VirtualHost {
            name: "test_virtual_host".to_string(),
            domains: vec!["*".to_string()],
            routes: vec![envoy_types::pb::envoy::config::route::v3::Route {
                name: "test_route_match".to_string(),
                r#match: Some(
                    envoy_types::pb::envoy::config::route::v3::RouteMatch {
                        path_specifier: Some(
                            envoy_types::pb::envoy::config::route::v3::route_match::PathSpecifier::Prefix(
                                "/".to_string(),
                            ),
                        ),
                        ..Default::default()
                    },
                ),
                action: Some(
                    envoy_types::pb::envoy::config::route::v3::route::Action::Route(
                        envoy_types::pb::envoy::config::route::v3::RouteAction {
                            cluster_specifier: Some(
                                envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::Cluster(
                                    resources.cluster_name.clone(),
                                ),
                            ),
                            ..Default::default()
                        },
                    ),
                ),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };

    let encoded = route_config.encode_to_vec();
    info!(resource = %route_config.name, bytes = encoded.len(), "Created route resource");

    Ok(vec![BuiltResource {
        name: route_config.name.clone(),
        resource: Any { type_url: ROUTE_TYPE_URL.to_string(), value: encoded },
    }])
}

/// Build route configuration resources from database entries
pub fn routes_from_database_entries(
    routes: Vec<RouteData>,
    phase: &str,
) -> Result<Vec<BuiltResource>> {
    let mut built = Vec::with_capacity(routes.len());
    let mut total_bytes = 0;

    for route_row in routes {
        let mut value: Value = serde_json::from_str(&route_row.configuration).map_err(|err| {
            Error::internal(format!(
                "Failed to parse stored route configuration for '{}': {}",
                route_row.name, err
            ))
        })?;

        strip_gateway_tags(&mut value);

        let route_config: RouteConfig = serde_json::from_value(value).map_err(|err| {
            Error::internal(format!(
                "Failed to deserialize route configuration for '{}': {}",
                route_row.name, err
            ))
        })?;

        let envoy_route = route_config.to_envoy_route_configuration()?;
        let encoded = envoy_route.encode_to_vec();

        debug!(
            phase,
            resource = %envoy_route.name,
            bytes = encoded.len(),
            "Built route configuration from repository"
        );

        total_bytes += encoded.len();
        built.push(BuiltResource {
            name: envoy_route.name.clone(),
            resource: Any { type_url: ROUTE_TYPE_URL.to_string(), value: encoded },
        });
    }

    if !built.is_empty() {
        debug!(
            phase,
            route_count = built.len(),
            total_bytes,
            "Built route configurations from repository"
        );
    }

    Ok(built)
}

/// Build listener resources from the static configuration
pub fn listeners_from_config(config: &SimpleXdsConfig) -> Result<Vec<BuiltResource>> {
    use envoy_types::pb::envoy::config::listener::v3::Filter;
    use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpConnectionManager;

    let resources = &config.resources;

    let http_filters = build_http_filters(&[])?;

    let http_conn_manager = HttpConnectionManager {
        stat_prefix: "ingress_http".to_string(),
        route_specifier: Some(
            envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::RouteSpecifier::Rds(
                envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::Rds {
                    config_source: Some(envoy_types::pb::envoy::config::core::v3::ConfigSource {
                        config_source_specifier: Some(
                            envoy_types::pb::envoy::config::core::v3::config_source::ConfigSourceSpecifier::Ads(
                                envoy_types::pb::envoy::config::core::v3::AggregatedConfigSource::default(),
                            ),
                        ),
                        ..Default::default()
                    }),
                    route_config_name: resources.route_name.clone(),
                },
            ),
        ),
        http_filters,
        ..Default::default()
    };

    let listener = Listener {
        name: resources.listener_name.clone(),
        address: Some(Address {
            address: Some(
                envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                    SocketAddress {
                        address: "0.0.0.0".to_string(),
                        port_specifier: Some(socket_address::PortSpecifier::PortValue(
                            resources.listener_port.into(),
                        )),
                        protocol: 0,
                        ..Default::default()
                    },
                ),
            ),
        }),
        filter_chains: vec![envoy_types::pb::envoy::config::listener::v3::FilterChain {
            filters: vec![Filter {
                name: "envoy.filters.network.http_connection_manager".to_string(),
                config_type: Some(
                    envoy_types::pb::envoy::config::listener::v3::filter::ConfigType::TypedConfig(
                        Any {
                            type_url: "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager"
                                .to_string(),
                            value: http_conn_manager.encode_to_vec(),
                        },
                    ),
                ),
            }],
            ..Default::default()
        }],
        ..Default::default()
    };

    let encoded = listener.encode_to_vec();
    info!(resource = %listener.name, bytes = encoded.len(), "Created listener resource");

    Ok(vec![BuiltResource {
        name: listener.name.clone(),
        resource: Any { type_url: LISTENER_TYPE_URL.to_string(), value: encoded },
    }])
}

/// Build listener resources from database entries
pub fn listeners_from_database_entries(
    entries: Vec<ListenerData>,
    context: &str,
) -> Result<Vec<BuiltResource>> {
    let mut resources = Vec::with_capacity(entries.len());
    let mut total_bytes = 0;

    for entry in entries {
        let mut value: Value = serde_json::from_str(&entry.configuration).map_err(|err| {
            Error::internal(format!(
                "Failed to parse stored listener configuration for '{}': {}",
                entry.name, err
            ))
        })?;

        strip_gateway_tags(&mut value);

        let config: ListenerConfig = serde_json::from_value(value).map_err(|err| {
            Error::internal(format!(
                "Failed to deserialize listener configuration for '{}': {}",
                entry.name, err
            ))
        })?;

        let envoy_listener = config.to_envoy_listener()?;
        let encoded = envoy_listener.encode_to_vec();

        debug!(
            phase = context,
            listener_name = %entry.name,
            protocol = %entry.protocol,
            version = entry.version,
            encoded_size = encoded.len(),
            "Built listener resource from database entry"
        );

        total_bytes += encoded.len();
        resources.push(BuiltResource {
            name: envoy_listener.name.clone(),
            resource: Any { type_url: LISTENER_TYPE_URL.to_string(), value: encoded },
        });
    }

    if !resources.is_empty() {
        debug!(
            phase = context,
            listener_count = resources.len(),
            total_bytes,
            "Built listener resources from database"
        );
    }

    Ok(resources)
}

/// Build cluster resources from database entries (JSON stored in `ClusterData`).
pub fn clusters_from_database_entries(
    entries: Vec<ClusterData>,
    context: &str,
) -> Result<Vec<BuiltResource>> {
    let mut resources = Vec::new();
    let mut total_bytes = 0;

    for entry in entries {
        let raw_config: Value = serde_json::from_str(&entry.configuration).map_err(|e| {
            Error::config(format!("Invalid cluster configuration JSON for '{}': {}", entry.name, e))
        })?;

        let spec = ClusterSpec::from_value(raw_config.clone())?;
        let cluster = cluster_from_spec(&entry.name, &spec)?;
        let encoded = cluster.encode_to_vec();

        debug!(
            phase = context,
            cluster_name = %entry.name,
            service_name = %entry.service_name,
            version = entry.version,
            encoded_size = encoded.len(),
            "Built cluster resource from database entry"
        );

        total_bytes += encoded.len();
        resources.push(BuiltResource {
            name: entry.name,
            resource: Any { type_url: CLUSTER_TYPE_URL.to_string(), value: encoded },
        });
    }

    if !resources.is_empty() {
        debug!(
            phase = context,
            cluster_count = resources.len(),
            total_bytes,
            "Built cluster resources from database"
        );
    }

    Ok(resources)
}

fn cluster_from_spec(name: &str, spec: &ClusterSpec) -> Result<Cluster> {
    let mut lb_endpoints = Vec::new();
    let mut has_hostname = false;
    let mut first_hostname: Option<String> = None;
    let mut tls_candidate_host: Option<String> = None;
    let mut has_tls_port = false;

    for endpoint in &spec.endpoints {
        let (host, port) = endpoint.host_port_or_error()?;
        let is_ip = host.parse::<IpAddr>().is_ok();
        if !is_ip {
            has_hostname = true;
            if first_hostname.is_none() {
                first_hostname = Some(host.clone());
            }
        }

        if port == 443 {
            has_tls_port = true;
            if !is_ip && tls_candidate_host.is_none() {
                tls_candidate_host = Some(host.clone());
            }
        }

        lb_endpoints.push(LbEndpoint {
            host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                address: Some(Address {
                    address: Some(
                        envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                            SocketAddress {
                                address: host,
                                port_specifier: Some(socket_address::PortSpecifier::PortValue(
                                    port,
                                )),
                                protocol: Protocol::Tcp as i32,
                                ..Default::default()
                            },
                        ),
                    ),
                }),
                ..Default::default()
            })),
            ..Default::default()
        });
    }

    if lb_endpoints.is_empty() {
        return Err(Error::config("No valid endpoints found in cluster configuration".to_string()));
    }

    let connect_timeout = spec.connect_timeout_seconds.unwrap_or(5);
    let mut cluster = Cluster {
        name: name.to_string(),
        connect_timeout: Some(seconds_to_duration(connect_timeout)),
        load_assignment: Some(ClusterLoadAssignment {
            cluster_name: name.to_string(),
            endpoints: vec![LocalityLbEndpoints { lb_endpoints, ..Default::default() }],
            ..Default::default()
        }),
        ..Default::default()
    };

    let (lb_policy, lb_config) = map_lb_policy(name, spec);
    cluster.lb_policy = lb_policy;
    if let Some(config) = lb_config {
        cluster.lb_config = Some(config);
    }

    if has_hostname {
        let endpoint_count = cluster
            .load_assignment
            .as_ref()
            .and_then(|assignment| assignment.endpoints.first())
            .map(|locality| locality.lb_endpoints.len())
            .unwrap_or_default();

        cluster.cluster_discovery_type = Some(if endpoint_count <= 1 {
            ClusterDiscoveryType::Type(DiscoveryType::LogicalDns as i32)
        } else {
            ClusterDiscoveryType::Type(DiscoveryType::StrictDns as i32)
        });

        if let Some(family) = map_dns_lookup_family(name, spec.dns_lookup_family.as_deref()) {
            cluster.dns_lookup_family = family;
        }
    } else {
        cluster.cluster_discovery_type =
            Some(ClusterDiscoveryType::Type(DiscoveryType::Static as i32));
    }

    let mut use_tls = spec.use_tls();
    if !use_tls && has_tls_port {
        use_tls = true;
    }

    let mut sni = spec
        .tls_server_name
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    if sni.is_none() {
        if let Some(host) = first_hostname.clone() {
            sni = Some(host);
        } else if let Some(host) = tls_candidate_host.clone() {
            sni = Some(host);
        }
    }

    if use_tls {
        let mut tls_context = UpstreamTlsContext {
            common_tls_context: Some(CommonTlsContext::default()),
            ..Default::default()
        };

        if let Some(ref server_name) = sni {
            tls_context.sni = server_name.clone();
        } else if has_hostname {
            warn!(
                cluster = %name,
                "TLS enabled but no SNI server name provided; upstream certificate verification may fail"
            );
        }

        let tls_any = Any {
            type_url:
                "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext"
                    .to_string(),
            value: tls_context.encode_to_vec(),
        };

        cluster.transport_socket = Some(TransportSocket {
            name: "envoy.transport_sockets.tls".to_string(),
            config_type: Some(TransportSocketConfigType::TypedConfig(tls_any)),
        });
    }

    if let Some(cb) = &spec.circuit_breakers {
        let built = build_circuit_breakers(cb);
        if !built.thresholds.is_empty() {
            cluster.circuit_breakers = Some(built);
        }
    }

    if !spec.health_checks.is_empty() {
        let mut checks = Vec::new();
        for check in &spec.health_checks {
            checks.push(build_health_check(name, check)?);
        }
        cluster.health_checks = checks;
    }

    if let Some(outlier) = &spec.outlier_detection {
        cluster.outlier_detection = Some(build_outlier_detection(outlier));
    }

    Ok(cluster)
}

fn map_lb_policy(name: &str, spec: &ClusterSpec) -> (i32, Option<cluster::LbConfig>) {
    let default_policy = LbPolicy::RoundRobin as i32;
    let policy = match spec.lb_policy.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(value) => value.to_uppercase(),
        None => return (default_policy, None),
    };

    match policy.as_str() {
        "ROUND_ROBIN" => (LbPolicy::RoundRobin as i32, None),
        "LEAST_REQUEST" => {
            let lb_config = spec.least_request.as_ref().map(|cfg| {
                let mut config = LeastRequestLbConfig::default();
                if let Some(choice) = cfg.choice_count {
                    config.choice_count = Some(UInt32Value { value: choice });
                }
                cluster::LbConfig::LeastRequestLbConfig(config)
            });
            (LbPolicy::LeastRequest as i32, lb_config)
        }
        "RING_HASH" => {
            let lb_config = spec.ring_hash.as_ref().map(|cfg| {
                let mut config = RingHashLbConfig::default();
                if let Some(min) = cfg.minimum_ring_size {
                    config.minimum_ring_size = uint64(Some(min));
                }
                if let Some(max) = cfg.maximum_ring_size {
                    config.maximum_ring_size = uint64(Some(max));
                }
                if let Some(function) = cfg.hash_function.as_ref() {
                    let hash_enum = match function.to_uppercase().as_str() {
                        "XX_HASH" | "XXHASH" => Some(RingHashFunction::XxHash as i32),
                        "MURMUR_HASH_2" | "MURMURHASH2" | "MURMUR_HASH2" => {
                            Some(RingHashFunction::MurmurHash2 as i32)
                        }
                        other => {
                            warn!(cluster = %name, hash = other, "Unknown ring hash function; defaulting to xxHash");
                            None
                        }
                    };
                    if let Some(hash_enum) = hash_enum {
                        config.hash_function = hash_enum;
                    }
                }
                cluster::LbConfig::RingHashLbConfig(config)
            });
            (LbPolicy::RingHash as i32, lb_config)
        }
        "MAGLEV" => {
            let lb_config = spec.maglev.as_ref().map(|cfg| {
                let mut config = MaglevLbConfig::default();
                if let Some(size) = cfg.table_size {
                    config.table_size = uint64(Some(size));
                }
                cluster::LbConfig::MaglevLbConfig(config)
            });
            (LbPolicy::Maglev as i32, lb_config)
        }
        "RANDOM" => (LbPolicy::Random as i32, None),
        "CLUSTER_PROVIDED" => (LbPolicy::ClusterProvided as i32, None),
        other => {
            warn!(cluster = %name, policy = other, "Unknown lb_policy value; defaulting to round robin");
            (default_policy, None)
        }
    }
}

fn map_dns_lookup_family(cluster: &str, value: Option<&str>) -> Option<i32> {
    value.map(|family| match family.to_uppercase().as_str() {
        "AUTO" => DnsLookupFamily::Auto as i32,
        "V4_ONLY" | "V4ONLY" => DnsLookupFamily::V4Only as i32,
        "V6_ONLY" | "V6ONLY" => DnsLookupFamily::V6Only as i32,
        "V4_PREFERRED" | "V4PREFERRED" => DnsLookupFamily::V4Preferred as i32,
        "ALL" => DnsLookupFamily::All as i32,
        other => {
            warn!(cluster = %cluster, family = other, "Unknown dns_lookup_family value; defaulting to AUTO");
            DnsLookupFamily::Auto as i32
        }
    })
}

fn seconds_to_duration(value: u64) -> Duration {
    Duration { seconds: value as i64, nanos: 0 }
}

fn optional_duration(value: Option<u64>) -> Option<Duration> {
    value.map(seconds_to_duration)
}

fn uint32(value: Option<u32>) -> Option<UInt32Value> {
    value.map(|v| UInt32Value { value: v })
}

fn uint64(value: Option<u64>) -> Option<UInt64Value> {
    value.map(|v| UInt64Value { value: v })
}

fn build_circuit_breakers(spec: &CircuitBreakersSpec) -> CircuitBreakers {
    let mut thresholds = Vec::new();

    if let Some(default) = &spec.default {
        thresholds.push(build_threshold(default, RoutingPriority::Default));
    }

    if let Some(high) = &spec.high {
        thresholds.push(build_threshold(high, RoutingPriority::High));
    }

    CircuitBreakers { thresholds, ..Default::default() }
}

fn build_threshold(
    spec: &CircuitBreakerThresholdsSpec,
    priority: RoutingPriority,
) -> CircuitThresholds {
    CircuitThresholds {
        priority: priority as i32,
        max_connections: uint32(spec.max_connections),
        max_pending_requests: uint32(spec.max_pending_requests),
        max_requests: uint32(spec.max_requests),
        max_retries: uint32(spec.max_retries),
        ..Default::default()
    }
}

fn build_health_check(cluster: &str, spec: &HealthCheckSpec) -> Result<HealthCheck> {
    let mut check = HealthCheck {
        timeout: optional_duration(match spec {
            HealthCheckSpec::Http { timeout_seconds, .. } => *timeout_seconds,
            HealthCheckSpec::Tcp { timeout_seconds, .. } => *timeout_seconds,
        })
        .or_else(|| Some(seconds_to_duration(5))),
        interval: optional_duration(match spec {
            HealthCheckSpec::Http { interval_seconds, .. } => *interval_seconds,
            HealthCheckSpec::Tcp { interval_seconds, .. } => *interval_seconds,
        })
        .or_else(|| Some(seconds_to_duration(10))),
        healthy_threshold: uint32(match spec {
            HealthCheckSpec::Http { healthy_threshold, .. } => *healthy_threshold,
            HealthCheckSpec::Tcp { healthy_threshold, .. } => *healthy_threshold,
        }),
        unhealthy_threshold: uint32(match spec {
            HealthCheckSpec::Http { unhealthy_threshold, .. } => *unhealthy_threshold,
            HealthCheckSpec::Tcp { unhealthy_threshold, .. } => *unhealthy_threshold,
        }),
        ..Default::default()
    };

    match spec {
        HealthCheckSpec::Http { path, host, method, expected_statuses, .. } => {
            let mut http_check = HttpHealthCheck {
                host: host.clone().unwrap_or_default(),
                path: path.clone(),
                ..Default::default()
            };

            if let Some(statuses) = expected_statuses {
                http_check.expected_statuses = statuses
                    .iter()
                    .map(|code| Int64Range { start: *code as i64, end: (*code as i64) + 1 })
                    .collect();
            }

            if let Some(request_method) = method.as_deref().and_then(request_method_from_str) {
                http_check.method = request_method as i32;
            }

            check.health_checker = Some(health_check::HealthChecker::HttpHealthCheck(http_check));
        }
        HealthCheckSpec::Tcp { .. } => {
            check.health_checker =
                Some(health_check::HealthChecker::TcpHealthCheck(TcpHealthCheck::default()));
        }
    }

    if check.health_checker.is_none() {
        return Err(Error::config(format!(
            "Unsupported health check configuration for cluster '{}'",
            cluster
        )));
    }

    Ok(check)
}

fn request_method_from_str(method: &str) -> Option<RequestMethod> {
    match method.to_uppercase().as_str() {
        "GET" => Some(RequestMethod::Get),
        "HEAD" => Some(RequestMethod::Head),
        "POST" => Some(RequestMethod::Post),
        "PUT" => Some(RequestMethod::Put),
        "DELETE" => Some(RequestMethod::Delete),
        "OPTIONS" => Some(RequestMethod::Options),
        "TRACE" => Some(RequestMethod::Trace),
        "PATCH" => Some(RequestMethod::Patch),
        _ => None,
    }
}

fn build_outlier_detection(spec: &OutlierDetectionSpec) -> OutlierDetection {
    OutlierDetection {
        consecutive_5xx: uint32(spec.consecutive_5xx),
        interval: optional_duration(spec.interval_seconds),
        base_ejection_time: optional_duration(spec.base_ejection_time_seconds),
        max_ejection_percent: uint32(spec.max_ejection_percent),
        ..Default::default()
    }
}

/// Build endpoint resources from the static configuration
pub fn endpoints_from_config(config: &SimpleXdsConfig) -> Result<Vec<BuiltResource>> {
    let resources = &config.resources;
    let cluster_load_assignment = ClusterLoadAssignment {
        cluster_name: resources.cluster_name.clone(),
        endpoints: vec![LocalityLbEndpoints {
            lb_endpoints: vec![LbEndpoint {
                host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                    address: Some(Address {
                        address: Some(
                            envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                SocketAddress {
                                    address: resources.backend_address.clone(),
                                    port_specifier: Some(
                                        socket_address::PortSpecifier::PortValue(
                                            resources.backend_port.into(),
                                        ),
                                    ),
                                    protocol: 0,
                                    ..Default::default()
                                },
                            ),
                        ),
                    }),
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };

    let encoded = cluster_load_assignment.encode_to_vec();
    info!(
        resource = %cluster_load_assignment.cluster_name,
        bytes = encoded.len(),
        "Created endpoint resources"
    );

    Ok(vec![BuiltResource {
        name: cluster_load_assignment.cluster_name.clone(),
        resource: Any {
            type_url: "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment"
                .to_string(),
            value: encoded,
        },
    }])
}

/// Helper function to create HTTP/2 protocol options using the non-deprecated approach.
///
/// Creates typed_extension_protocol_options with HttpProtocolOptions configured for HTTP/2.
/// This is the modern replacement for the deprecated `http2_protocol_options` field.
fn create_http2_typed_extension_protocol_options() -> Result<HashMap<String, Any>> {
    let http_protocol_options = UpstreamHttpProtocolOptionsV3 {
        upstream_protocol_options: Some(UpstreamProtocolOptions::ExplicitHttpConfig(
            ExplicitHttpConfig {
                protocol_config: Some(ProtocolConfig::Http2ProtocolOptions(
                    Http2ProtocolOptions::default(),
                )),
            },
        )),
        ..Default::default()
    };

    let encoded = http_protocol_options.encode_to_vec();
    let any = Any { type_url: HTTP_PROTOCOL_OPTIONS_TYPE_URL.to_string(), value: encoded };

    let mut options = HashMap::new();
    options.insert("envoy.extensions.upstreams.http.v3.HttpProtocolOptions".to_string(), any);

    Ok(options)
}

/// Create built-in internal cluster for ExtProc gRPC service
///
/// This cluster enables Envoy to route External Processor filter requests
/// to the FlowplaneExtProcService running on the xDS server.
pub fn create_ext_proc_cluster(xds_bind_address: &str, xds_port: u16) -> Result<BuiltResource> {
    // Envoy cannot connect to 0.0.0.0; map to loopback for in-process access
    let target_address = if xds_bind_address == "0.0.0.0" { "127.0.0.1" } else { xds_bind_address };

    let cluster = Cluster {
        name: "flowplane_ext_proc_service".to_string(),
        connect_timeout: Some(Duration { seconds: 5, nanos: 0 }),
        // Use STATIC discovery for localhost
        cluster_discovery_type: Some(ClusterDiscoveryType::Type(DiscoveryType::Static as i32)),
        // Configure HTTP/2 for gRPC communication using modern typed extension approach
        typed_extension_protocol_options: create_http2_typed_extension_protocol_options()?,
        load_assignment: Some(ClusterLoadAssignment {
            cluster_name: "flowplane_ext_proc_service".to_string(),
            endpoints: vec![LocalityLbEndpoints {
                lb_endpoints: vec![LbEndpoint {
                    host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                        address: Some(Address {
                            address: Some(
                                envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                    SocketAddress {
                                        address: target_address.to_string(),
                                        port_specifier: Some(socket_address::PortSpecifier::PortValue(
                                            xds_port.into(),
                                        )),
                                        ..Default::default()
                                    },
                                ),
                            ),
                        }),
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };

    let encoded = cluster.encode_to_vec();
    Ok(BuiltResource {
        name: "flowplane_ext_proc_service".to_string(),
        resource: Any { type_url: CLUSTER_TYPE_URL.to_string(), value: encoded },
    })
}

/// Create built-in internal cluster for Access Log gRPC service
///
/// This cluster enables Envoy to send HTTP access logs to the
/// FlowplaneAccessLogService running on the same xDS server.
pub fn create_access_log_cluster(xds_bind_address: &str, xds_port: u16) -> Result<BuiltResource> {
    // Envoy cannot connect to 0.0.0.0; map to loopback for in-process access
    let target_address = if xds_bind_address == "0.0.0.0" { "127.0.0.1" } else { xds_bind_address };

    let cluster = Cluster {
        name: "flowplane_access_log_service".to_string(),
        connect_timeout: Some(Duration { seconds: 5, nanos: 0 }),
        // Use STATIC discovery for localhost
        cluster_discovery_type: Some(ClusterDiscoveryType::Type(DiscoveryType::Static as i32)),
        // Configure HTTP/2 for gRPC communication using modern typed extension approach
        typed_extension_protocol_options: create_http2_typed_extension_protocol_options()?,
        load_assignment: Some(ClusterLoadAssignment {
            cluster_name: "flowplane_access_log_service".to_string(),
            endpoints: vec![LocalityLbEndpoints {
                lb_endpoints: vec![LbEndpoint {
                    host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                        address: Some(Address {
                            address: Some(
                                envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                    SocketAddress {
                                        address: target_address.to_string(),
                                        port_specifier: Some(socket_address::PortSpecifier::PortValue(
                                            xds_port.into(),
                                        )),
                                        ..Default::default()
                                    },
                                ),
                            ),
                        }),
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };

    let encoded = cluster.encode_to_vec();
    Ok(BuiltResource {
        name: "flowplane_access_log_service".to_string(),
        resource: Any { type_url: CLUSTER_TYPE_URL.to_string(), value: encoded },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_api::filter_overrides::canonicalize_filter_overrides;
    use crate::storage::{ApiDefinitionData, ApiRouteData, ListenerData};
    use crate::xds::filters::http::cors::FILTER_CORS_POLICY_TYPE_URL;
    use crate::xds::{
        listener::{FilterChainConfig, FilterConfig, FilterType, ListenerConfig},
        CircuitBreakerThresholdsSpec, CircuitBreakersSpec, ClusterSpec, EndpointSpec,
        HealthCheckSpec, LeastRequestPolicy, MaglevPolicy, OutlierDetectionSpec, RingHashPolicy,
    };
    use chrono::Utc;
    use envoy_types::pb::envoy::config::cluster::v3::cluster::LbConfig;
    use envoy_types::pb::envoy::extensions::transport_sockets::tls::v3::UpstreamTlsContext;
    use prost::Message;
    use serde_json::json;

    fn decode_tls_context(cluster: &Cluster) -> UpstreamTlsContext {
        let transport_socket = cluster.transport_socket.as_ref().expect("transport socket");
        let typed = match transport_socket.config_type.as_ref() {
            Some(TransportSocketConfigType::TypedConfig(any)) => any,
            None => panic!("missing typed config"),
        };
        UpstreamTlsContext::decode(&*typed.value).expect("decode tls context")
    }

    #[test]
    fn platform_api_route_generates_clusters_and_routes() {
        let definition = ApiDefinitionData {
            id: crate::domain::ApiDefinitionId::from_str_unchecked("def-1234"),
            team: "payments".into(),
            domain: "payments.flowplane.dev".into(),
            listener_isolation: true,
            target_listeners: None,
            tls_config: None,
            metadata: None,
            bootstrap_uri: None,
            bootstrap_revision: 1,
            generated_listener_id: None,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let override_config = canonicalize_filter_overrides(Some(json!({
            "cors": "allow-authenticated"
        })))
        .expect("canonicalize");

        let route = ApiRouteData {
            id: crate::domain::ApiRouteId::from_str_unchecked("route-1234"),
            api_definition_id: definition.id.clone(),
            match_type: "prefix".into(),
            match_value: "/api".into(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: json!({
                "targets": [
                    { "name": "blue", "endpoint": "blue.svc.local:8080", "weight": 80 },
                    { "name": "green", "endpoint": "green.svc.local:8080", "weight": 20 }
                ]
            }),
            timeout_seconds: Some(15),
            override_config,
            deployment_note: None,
            route_order: 0,
            generated_route_id: None,
            generated_cluster_id: None,
            filter_config: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let resources =
            resources_from_api_definitions(vec![definition.clone()], vec![route.clone()])
                .expect("build resources");

        let route_any = resources
            .iter()
            .find(|res| res.resource.type_url == ROUTE_TYPE_URL)
            .expect("route resource present");
        let route_config = RouteConfiguration::decode(route_any.resource.value.as_slice())
            .expect("decode route config");
        assert_eq!(route_config.virtual_hosts.len(), 1);
        let route_rule = route_config.virtual_hosts[0].routes.first().expect("route rule");
        assert_eq!(route_rule.name, format!("{}-{}", PLATFORM_ROUTE_PREFIX, "route1234"));
        let cluster_name = match &route_rule.action {
            Some(envoy_types::pb::envoy::config::route::v3::route::Action::Route(action)) => {
                match &action.cluster_specifier {
                    Some(envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::Cluster(name)) => name.clone(),
                    other => panic!("unexpected cluster specifier: {:?}", other),
                }
            }
            other => panic!("unexpected route action: {:?}", other),
        };

        let cors_any = route_rule
            .typed_per_filter_config
            .get("envoy.filters.http.cors")
            .expect("cors override present");
        assert_eq!(cors_any.type_url, FILTER_CORS_POLICY_TYPE_URL);

        let cluster_any = resources
            .iter()
            .find(|res| res.resource.type_url == CLUSTER_TYPE_URL)
            .expect("cluster resource");
        let cluster = Cluster::decode(cluster_any.resource.value.as_slice()).expect("cluster");
        assert_eq!(cluster.name, cluster_name);
        let endpoints =
            &cluster.load_assignment.as_ref().expect("assignment").endpoints[0].lb_endpoints;
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].load_balancing_weight.as_ref().unwrap().value, 80);
        assert_eq!(endpoints[1].load_balancing_weight.as_ref().unwrap().value, 20);

        // Note: Listeners are NOT generated by resources_from_api_definitions when
        // listener_isolation=false. They are created separately via the database
        // materialization process (materialize_isolated_listener) when listener_isolation=true,
        // or routes are merged into existing listeners when listener_isolation=false.
        assert!(
            !resources.iter().any(|res| res.resource.type_url == LISTENER_TYPE_URL),
            "No listener should be generated for listener_isolation=false"
        );
    }

    #[test]
    fn static_cluster_preserves_ip_endpoints() {
        let spec = ClusterSpec {
            endpoints: vec![
                EndpointSpec::String("10.0.0.1:8080".to_string()),
                EndpointSpec::String("10.0.0.2:8080".to_string()),
            ],
            connect_timeout_seconds: Some(3),
            ..Default::default()
        };

        let cluster = cluster_from_spec("ip-cluster", &spec).expect("cluster should build");

        assert_eq!(
            cluster.cluster_discovery_type,
            Some(ClusterDiscoveryType::Type(DiscoveryType::Static as i32))
        );
        assert!(cluster.transport_socket.is_none());
        assert_eq!(cluster.lb_policy, LbPolicy::RoundRobin as i32);
    }

    #[test]
    fn dns_endpoint_auto_enables_tls_and_dns_resolution() {
        let spec = ClusterSpec {
            endpoints: vec![EndpointSpec::String("example.com:443".to_string())],
            use_tls: Some(true),
            lb_policy: Some("LEAST_REQUEST".to_string()),
            ..Default::default()
        };

        let cluster = cluster_from_spec("dns-cluster", &spec).expect("cluster should build");

        assert_eq!(
            cluster.cluster_discovery_type,
            Some(ClusterDiscoveryType::Type(DiscoveryType::LogicalDns as i32))
        );
        assert_eq!(cluster.lb_policy, LbPolicy::LeastRequest as i32);

        let tls = decode_tls_context(&cluster);
        assert_eq!(tls.sni, "example.com");
    }

    #[test]
    fn least_request_policy_sets_lb_config() {
        let spec = ClusterSpec {
            endpoints: vec![EndpointSpec::String("10.0.0.1:8080".to_string())],
            lb_policy: Some("LEAST_REQUEST".to_string()),
            least_request: Some(LeastRequestPolicy { choice_count: Some(4) }),
            ..Default::default()
        };

        let cluster = cluster_from_spec("least", &spec).expect("cluster build");

        assert_eq!(cluster.lb_policy, LbPolicy::LeastRequest as i32);
        match cluster.lb_config.unwrap() {
            LbConfig::LeastRequestLbConfig(config) => {
                assert_eq!(config.choice_count.unwrap().value, 4);
            }
            other => panic!("unexpected lb config: {:?}", other),
        }
    }

    #[test]
    fn ring_hash_policy_sets_lb_config() {
        let spec = ClusterSpec {
            endpoints: vec![EndpointSpec::String("10.0.0.1:8080".to_string())],
            lb_policy: Some("RING_HASH".to_string()),
            ring_hash: Some(RingHashPolicy {
                minimum_ring_size: Some(1024),
                maximum_ring_size: Some(2048),
                hash_function: Some("murmur_hash_2".to_string()),
            }),
            ..Default::default()
        };

        let cluster = cluster_from_spec("ring", &spec).expect("cluster build");

        match cluster.lb_config.unwrap() {
            LbConfig::RingHashLbConfig(config) => {
                assert_eq!(config.minimum_ring_size.unwrap().value, 1024);
                assert_eq!(config.maximum_ring_size.unwrap().value, 2048);
                assert_eq!(config.hash_function, RingHashFunction::MurmurHash2 as i32);
            }
            other => panic!("unexpected lb config: {:?}", other),
        }
    }

    #[test]
    fn maglev_policy_sets_table_size() {
        let spec = ClusterSpec {
            endpoints: vec![EndpointSpec::String("10.0.0.1:8080".to_string())],
            lb_policy: Some("MAGLEV".to_string()),
            maglev: Some(MaglevPolicy { table_size: Some(65537) }),
            ..Default::default()
        };

        let cluster = cluster_from_spec("maglev", &spec).expect("cluster build");

        match cluster.lb_config.unwrap() {
            LbConfig::MaglevLbConfig(config) => {
                assert_eq!(config.table_size.unwrap().value, 65537);
            }
            other => panic!("unexpected lb config: {:?}", other),
        }
    }

    #[test]
    fn circuit_breakers_and_health_checks_are_applied() {
        let spec = ClusterSpec {
            endpoints: vec![EndpointSpec::String("10.0.0.1:8080".to_string())],
            circuit_breakers: Some(CircuitBreakersSpec {
                default: Some(CircuitBreakerThresholdsSpec {
                    max_connections: Some(100),
                    max_pending_requests: Some(200),
                    max_requests: None,
                    max_retries: None,
                }),
                high: Some(CircuitBreakerThresholdsSpec {
                    max_connections: Some(50),
                    max_pending_requests: None,
                    max_requests: Some(150),
                    max_retries: Some(2),
                }),
            }),
            health_checks: vec![HealthCheckSpec::Http {
                path: "/healthz".to_string(),
                host: Some("example.com".to_string()),
                method: Some("GET".to_string()),
                interval_seconds: Some(3),
                timeout_seconds: Some(1),
                healthy_threshold: Some(2),
                unhealthy_threshold: Some(3),
                expected_statuses: Some(vec![200]),
            }],
            ..Default::default()
        };

        let cluster = cluster_from_spec("cb-cluster", &spec).expect("cluster should build");

        let breakers = cluster.circuit_breakers.expect("circuit breakers");
        assert_eq!(breakers.thresholds.len(), 2);

        let http_check = match cluster.health_checks[0].health_checker.as_ref() {
            Some(health_check::HealthChecker::HttpHealthCheck(h)) => h,
            other => panic!("expected http health check, got {:?}", other),
        };
        assert_eq!(http_check.path, "/healthz");
        assert_eq!(http_check.host, "example.com");
    }

    #[test]
    fn tcp_health_check_is_supported() {
        let spec = ClusterSpec {
            endpoints: vec![EndpointSpec::String("10.0.0.1:8080".to_string())],
            health_checks: vec![HealthCheckSpec::Tcp {
                interval_seconds: Some(5),
                timeout_seconds: Some(2),
                healthy_threshold: Some(1),
                unhealthy_threshold: Some(3),
            }],
            ..Default::default()
        };

        let cluster = cluster_from_spec("tcp-health", &spec).expect("cluster build");

        match cluster.health_checks[0].health_checker.as_ref() {
            Some(health_check::HealthChecker::TcpHealthCheck(_)) => {}
            other => panic!("expected tcp health check, got {:?}", other),
        }
    }

    #[test]
    fn outlier_detection_is_applied() {
        let spec = ClusterSpec {
            endpoints: vec![EndpointSpec::String("10.0.0.1:8080".to_string())],
            outlier_detection: Some(OutlierDetectionSpec {
                consecutive_5xx: Some(7),
                interval_seconds: Some(5),
                base_ejection_time_seconds: Some(30),
                max_ejection_percent: Some(50),
            }),
            ..Default::default()
        };

        let cluster = cluster_from_spec("outlier", &spec).expect("cluster build");

        let outlier = cluster.outlier_detection.expect("outlier detection");
        assert_eq!(outlier.consecutive_5xx.unwrap().value, 7);
        assert_eq!(outlier.max_ejection_percent.unwrap().value, 50);
    }

    #[test]
    fn listeners_from_database_entries_build_listener_resource() {
        let listener_config = ListenerConfig {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            filter_chains: vec![FilterChainConfig {
                name: Some("default".to_string()),
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.tcp_proxy".to_string(),
                    filter_type: FilterType::TcpProxy {
                        cluster: "backend".to_string(),
                        access_log: None,
                    },
                }],
                tls_context: None,
            }],
        };

        let listener_data = ListenerData {
            id: crate::domain::ListenerId::from_str_unchecked("listener-1"),
            name: listener_config.name.clone(),
            address: listener_config.address.clone(),
            port: Some(listener_config.port as i64),
            protocol: "TCP".to_string(),
            configuration: serde_json::to_string(&listener_config).unwrap(),
            version: 1,
            source: "native_api".to_string(),
            team: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let built = listeners_from_database_entries(vec![listener_data], "test")
            .expect("listener resource build");

        assert_eq!(built.len(), 1);
        assert_eq!(built[0].name, "test-listener");
        assert_eq!(built[0].resource.type_url, LISTENER_TYPE_URL);
        assert!(!built[0].resource.value.is_empty());
    }

    fn contains_gateway_tag(value: &Value) -> bool {
        match value {
            Value::Object(map) => {
                if map.contains_key("flowplaneGateway") {
                    return true;
                }
                map.values().any(contains_gateway_tag)
            }
            Value::Array(items) => items.iter().any(contains_gateway_tag),
            _ => false,
        }
    }

    #[test]
    fn strip_gateway_tags_removes_tags_recursively() {
        let mut value = json!({
            "flowplaneGateway": "example",
            "Template": {
                "flowplaneGateway": "example",
                "child": {
                    "flowplaneGateway": "example"
                }
            },
            "array": [
                {
                    "flowplaneGateway": "example",
                    "Cluster": {
                        "flowplaneGateway": "example"
                    }
                }
            ]
        });

        strip_gateway_tags(&mut value);

        assert!(!contains_gateway_tag(&value));
    }

    #[test]
    fn test_create_ext_proc_cluster() {
        let bind_address = "127.0.0.1";
        let port = 18000;

        let built_resource = create_ext_proc_cluster(bind_address, port).unwrap();

        // Verify resource metadata
        assert_eq!(built_resource.name, "flowplane_ext_proc_service");
        assert_eq!(built_resource.resource.type_url, CLUSTER_TYPE_URL);

        // Decode the cluster
        let cluster = Cluster::decode(&built_resource.resource.value[..]).unwrap();

        // Verify cluster properties
        assert_eq!(cluster.name, "flowplane_ext_proc_service");
        assert!(cluster.connect_timeout.is_some());
        assert_eq!(cluster.connect_timeout.unwrap().seconds, 5);

        // Verify discovery type is STATIC
        assert!(cluster.cluster_discovery_type.is_some());
        match cluster.cluster_discovery_type.unwrap() {
            ClusterDiscoveryType::Type(t) => assert_eq!(t, DiscoveryType::Static as i32),
            _ => panic!("Expected STATIC discovery type"),
        }

        // Verify load assignment
        let load_assignment = cluster.load_assignment.unwrap();
        assert_eq!(load_assignment.cluster_name, "flowplane_ext_proc_service");
        assert_eq!(load_assignment.endpoints.len(), 1);

        let locality_endpoints = &load_assignment.endpoints[0];
        assert_eq!(locality_endpoints.lb_endpoints.len(), 1);

        // Verify endpoint address
        let lb_endpoint = &locality_endpoints.lb_endpoints[0];
        if let Some(lb_endpoint::HostIdentifier::Endpoint(endpoint)) = &lb_endpoint.host_identifier
        {
            if let Some(address) = &endpoint.address {
                if let Some(
                    envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                        socket_addr,
                    ),
                ) = &address.address
                {
                    assert_eq!(socket_addr.address, bind_address);
                    if let Some(socket_address::PortSpecifier::PortValue(p)) =
                        &socket_addr.port_specifier
                    {
                        assert_eq!(*p, port as u32);
                    } else {
                        panic!("Expected port value");
                    }
                } else {
                    panic!("Expected socket address");
                }
            } else {
                panic!("Expected address");
            }
        } else {
            panic!("Expected endpoint host identifier");
        }
    }

    #[test]
    fn test_create_access_log_cluster() {
        let bind_address = "127.0.0.1";
        let port = 18000u16;

        let built_resource = create_access_log_cluster(bind_address, port).unwrap();
        assert_eq!(built_resource.name, "flowplane_access_log_service");

        // Decode and verify cluster properties
        let cluster: Cluster = Cluster::decode(&*built_resource.resource.value).unwrap();
        assert_eq!(cluster.name, "flowplane_access_log_service");
        let load_assignment = cluster.load_assignment.unwrap();
        assert_eq!(load_assignment.cluster_name, "flowplane_access_log_service");
    }
}
