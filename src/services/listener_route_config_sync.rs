//! Listener-Route Synchronization Service
//!
//! This module provides synchronization of listener-route relationships
//! by extracting route references from listener HCM configuration.

use crate::domain::{ListenerId, RouteConfigId};
use crate::errors::Result;
use crate::storage::{DbPool, ListenerRouteConfigRepository, RouteConfigRepository};
use tracing::{debug, info, instrument};

/// Service for synchronizing listener-route config relationships.
#[derive(Debug, Clone)]
pub struct ListenerRouteSyncService {
    listener_route_config_repo: ListenerRouteConfigRepository,
    route_config_repo: RouteConfigRepository,
}

impl ListenerRouteSyncService {
    /// Creates a new sync service.
    pub fn new(pool: DbPool) -> Self {
        Self {
            listener_route_config_repo: ListenerRouteConfigRepository::new(pool.clone()),
            route_config_repo: RouteConfigRepository::new(pool),
        }
    }

    /// Synchronizes listener-route config relationships for a given listener.
    ///
    /// This extracts route_config_name references from the listener's HCM filter chains
    /// and creates corresponding records in the listener_route_configs table.
    #[instrument(skip(self, config_json), fields(listener_id = %listener_id), name = "sync_listener_routes")]
    pub async fn sync(&self, listener_id: &ListenerId, config_json: &str) -> Result<SyncResult> {
        let mut result = SyncResult::default();

        // Parse the listener configuration to extract route config names
        let route_config_names = self.extract_route_names(config_json)?;

        // Delete existing listener-route config relationships for this listener
        let deleted = self.listener_route_config_repo.delete_by_listener(listener_id).await?;
        result.routes_deleted = deleted as usize;

        // Create new listener-route config relationships
        for (order, route_config_name) in route_config_names.iter().enumerate() {
            match self.route_config_repo.get_by_name(route_config_name).await {
                Ok(route_config) => {
                    self.listener_route_config_repo
                        .create(listener_id, &route_config.id, order as i64)
                        .await?;
                    result.routes_created += 1;
                }
                Err(_) => {
                    // Route config doesn't exist yet, skip it
                    debug!(
                        listener_id = %listener_id,
                        route_config_name = %route_config_name,
                        "Route config not found, skipping listener-route config link"
                    );
                    result.routes_skipped += 1;
                }
            }
        }

        info!(
            listener_id = %listener_id,
            routes_created = result.routes_created,
            routes_deleted = result.routes_deleted,
            routes_skipped = result.routes_skipped,
            "Listener-route relationships synchronized"
        );

        Ok(result)
    }

    /// Extracts route names from a listener configuration JSON.
    ///
    /// This parses the filter_chains looking for http_connection_manager filters
    /// that reference route configurations. Supports two formats:
    /// - Rust serde format: `filter_type.HttpConnectionManager.route_config_name`
    /// - Raw Envoy xDS format: `typed_config.rds.route_config_name`
    fn extract_route_names(&self, config_json: &str) -> Result<Vec<String>> {
        let config: serde_json::Value = serde_json::from_str(config_json).map_err(|e| {
            crate::errors::FlowplaneError::internal(format!(
                "Failed to parse listener config: {}",
                e
            ))
        })?;

        let mut route_names = Vec::new();

        // Look for filter_chains array
        if let Some(filter_chains) = config.get("filter_chains").and_then(|v| v.as_array()) {
            for filter_chain in filter_chains {
                if let Some(filters) = filter_chain.get("filters").and_then(|v| v.as_array()) {
                    for filter in filters {
                        // Format 1: Rust serde serialized ListenerConfig
                        // filter_type.HttpConnectionManager.route_config_name
                        if let Some(route_name) = filter
                            .get("filter_type")
                            .and_then(|ft| ft.get("HttpConnectionManager"))
                            .and_then(|hcm| hcm.get("route_config_name"))
                            .and_then(|v| v.as_str())
                        {
                            if !route_names.contains(&route_name.to_string()) {
                                route_names.push(route_name.to_string());
                            }
                            continue;
                        }

                        // Format 2: Raw Envoy xDS JSON
                        // typed_config.rds.route_config_name
                        let is_hcm = filter.get("name").and_then(|v| v.as_str())
                            == Some("envoy.filters.network.http_connection_manager");

                        if is_hcm {
                            if let Some(route_name) = filter
                                .get("typed_config")
                                .and_then(|tc| tc.get("rds"))
                                .and_then(|rds| rds.get("route_config_name"))
                                .and_then(|v| v.as_str())
                            {
                                if !route_names.contains(&route_name.to_string()) {
                                    route_names.push(route_name.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Also check for a simpler structure where route_config_name is at the top level
        // (used by some simpler configurations)
        if let Some(route_name) = config.get("route_config_name").and_then(|v| v.as_str()) {
            if !route_names.contains(&route_name.to_string()) {
                route_names.push(route_name.to_string());
            }
        }

        Ok(route_names)
    }

    /// Clears all route config relationships for a listener.
    #[instrument(skip(self), fields(listener_id = %listener_id), name = "clear_listener_routes")]
    pub async fn clear(&self, listener_id: &ListenerId) -> Result<u64> {
        let deleted = self.listener_route_config_repo.delete_by_listener(listener_id).await?;
        info!(listener_id = %listener_id, deleted = deleted, "Listener-route config relationships cleared");
        Ok(deleted)
    }

    /// Gets all route config IDs for a listener.
    pub async fn get_route_configs_for_listener(
        &self,
        listener_id: &ListenerId,
    ) -> Result<Vec<RouteConfigId>> {
        self.listener_route_config_repo.list_route_config_ids_by_listener(listener_id).await
    }

    /// Gets all listener IDs using a specific route config.
    pub async fn get_listeners_for_route_config(
        &self,
        route_config_id: &RouteConfigId,
    ) -> Result<Vec<ListenerId>> {
        self.listener_route_config_repo.list_listener_ids_by_route_config(route_config_id).await
    }
}

/// Result of a listener-route sync operation.
#[derive(Debug, Default)]
pub struct SyncResult {
    /// Number of listener-route relationships created.
    pub routes_created: usize,
    /// Number of listener-route relationships deleted.
    pub routes_deleted: usize,
    /// Number of routes skipped (not found in database).
    pub routes_skipped: usize,
}

#[cfg(test)]
mod tests {
    // Rust serde format (what ListenerConfig actually serializes to)
    fn create_serde_config_json() -> &'static str {
        r#"{
            "name": "test-listener",
            "address": "0.0.0.0",
            "port": 8080,
            "filter_chains": [
                {
                    "name": "default",
                    "filters": [
                        {
                            "name": "envoy.filters.network.http_connection_manager",
                            "filter_type": {
                                "HttpConnectionManager": {
                                    "route_config_name": "main-route",
                                    "inline_route_config": null,
                                    "access_log": null,
                                    "tracing": null,
                                    "http_filters": []
                                }
                            }
                        }
                    ],
                    "tls_context": null
                }
            ]
        }"#
    }

    // Raw Envoy xDS format
    fn create_xds_config_json() -> &'static str {
        r#"{
            "filter_chains": [
                {
                    "filters": [
                        {
                            "name": "envoy.filters.network.http_connection_manager",
                            "typed_config": {
                                "@type": "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager",
                                "stat_prefix": "ingress_http",
                                "rds": {
                                    "config_source": {
                                        "ads": {}
                                    },
                                    "route_config_name": "main-route"
                                }
                            }
                        }
                    ]
                }
            ]
        }"#
    }

    fn create_multi_route_serde_json() -> &'static str {
        r#"{
            "name": "multi-listener",
            "address": "0.0.0.0",
            "port": 8080,
            "filter_chains": [
                {
                    "name": "chain-1",
                    "filters": [
                        {
                            "name": "envoy.filters.network.http_connection_manager",
                            "filter_type": {
                                "HttpConnectionManager": {
                                    "route_config_name": "api-route",
                                    "inline_route_config": null,
                                    "access_log": null,
                                    "tracing": null,
                                    "http_filters": []
                                }
                            }
                        }
                    ],
                    "tls_context": null
                },
                {
                    "name": "chain-2",
                    "filters": [
                        {
                            "name": "envoy.filters.network.http_connection_manager",
                            "filter_type": {
                                "HttpConnectionManager": {
                                    "route_config_name": "web-route",
                                    "inline_route_config": null,
                                    "access_log": null,
                                    "tracing": null,
                                    "http_filters": []
                                }
                            }
                        }
                    ],
                    "tls_context": null
                }
            ]
        }"#
    }

    fn create_multi_route_xds_json() -> &'static str {
        r#"{
            "filter_chains": [
                {
                    "filters": [
                        {
                            "name": "envoy.filters.network.http_connection_manager",
                            "typed_config": {
                                "rds": {
                                    "route_config_name": "api-route"
                                }
                            }
                        }
                    ]
                },
                {
                    "filters": [
                        {
                            "name": "envoy.filters.network.http_connection_manager",
                            "typed_config": {
                                "rds": {
                                    "route_config_name": "web-route"
                                }
                            }
                        }
                    ]
                }
            ]
        }"#
    }

    /// Helper: parse route names using the same logic as extract_route_names
    fn parse_route_names(config_json: &str) -> Vec<String> {
        let config: serde_json::Value = serde_json::from_str(config_json).unwrap();
        let mut route_names = Vec::new();

        if let Some(filter_chains) = config.get("filter_chains").and_then(|v| v.as_array()) {
            for filter_chain in filter_chains {
                if let Some(filters) = filter_chain.get("filters").and_then(|v| v.as_array()) {
                    for filter in filters {
                        // Format 1: Rust serde format
                        if let Some(route_name) = filter
                            .get("filter_type")
                            .and_then(|ft| ft.get("HttpConnectionManager"))
                            .and_then(|hcm| hcm.get("route_config_name"))
                            .and_then(|v| v.as_str())
                        {
                            if !route_names.contains(&route_name.to_string()) {
                                route_names.push(route_name.to_string());
                            }
                            continue;
                        }

                        // Format 2: Raw Envoy xDS format
                        let is_hcm = filter.get("name").and_then(|v| v.as_str())
                            == Some("envoy.filters.network.http_connection_manager");

                        if is_hcm {
                            if let Some(route_name) = filter
                                .get("typed_config")
                                .and_then(|tc| tc.get("rds"))
                                .and_then(|rds| rds.get("route_config_name"))
                                .and_then(|v| v.as_str())
                            {
                                if !route_names.contains(&route_name.to_string()) {
                                    route_names.push(route_name.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        route_names
    }

    #[test]
    fn test_extract_single_route_serde_format() {
        let route_names = parse_route_names(create_serde_config_json());
        assert_eq!(route_names, vec!["main-route"]);
    }

    #[test]
    fn test_extract_single_route_xds_format() {
        let route_names = parse_route_names(create_xds_config_json());
        assert_eq!(route_names, vec!["main-route"]);
    }

    #[test]
    fn test_extract_multiple_routes_serde_format() {
        let route_names = parse_route_names(create_multi_route_serde_json());
        assert_eq!(route_names, vec!["api-route", "web-route"]);
    }

    #[test]
    fn test_extract_multiple_routes_xds_format() {
        let route_names = parse_route_names(create_multi_route_xds_json());
        assert_eq!(route_names, vec!["api-route", "web-route"]);
    }

    #[test]
    fn test_extract_no_routes() {
        let route_names = parse_route_names(r#"{"filter_chains": []}"#);
        assert!(route_names.is_empty());
    }

    #[test]
    fn test_extract_tcp_proxy_has_no_routes() {
        let config = r#"{
            "name": "tcp-listener",
            "address": "0.0.0.0",
            "port": 9090,
            "filter_chains": [
                {
                    "name": "default",
                    "filters": [
                        {
                            "name": "envoy.filters.network.tcp_proxy",
                            "filter_type": {
                                "TcpProxy": {
                                    "cluster": "some-cluster",
                                    "access_log": null
                                }
                            }
                        }
                    ],
                    "tls_context": null
                }
            ]
        }"#;
        let route_names = parse_route_names(config);
        assert!(route_names.is_empty());
    }
}
