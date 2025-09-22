use crate::{config::SimpleXdsConfig, Result};
use envoy_types::pb::envoy::config::cluster::v3::Cluster;
use envoy_types::pb::envoy::config::core::v3::{socket_address, Address, SocketAddress};
use envoy_types::pb::envoy::config::endpoint::v3::{
    lb_endpoint, ClusterLoadAssignment, Endpoint, LbEndpoint, LocalityLbEndpoints,
};
use envoy_types::pb::envoy::config::listener::v3::Listener;
use envoy_types::pb::envoy::config::route::v3::RouteConfiguration;
use envoy_types::pb::google::protobuf::{Any, Duration};
use prost::Message;
use tracing::info;

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
            type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
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
                        ..Default::default()
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
