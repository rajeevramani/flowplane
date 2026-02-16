//! Cluster Endpoint Synchronization Service
//!
//! This module provides synchronization of cluster endpoints
//! from cluster configuration JSON into the cluster_endpoints table.

use crate::domain::{ClusterId, EndpointHealthStatus};
use crate::errors::{FlowplaneError, Result};
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
                    .ok_or_else(|| {
                        FlowplaneError::internal(format!(
                            "Endpoint inconsistency: {:?} found in keys but not in list",
                            key
                        ))
                    })?;

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
    /// Supports two formats:
    /// - Rust serde format: `endpoints` array with string ("host:port") or object ({host, port})
    /// - Raw Envoy xDS format: `load_assignment.endpoints[].lb_endpoints[].endpoint.address.socket_address`
    fn extract_endpoints(&self, config_json: &str) -> Result<Vec<EndpointConfig>> {
        let config: serde_json::Value = serde_json::from_str(config_json).map_err(|e| {
            crate::errors::FlowplaneError::internal(format!(
                "Failed to parse cluster config: {}",
                e
            ))
        })?;

        let mut endpoints = Vec::new();

        // Format 1: Rust serde serialized ClusterSpec
        // endpoints: ["host:port"] or [{"host": "...", "port": N}]
        if let Some(ep_array) = config.get("endpoints").and_then(|v| v.as_array()) {
            for ep in ep_array {
                let (address, port) = if let Some(s) = ep.as_str() {
                    // String format: "host:port"
                    let parts: Vec<&str> = s.split(':').collect();
                    if parts.len() == 2 {
                        let host = parts[0].trim().to_string();
                        let p = parts[1].trim().parse::<i32>().unwrap_or(0);
                        (host, p)
                    } else {
                        continue;
                    }
                } else if let Some(obj) = ep.as_object() {
                    // Object format: {"host": "...", "port": N}
                    let host = obj.get("host").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let p = obj.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as i32;
                    (host, p)
                } else {
                    continue;
                };

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

        // Format 2: Raw Envoy xDS load_assignment format
        if endpoints.is_empty() {
            if let Some(load_assignment) = config.get("load_assignment") {
                if let Some(locality_endpoints) =
                    load_assignment.get("endpoints").and_then(|v| v.as_array())
                {
                    for (priority, locality_ep) in locality_endpoints.iter().enumerate() {
                        let ep_priority = locality_ep
                            .get("priority")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(priority as u64)
                            as i32;

                        if let Some(lb_endpoints) =
                            locality_ep.get("lb_endpoints").and_then(|v| v.as_array())
                        {
                            for lb_ep in lb_endpoints {
                                let weight = lb_ep
                                    .get("load_balancing_weight")
                                    .and_then(|v| v.get("value"))
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(1)
                                    as i32;

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

                                let metadata = lb_ep.get("metadata").map(|v| v.to_string());

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
                                        .unwrap_or(0)
                                        as i32;

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
        }

        // Format 3: Legacy hosts array
        if endpoints.is_empty() {
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
    use super::*;

    // Rust serde format (what ClusterSpec actually serializes to)
    fn create_serde_string_config() -> &'static str {
        r#"{"endpoints": ["10.0.0.1:8080", "10.0.0.2:9090"]}"#
    }

    fn create_serde_object_config() -> &'static str {
        r#"{"endpoints": [{"host": "10.0.0.1", "port": 8080}, {"host": "10.0.0.2", "port": 9090}]}"#
    }

    // Raw Envoy xDS format
    fn create_xds_config_json() -> &'static str {
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

    /// Helper: extract endpoints using the service method
    fn parse_endpoints(config_json: &str) -> Vec<EndpointConfig> {
        let config: serde_json::Value = serde_json::from_str(config_json).unwrap();
        let mut endpoints = Vec::new();

        // Format 1: Serde endpoints array
        if let Some(ep_array) = config.get("endpoints").and_then(|v| v.as_array()) {
            for ep in ep_array {
                let (address, port) = if let Some(s) = ep.as_str() {
                    let parts: Vec<&str> = s.split(':').collect();
                    if parts.len() == 2 {
                        (parts[0].trim().to_string(), parts[1].trim().parse::<i32>().unwrap_or(0))
                    } else {
                        continue;
                    }
                } else if let Some(obj) = ep.as_object() {
                    let host = obj.get("host").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let p = obj.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as i32;
                    (host, p)
                } else {
                    continue;
                };

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

        // Format 2: xDS load_assignment
        if endpoints.is_empty() {
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
                                        .unwrap_or("")
                                        .to_string();
                                    let port = socket_address
                                        .get("port_value")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
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
                    }
                }
            }
        }

        endpoints
    }

    #[test]
    fn test_parse_serde_string_endpoints() {
        let endpoints = parse_endpoints(create_serde_string_config());
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].address, "10.0.0.1");
        assert_eq!(endpoints[0].port, 8080);
        assert_eq!(endpoints[1].address, "10.0.0.2");
        assert_eq!(endpoints[1].port, 9090);
    }

    #[test]
    fn test_parse_serde_object_endpoints() {
        let endpoints = parse_endpoints(create_serde_object_config());
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].address, "10.0.0.1");
        assert_eq!(endpoints[0].port, 8080);
        assert_eq!(endpoints[1].address, "10.0.0.2");
        assert_eq!(endpoints[1].port, 9090);
    }

    #[test]
    fn test_parse_xds_endpoints() {
        let endpoints = parse_endpoints(create_xds_config_json());
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].address, "10.0.0.1");
        assert_eq!(endpoints[0].port, 8080);
        assert_eq!(endpoints[1].address, "10.0.0.2");
        assert_eq!(endpoints[1].port, 8080);
    }

    #[test]
    fn test_parse_xds_health_status() {
        let config: serde_json::Value = serde_json::from_str(create_xds_config_json()).unwrap();
        let lb_endpoints =
            config["load_assignment"]["endpoints"][0]["lb_endpoints"].as_array().unwrap();

        let statuses: Vec<&str> = lb_endpoints
            .iter()
            .map(|ep| ep.get("health_status").and_then(|v| v.as_str()).unwrap_or("UNKNOWN"))
            .collect();

        assert_eq!(statuses, vec!["HEALTHY", "DEGRADED"]);
    }

    #[test]
    fn test_parse_xds_weights() {
        let config: serde_json::Value = serde_json::from_str(create_xds_config_json()).unwrap();
        let lb_endpoints =
            config["load_assignment"]["endpoints"][0]["lb_endpoints"].as_array().unwrap();

        let weights: Vec<u64> = lb_endpoints
            .iter()
            .map(|ep| {
                ep.get("load_balancing_weight")
                    .and_then(|v| v.get("value"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1)
            })
            .collect();

        assert_eq!(weights, vec![100, 50]);
    }

    #[test]
    fn test_empty_config() {
        let endpoints = parse_endpoints(r#"{}"#);
        assert!(endpoints.is_empty());
    }

    #[test]
    fn test_empty_endpoints_array() {
        let endpoints = parse_endpoints(r#"{"endpoints": []}"#);
        assert!(endpoints.is_empty());
    }
}
