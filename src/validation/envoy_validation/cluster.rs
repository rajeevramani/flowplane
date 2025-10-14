use crate::errors::{FlowplaneError, Result};
use envoy_types::pb::envoy::config::{
    cluster::v3::{cluster::LbPolicy, Cluster},
    core::v3::{address::Address as AddressType, Address, SocketAddress},
    endpoint::v3::{lb_endpoint, ClusterLoadAssignment, Endpoint, LbEndpoint, LocalityLbEndpoints},
};

use super::helpers::encode_check;

pub fn validate_envoy_cluster(cluster: &Cluster) -> Result<()> {
    encode_check(cluster, "Invalid cluster configuration")?;

    if cluster.name.is_empty() {
        return Err(FlowplaneError::validation_field(
            "Cluster name cannot be empty",
            "name",
        ));
    }

    if !is_valid_lb_policy(cluster.lb_policy) {
        return Err(FlowplaneError::validation_field(
            "Invalid load balancing policy",
            "lb_policy",
        ));
    }

    if let Some(load_assignment) = &cluster.load_assignment {
        validate_cluster_load_assignment(load_assignment)?;
    }

    Ok(())
}

fn validate_cluster_load_assignment(load_assignment: &ClusterLoadAssignment) -> Result<()> {
    if load_assignment.cluster_name.is_empty() {
        return Err(FlowplaneError::validation_field(
            "Cluster name cannot be empty",
            "cluster_name",
        ));
    }

    if load_assignment.endpoints.is_empty() {
        return Err(FlowplaneError::validation_field(
            "At least one endpoint is required",
            "endpoints",
        ));
    }

    for (index, locality_endpoints) in load_assignment.endpoints.iter().enumerate() {
        validate_locality_lb_endpoints(locality_endpoints).map_err(|e| {
            FlowplaneError::validation_field(
                format!("Locality endpoints {} validation failed: {}", index, e),
                "endpoints",
            )
        })?;
    }

    Ok(())
}

fn validate_locality_lb_endpoints(locality_endpoints: &LocalityLbEndpoints) -> Result<()> {
    if locality_endpoints.lb_endpoints.is_empty() {
        return Err(FlowplaneError::validation(
            "At least one load balancing endpoint is required",
        ));
    }

    for (index, lb_endpoint) in locality_endpoints.lb_endpoints.iter().enumerate() {
        validate_lb_endpoint(lb_endpoint).map_err(|e| {
            FlowplaneError::validation(format!(
                "Load balancing endpoint {} validation failed: {}",
                index, e
            ))
        })?;
    }

    Ok(())
}

fn validate_lb_endpoint(lb_endpoint: &LbEndpoint) -> Result<()> {
    match &lb_endpoint.host_identifier {
        Some(lb_endpoint::HostIdentifier::Endpoint(endpoint)) => validate_endpoint(endpoint),
        Some(lb_endpoint::HostIdentifier::EndpointName(name)) => {
            if name.is_empty() {
                Err(FlowplaneError::validation("Endpoint name cannot be empty"))
            } else {
                Ok(())
            }
        }
        None => Err(FlowplaneError::validation("Host identifier is required")),
    }
}

pub(crate) fn validate_endpoint(endpoint: &Endpoint) -> Result<()> {
    if let Some(address) = &endpoint.address {
        validate_address(address)
    } else {
        Err(FlowplaneError::validation("Endpoint address is required"))
    }
}

pub(crate) fn validate_address(address: &Address) -> Result<()> {
    match &address.address {
        Some(AddressType::SocketAddress(socket_addr)) => validate_socket_address(socket_addr),
        Some(AddressType::Pipe(pipe)) => {
            if pipe.path.is_empty() {
                Err(FlowplaneError::validation("Pipe path cannot be empty"))
            } else {
                Ok(())
            }
        }
        Some(AddressType::EnvoyInternalAddress(_)) => Ok(()),
        None => Err(FlowplaneError::validation("Address type is required")),
    }
}

pub(crate) fn validate_socket_address(socket_addr: &SocketAddress) -> Result<()> {
    if socket_addr.address.is_empty() {
        return Err(FlowplaneError::validation("Socket address cannot be empty"));
    }

    match &socket_addr.port_specifier {
        Some(envoy_types::pb::envoy::config::core::v3::socket_address::PortSpecifier::PortValue(port)) => {
            if *port == 0 || *port > 65535 {
                return Err(FlowplaneError::validation("Port must be between 1 and 65535"));
            }
        }
        Some(envoy_types::pb::envoy::config::core::v3::socket_address::PortSpecifier::NamedPort(name)) => {
            if name.is_empty() {
                return Err(FlowplaneError::validation("Named port cannot be empty"));
            }
        }
        None => return Err(FlowplaneError::validation("Port specifier is required")),
    }

    Ok(())
}

fn is_valid_lb_policy(policy: i32) -> bool {
    matches!(
        policy,
        x if x == LbPolicy::RoundRobin as i32
            || x == LbPolicy::LeastRequest as i32
            || x == LbPolicy::Random as i32
            || x == LbPolicy::RingHash as i32
            || x == LbPolicy::Maglev as i32
            || x == LbPolicy::ClusterProvided as i32
            || x == LbPolicy::LoadBalancingPolicyConfig as i32
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoy_types::pb::envoy::config::core::v3::socket_address::PortSpecifier;

    #[test]
    fn envoy_cluster_validation() {
        let cluster = Cluster {
            name: "test-cluster".to_string(),
            lb_policy: LbPolicy::RoundRobin as i32,
            ..Default::default()
        };
        assert!(validate_envoy_cluster(&cluster).is_ok());

        let invalid_cluster = Cluster {
            name: "".to_string(),
            ..Default::default()
        };
        assert!(validate_envoy_cluster(&invalid_cluster).is_err());
    }

    #[test]
    fn socket_address_validation() {
        let valid = SocketAddress {
            address: "127.0.0.1".to_string(),
            port_specifier: Some(PortSpecifier::PortValue(8080)),
            ..Default::default()
        };
        assert!(validate_socket_address(&valid).is_ok());

        let invalid_port = SocketAddress {
            address: "127.0.0.1".to_string(),
            port_specifier: Some(PortSpecifier::PortValue(0)),
            ..Default::default()
        };
        assert!(validate_socket_address(&invalid_port).is_err());

        let missing_port = SocketAddress {
            address: "127.0.0.1".to_string(),
            port_specifier: None,
            ..Default::default()
        };
        assert!(validate_socket_address(&missing_port).is_err());
    }
}
