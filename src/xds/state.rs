use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::domain::filter_schema::FilterSchemaRegistry;
use crate::secrets::backends::SecretBackendRegistry;
use crate::xds::resources::{
    clusters_from_config, clusters_from_database_entries, create_ext_proc_cluster,
    listeners_from_config, listeners_from_database_entries, routes_from_config,
    routes_from_database_entries, BuiltResource, CLUSTER_TYPE_URL, LISTENER_TYPE_URL,
    ROUTE_TYPE_URL,
};
use crate::xds::secret::{secrets_from_database_entries, SECRET_TYPE_URL};
use crate::{
    config::SimpleXdsConfig,
    services::{LearningSessionService, RouteHierarchySyncService, SecretEncryption},
    storage::{
        AggregatedSchemaRepository, ClusterRepository, DbPool, FilterRepository,
        ListenerAutoFilterRepository, ListenerRepository, RouteConfigRepository,
        RouteFilterRepository, RouteRepository, SecretRepository, VirtualHostFilterRepository,
        VirtualHostRepository,
    },
    xds::services::{
        access_log_service::FlowplaneAccessLogService, ext_proc_service::FlowplaneExtProcService,
    },
    Result,
};
use envoy_types::pb::google::protobuf::Any;
use tokio::sync::broadcast;
use tracing::{debug, info, instrument, warn};

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
    pub route_config_repository: Option<RouteConfigRepository>,
    pub listener_repository: Option<ListenerRepository>,
    pub filter_repository: Option<FilterRepository>,
    pub listener_auto_filter_repository: Option<ListenerAutoFilterRepository>,
    pub aggregated_schema_repository: Option<AggregatedSchemaRepository>,
    // Hierarchical filter attachment repositories
    pub virtual_host_repository: Option<VirtualHostRepository>,
    pub route_repository: Option<RouteRepository>,
    pub virtual_host_filter_repository: Option<VirtualHostFilterRepository>,
    pub route_filter_repository: Option<RouteFilterRepository>,
    // Sync services
    pub route_hierarchy_sync_service: Option<RouteHierarchySyncService>,
    pub access_log_service: Option<Arc<FlowplaneAccessLogService>>,
    pub ext_proc_service: Option<Arc<FlowplaneExtProcService>>,
    pub learning_session_service: RwLock<Option<Arc<LearningSessionService>>>,
    /// Secret repository for SDS (Secret Discovery Service)
    pub secret_repository: Option<SecretRepository>,
    /// Encryption service for secret data
    pub encryption_service: Option<Arc<SecretEncryption>>,
    /// Secret backend registry for external secrets (Vault, AWS, GCP)
    pub secret_backend_registry: Option<SecretBackendRegistry>,
    /// Dynamic filter schema registry for schema-driven filter conversion
    pub filter_schema_registry: FilterSchemaRegistry,
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
            route_config_repository: None,
            listener_repository: None,
            filter_repository: None,
            listener_auto_filter_repository: None,
            aggregated_schema_repository: None,
            virtual_host_repository: None,
            route_repository: None,
            virtual_host_filter_repository: None,
            route_filter_repository: None,
            route_hierarchy_sync_service: None,
            access_log_service: None,
            ext_proc_service: None,
            learning_session_service: RwLock::new(None),
            secret_repository: None,
            encryption_service: None,
            secret_backend_registry: None,
            filter_schema_registry: FilterSchemaRegistry::with_builtin_schemas(),
            update_tx,
            resource_caches: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_database(config: SimpleXdsConfig, pool: DbPool) -> Self {
        let (update_tx, _) = broadcast::channel(128);
        let cluster_repository = ClusterRepository::new(pool.clone());
        let route_config_repository = RouteConfigRepository::new(pool.clone());
        let listener_repository = ListenerRepository::new(pool.clone());
        let filter_repository = FilterRepository::new(pool.clone());
        let listener_auto_filter_repository = ListenerAutoFilterRepository::new(pool.clone());
        let aggregated_schema_repository = AggregatedSchemaRepository::new(pool.clone());
        // Hierarchical filter attachment repositories
        let virtual_host_repository = VirtualHostRepository::new(pool.clone());
        let route_repository = RouteRepository::new(pool.clone());
        let virtual_host_filter_repository = VirtualHostFilterRepository::new(pool.clone());
        let route_filter_repository = RouteFilterRepository::new(pool.clone());
        // Sync services
        let route_hierarchy_sync_service = RouteHierarchySyncService::new(pool.clone());

        // Initialize secret repository and encryption service if encryption key is configured
        let (secret_repository, encryption_service) =
            match crate::services::SecretEncryptionConfig::from_env() {
                Ok(encryption_config) => match SecretEncryption::new(&encryption_config) {
                    Ok(encryption) => {
                        let encryption = Arc::new(encryption);
                        let secret_repo = SecretRepository::new(pool, encryption.clone());
                        info!("Secret encryption configured, SDS enabled");
                        (Some(secret_repo), Some(encryption))
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            "Failed to initialize secret encryption service, SDS disabled"
                        );
                        (None, None)
                    }
                },
                Err(_) => {
                    debug!(
                        "FLOWPLANE_SECRET_ENCRYPTION_KEY not set, SDS disabled. \
                         Set this env var to enable secret management."
                    );
                    (None, None)
                }
            };

        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: Some(cluster_repository),
            route_config_repository: Some(route_config_repository),
            listener_repository: Some(listener_repository),
            filter_repository: Some(filter_repository),
            listener_auto_filter_repository: Some(listener_auto_filter_repository),
            aggregated_schema_repository: Some(aggregated_schema_repository),
            virtual_host_repository: Some(virtual_host_repository),
            route_repository: Some(route_repository),
            virtual_host_filter_repository: Some(virtual_host_filter_repository),
            route_filter_repository: Some(route_filter_repository),
            route_hierarchy_sync_service: Some(route_hierarchy_sync_service),
            access_log_service: None,
            ext_proc_service: None,
            learning_session_service: RwLock::new(None),
            secret_repository,
            encryption_service,
            secret_backend_registry: None, // Initialized separately via set_secret_backend_registry
            filter_schema_registry: FilterSchemaRegistry::with_builtin_schemas(),
            update_tx,
            resource_caches: RwLock::new(HashMap::new()),
        }
    }

    /// Set the secret backend registry for external secret backends
    pub fn set_secret_backend_registry(&mut self, registry: SecretBackendRegistry) {
        self.secret_backend_registry = Some(registry);
    }

    /// Get the secret backend registry
    pub fn get_secret_backend_registry(&self) -> Option<&SecretBackendRegistry> {
        self.secret_backend_registry.as_ref()
    }

    pub fn get_version(&self) -> String {
        self.version.load(std::sync::atomic::Ordering::Relaxed).to_string()
    }

    pub fn get_version_number(&self) -> u64 {
        self.version.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Create a new XdsState with services (builder pattern)
    pub fn with_services(
        mut self,
        access_log_service: Arc<FlowplaneAccessLogService>,
        ext_proc_service: Arc<FlowplaneExtProcService>,
        learning_session_service: Arc<LearningSessionService>,
    ) -> Self {
        self.access_log_service = Some(access_log_service);
        self.ext_proc_service = Some(ext_proc_service);
        *self.learning_session_service.write().expect("lock poisoned") =
            Some(learning_session_service);
        self
    }

    /// Set the learning session service (safe mutation)
    pub fn set_learning_session_service(&self, service: Arc<LearningSessionService>) {
        *self.learning_session_service.write().expect("lock poisoned") = Some(service);
    }

    /// Get the learning session service if available
    pub fn get_learning_session_service(&self) -> Option<Arc<LearningSessionService>> {
        self.learning_session_service.read().ok()?.clone()
    }

    /// Apply a new snapshot of built resources for `type_url` and broadcast changes.
    /// Returns `Some(ResourceUpdate)` when a delta was published.
    #[instrument(skip(self, built_resources), fields(type_url = %type_url, resource_count = built_resources.len()), name = "xds_apply_resources")]
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
    #[instrument(skip(self), name = "xds_refresh_clusters")]
    pub async fn refresh_clusters_from_repository(&self) -> Result<()> {
        let repository = match &self.cluster_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        let cluster_rows = repository.list(Some(1000), None).await?;

        let mut built = if cluster_rows.is_empty() {
            clusters_from_config(&self.config)?
        } else {
            clusters_from_database_entries(cluster_rows, "cache_refresh")?
        };

        // Always include the built-in ExtProc gRPC cluster for body capture
        let ext_proc_cluster =
            create_ext_proc_cluster(&self.config.bind_address, self.config.port)?;
        built.push(ext_proc_cluster);

        // Always include the built-in Access Log Service gRPC cluster for ALS
        let access_log_cluster = crate::xds::resources::create_access_log_cluster(
            &self.config.bind_address,
            self.config.port,
        )?;
        built.push(access_log_cluster);

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
    #[instrument(skip(self), name = "xds_refresh_routes")]
    pub async fn refresh_routes_from_repository(&self) -> Result<()> {
        let repository = match &self.route_config_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        let mut route_config_rows = repository.list(Some(1000), None).await?;

        // Inject attached filters into route configurations
        if let Err(e) = self.inject_route_config_filters(&mut route_config_rows).await {
            warn!(error = %e, "Failed to inject filters into route configs, continuing without filters");
        }

        let built = if route_config_rows.is_empty() {
            routes_from_config(&self.config)?
        } else {
            routes_from_database_entries(route_config_rows, "cache_refresh")?
        };

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

    /// Inject attached filters into route configurations
    ///
    /// This modifies the route config's JSON configuration to include any filters
    /// attached via the route_config_filters junction table.
    ///
    /// Supports 3-level hierarchical filter attachment:
    /// 1. RouteConfig-level filters - applied to ALL routes
    /// 2. VirtualHost-level filters - applied to routes in that vhost
    /// 3. Route-level filters - applied to specific routes only (most specific wins)
    ///
    /// Delegates to [`crate::xds::filters::injection::inject_route_filters_hierarchical`]
    /// when hierarchical repositories are available, otherwise falls back to
    /// [`crate::xds::filters::injection::inject_route_config_filters`].
    pub async fn inject_route_config_filters(
        &self,
        route_configs: &mut [crate::storage::RouteConfigData],
    ) -> Result<()> {
        let filter_repository = match &self.filter_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        // Try hierarchical injection if all required repositories are available
        if let (
            Some(vhost_repo),
            Some(route_repo),
            Some(vhost_filter_repo),
            Some(route_filter_repo),
        ) = (
            &self.virtual_host_repository,
            &self.route_repository,
            &self.virtual_host_filter_repository,
            &self.route_filter_repository,
        ) {
            let ctx = crate::xds::filters::injection::HierarchicalFilterContext {
                filter_repo: filter_repository,
                vhost_repo: vhost_repo.clone(),
                route_repo: route_repo.clone(),
                vhost_filter_repo: vhost_filter_repo.clone(),
                route_filter_repo: route_filter_repo.clone(),
                schema_registry: &self.filter_schema_registry,
            };

            return crate::xds::filters::injection::inject_route_filters_hierarchical(
                route_configs,
                &ctx,
            )
            .await;
        }

        // Fallback to simple route-config-only injection
        crate::xds::filters::injection::inject_route_config_filters(
            route_configs,
            &filter_repository,
            &self.filter_schema_registry,
        )
        .await
    }

    /// Refresh the listener cache from the backing repository (if available).
    #[instrument(skip(self), name = "xds_refresh_listeners")]
    pub async fn refresh_listeners_from_repository(&self) -> Result<()> {
        let repository = match &self.listener_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        let listener_rows = repository.list(Some(1000), None).await?;

        let mut built = if listener_rows.is_empty() {
            listeners_from_config(&self.config)?
        } else {
            listeners_from_database_entries(listener_rows, "cache_refresh")?
        };

        // Inject listener-attached filters (e.g., JWT authentication)
        if let Err(e) = self.inject_listener_auto_filters(&mut built).await {
            warn!(
                error = %e,
                "Failed to inject listener-attached filters"
            );
        }

        // Inject access log configuration for active learning sessions
        if let Err(e) = self.inject_learning_session_access_logs(&mut built).await {
            warn!(
                error = %e,
                "Failed to inject access log configuration for learning sessions"
            );
        }

        // Inject ExtProc filter configuration for active learning sessions (body capture)
        if let Err(e) = self.inject_learning_session_ext_proc(&mut built).await {
            warn!(
                error = %e,
                "Failed to inject ExtProc configuration for learning sessions"
            );
        }

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

    /// Refresh the secret cache from the backing repository (if available).
    #[instrument(skip(self), name = "xds_refresh_secrets")]
    pub async fn refresh_secrets_from_repository(&self) -> Result<()> {
        let repository = match &self.secret_repository {
            Some(repo) => repo.clone(),
            None => {
                debug!("No secret repository available, skipping secret refresh");
                return Ok(());
            }
        };

        let mut secret_rows = repository.list(Some(1000), None).await?;

        // Resolve reference-based secrets from external backends (Vault, AWS, GCP)
        if let Some(registry) = &self.secret_backend_registry {
            info!(
                secret_count = secret_rows.len(),
                phase = "cache_refresh",
                "Resolving reference-based secrets from external backends"
            );
            self.resolve_reference_secrets(&mut secret_rows, registry).await;
        } else {
            warn!(phase = "cache_refresh", "No secret backend registry available, reference-based secrets will not be resolved");
        }

        // Convert database entries to Envoy Secret resources
        let built = secrets_from_database_entries(secret_rows, "cache_refresh")?;

        let total_resources = built.len();
        match self.apply_built_resources(SECRET_TYPE_URL, built) {
            Some(update) => {
                for delta in &update.deltas {
                    info!(
                        phase = "cache_refresh",
                        type_url = %delta.type_url,
                        added = delta.added_or_updated.len(),
                        removed = delta.removed.len(),
                        version = update.version,
                        total_resources,
                        "Secret cache refresh produced delta"
                    );
                }
            }
            None => {
                debug!(
                    phase = "cache_refresh",
                    type_url = SECRET_TYPE_URL,
                    total_resources,
                    "Secret cache refresh detected no changes"
                );
            }
        }

        Ok(())
    }

    /// Resolve reference-based secrets by fetching from external backends
    ///
    /// For secrets with `backend` and `reference` fields set, this method
    /// fetches the actual secret from the external backend (Vault, AWS, GCP)
    /// and populates the `configuration` field with the JSON serialization.
    async fn resolve_reference_secrets(
        &self,
        secrets: &mut [crate::storage::SecretData],
        registry: &SecretBackendRegistry,
    ) {
        use crate::secrets::backends::SecretBackendType;

        for secret in secrets.iter_mut() {
            // Skip if not a reference-based secret
            let (Some(backend_str), Some(reference)) = (&secret.backend, &secret.reference) else {
                continue;
            };

            // Parse backend type
            let Some(backend_type) = backend_str.parse::<SecretBackendType>().ok() else {
                tracing::warn!(
                    secret_name = %secret.name,
                    backend = %backend_str,
                    "Unknown backend type for secret, skipping"
                );
                continue;
            };

            // Fetch from backend
            info!(
                secret_name = %secret.name,
                backend = %backend_str,
                reference = %reference,
                "Fetching reference-based secret from backend (cache_refresh)"
            );
            match registry.fetch_secret(backend_type, reference, secret.secret_type).await {
                Ok(spec) => {
                    // Serialize the spec to JSON for consumption by secrets_from_database_entries
                    match serde_json::to_string(&spec) {
                        Ok(json) => {
                            info!(
                                secret_name = %secret.name,
                                backend = %backend_str,
                                reference = %reference,
                                json_len = json.len(),
                                "Resolved reference-based secret from backend (cache_refresh)"
                            );
                            secret.configuration = json;
                        }
                        Err(e) => {
                            tracing::warn!(
                                secret_name = %secret.name,
                                error = %e,
                                "Failed to serialize secret spec"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        secret_name = %secret.name,
                        backend = %backend_str,
                        reference = %reference,
                        error = %e,
                        "Failed to fetch secret from backend"
                    );
                }
            }
        }
    }

    /// Inject access log configuration into listeners for active learning sessions.
    ///
    /// Delegates to [`crate::xds::filters::injection::inject_access_logs`].
    #[instrument(skip(self, built_listeners), name = "xds_inject_access_logs")]
    pub(crate) async fn inject_learning_session_access_logs(
        &self,
        built_listeners: &mut [BuiltResource],
    ) -> Result<()> {
        let session_service = match self.get_learning_session_service() {
            Some(service) => service,
            None => {
                debug!("No learning session service available, skipping access log injection");
                return Ok(());
            }
        };

        let grpc_address = format!("{}:{}", self.config.bind_address, self.config.port);
        crate::xds::filters::injection::inject_access_logs(
            built_listeners,
            &session_service,
            &grpc_address,
        )
        .await
    }

    /// Inject listener-attached filters into the HTTP connection manager filter chain
    ///
    /// This method:
    /// 1. For each listener, queries the database for attached filters
    /// 2. Also queries filters attached to the route configuration used by the listener
    /// 3. Merges compatible filters (specifically JWT authentication)
    /// 4. Injects the filters into the listener's HCM filter chain
    ///
    /// This enables user-defined filters (like JWT authentication) to be dynamically
    /// applied to listeners without modifying the stored listener configuration.
    ///
    /// Delegates to [`crate::xds::filters::injection::inject_listener_filters`].
    #[instrument(skip(self, built_listeners), name = "xds_inject_listener_filters")]
    pub(crate) async fn inject_listener_auto_filters(
        &self,
        built_listeners: &mut [BuiltResource],
    ) -> Result<()> {
        let filter_repository = match &self.filter_repository {
            Some(repo) => repo,
            None => {
                debug!("No filter repository available, skipping listener filter injection");
                return Ok(());
            }
        };

        let listener_repository = match &self.listener_repository {
            Some(repo) => repo,
            None => {
                debug!("No listener repository available, skipping listener filter injection");
                return Ok(());
            }
        };

        let route_config_repository = self.route_config_repository.as_ref();

        crate::xds::filters::injection::inject_listener_filters(
            built_listeners,
            filter_repository,
            listener_repository,
            route_config_repository,
            self,
            &self.filter_schema_registry,
        )
        .await
    }

    /// Inject ExtProc filter configuration into listeners for active learning sessions.
    ///
    /// Delegates to [`crate::xds::filters::injection::inject_ext_proc`].
    #[instrument(skip(self, built_listeners), name = "xds_inject_ext_proc")]
    pub(crate) async fn inject_learning_session_ext_proc(
        &self,
        built_listeners: &mut [BuiltResource],
    ) -> Result<()> {
        let session_service = match self.get_learning_session_service() {
            Some(service) => service,
            None => {
                debug!("No learning session service available, skipping ExtProc injection");
                return Ok(());
            }
        };

        crate::xds::filters::injection::inject_ext_proc(built_listeners, &session_service).await
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

    #[tokio::test]
    async fn ext_proc_cluster_created_with_correct_endpoint() {
        use crate::xds::resources::create_ext_proc_cluster;
        use prost::Message;

        let bind_address = "192.168.1.100";
        let port = 19000;

        // Create the ExtProc cluster
        let built_resource = create_ext_proc_cluster(bind_address, port).unwrap();

        // Decode the cluster
        let cluster = envoy_types::pb::envoy::config::cluster::v3::Cluster::decode(
            &built_resource.resource.value[..],
        )
        .unwrap();

        // Verify cluster name
        assert_eq!(cluster.name, "flowplane_ext_proc_service");

        // Verify endpoint address and port match the provided values
        let load_assignment = cluster.load_assignment.unwrap();
        let endpoints = &load_assignment.endpoints[0].lb_endpoints[0];

        if let Some(
            envoy_types::pb::envoy::config::endpoint::v3::lb_endpoint::HostIdentifier::Endpoint(
                endpoint,
            ),
        ) = &endpoints.host_identifier
        {
            if let Some(address) = &endpoint.address {
                if let Some(
                    envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                        socket_addr,
                    ),
                ) = &address.address
                {
                    assert_eq!(socket_addr.address, bind_address);
                    if let Some(
                        envoy_types::pb::envoy::config::core::v3::socket_address::PortSpecifier::PortValue(
                            p,
                        ),
                    ) = &socket_addr.port_specifier
                    {
                        assert_eq!(*p, port as u32);
                    } else {
                        panic!("Expected port value");
                    }
                } else {
                    panic!("Expected socket address");
                }
            }
        }
    }
}
