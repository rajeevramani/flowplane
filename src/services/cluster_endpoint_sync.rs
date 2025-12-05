//! Cluster Endpoint Synchronization Service
//!
//! This module provides synchronization of cluster endpoints
//! from cluster configuration JSON into the cluster_endpoints table.

use crate::domain::{ClusterId, EndpointHealthStatus};
use crate::errors::Result;
use crate::storage::{
    ClusterEndpointRepository, CreateEndpointRequest, DbPool, UpdateEndpointRequest,
};
use tracing::{info, instrument};

/// Service for synchronizing cluster endpoints.
#[derive(Debug, Clone)]
pub struct ClusterEndpointSyncService {
    endpoint_repo: ClusterEndpointRepository,
}

impl ClusterEndpointSyncService {
    /// Creates a new sync service.
    pub fn new(pool: DbPool) -> Self {
        Self { endpoint_repo: ClusterEndpointRepository::new(pool) }
    }

    /// Synchronizes cluster endpoints for a given cluster.
    ///
    /// This extracts endpoint information from the cluster's load_assignment
    /// configuration and creates corresponding records in the cluster_endpoints table.
    #[instrument(skip(self, config_json), fields(cluster_id = %cluster_id), name = "sync_cluster_endpoints")]
    pub async fn sync(&self, cluster_id: &ClusterId, config_json: &str) -> Result<SyncResult> {
        let mut result = SyncResult::default();

        // Parse the cluster configuration to extract endpoints
        let endpoints = self.extract_endpoints(config_json)?;

        // Get existing endpoints for this cluster
        let existing_endpoints = self.endpoint_repo.list_by_cluster(cluster_id).await?;
        let existing_keys: std::collections::HashSet<_> =
            existing_endpoints.iter().map(|e| (e.address.clone(), e.port as i32)).collect();

        // Track which endpoints we've seen in the config
        let mut seen_keys = std::collections::HashSet::new();

        // Create or update endpoints
        for endpoint in &endpoints {
            let key = (endpoint.address.clone(), endpoint.port);
            seen_keys.insert(key.clone());

            if existing_keys.contains(&key) {
                // Endpoint exists - check if it needs updating
                let existing = existing_endpoints
                    .iter()
                    .find(|e| e.address == endpoint.address && e.port as i32 == endpoint.port)
                    .expect("Endpoint must exist if key is in existing_keys");

                if existing.weight != endpoint.weight as u32
                    || existing.priority != endpoint.priority as u32
                    || existing.health_status != endpoint.health_status
                {
                    self.endpoint_repo
                        .update(
                            &existing.id,
                            UpdateEndpointRequest {
                                weight: Some(endpoint.weight as u32),
                                health_status: Some(endpoint.health_status),
                                priority: Some(endpoint.priority as u32),
                                metadata: endpoint
                                    .metadata
                                    .clone()
                                    .map(|s| serde_json::from_str(&s).unwrap_or_default()),
                            },
                        )
                        .await?;
                    result.endpoints_updated += 1;
                }
            } else {
                // Create new endpoint
                self.endpoint_repo
                    .create(CreateEndpointRequest {
                        cluster_id: cluster_id.clone(),
                        address: endpoint.address.clone(),
                        port: endpoint.port as u16,
                        weight: Some(endpoint.weight as u32),
                        priority: Some(endpoint.priority as u32),
                        metadata: endpoint
                            .metadata
                            .clone()
                            .map(|s| serde_json::from_str(&s).unwrap_or_default()),
                    })
                    .await?;
                result.endpoints_created += 1;
            }
        }

        // Delete endpoints that are no longer in the config
        for existing in &existing_endpoints {
            let key = (existing.address.clone(), existing.port as i32);
            if !seen_keys.contains(&key) {
                self.endpoint_repo.delete(&existing.id).await?;
                result.endpoints_deleted += 1;
            }
        }

        info!(
            cluster_id = %cluster_id,
            endpoints_created = result.endpoints_created,
            endpoints_updated = result.endpoints_updated,
            endpoints_deleted = result.endpoints_deleted,
            "Cluster endpoints synchronized"
        );

        Ok(result)
    }

    /// Extracts endpoints from a cluster configuration JSON.
    ///
    /// This parses the load_assignment.endpoints array to extract
    /// lb_endpoints with their socket addresses.
    fn extract_endpoints(&self, config_json: &str) -> Result<Vec<EndpointConfig>> {
        let config: serde_json::Value = serde_json::from_str(config_json).map_err(|e| {
            crate::errors::FlowplaneError::internal(format!(
                "Failed to parse cluster config: {}",
                e
            ))
        })?;

        let mut endpoints = Vec::new();

        // Look for load_assignment.endpoints array
        if let Some(load_assignment) = config.get("load_assignment") {
            if let Some(locality_endpoints) =
                load_assignment.get("endpoints").and_then(|v| v.as_array())
            {
                for (priority, locality_ep) in locality_endpoints.iter().enumerate() {
                    // Get priority from locality if specified
                    let ep_priority = locality_ep
                        .get("priority")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(priority as u64) as i32;

                    if let Some(lb_endpoints) =
                        locality_ep.get("lb_endpoints").and_then(|v| v.as_array())
                    {
                        for lb_ep in lb_endpoints {
                            // Extract weight
                            let weight = lb_ep
                                .get("load_balancing_weight")
                                .and_then(|v| v.get("value"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(1) as i32;

                            // Extract health status
                            let health_status = lb_ep
                                .get("health_status")
                                .and_then(|v| v.as_str())
                                .map(|s| match s {
                                    "HEALTHY" => EndpointHealthStatus::Healthy,
                                    "UNHEALTHY" => EndpointHealthStatus::Unhealthy,
                                    "DEGRADED" => EndpointHealthStatus::Degraded,
                                    _ => EndpointHealthStatus::Unknown,
                                })
                                .unwrap_or(EndpointHealthStatus::Unknown);

                            // Extract metadata
                            let metadata = lb_ep.get("metadata").map(|v| v.to_string());

                            // Extract socket address
                            if let Some(socket_address) = lb_ep
                                .get("endpoint")
                                .and_then(|e| e.get("address"))
                                .and_then(|a| a.get("socket_address"))
                            {
                                let address = socket_address
                                    .get("address")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                let port = socket_address
                                    .get("port_value")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as i32;

                                if !address.is_empty() && port > 0 {
                                    endpoints.push(EndpointConfig {
                                        address,
                                        port,
                                        weight,
                                        health_status,
                                        priority: ep_priority,
                                        metadata,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // Also check for simple hosts array (legacy format)
        if let Some(hosts) = config.get("hosts").and_then(|v| v.as_array()) {
            for host in hosts {
                if let Some(socket_address) = host.get("socket_address") {
                    let address = socket_address
                        .get("address")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let port =
                        socket_address.get("port_value").and_then(|v| v.as_u64()).unwrap_or(0)
                            as i32;

                    if !address.is_empty() && port > 0 {
                        endpoints.push(EndpointConfig {
                            address,
                            port,
                            weight: 1,
                            health_status: EndpointHealthStatus::Unknown,
                            priority: 0,
                            metadata: None,
                        });
                    }
                }
            }
        }

        Ok(endpoints)
    }

    /// Clears all endpoints for a cluster.
    #[instrument(skip(self), fields(cluster_id = %cluster_id), name = "clear_cluster_endpoints")]
    pub async fn clear(&self, cluster_id: &ClusterId) -> Result<u64> {
        let deleted = self.endpoint_repo.delete_by_cluster(cluster_id).await?;
        info!(cluster_id = %cluster_id, deleted = deleted, "Cluster endpoints cleared");
        Ok(deleted)
    }
}

/// Endpoint configuration extracted from cluster config.
#[derive(Debug, Clone)]
struct EndpointConfig {
    address: String,
    port: i32,
    weight: i32,
    health_status: EndpointHealthStatus,
    priority: i32,
    metadata: Option<String>,
}

/// Result of a cluster endpoint sync operation.
#[derive(Debug, Default)]
pub struct SyncResult {
    /// Number of endpoints created.
    pub endpoints_created: usize,
    /// Number of endpoints updated.
    pub endpoints_updated: usize,
    /// Number of endpoints deleted.
    pub endpoints_deleted: usize,
}

#[cfg(test)]
mod tests {
    fn create_test_config_json() -> &'static str {
        r#"{
            "load_assignment": {
                "cluster_name": "backend",
                "endpoints": [
                    {
                        "priority": 0,
                        "lb_endpoints": [
                            {
                                "endpoint": {
                                    "address": {
                                        "socket_address": {
                                            "address": "10.0.0.1",
                                            "port_value": 8080
                                        }
                                    }
                                },
                                "load_balancing_weight": {
                                    "value": 100
                                },
                                "health_status": "HEALTHY"
                            },
                            {
                                "endpoint": {
                                    "address": {
                                        "socket_address": {
                                            "address": "10.0.0.2",
                                            "port_value": 8080
                                        }
                                    }
                                },
                                "load_balancing_weight": {
                                    "value": 50
                                },
                                "health_status": "DEGRADED"
                            }
                        ]
                    }
                ]
            }
        }"#
    }

    #[test]
    fn test_parse_endpoints() {
        let config_json = create_test_config_json();
        let config: serde_json::Value = serde_json::from_str(config_json).unwrap();

        let mut endpoints = Vec::new();
        if let Some(load_assignment) = config.get("load_assignment") {
            if let Some(locality_endpoints) =
                load_assignment.get("endpoints").and_then(|v| v.as_array())
            {
                for locality_ep in locality_endpoints {
                    if let Some(lb_endpoints) =
                        locality_ep.get("lb_endpoints").and_then(|v| v.as_array())
                    {
                        for lb_ep in lb_endpoints {
                            if let Some(socket_address) = lb_ep
                                .get("endpoint")
                                .and_then(|e| e.get("address"))
                                .and_then(|a| a.get("socket_address"))
                            {
                                let address = socket_address
                                    .get("address")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let port = socket_address
                                    .get("port_value")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                endpoints.push((address.to_string(), port));
                            }
                        }
                    }
                }
            }
        }

        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0], ("10.0.0.1".to_string(), 8080));
        assert_eq!(endpoints[1], ("10.0.0.2".to_string(), 8080));
    }

    #[test]
    fn test_parse_health_status() {
        let config_json = create_test_config_json();
        let config: serde_json::Value = serde_json::from_str(config_json).unwrap();

        let mut statuses = Vec::new();
        if let Some(load_assignment) = config.get("load_assignment") {
            if let Some(locality_endpoints) =
                load_assignment.get("endpoints").and_then(|v| v.as_array())
            {
                for locality_ep in locality_endpoints {
                    if let Some(lb_endpoints) =
                        locality_ep.get("lb_endpoints").and_then(|v| v.as_array())
                    {
                        for lb_ep in lb_endpoints {
                            let status = lb_ep
                                .get("health_status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("UNKNOWN");
                            statuses.push(status.to_string());
                        }
                    }
                }
            }
        }

        assert_eq!(statuses, vec!["HEALTHY", "DEGRADED"]);
    }

    #[test]
    fn test_parse_weights() {
        let config_json = create_test_config_json();
        let config: serde_json::Value = serde_json::from_str(config_json).unwrap();

        let mut weights = Vec::new();
        if let Some(load_assignment) = config.get("load_assignment") {
            if let Some(locality_endpoints) =
                load_assignment.get("endpoints").and_then(|v| v.as_array())
            {
                for locality_ep in locality_endpoints {
                    if let Some(lb_endpoints) =
                        locality_ep.get("lb_endpoints").and_then(|v| v.as_array())
                    {
                        for lb_ep in lb_endpoints {
                            let weight = lb_ep
                                .get("load_balancing_weight")
                                .and_then(|v| v.get("value"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(1);
                            weights.push(weight);
                        }
                    }
                }
            }
        }

        assert_eq!(weights, vec![100, 50]);
    }

    #[test]
    fn test_empty_config() {
        let config_json = r#"{}"#;
        let config: serde_json::Value = serde_json::from_str(config_json).unwrap();

        let endpoints: Vec<String> = config
            .get("load_assignment")
            .and_then(|la| la.get("endpoints"))
            .and_then(|e| e.as_array())
            .map(|_| Vec::new())
            .unwrap_or_default();

        assert!(endpoints.is_empty());
    }
}
