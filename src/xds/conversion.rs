//! Conversion layers between REST API types and envoy-types
//!
//! This module provides centralized conversion functionality between the REST API
//! representations and the proper envoy-types protobuf structures.

use crate::Error;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{
    cluster::{ClusterConfig, ClusterManager},
    route::{RouteConfig, RouteManager},
    listener::{ListenerConfig, ListenerManager},
    ConfigCache,
};

use envoy_types::pb::envoy::config::{
    cluster::v3::Cluster,
    route::v3::RouteConfiguration,
    listener::v3::Listener,
};

/// Central configuration manager that handles conversions and caching
#[derive(Debug)]
pub struct ConfigurationManager {
    pub cluster_manager: ClusterManager,
    pub route_manager: RouteManager,
    pub listener_manager: ListenerManager,
    cache: ConfigCache,
}

impl ConfigurationManager {
    /// Create a new configuration manager
    pub fn new() -> Self {
        Self {
            cluster_manager: ClusterManager::new(),
            route_manager: RouteManager::new(),
            listener_manager: ListenerManager::new(),
            cache: ConfigCache::new(),
        }
    }

    /// Add or update a cluster configuration
    pub fn upsert_cluster(&mut self, config: ClusterConfig) -> Result<(), Error> {
        // Convert and store in cluster manager
        self.cluster_manager.upsert_cluster(config.clone())?;

        // Convert to envoy-types and update cache
        let envoy_cluster = config.to_envoy_cluster()?;
        self.cache.update_cluster(envoy_cluster.name.clone(), envoy_cluster);

        Ok(())
    }

    /// Add or update a route configuration
    pub fn upsert_route(&mut self, config: RouteConfig) -> Result<(), Error> {
        // Convert and store in route manager
        self.route_manager.upsert_route(config.clone())?;

        // Convert to envoy-types and update cache
        let envoy_route = config.to_envoy_route_configuration()?;
        self.cache.update_route(envoy_route.name.clone(), envoy_route);

        Ok(())
    }

    /// Add or update a listener configuration
    pub fn upsert_listener(&mut self, config: ListenerConfig) -> Result<(), Error> {
        // Convert and store in listener manager
        self.listener_manager.upsert_listener(config.clone())?;

        // Convert to envoy-types and update cache
        let envoy_listener = config.to_envoy_listener()?;
        self.cache.update_listener(envoy_listener.name.clone(), envoy_listener);

        Ok(())
    }

    /// Remove a cluster
    pub fn remove_cluster(&mut self, name: &str) -> Option<Cluster> {
        let removed = self.cluster_manager.remove_cluster(name);
        if removed.is_some() {
            // Update cache by rebuilding it
            self.rebuild_cluster_cache();
        }
        removed
    }

    /// Remove a route
    pub fn remove_route(&mut self, name: &str) -> Option<RouteConfiguration> {
        let removed = self.route_manager.remove_route(name);
        if removed.is_some() {
            // Update cache by rebuilding it
            self.rebuild_route_cache();
        }
        removed
    }

    /// Remove a listener
    pub fn remove_listener(&mut self, name: &str) -> Option<Listener> {
        let removed = self.listener_manager.remove_listener(name);
        if removed.is_some() {
            // Update cache by rebuilding it
            self.rebuild_listener_cache();
        }
        removed
    }

    /// Get the current configuration cache for xDS responses
    pub fn get_cache(&self) -> &ConfigCache {
        &self.cache
    }

    /// Get current configuration version
    pub fn get_version(&self) -> String {
        self.cache.version_string()
    }

    /// Get all clusters for xDS response
    pub fn get_clusters(&self) -> Vec<Cluster> {
        self.cache.get_clusters()
    }

    /// Get all routes for xDS response
    pub fn get_routes(&self) -> Vec<RouteConfiguration> {
        self.cache.get_routes()
    }

    /// Get all listeners for xDS response
    pub fn get_listeners(&self) -> Vec<Listener> {
        self.cache.get_listeners()
    }

    /// Rebuild cluster cache from cluster manager
    fn rebuild_cluster_cache(&mut self) {
        self.cache.clusters.clear();
        for cluster in self.cluster_manager.get_all_clusters() {
            self.cache.clusters.insert(cluster.name.clone(), cluster);
        }
        self.cache.version += 1;
    }

    /// Rebuild route cache from route manager
    fn rebuild_route_cache(&mut self) {
        self.cache.routes.clear();
        for route in self.route_manager.get_all_routes() {
            self.cache.routes.insert(route.name.clone(), route);
        }
        self.cache.version += 1;
    }

    /// Rebuild listener cache from listener manager
    fn rebuild_listener_cache(&mut self) {
        self.cache.listeners.clear();
        for listener in self.listener_manager.get_all_listeners() {
            self.cache.listeners.insert(listener.name.clone(), listener);
        }
        self.cache.version += 1;
    }

    /// Create a complete configuration snapshot
    pub fn create_snapshot(&self) -> ConfigurationSnapshot {
        ConfigurationSnapshot {
            version: self.get_version(),
            clusters: self.get_clusters(),
            routes: self.get_routes(),
            listeners: self.get_listeners(),
            timestamp: chrono::Utc::now(),
        }
    }
}

impl Default for ConfigurationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration snapshot for debugging and persistence
#[derive(Debug, Clone)]
pub struct ConfigurationSnapshot {
    pub version: String,
    pub clusters: Vec<Cluster>,
    pub routes: Vec<RouteConfiguration>,
    pub listeners: Vec<Listener>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ConfigurationSnapshot {
    /// Get the number of resources in this snapshot
    pub fn resource_count(&self) -> usize {
        self.clusters.len() + self.routes.len() + self.listeners.len()
    }

    /// Check if snapshot is empty
    pub fn is_empty(&self) -> bool {
        self.resource_count() == 0
    }
}

/// REST API wrapper for bulk configuration updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkConfigurationUpdate {
    pub clusters: Option<Vec<ClusterConfig>>,
    pub routes: Option<Vec<RouteConfig>>,
    pub listeners: Option<Vec<ListenerConfig>>,
}

impl ConfigurationManager {
    /// Apply bulk configuration updates
    pub fn apply_bulk_update(&mut self, update: BulkConfigurationUpdate) -> Result<ConfigurationSnapshot, Error> {
        // Apply cluster updates
        if let Some(clusters) = update.clusters {
            for cluster in clusters {
                self.upsert_cluster(cluster)?;
            }
        }

        // Apply route updates
        if let Some(routes) = update.routes {
            for route in routes {
                self.upsert_route(route)?;
            }
        }

        // Apply listener updates
        if let Some(listeners) = update.listeners {
            for listener in listeners {
                self.upsert_listener(listener)?;
            }
        }

        Ok(self.create_snapshot())
    }

    /// Validate configuration consistency
    pub fn validate_configuration(&self) -> Result<ValidationReport, Error> {
        let mut report = ValidationReport::new();

        // Validate cluster references in routes
        for route in &self.cache.routes {
            self.validate_route_cluster_references(&route.1, &mut report);
        }

        // Validate route references in listeners
        for listener in &self.cache.listeners {
            self.validate_listener_route_references(&listener.1, &mut report);
        }

        Ok(report)
    }

    fn validate_route_cluster_references(&self, route: &RouteConfiguration, report: &mut ValidationReport) {
        // TODO: Implement cluster reference validation in routes
        // This would check that all cluster references in route actions exist
    }

    fn validate_listener_route_references(&self, listener: &Listener, report: &mut ValidationReport) {
        // TODO: Implement route reference validation in listeners
        // This would check that all route config references in HTTP connection managers exist
    }
}

/// Configuration validation report
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub is_valid: bool,
}

impl ValidationReport {
    fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
            is_valid: true,
        }
    }

    pub fn add_error(&mut self, error: String) {
        self.errors.push(error);
        self.is_valid = false;
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xds::{
        cluster::{ClusterConfig, EndpointConfig, LoadBalancingPolicy},
        route::{RouteConfig, VirtualHostConfig, RouteRule, RouteMatchConfig, RouteActionConfig, PathMatch},
        listener::ListenerManager,
    };

    #[test]
    fn test_configuration_manager() {
        let mut manager = ConfigurationManager::new();

        // Add a cluster
        let cluster_config = ClusterConfig {
            name: "test-cluster".to_string(),
            endpoints: vec![
                EndpointConfig {
                    address: "127.0.0.1".to_string(),
                    port: 8080,
                    weight: Some(100),
                },
            ],
            load_balancing_policy: LoadBalancingPolicy::RoundRobin,
            connect_timeout: Some(5),
            health_checks: None,
        };

        manager.upsert_cluster(cluster_config).expect("Failed to add cluster");

        // Add a route
        let route_config = RouteConfig {
            name: "test-route".to_string(),
            virtual_hosts: vec![
                VirtualHostConfig {
                    name: "test-vhost".to_string(),
                    domains: vec!["example.com".to_string()],
                    routes: vec![
                        RouteRule {
                            name: Some("default".to_string()),
                            r#match: RouteMatchConfig {
                                path: PathMatch::Prefix("/".to_string()),
                                headers: None,
                                query_parameters: None,
                            },
                            action: RouteActionConfig::Cluster {
                                name: "test-cluster".to_string(),
                                timeout: None,
                                prefix_rewrite: None,
                                path_template_rewrite: None,
                                retry_policy: None,
                            },
                            typed_per_filter_config: HashMap::new(),
                        },
                    ],
                    typed_per_filter_config: HashMap::new(),
                },
            ],
        };

        manager.upsert_route(route_config).expect("Failed to add route");

        // Add a listener
        let listener_config = ListenerManager::create_http_listener(
            "test-listener".to_string(),
            "0.0.0.0".to_string(),
            8080,
            "test-route".to_string(),
        );

        manager.upsert_listener(listener_config).expect("Failed to add listener");

        // Verify configuration
        let snapshot = manager.create_snapshot();
        assert_eq!(snapshot.clusters.len(), 1);
        assert_eq!(snapshot.routes.len(), 1);
        assert_eq!(snapshot.listeners.len(), 1);
        assert!(!snapshot.is_empty());

        // Test removal
        manager.remove_cluster("test-cluster");
        let updated_snapshot = manager.create_snapshot();
        assert_eq!(updated_snapshot.clusters.len(), 0);
    }

    #[test]
    fn test_bulk_configuration_update() {
        let mut manager = ConfigurationManager::new();

        let bulk_update = BulkConfigurationUpdate {
            clusters: Some(vec![
                ClusterConfig {
                    name: "cluster1".to_string(),
                    endpoints: vec![
                        EndpointConfig {
                            address: "127.0.0.1".to_string(),
                            port: 8001,
                            weight: None,
                        },
                    ],
                    load_balancing_policy: LoadBalancingPolicy::Random,
                    connect_timeout: None,
                    health_checks: None,
                },
                ClusterConfig {
                    name: "cluster2".to_string(),
                    endpoints: vec![
                        EndpointConfig {
                            address: "127.0.0.1".to_string(),
                            port: 8002,
                            weight: None,
                        },
                    ],
                    load_balancing_policy: LoadBalancingPolicy::LeastRequest,
                    connect_timeout: None,
                    health_checks: None,
                },
            ]),
            routes: None,
            listeners: None,
        };

        let snapshot = manager.apply_bulk_update(bulk_update).expect("Failed to apply bulk update");
        assert_eq!(snapshot.clusters.len(), 2);
        assert_eq!(snapshot.routes.len(), 0);
        assert_eq!(snapshot.listeners.len(), 0);
    }
}
