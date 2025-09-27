//! Cluster management using envoy-types
//!
//! This module provides functionality for creating and managing Envoy cluster configurations
//! using the proper envoy-types protobuf definitions.

use envoy_types::pb::envoy::config::{
    cluster::v3::{cluster::LbPolicy, Cluster},
    core::v3::{address::Address as AddressType, Address, SocketAddress},
    endpoint::v3::{Endpoint, LbEndpoint, LocalityLbEndpoints},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use validator::Validate;

/// REST API representation of a cluster configuration
/// This will be converted to the proper envoy-types Cluster
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ClusterConfig {
    #[validate(length(min = 1, message = "Cluster name cannot be empty"))]
    #[validate(regex(
        path = "crate::utils::VALID_NAME_REGEX",
        message = "Cluster name must be a valid identifier"
    ))]
    pub name: String,

    #[validate(length(min = 1, message = "At least one endpoint is required"))]
    #[validate(nested)]
    pub endpoints: Vec<EndpointConfig>,

    pub load_balancing_policy: LoadBalancingPolicy,

    #[validate(range(
        min = 1,
        max = 300,
        message = "Connect timeout must be between 1 and 300 seconds"
    ))]
    pub connect_timeout: Option<u64>, // seconds

    #[validate(nested)]
    pub health_checks: Option<Vec<HealthCheckConfig>>,
}

/// REST API representation of an endpoint
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct EndpointConfig {
    #[validate(length(min = 1, message = "Endpoint address cannot be empty"))]
    pub address: String,

    #[validate(range(min = 1, max = 65535, message = "Port must be between 1 and 65535"))]
    pub port: u32,

    #[validate(range(min = 1, max = 1000, message = "Weight must be between 1 and 1000"))]
    pub weight: Option<u32>,
}

/// REST API representation of load balancing policies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoadBalancingPolicy {
    RoundRobin,
    LeastRequest,
    Random,
}

/// REST API representation of health check configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct HealthCheckConfig {
    #[validate(range(
        min = 1,
        max = 60,
        message = "Health check timeout must be between 1 and 60 seconds"
    ))]
    pub timeout: u64,

    #[validate(range(
        min = 1,
        max = 300,
        message = "Health check interval must be between 1 and 300 seconds"
    ))]
    pub interval: u64,

    #[validate(range(min = 1, max = 10, message = "Healthy threshold must be between 1 and 10"))]
    pub healthy_threshold: u32,

    #[validate(range(
        min = 1,
        max = 10,
        message = "Unhealthy threshold must be between 1 and 10"
    ))]
    pub unhealthy_threshold: u32,

    #[validate(length(min = 1, message = "Health check path cannot be empty if specified"))]
    pub path: Option<String>, // For HTTP health checks
}

impl ClusterConfig {
    /// Validate the cluster configuration
    pub fn validate_config(&self) -> Result<(), crate::Error> {
        self.validate()
            .map_err(|e| crate::Error::Validation(format!("Cluster validation failed: {}", e)))?;
        Ok(())
    }

    /// Convert REST API ClusterConfig to envoy-types Cluster
    pub fn to_envoy_cluster(&self) -> Result<Cluster, crate::Error> {
        // Validate before conversion
        self.validate_config()?;
        let cluster = Cluster {
            name: self.name.clone(),
            lb_policy: self.load_balancing_policy.to_envoy_lb_policy() as i32,
            load_assignment: Some(self.create_cluster_load_assignment()?),
            connect_timeout: self.connect_timeout.map(|t| {
                envoy_types::pb::google::protobuf::Duration { seconds: t as i64, nanos: 0 }
            }),
            // TODO: Add health checks, circuit breakers, etc.
            ..Default::default()
        };

        Ok(cluster)
    }

    /// Create cluster load assignment from endpoints
    fn create_cluster_load_assignment(
        &self,
    ) -> Result<envoy_types::pb::envoy::config::endpoint::v3::ClusterLoadAssignment, crate::Error>
    {
        let lb_endpoints: Vec<LbEndpoint> = self
            .endpoints
            .iter()
            .map(|endpoint| endpoint.to_envoy_lb_endpoint())
            .collect::<Result<Vec<_>, _>>()?;

        let locality_lb_endpoints = LocalityLbEndpoints { lb_endpoints, ..Default::default() };

        Ok(envoy_types::pb::envoy::config::endpoint::v3::ClusterLoadAssignment {
            cluster_name: self.name.clone(),
            endpoints: vec![locality_lb_endpoints],
            ..Default::default()
        })
    }
}

impl EndpointConfig {
    /// Convert REST API EndpointConfig to envoy-types LbEndpoint
    fn to_envoy_lb_endpoint(&self) -> Result<LbEndpoint, crate::Error> {
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

        let endpoint = Endpoint { address: Some(address), ..Default::default() };

        let lb_endpoint = LbEndpoint {
            host_identifier: Some(
                envoy_types::pb::envoy::config::endpoint::v3::lb_endpoint::HostIdentifier::Endpoint(
                    endpoint,
                ),
            ),
            load_balancing_weight: self
                .weight
                .map(|w| envoy_types::pb::google::protobuf::UInt32Value { value: w }),
            ..Default::default()
        };

        Ok(lb_endpoint)
    }
}

impl LoadBalancingPolicy {
    /// Convert REST API LoadBalancingPolicy to envoy-types LbPolicy
    fn to_envoy_lb_policy(&self) -> LbPolicy {
        match self {
            LoadBalancingPolicy::RoundRobin => LbPolicy::RoundRobin,
            LoadBalancingPolicy::LeastRequest => LbPolicy::LeastRequest,
            LoadBalancingPolicy::Random => LbPolicy::Random,
        }
    }
}

/// Cluster manager for handling cluster operations
#[derive(Debug)]
pub struct ClusterManager {
    clusters: HashMap<String, Cluster>,
}

impl ClusterManager {
    /// Create a new cluster manager
    pub fn new() -> Self {
        Self { clusters: HashMap::new() }
    }

    /// Add or update a cluster
    pub fn upsert_cluster(&mut self, config: ClusterConfig) -> Result<(), crate::Error> {
        let cluster = config.to_envoy_cluster()?;
        self.clusters.insert(cluster.name.clone(), cluster);
        Ok(())
    }

    /// Remove a cluster
    pub fn remove_cluster(&mut self, name: &str) -> Option<Cluster> {
        self.clusters.remove(name)
    }

    /// Get a cluster by name
    pub fn get_cluster(&self, name: &str) -> Option<&Cluster> {
        self.clusters.get(name)
    }

    /// Get all clusters
    pub fn get_all_clusters(&self) -> Vec<Cluster> {
        self.clusters.values().cloned().collect()
    }

    /// List cluster names
    pub fn list_cluster_names(&self) -> Vec<String> {
        self.clusters.keys().cloned().collect()
    }
}

impl Default for ClusterManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_config_conversion() {
        let config = ClusterConfig {
            name: "test-cluster".to_string(),
            endpoints: vec![
                EndpointConfig { address: "127.0.0.1".to_string(), port: 8080, weight: Some(100) },
                EndpointConfig { address: "127.0.0.1".to_string(), port: 8081, weight: Some(100) },
            ],
            load_balancing_policy: LoadBalancingPolicy::RoundRobin,
            connect_timeout: Some(5),
            health_checks: None,
        };

        let cluster = config.to_envoy_cluster().expect("Failed to convert cluster config");

        assert_eq!(cluster.name, "test-cluster");
        assert_eq!(cluster.lb_policy, LbPolicy::RoundRobin as i32);
        assert!(cluster.load_assignment.is_some());

        let load_assignment = cluster.load_assignment.unwrap();
        assert_eq!(load_assignment.cluster_name, "test-cluster");
        assert_eq!(load_assignment.endpoints.len(), 1);
        assert_eq!(load_assignment.endpoints[0].lb_endpoints.len(), 2);
    }

    #[test]
    fn test_cluster_manager() {
        let mut manager = ClusterManager::new();

        let config = ClusterConfig {
            name: "test-cluster".to_string(),
            endpoints: vec![EndpointConfig {
                address: "127.0.0.1".to_string(),
                port: 8080,
                weight: None,
            }],
            load_balancing_policy: LoadBalancingPolicy::Random,
            connect_timeout: None,
            health_checks: None,
        };

        manager.upsert_cluster(config).expect("Failed to add cluster");

        assert!(manager.get_cluster("test-cluster").is_some());
        assert_eq!(manager.list_cluster_names().len(), 1);

        let removed = manager.remove_cluster("test-cluster");
        assert!(removed.is_some());
        assert_eq!(manager.list_cluster_names().len(), 0);
    }

    #[test]
    fn test_endpoint_config_conversion() {
        let endpoint =
            EndpointConfig { address: "192.168.1.1".to_string(), port: 9090, weight: Some(50) };

        let lb_endpoint = endpoint.to_envoy_lb_endpoint().expect("Failed to convert endpoint");

        assert!(lb_endpoint.host_identifier.is_some());
        assert!(lb_endpoint.load_balancing_weight.is_some());
        assert_eq!(lb_endpoint.load_balancing_weight.unwrap().value, 50);
    }
}
