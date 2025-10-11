use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::xds::resources::{
    clusters_from_config, clusters_from_database_entries, listeners_from_config,
    listeners_from_database_entries, resources_from_api_definitions, routes_from_config,
    routes_from_database_entries, BuiltResource, CLUSTER_TYPE_URL, LISTENER_TYPE_URL,
    ROUTE_TYPE_URL,
};
use crate::{
    config::SimpleXdsConfig,
    storage::{
        ApiDefinitionRepository, ClusterRepository, DbPool, ListenerRepository, RouteRepository,
    },
    Result,
};
use envoy_types::pb::google::protobuf::Any;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Cached Envoy resource along with metadata required for delta semantics.
#[derive(Clone, Debug)]
pub struct CachedResource {
    pub name: String,
    pub type_url: String,
    pub version: u64,
    pub body: Any,
}

impl CachedResource {
    pub fn new(name: String, type_url: String, version: u64, body: Any) -> Self {
        Self { name, type_url, version, body }
    }
}

/// Delta information for a single type URL.
#[derive(Clone, Debug, Default)]
pub struct ResourceDelta {
    pub type_url: String,
    pub added_or_updated: Vec<CachedResource>,
    pub removed: Vec<String>,
}

/// Broadcast payload describing all resources changed in a particular update.
#[derive(Clone, Debug, Default)]
pub struct ResourceUpdate {
    pub version: u64,
    pub deltas: Vec<ResourceDelta>,
}

/// Shared xDS server state, providing configuration, persistence access, and
/// cached resource snapshots for delta streaming.
#[derive(Debug)]
pub struct XdsState {
    pub config: SimpleXdsConfig,
    pub version: Arc<std::sync::atomic::AtomicU64>,
    pub cluster_repository: Option<ClusterRepository>,
    pub route_repository: Option<RouteRepository>,
    pub listener_repository: Option<ListenerRepository>,
    pub api_definition_repository: Option<ApiDefinitionRepository>,
    update_tx: broadcast::Sender<Arc<ResourceUpdate>>,
    resource_caches: RwLock<HashMap<String, HashMap<String, CachedResource>>>,
}

impl XdsState {
    pub fn new(config: SimpleXdsConfig) -> Self {
        let (update_tx, _) = broadcast::channel(128);
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: None,
            route_repository: None,
            listener_repository: None,
            api_definition_repository: None,
            update_tx,
            resource_caches: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_database(config: SimpleXdsConfig, pool: DbPool) -> Self {
        let (update_tx, _) = broadcast::channel(128);
        let cluster_repository = ClusterRepository::new(pool.clone());
        let route_repository = RouteRepository::new(pool.clone());
        let listener_repository = ListenerRepository::new(pool.clone());
        let api_definition_repository = ApiDefinitionRepository::new(pool);
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: Some(cluster_repository),
            route_repository: Some(route_repository),
            listener_repository: Some(listener_repository),
            api_definition_repository: Some(api_definition_repository),
            update_tx,
            resource_caches: RwLock::new(HashMap::new()),
        }
    }

    pub fn get_version(&self) -> String {
        self.version.load(std::sync::atomic::Ordering::Relaxed).to_string()
    }

    pub fn get_version_number(&self) -> u64 {
        self.version.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Apply a new snapshot of built resources for `type_url` and broadcast changes.
    /// Returns `Some(ResourceUpdate)` when a delta was published.
    pub fn apply_built_resources(
        &self,
        type_url: &str,
        built_resources: Vec<BuiltResource>,
    ) -> Option<Arc<ResourceUpdate>> {
        let mut caches = self.resource_caches.write().expect("resource cache lock poisoned");
        let cache = caches.entry(type_url.to_string()).or_default();

        let incoming_names: HashSet<String> =
            built_resources.iter().map(|resource| resource.name.clone()).collect();

        let removed: Vec<String> = cache
            .keys()
            .filter(|existing_name| !incoming_names.contains(*existing_name))
            .cloned()
            .collect();

        let mut pending_updates: Vec<BuiltResource> = Vec::new();

        for built in built_resources {
            match cache.get(&built.name) {
                Some(existing) if existing.body == built.resource => {}
                _ => pending_updates.push(built),
            }
        }

        if pending_updates.is_empty() && removed.is_empty() {
            return None;
        }

        let new_version = self.version.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;

        for name in &removed {
            cache.remove(name);
        }

        let mut delta =
            ResourceDelta { type_url: type_url.to_string(), added_or_updated: Vec::new(), removed };

        for built in pending_updates {
            let cached = CachedResource::new(
                built.name.clone(),
                type_url.to_string(),
                new_version,
                built.resource.clone(),
            );
            cache.insert(built.name.clone(), cached.clone());
            delta.added_or_updated.push(cached);
        }

        let update = Arc::new(ResourceUpdate { version: new_version, deltas: vec![delta] });

        let _ = self.update_tx.send(update.clone());
        Some(update)
    }

    pub fn subscribe_updates(&self) -> broadcast::Receiver<Arc<ResourceUpdate>> {
        self.update_tx.subscribe()
    }

    /// Return a clone of the cached resources for the provided type URL.
    pub fn cached_resources(&self, type_url: &str) -> Vec<CachedResource> {
        let caches = self.resource_caches.read().expect("resource cache lock poisoned");
        caches.get(type_url).map(|cache| cache.values().cloned().collect()).unwrap_or_default()
    }

    /// Refresh the cluster cache from the backing repository (if available).
    pub async fn refresh_clusters_from_repository(&self) -> Result<()> {
        let repository = match &self.cluster_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        let cluster_rows = repository.list(Some(1000), None).await?;

        let built = if cluster_rows.is_empty() {
            clusters_from_config(&self.config)?
        } else {
            clusters_from_database_entries(cluster_rows, "cache_refresh")?
        };

        let total_resources = built.len();
        match self.apply_built_resources(CLUSTER_TYPE_URL, built) {
            Some(update) => {
                for delta in &update.deltas {
                    info!(
                        phase = "cache_refresh",
                        type_url = %delta.type_url,
                        added = delta.added_or_updated.len(),
                        removed = delta.removed.len(),
                        version = update.version,
                        total_resources,
                        "Cache refresh produced delta"
                    );
                }
            }
            None => {
                debug!(
                    phase = "cache_refresh",
                    type_url = CLUSTER_TYPE_URL,
                    total_resources,
                    "Cache refresh detected no changes"
                );
            }
        }
        Ok(())
    }

    /// Refresh the route cache from the backing repository (if available).
    pub async fn refresh_routes_from_repository(&self) -> Result<()> {
        let repository = match &self.route_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        let route_rows = repository.list(Some(1000), None).await?;

        let mut built = if route_rows.is_empty() {
            routes_from_config(&self.config)?
        } else {
            routes_from_database_entries(route_rows, "cache_refresh")?
        };

        // IMPORTANT: Merge Platform API route configs with native routes
        // Platform API routes are generated dynamically and not stored in the routes table
        // If we don't merge them, they will be removed from the cache
        if let Some(api_repo) = &self.api_definition_repository {
            match (
                api_repo.list_definitions(None, None, None).await,
                api_repo.list_all_routes().await,
            ) {
                (Ok(definitions), Ok(api_routes)) if !definitions.is_empty() => {
                    match resources_from_api_definitions(definitions, api_routes) {
                        Ok(platform_resources) => {
                            // Only include route resources (skip clusters and listeners)
                            let platform_routes: Vec<_> = platform_resources
                                .into_iter()
                                .filter(|res| res.type_url() == ROUTE_TYPE_URL)
                                .collect();
                            built.extend(platform_routes);
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to build Platform API routes during refresh");
                        }
                    }
                }
                _ => {}
            }
        }

        let total_resources = built.len();
        match self.apply_built_resources(ROUTE_TYPE_URL, built) {
            Some(update) => {
                for delta in &update.deltas {
                    info!(
                        phase = "cache_refresh",
                        type_url = %delta.type_url,
                        added = delta.added_or_updated.len(),
                        removed = delta.removed.len(),
                        version = update.version,
                        total_resources,
                        "Route cache refresh produced delta"
                    );
                }
            }
            None => {
                debug!(
                    phase = "cache_refresh",
                    type_url = ROUTE_TYPE_URL,
                    total_resources,
                    "Route cache refresh detected no changes"
                );
            }
        }

        Ok(())
    }

    pub async fn refresh_platform_api_resources(&self) -> Result<()> {
        let repository = match &self.api_definition_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        let definitions = repository.list_definitions(None, None, None).await?;
        let routes = repository.list_all_routes().await?;

        if definitions.is_empty() {
            return Ok(());
        }

        let built = resources_from_api_definitions(definitions, routes)?;
        if built.is_empty() {
            return Ok(());
        }

        // IMPORTANT: Merge native routes with Platform API routes
        // Native routes are stored in the routes table, Platform API routes are generated dynamically
        // If we don't merge them, native routes will be removed from the cache
        let mut route_resources: Vec<_> =
            built.iter().filter(|res| res.type_url() == ROUTE_TYPE_URL).cloned().collect();

        if !route_resources.is_empty() {
            // Merge native routes from the database
            if let Some(route_repo) = &self.route_repository {
                match route_repo.list(Some(1000), None).await {
                    Ok(route_rows) if !route_rows.is_empty() => {
                        match routes_from_database_entries(route_rows, "platform_api_refresh") {
                            Ok(native_routes) => {
                                route_resources.extend(native_routes);
                            }
                            Err(e) => {
                                warn!(error = %e, "Failed to load native routes during Platform API refresh");
                            }
                        }
                    }
                    _ => {}
                }
            }

            self.apply_built_resources(ROUTE_TYPE_URL, route_resources);
        }

        let listener_resources: Vec<_> =
            built.iter().filter(|res| res.type_url() == LISTENER_TYPE_URL).cloned().collect();
        if !listener_resources.is_empty() {
            self.apply_built_resources(LISTENER_TYPE_URL, listener_resources);
        }

        // NOTE: Cluster resources are NOT sent here to avoid duplicates
        // Platform API clusters are created in the database by materialize_native_resources()
        // and loaded via refresh_clusters_from_repository(), just like listeners.
        // If we send them here too, Envoy will reject the update with "duplicate cluster" errors.

        Ok(())
    }

    /// Refresh the listener cache from the backing repository (if available).
    pub async fn refresh_listeners_from_repository(&self) -> Result<()> {
        let repository = match &self.listener_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        let listener_rows = repository.list(Some(1000), None).await?;

        let built = if listener_rows.is_empty() {
            listeners_from_config(&self.config)?
        } else {
            listeners_from_database_entries(listener_rows, "cache_refresh")?
        };

        let total_resources = built.len();
        match self.apply_built_resources(LISTENER_TYPE_URL, built) {
            Some(update) => {
                for delta in &update.deltas {
                    info!(
                        phase = "cache_refresh",
                        type_url = %delta.type_url,
                        added = delta.added_or_updated.len(),
                        removed = delta.removed.len(),
                        version = update.version,
                        total_resources,
                        "Listener cache refresh produced delta"
                    );
                }
            }
            None => {
                debug!(
                    phase = "cache_refresh",
                    type_url = LISTENER_TYPE_URL,
                    total_resources,
                    "Listener cache refresh detected no changes"
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::XdsResourceConfig;

    fn build_state() -> XdsState {
        let config = SimpleXdsConfig {
            resources: XdsResourceConfig {
                cluster_name: "test_cluster".into(),
                route_name: "test_route".into(),
                listener_name: "test_listener".into(),
                backend_address: "127.0.0.1".into(),
                backend_port: 9090,
                listener_port: 10000,
            },
            ..Default::default()
        };
        XdsState::new(config)
    }

    fn fake_resource(name: &str, payload: &[u8]) -> BuiltResource {
        BuiltResource {
            name: name.to_string(),
            resource: Any { type_url: CLUSTER_TYPE_URL.to_string(), value: payload.to_vec() },
        }
    }

    #[tokio::test]
    async fn apply_built_resources_tracks_add_update_and_remove() {
        let state = build_state();
        let mut receiver = state.subscribe_updates();

        // Initial add: expect update with single resource
        state
            .apply_built_resources(CLUSTER_TYPE_URL, vec![fake_resource("cluster-1", b"payload-1")])
            .expect("update expected");

        let update = receiver.recv().await.expect("first update");
        assert_eq!(update.version, 2); // initial value is 1, incremented to 2
        assert_eq!(update.deltas.len(), 1);
        let delta = &update.deltas[0];
        assert_eq!(delta.added_or_updated.len(), 1);
        assert!(delta.removed.is_empty());

        // Re-applying the same snapshot should produce no new update
        assert!(
            state
                .apply_built_resources(
                    CLUSTER_TYPE_URL,
                    vec![fake_resource("cluster-1", b"payload-1")],
                )
                .is_none()
        );

        // Updating payload introduces another delta
        state
            .apply_built_resources(CLUSTER_TYPE_URL, vec![fake_resource("cluster-1", b"payload-2")])
            .expect("update expected");

        let update = receiver.recv().await.expect("second update");
        assert_eq!(update.deltas[0].added_or_updated.len(), 1);

        // Removing the resource emits a removal delta
        state.apply_built_resources(CLUSTER_TYPE_URL, Vec::new()).expect("removal delta expected");

        let update = receiver.recv().await.expect("third update");
        assert_eq!(update.deltas[0].removed, vec!["cluster-1".to_string()]);
    }

    #[tokio::test]
    async fn broadcast_updates_to_multiple_subscribers() {
        let state = build_state();
        let mut rx1 = state.subscribe_updates();
        let mut rx2 = state.subscribe_updates();

        let _ = state
            .apply_built_resources(CLUSTER_TYPE_URL, vec![fake_resource("cluster-1", b"payload")]);

        let update1 = rx1.recv().await.expect("subscriber one update");
        let update2 = rx2.recv().await.expect("subscriber two update");

        assert_eq!(update1.version, update2.version);
        assert_eq!(update1.deltas[0].added_or_updated.len(), 1);
        assert_eq!(update2.deltas[0].added_or_updated.len(), 1);
    }

    #[tokio::test]
    async fn refresh_listeners_without_repository_is_noop() {
        let state = build_state();
        assert!(state.refresh_listeners_from_repository().await.is_ok());
    }
}
