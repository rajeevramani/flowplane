//! # Envoy-types Protocol Validation
//!
//! This module provides validation functions that use envoy-types to validate
//! protocol buffer structures. The key insight is using `.encode_to_vec()` to
//! test if a configuration is valid for Envoy.

use prost::Message;
use validator::ValidationError;

use envoy_types::pb::envoy::{
    config::{
        cluster::v3::{Cluster, cluster::LbPolicy},
        core::v3::{Address, SocketAddress, address::Address as AddressType},
        endpoint::v3::{Endpoint, LbEndpoint, LocalityLbEndpoints, ClusterLoadAssignment},
        listener::v3::{Listener, Filter, FilterChain},
        route::v3::{RouteConfiguration, VirtualHost, Route, RouteMatch, RouteAction},
    },
};

use crate::errors::types::{MagayaError, Result};

/// Validate a Cluster configuration using envoy-types
pub fn validate_envoy_cluster(cluster: &Cluster) -> Result<()> {
    // Try to encode the cluster - if this fails, the cluster is invalid
    match cluster.encode_to_vec().len() {
        0 => Err(MagayaError::validation("Invalid cluster configuration: failed envoy-types encoding")),
        _ => {
            // Additional validation checks
            if cluster.name.is_empty() {
                return Err(MagayaError::validation_field("Cluster name cannot be empty", "name"));
            }

            // Validate load balancing policy
            if !is_valid_lb_policy(cluster.lb_policy) {
                return Err(MagayaError::validation_field("Invalid load balancing policy", "lb_policy"));
            }

            // If load assignment is present, validate it
            if let Some(load_assignment) = &cluster.load_assignment {
                validate_cluster_load_assignment(load_assignment)?;
            }

            Ok(())
        }
    }
}

/// Validate a RouteConfiguration using envoy-types
pub fn validate_envoy_route_configuration(route_config: &RouteConfiguration) -> Result<()> {
    // Try to encode the route configuration - if this fails, it's invalid
    match route_config.encode_to_vec().len() {
        0 => Err(MagayaError::validation("Invalid route configuration: failed envoy-types encoding")),
        _ => {
            // Additional validation checks
            if route_config.name.is_empty() {
                return Err(MagayaError::validation_field("Route configuration name cannot be empty", "name"));
            }

            if route_config.virtual_hosts.is_empty() {
                return Err(MagayaError::validation_field("At least one virtual host is required", "virtual_hosts"));
            }

            // Validate each virtual host
            for (index, vhost) in route_config.virtual_hosts.iter().enumerate() {
                validate_virtual_host(vhost)
                    .map_err(|e| MagayaError::validation_field(
                        format!("Virtual host {} validation failed: {}", index, e),
                        "virtual_hosts"
                    ))?;
            }

            Ok(())
        }
    }
}

/// Validate a Listener configuration using envoy-types
pub fn validate_envoy_listener(listener: &Listener) -> Result<()> {
    // Try to encode the listener - if this fails, it's invalid
    match listener.encode_to_vec().len() {
        0 => Err(MagayaError::validation("Invalid listener configuration: failed envoy-types encoding")),
        _ => {
            // Additional validation checks
            if listener.name.is_empty() {
                return Err(MagayaError::validation_field("Listener name cannot be empty", "name"));
            }

            // Validate address
            if let Some(address) = &listener.address {
                validate_address(address)?;
            } else {
                return Err(MagayaError::validation_field("Listener address is required", "address"));
            }

            // Validate filter chains
            if listener.filter_chains.is_empty() {
                return Err(MagayaError::validation_field("At least one filter chain is required", "filter_chains"));
            }

            for (index, filter_chain) in listener.filter_chains.iter().enumerate() {
                validate_filter_chain(filter_chain)
                    .map_err(|e| MagayaError::validation_field(
                        format!("Filter chain {} validation failed: {}", index, e),
                        "filter_chains"
                    ))?;
            }

            Ok(())
        }
    }
}

/// Validate a ClusterLoadAssignment
fn validate_cluster_load_assignment(load_assignment: &ClusterLoadAssignment) -> Result<()> {
    if load_assignment.cluster_name.is_empty() {
        return Err(MagayaError::validation_field("Cluster name cannot be empty", "cluster_name"));
    }

    if load_assignment.endpoints.is_empty() {
        return Err(MagayaError::validation_field("At least one endpoint is required", "endpoints"));
    }

    // Validate each locality endpoints
    for (index, locality_endpoints) in load_assignment.endpoints.iter().enumerate() {
        validate_locality_lb_endpoints(locality_endpoints)
            .map_err(|e| MagayaError::validation_field(
                format!("Locality endpoints {} validation failed: {}", index, e),
                "endpoints"
            ))?;
    }

    Ok(())
}

/// Validate LocalityLbEndpoints
fn validate_locality_lb_endpoints(locality_endpoints: &LocalityLbEndpoints) -> Result<()> {
    if locality_endpoints.lb_endpoints.is_empty() {
        return Err(MagayaError::validation("At least one load balancing endpoint is required"));
    }

    for (index, lb_endpoint) in locality_endpoints.lb_endpoints.iter().enumerate() {
        validate_lb_endpoint(lb_endpoint)
            .map_err(|e| MagayaError::validation(
                format!("Load balancing endpoint {} validation failed: {}", index, e)
            ))?;
    }

    Ok(())
}

/// Validate LbEndpoint
fn validate_lb_endpoint(lb_endpoint: &LbEndpoint) -> Result<()> {
    if let Some(host_identifier) = &lb_endpoint.host_identifier {
        match host_identifier {
            envoy_types::pb::envoy::config::endpoint::v3::lb_endpoint::HostIdentifier::Endpoint(endpoint) => {
                validate_endpoint(endpoint)?;
            }
            envoy_types::pb::envoy::config::endpoint::v3::lb_endpoint::HostIdentifier::EndpointName(name) => {
                if name.is_empty() {
                    return Err(MagayaError::validation("Endpoint name cannot be empty"));
                }
            }
        }
    } else {
        return Err(MagayaError::validation("Host identifier is required"));
    }

    Ok(())
}

/// Validate Endpoint
fn validate_endpoint(endpoint: &Endpoint) -> Result<()> {
    if let Some(address) = &endpoint.address {
        validate_address(address)?;
    } else {
        return Err(MagayaError::validation("Endpoint address is required"));
    }

    Ok(())
}

/// Validate Address
fn validate_address(address: &Address) -> Result<()> {
    if let Some(address_type) = &address.address {
        match address_type {
            AddressType::SocketAddress(socket_addr) => {
                validate_socket_address(socket_addr)?;
            }
            AddressType::Pipe(pipe) => {
                if pipe.path.is_empty() {
                    return Err(MagayaError::validation("Pipe path cannot be empty"));
                }
            }
            AddressType::EnvoyInternalAddress(_) => {
                // Internal addresses are handled by Envoy
            }
        }
    } else {
        return Err(MagayaError::validation("Address type is required"));
    }

    Ok(())
}

/// Validate SocketAddress
fn validate_socket_address(socket_addr: &SocketAddress) -> Result<()> {
    if socket_addr.address.is_empty() {
        return Err(MagayaError::validation("Socket address cannot be empty"));
    }

    // Validate port
    if let Some(port_specifier) = &socket_addr.port_specifier {
        match port_specifier {
            envoy_types::pb::envoy::config::core::v3::socket_address::PortSpecifier::PortValue(port) => {
                if *port == 0 || *port > 65535 {
                    return Err(MagayaError::validation("Port must be between 1 and 65535"));
                }
            }
            envoy_types::pb::envoy::config::core::v3::socket_address::PortSpecifier::NamedPort(name) => {
                if name.is_empty() {
                    return Err(MagayaError::validation("Named port cannot be empty"));
                }
            }
        }
    } else {
        return Err(MagayaError::validation("Port specifier is required"));
    }

    Ok(())
}

/// Validate VirtualHost
fn validate_virtual_host(vhost: &VirtualHost) -> Result<()> {
    if vhost.name.is_empty() {
        return Err(MagayaError::validation("Virtual host name cannot be empty"));
    }

    if vhost.domains.is_empty() {
        return Err(MagayaError::validation("At least one domain is required"));
    }

    // Validate domains
    for domain in &vhost.domains {
        if domain.is_empty() {
            return Err(MagayaError::validation("Domain cannot be empty"));
        }
    }

    // Validate routes
    if vhost.routes.is_empty() {
        return Err(MagayaError::validation("At least one route is required"));
    }

    for (index, route) in vhost.routes.iter().enumerate() {
        validate_route(route)
            .map_err(|e| MagayaError::validation(
                format!("Route {} validation failed: {}", index, e)
            ))?;
    }

    Ok(())
}

/// Validate Route
fn validate_route(route: &Route) -> Result<()> {
    // Validate match
    if let Some(route_match) = &route.r#match {
        validate_route_match(route_match)?;
    } else {
        return Err(MagayaError::validation("Route match is required"));
    }

    // Validate action
    if route.action.is_none() {
        return Err(MagayaError::validation("Route action is required"));
    }

    Ok(())
}

/// Validate RouteMatch
fn validate_route_match(route_match: &RouteMatch) -> Result<()> {
    // Path specifier is required
    if route_match.path_specifier.is_none() {
        return Err(MagayaError::validation("Path specifier is required"));
    }

    Ok(())
}

/// Validate FilterChain
fn validate_filter_chain(filter_chain: &FilterChain) -> Result<()> {
    if filter_chain.filters.is_empty() {
        return Err(MagayaError::validation("At least one filter is required"));
    }

    for (index, filter) in filter_chain.filters.iter().enumerate() {
        validate_filter(filter)
            .map_err(|e| MagayaError::validation(
                format!("Filter {} validation failed: {}", index, e)
            ))?;
    }

    Ok(())
}

/// Validate Filter
fn validate_filter(filter: &Filter) -> Result<()> {
    if filter.name.is_empty() {
        return Err(MagayaError::validation("Filter name cannot be empty"));
    }

    if filter.config_type.is_none() {
        return Err(MagayaError::validation("Filter configuration is required"));
    }

    Ok(())
}

/// Check if load balancing policy is valid
fn is_valid_lb_policy(policy: i32) -> bool {
    matches!(
        policy,
        x if x == LbPolicy::RoundRobin as i32 ||
             x == LbPolicy::LeastRequest as i32 ||
             x == LbPolicy::Random as i32 ||
             x == LbPolicy::RingHash as i32 ||
             x == LbPolicy::Maglev as i32 ||
             x == LbPolicy::ClusterProvided as i32 ||
             x == LbPolicy::LoadBalancingPolicyConfig as i32
    )
}

/// Convenience function to validate any envoy-types Message
pub fn validate_envoy_message<T: Message>(message: &T) -> Result<()> {
    match message.encode_to_vec().len() {
        0 => Err(MagayaError::validation("Invalid configuration: failed envoy-types encoding")),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoy_types::pb::envoy::config::core::v3::socket_address::PortSpecifier;

    #[test]
    fn test_validate_envoy_cluster() {
        let cluster = Cluster {
            name: "test-cluster".to_string(),
            lb_policy: LbPolicy::RoundRobin as i32,
            ..Default::default()
        };

        assert!(validate_envoy_cluster(&cluster).is_ok());

        // Test invalid cluster (empty name)
        let invalid_cluster = Cluster {
            name: "".to_string(),
            ..Default::default()
        };

        assert!(validate_envoy_cluster(&invalid_cluster).is_err());
    }

    #[test]
    fn test_validate_socket_address() {
        let valid_socket_addr = SocketAddress {
            address: "127.0.0.1".to_string(),
            port_specifier: Some(PortSpecifier::PortValue(8080)),
            ..Default::default()
        };

        assert!(validate_socket_address(&valid_socket_addr).is_ok());

        // Test invalid port
        let invalid_port_socket_addr = SocketAddress {
            address: "127.0.0.1".to_string(),
            port_specifier: Some(PortSpecifier::PortValue(0)),
            ..Default::default()
        };

        assert!(validate_socket_address(&invalid_port_socket_addr).is_err());

        // Test missing port
        let missing_port_socket_addr = SocketAddress {
            address: "127.0.0.1".to_string(),
            port_specifier: None,
            ..Default::default()
        };

        assert!(validate_socket_address(&missing_port_socket_addr).is_err());
    }

    #[test]
    fn test_validate_envoy_route_configuration() {
        let route_config = RouteConfiguration {
            name: "test-route".to_string(),
            virtual_hosts: vec![
                VirtualHost {
                    name: "test-vhost".to_string(),
                    domains: vec!["example.com".to_string()],
                    routes: vec![
                        Route {
                            r#match: Some(RouteMatch {
                                path_specifier: Some(
                                    envoy_types::pb::envoy::config::route::v3::route_match::PathSpecifier::Prefix("/".to_string())
                                ),
                                ..Default::default()
                            }),
                            action: Some(
                                envoy_types::pb::envoy::config::route::v3::route::Action::Route(
                                    RouteAction {
                                        cluster_specifier: Some(
                                            envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::Cluster("test-cluster".to_string())
                                        ),
                                        ..Default::default()
                                    }
                                )
                            ),
                            ..Default::default()
                        }
                    ],
                    ..Default::default()
                }
            ],
            ..Default::default()
        };

        assert!(validate_envoy_route_configuration(&route_config).is_ok());

        // Test invalid route (empty name)
        let invalid_route = RouteConfiguration {
            name: "".to_string(),
            virtual_hosts: vec![],
            ..Default::default()
        };

        assert!(validate_envoy_route_configuration(&invalid_route).is_err());
    }

    #[test]
    fn test_validate_envoy_listener() {
        let listener = Listener {
            name: "test-listener".to_string(),
            address: Some(Address {
                address: Some(AddressType::SocketAddress(SocketAddress {
                    address: "0.0.0.0".to_string(),
                    port_specifier: Some(PortSpecifier::PortValue(8080)),
                    ..Default::default()
                })),
            }),
            filter_chains: vec![
                FilterChain {
                    filters: vec![
                        Filter {
                            name: "test-filter".to_string(),
                            config_type: Some(
                                envoy_types::pb::envoy::config::listener::v3::filter::ConfigType::TypedConfig(
                                    prost_types::Any::default()
                                )
                            ),
                        }
                    ],
                    ..Default::default()
                }
            ],
            ..Default::default()
        };

        assert!(validate_envoy_listener(&listener).is_ok());

        // Test invalid listener (empty name)
        let invalid_listener = Listener {
            name: "".to_string(),
            ..Default::default()
        };

        assert!(validate_envoy_listener(&invalid_listener).is_err());
    }

    #[test]
    fn test_is_valid_lb_policy() {
        assert!(is_valid_lb_policy(LbPolicy::RoundRobin as i32));
        assert!(is_valid_lb_policy(LbPolicy::LeastRequest as i32));
        assert!(is_valid_lb_policy(LbPolicy::Random as i32));
        assert!(is_valid_lb_policy(LbPolicy::RingHash as i32));
        assert!(!is_valid_lb_policy(999)); // Invalid policy
    }

    #[test]
    fn test_validate_envoy_message() {
        let cluster = Cluster {
            name: "test".to_string(),
            ..Default::default()
        };

        assert!(validate_envoy_message(&cluster).is_ok());
    }
}