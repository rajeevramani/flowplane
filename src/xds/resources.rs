use std::net::IpAddr;

use crate::{config::SimpleXdsConfig, storage::ClusterData, Error, Result};
use envoy_types::pb::envoy::config::cluster::v3::cluster::{
    ClusterDiscoveryType, DiscoveryType, DnsLookupFamily, LbPolicy,
};
use envoy_types::pb::envoy::config::cluster::v3::Cluster;
use envoy_types::pb::envoy::config::core::v3::transport_socket::ConfigType as TransportSocketConfigType;
use envoy_types::pb::envoy::config::core::v3::{
    socket_address::{self, Protocol},
    Address, SocketAddress, TransportSocket,
};
use envoy_types::pb::envoy::config::endpoint::v3::{
    lb_endpoint, ClusterLoadAssignment, Endpoint, LbEndpoint, LocalityLbEndpoints,
};
use envoy_types::pb::envoy::config::listener::v3::Listener;
use envoy_types::pb::envoy::config::route::v3::RouteConfiguration;
use envoy_types::pb::envoy::extensions::transport_sockets::tls::v3::{
    CommonTlsContext, UpstreamTlsContext,
};
use envoy_types::pb::google::protobuf::{Any, Duration};
use prost::Message;
use tracing::{info, warn};

pub const CLUSTER_TYPE_URL: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";

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
        resource: Any {
            type_url: CLUSTER_TYPE_URL.to_string(),
            value: encoded,
        },
    }])
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
        resource: Any {
            type_url: "type.googleapis.com/envoy.config.route.v3.RouteConfiguration".to_string(),
            value: encoded,
        },
    }])
}

/// Build listener resources from the static configuration
pub fn listeners_from_config(config: &SimpleXdsConfig) -> Result<Vec<BuiltResource>> {
    use envoy_types::pb::envoy::config::listener::v3::Filter;
    use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpConnectionManager;

    let resources = &config.resources;

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
        resource: Any {
            type_url: "type.googleapis.com/envoy.config.listener.v3.Listener".to_string(),
            value: encoded,
        },
    }])
}

/// Build cluster resources from database entries (JSON stored in `ClusterData`).
pub fn clusters_from_database_entries(
    entries: Vec<ClusterData>,
    context: &str,
) -> Result<Vec<BuiltResource>> {
    let mut resources = Vec::new();

    for entry in entries {
        let config: serde_json::Value =
            serde_json::from_str(&entry.configuration).map_err(|e| {
                Error::config(format!(
                    "Invalid cluster configuration JSON for '{}': {}",
                    entry.name, e
                ))
            })?;

        let cluster = cluster_from_json(&entry.name, &config)?;
        let encoded = cluster.encode_to_vec();

        info!(
            phase = context,
            cluster_name = %entry.name,
            service_name = %entry.service_name,
            version = entry.version,
            encoded_size = encoded.len(),
            "Built cluster resource from database entry"
        );

        resources.push(BuiltResource {
            name: entry.name,
            resource: Any {
                type_url: CLUSTER_TYPE_URL.to_string(),
                value: encoded,
            },
        });
    }

    Ok(resources)
}

fn cluster_from_json(name: &str, config: &serde_json::Value) -> Result<Cluster> {
    // Parse endpoints from configuration
    let endpoints = config
        .get("endpoints")
        .and_then(|e| e.as_array())
        .ok_or_else(|| {
            Error::config("Cluster configuration missing 'endpoints' array".to_string())
        })?;

    let mut lb_endpoints = Vec::new();
    let mut has_hostname = false;
    let mut first_hostname: Option<String> = None;
    let mut tls_candidate_host: Option<String> = None;
    let mut has_tls_port = false;

    for endpoint in endpoints {
        if let Some((host, port)) = parse_endpoint(endpoint) {
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
                                    port_specifier: Some(socket_address::PortSpecifier::PortValue(port)),
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
        } else {
            warn!(cluster = %name, ?endpoint, "Skipping invalid endpoint entry");
        }
    }

    if lb_endpoints.is_empty() {
        return Err(Error::config(
            "No valid endpoints found in cluster configuration".to_string(),
        ));
    }

    let mut cluster = Cluster {
        name: name.to_string(),
        connect_timeout: Some(Duration {
            seconds: config
                .get("connect_timeout_seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(5) as i64,
            nanos: 0,
        }),
        load_assignment: Some(ClusterLoadAssignment {
            cluster_name: name.to_string(),
            endpoints: vec![LocalityLbEndpoints {
                lb_endpoints,
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };

    // Load balancing policy
    if let Some(lb_policy_value) = config
        .get("lb_policy")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        cluster.lb_policy = match lb_policy_value.to_uppercase().as_str() {
            "ROUND_ROBIN" => LbPolicy::RoundRobin as i32,
            "LEAST_REQUEST" => LbPolicy::LeastRequest as i32,
            "RING_HASH" => LbPolicy::RingHash as i32,
            "RANDOM" => LbPolicy::Random as i32,
            "MAGLEV" => LbPolicy::Maglev as i32,
            "CLUSTER_PROVIDED" => LbPolicy::ClusterProvided as i32,
            other => {
                warn!(cluster = %name, policy = other, "Unknown lb_policy value; defaulting to round robin");
                LbPolicy::RoundRobin as i32
            }
        };
    } else {
        cluster.lb_policy = LbPolicy::RoundRobin as i32;
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

        cluster.dns_lookup_family =
            parse_dns_lookup_family(config).unwrap_or(DnsLookupFamily::Auto as i32);
    } else {
        cluster.cluster_discovery_type =
            Some(ClusterDiscoveryType::Type(DiscoveryType::Static as i32));
    }

    let mut use_tls = config
        .get("use_tls")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !use_tls {
        use_tls = config
            .get("tls")
            .and_then(|v| v.get("enabled"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
    }

    if !use_tls && has_tls_port {
        use_tls = true;
    }

    let mut sni = config
        .get("tls_server_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            config
                .get("tls")
                .and_then(|v| v.get("server_name"))
                .and_then(|s| s.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });

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
        } else {
            warn!(cluster = %name, "TLS enabled but no SNI server name provided; upstream certificate verification may fail");
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

    Ok(cluster)
}

pub(crate) fn parse_endpoint(value: &serde_json::Value) -> Option<(String, u32)> {
    if let Some(endpoint_str) = value.as_str() {
        let parts: Vec<&str> = endpoint_str.split(':').collect();
        if parts.len() == 2 {
            let host = parts[0].trim();
            let port = parts[1].trim().parse::<u32>().ok()?;
            if host.is_empty() {
                return None;
            }
            return Some((host.to_string(), port));
        }
    }

    if let Some(obj) = value.as_object() {
        let host = obj
            .get("host")
            .or_else(|| obj.get("address"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|h| !h.is_empty())?
            .to_string();

        let port_value = obj
            .get("port")
            .or_else(|| obj.get("port_value"))
            .and_then(|v| {
                if let Some(p) = v.as_u64() {
                    Some(p)
                } else if let Some(p_str) = v.as_str() {
                    p_str.trim().parse::<u64>().ok()
                } else {
                    None
                }
            })?;

        if port_value == 0 || port_value > u32::MAX as u64 {
            return None;
        }

        return Some((host, port_value as u32));
    }

    None
}

fn parse_dns_lookup_family(config: &serde_json::Value) -> Option<i32> {
    let family_value = config
        .get("dns_lookup_family")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_uppercase());

    match family_value.as_deref() {
        Some("AUTO") => Some(DnsLookupFamily::Auto as i32),
        Some("V4_ONLY") | Some("V4ONLY") => Some(DnsLookupFamily::V4Only as i32),
        Some("V6_ONLY") | Some("V6ONLY") => Some(DnsLookupFamily::V6Only as i32),
        Some("V4_PREFERRED") | Some("V4PREFERRED") => Some(DnsLookupFamily::V4Preferred as i32),
        Some("ALL") => Some(DnsLookupFamily::All as i32),
        Some(other) => {
            warn!(
                family = other,
                "Unknown dns_lookup_family value; defaulting to AUTO"
            );
            Some(DnsLookupFamily::Auto as i32)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoy_types::pb::envoy::config::core::v3::transport_socket::ConfigType;
    use prost::Message;
    use serde_json::json;

    #[test]
    fn static_cluster_preserves_ip_endpoints() {
        let cluster = cluster_from_json(
            "ip-cluster",
            &json!({
                "endpoints": ["10.0.0.1:8080", "10.0.0.2:8080"],
                "connect_timeout_seconds": 3,
                "lb_policy": "ROUND_ROBIN"
            }),
        )
        .expect("cluster should build");

        assert_eq!(
            cluster.cluster_discovery_type,
            Some(ClusterDiscoveryType::Type(DiscoveryType::Static as i32))
        );
        assert!(cluster.transport_socket.is_none());
        assert_eq!(cluster.lb_policy, LbPolicy::RoundRobin as i32);
    }

    #[test]
    fn dns_endpoint_auto_enables_tls_and_dns_resolution() {
        let cluster = cluster_from_json(
            "dns-cluster",
            &json!({
                "endpoints": ["example.com:443"],
                "lb_policy": "LEAST_REQUEST"
            }),
        )
        .expect("cluster should build");

        assert_eq!(
            cluster.cluster_discovery_type,
            Some(ClusterDiscoveryType::Type(DiscoveryType::LogicalDns as i32))
        );
        assert_eq!(cluster.lb_policy, LbPolicy::LeastRequest as i32);

        let transport_socket = cluster.transport_socket.expect("tls transport socket");
        let tls_any = match transport_socket.config_type.expect("config type") {
            ConfigType::TypedConfig(any) => any,
        };

        let tls_context =
            UpstreamTlsContext::decode(&*tls_any.value).expect("decode upstream tls context");
        assert_eq!(tls_context.sni, "example.com");
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
        "Created endpoint resource"
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
