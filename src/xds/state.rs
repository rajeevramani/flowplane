use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::xds::resources::{
    clusters_from_config, clusters_from_database_entries, create_ext_proc_cluster,
    create_jwks_cluster, listeners_from_config, listeners_from_database_entries,
    routes_from_config, routes_from_database_entries, BuiltResource, CLUSTER_TYPE_URL,
    LISTENER_TYPE_URL, ROUTE_TYPE_URL,
};
use crate::{
    config::SimpleXdsConfig,
    services::LearningSessionService,
    storage::{
        AggregatedSchemaRepository, ClusterRepository, DbPool, FilterRepository,
        ListenerAutoFilterRepository, ListenerRepository, RouteRepository,
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
    pub route_repository: Option<RouteRepository>,
    pub listener_repository: Option<ListenerRepository>,
    pub filter_repository: Option<FilterRepository>,
    pub listener_auto_filter_repository: Option<ListenerAutoFilterRepository>,
    pub aggregated_schema_repository: Option<AggregatedSchemaRepository>,
    pub access_log_service: Option<Arc<FlowplaneAccessLogService>>,
    pub ext_proc_service: Option<Arc<FlowplaneExtProcService>>,
    pub learning_session_service: RwLock<Option<Arc<LearningSessionService>>>,
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
            filter_repository: None,
            listener_auto_filter_repository: None,
            aggregated_schema_repository: None,
            access_log_service: None,
            ext_proc_service: None,
            learning_session_service: RwLock::new(None),
            update_tx,
            resource_caches: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_database(config: SimpleXdsConfig, pool: DbPool) -> Self {
        let (update_tx, _) = broadcast::channel(128);
        let cluster_repository = ClusterRepository::new(pool.clone());
        let route_repository = RouteRepository::new(pool.clone());
        let listener_repository = ListenerRepository::new(pool.clone());
        let filter_repository = FilterRepository::new(pool.clone());
        let listener_auto_filter_repository = ListenerAutoFilterRepository::new(pool.clone());
        let aggregated_schema_repository = AggregatedSchemaRepository::new(pool);
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: Some(cluster_repository),
            route_repository: Some(route_repository),
            listener_repository: Some(listener_repository),
            filter_repository: Some(filter_repository),
            listener_auto_filter_repository: Some(listener_auto_filter_repository),
            aggregated_schema_repository: Some(aggregated_schema_repository),
            access_log_service: None,
            ext_proc_service: None,
            learning_session_service: RwLock::new(None),
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
        let repository = match &self.route_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        let mut route_rows = repository.list(Some(1000), None).await?;

        // Inject attached filters into route configurations
        if let Err(e) = self.inject_route_filters(&mut route_rows).await {
            warn!(error = %e, "Failed to inject filters into routes, continuing without filters");
        }

        let built = if route_rows.is_empty() {
            routes_from_config(&self.config)?
        } else {
            routes_from_database_entries(route_rows, "cache_refresh")?
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
    /// This modifies the route's JSON configuration to include any filters
    /// attached via the route_filters junction table.
    pub async fn inject_route_filters(
        &self,
        routes: &mut [crate::storage::RouteData],
    ) -> Result<()> {
        use crate::domain::FilterConfig;
        use crate::xds::filters::http::header_mutation::{
            HeaderMutationEntry, HeaderMutationPerRouteConfig,
        };
        use crate::xds::filters::http::HttpScopedConfig;

        let filter_repository = match &self.filter_repository {
            Some(repo) => repo.clone(),
            None => return Ok(()),
        };

        for route in routes.iter_mut() {
            // Get filters attached to this route
            let filters = filter_repository.list_route_filters(&route.id).await?;

            if filters.is_empty() {
                continue;
            }

            // Parse the route configuration
            let mut config: serde_json::Value = serde_json::from_str(&route.configuration)
                .map_err(|e| {
                    crate::Error::internal(format!(
                        "Failed to parse route configuration for '{}': {}",
                        route.name, e
                    ))
                })?;

            // Process each filter and add to typed_per_filter_config
            for filter_data in filters {
                // Parse the filter configuration
                let filter_config: FilterConfig = serde_json::from_str(&filter_data.configuration)
                    .map_err(|e| {
                        warn!(
                            filter_id = %filter_data.id,
                            filter_name = %filter_data.name,
                            error = %e,
                            "Failed to parse filter configuration, skipping"
                        );
                        crate::Error::internal(format!(
                            "Failed to parse filter configuration: {}",
                            e
                        ))
                    })?;

                // Convert to per-route config based on filter type
                let (filter_name, scoped_config) = match filter_config {
                    FilterConfig::HeaderMutation(hm_config) => {
                        let per_route = HeaderMutationPerRouteConfig {
                            request_headers_to_add: hm_config
                                .request_headers_to_add
                                .into_iter()
                                .map(|e| HeaderMutationEntry {
                                    key: e.key,
                                    value: e.value,
                                    append: e.append,
                                })
                                .collect(),
                            request_headers_to_remove: hm_config.request_headers_to_remove,
                            response_headers_to_add: hm_config
                                .response_headers_to_add
                                .into_iter()
                                .map(|e| HeaderMutationEntry {
                                    key: e.key,
                                    value: e.value,
                                    append: e.append,
                                })
                                .collect(),
                            response_headers_to_remove: hm_config.response_headers_to_remove,
                        };

                        (
                            "envoy.filters.http.header_mutation".to_string(),
                            HttpScopedConfig::HeaderMutation(per_route),
                        )
                    }
                    FilterConfig::JwtAuth(jwt_config) => {
                        // Per-route JWT uses requirement_name to reference listener-level config
                        // Use the first provider name as the default requirement
                        use crate::xds::filters::http::jwt_auth::JwtPerRouteConfig;
                        let per_route = jwt_config
                            .providers
                            .keys()
                            .next()
                            .map(|name| JwtPerRouteConfig::RequirementName {
                                requirement_name: name.clone(),
                            })
                            .unwrap_or(JwtPerRouteConfig::Disabled { disabled: true });

                        (
                            "envoy.filters.http.jwt_authn".to_string(),
                            HttpScopedConfig::JwtAuthn(per_route),
                        )
                    }
                };

                // Inject into the route's virtual hosts
                if let Some(virtual_hosts) = config.get_mut("virtual_hosts") {
                    if let Some(vhosts_arr) = virtual_hosts.as_array_mut() {
                        for vhost in vhosts_arr {
                            // Add to each route within the virtual host
                            if let Some(routes_arr) = vhost.get_mut("routes") {
                                if let Some(routes) = routes_arr.as_array_mut() {
                                    for route_entry in routes {
                                        // Add typed_per_filter_config to the route
                                        let tpfc = route_entry.as_object_mut().and_then(|obj| {
                                            obj.entry("typed_per_filter_config")
                                                .or_insert_with(|| serde_json::json!({}))
                                                .as_object_mut()
                                        });

                                        if let Some(tpfc_obj) = tpfc {
                                            // Serialize the scoped config
                                            if let Ok(config_value) =
                                                serde_json::to_value(&scoped_config)
                                            {
                                                tpfc_obj.insert(filter_name.clone(), config_value);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                info!(
                    route_name = %route.name,
                    filter_name = %filter_data.name,
                    filter_type = %filter_data.filter_type,
                    "Injected filter into route configuration"
                );
            }

            // Update the route configuration with the modified JSON
            route.configuration = serde_json::to_string(&config).map_err(|e| {
                crate::Error::internal(format!(
                    "Failed to serialize modified route configuration: {}",
                    e
                ))
            })?;
        }

        Ok(())
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

    /// Inject access log configuration into listeners for active learning sessions
    ///
    /// This method:
    /// 1. Queries active learning sessions from the service
    /// 2. For each active session, decodes the listener protobuf
    /// 3. Injects HttpGrpcAccessLogConfig into the listener's filter chains
    /// 4. Re-encodes the modified listener back into the BuiltResource
    ///
    /// This enables dynamic access logging for routes in active learning sessions
    /// without modifying the stored listener configuration.
    #[instrument(skip(self, built_listeners), name = "xds_inject_access_logs")]
    pub(crate) async fn inject_learning_session_access_logs(
        &self,
        built_listeners: &mut [BuiltResource],
    ) -> Result<()> {
        use crate::xds::access_log::LearningSessionAccessLogConfig;
        use envoy_types::pb::envoy::config::listener::v3::Listener;
        use prost::Message;

        // Get learning session service
        let session_service = match self.get_learning_session_service() {
            Some(service) => service,
            None => {
                debug!("No learning session service available, skipping access log injection");
                return Ok(());
            }
        };

        // Query active learning sessions
        let active_sessions = session_service.list_active_sessions().await?;

        if active_sessions.is_empty() {
            debug!("No active learning sessions, skipping access log injection");
            return Ok(());
        }

        info!(
            session_count = active_sessions.len(),
            "Injecting access log configuration for active learning sessions"
        );

        // For each listener, check if it needs access log injection
        for built in built_listeners.iter_mut() {
            debug!(
                listener = %built.name,
                "Processing listener for access log injection"
            );

            // Decode the listener protobuf
            let mut listener = Listener::decode(&built.resource.value[..]).map_err(|e| {
                crate::Error::internal(format!("Failed to decode listener '{}': {}", built.name, e))
            })?;

            debug!(
                listener = %built.name,
                filter_chain_count = listener.filter_chains.len(),
                "Decoded listener, checking filter chains"
            );

            // Track if we modified this listener
            let mut modified = false;

            // Check each active session to see if it applies to this listener
            for session in &active_sessions {
                debug!(
                    listener = %built.name,
                    session_id = %session.id,
                    "Checking session for injection"
                );
                // For now, we inject access log into ALL listeners when ANY session is active
                // TODO: In the future, we could be more selective based on session.route_pattern
                // and match it against the listener's routes

                // Create access log config for this session
                let access_log_config = LearningSessionAccessLogConfig::new(
                    session.id.clone(),
                    session.team.clone(),
                    self.config.bind_address.clone() + ":" + &self.config.port.to_string(),
                );

                let access_log = access_log_config.build_access_log()?;

                // Inject access log into each filter chain's HTTP connection manager
                for (fc_idx, filter_chain) in listener.filter_chains.iter_mut().enumerate() {
                    debug!(
                        listener = %built.name,
                        filter_chain_index = fc_idx,
                        filter_count = filter_chain.filters.len(),
                        "Processing filter chain"
                    );

                    for (f_idx, filter) in filter_chain.filters.iter_mut().enumerate() {
                        debug!(
                            listener = %built.name,
                            filter_chain_index = fc_idx,
                            filter_index = f_idx,
                            filter_name = %filter.name,
                            "Examining filter"
                        );

                        // Check if this is an HTTP connection manager
                        if filter.name == "envoy.filters.network.http_connection_manager" {
                            if let Some(config_type) = &mut filter.config_type {
                                use envoy_types::pb::envoy::config::listener::v3::filter::ConfigType;

                                if let ConfigType::TypedConfig(typed_config) = config_type {
                                    // Decode HCM, add access log, re-encode
                                    use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpConnectionManager;

                                    let mut hcm =
                                        HttpConnectionManager::decode(&typed_config.value[..])
                                            .map_err(|e| {
                                                crate::Error::internal(format!(
                                                    "Failed to decode HCM for listener '{}': {}",
                                                    built.name, e
                                                ))
                                            })?;

                                    // Add access log to the HCM (avoid duplicates)
                                    let already_has_log = hcm
                                        .access_log
                                        .iter()
                                        .any(|al| al.name.contains(&session.id));

                                    if !already_has_log {
                                        hcm.access_log.push(access_log.clone());
                                        modified = true;

                                        debug!(
                                            listener = %built.name,
                                            session_id = %session.id,
                                            "Injected access log configuration"
                                        );
                                    }

                                    // Re-encode HCM back into typed_config
                                    typed_config.value = hcm.encode_to_vec();
                                }
                            }
                        }
                    }
                }
            }

            // If we modified the listener, re-encode it
            if modified {
                built.resource.value = listener.encode_to_vec();
                debug!(
                    listener = %built.name,
                    "Re-encoded listener with access log configuration"
                );
            }
        }

        Ok(())
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
    #[instrument(skip(self, built_listeners), name = "xds_inject_listener_filters")]
    pub(crate) async fn inject_listener_auto_filters(
        &self,
        built_listeners: &mut [BuiltResource],
    ) -> Result<()> {
        use crate::domain::FilterConfig;
        use crate::xds::filters::http::jwt_auth::{
            JwtAuthenticationConfig, JwtJwksSourceConfig, JwtRequirementConfig,
        };
        use envoy_types::pb::envoy::config::listener::v3::Listener;
        use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::RouteSpecifier;
        use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::{
            HttpConnectionManager, HttpFilter,
        };
        use prost::Message;

        let filter_repository = match &self.filter_repository {
            Some(repo) => repo.clone(),
            None => {
                debug!("No filter repository available, skipping listener filter injection");
                return Ok(());
            }
        };

        let listener_repository = match &self.listener_repository {
            Some(repo) => repo.clone(),
            None => {
                debug!("No listener repository available, skipping listener filter injection");
                return Ok(());
            }
        };

        let route_repository = self.route_repository.clone();

        for built in built_listeners.iter_mut() {
            // Get the listener data by name to retrieve the ListenerId
            let listener_data = match listener_repository.get_by_name(&built.name).await {
                Ok(data) => data,
                Err(e) => {
                    debug!(
                        listener = %built.name,
                        error = %e,
                        "Could not find listener in database, skipping filter injection"
                    );
                    continue;
                }
            };

            // 1. Get filters attached directly to this listener
            let mut filters = match filter_repository.list_listener_filters(&listener_data.id).await
            {
                Ok(filters) => filters,
                Err(e) => {
                    warn!(
                        listener = %built.name,
                        error = %e,
                        "Failed to load listener filters, skipping"
                    );
                    Vec::new()
                }
            };

            // Decode the listener protobuf to find associated routes
            let mut listener = Listener::decode(&built.resource.value[..]).map_err(|e| {
                crate::Error::internal(format!("Failed to decode listener '{}': {}", built.name, e))
            })?;

            // 2. Find route configs used by this listener and get their filters
            if let Some(route_repo) = &route_repository {
                let mut route_config_names = HashSet::new();
                
                // Scan all filter chains for HCMs and their route configs
                for filter_chain in &listener.filter_chains {
                    for filter in &filter_chain.filters {
                        if filter.name == "envoy.filters.network.http_connection_manager" {
                            if let Some(config_type) = &filter.config_type {
                                use envoy_types::pb::envoy::config::listener::v3::filter::ConfigType;
                                if let ConfigType::TypedConfig(typed_config) = config_type {
                                    if let Ok(hcm) = HttpConnectionManager::decode(&typed_config.value[..]) {
                                        if let Some(route_specifier) = hcm.route_specifier {
                                            match route_specifier {
                                                RouteSpecifier::Rds(rds) => {
                                                    info!(
                                                        listener = %built.name,
                                                        route_config = %rds.route_config_name,
                                                        "Found RDS route config for listener"
                                                    );
                                                    route_config_names.insert(rds.route_config_name);
                                                }
                                                RouteSpecifier::RouteConfig(rc) => {
                                                    info!(
                                                        listener = %built.name,
                                                        route_config = %rc.name,
                                                        "Found inline route config for listener"
                                                    );
                                                    route_config_names.insert(rc.name);
                                                }
                                                RouteSpecifier::ScopedRoutes(_) => {
                                                    info!(listener = %built.name, "Scoped routes not yet supported for filter injection");
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                for route_name in route_config_names {
                    match route_repo.get_by_name(&route_name).await {
                        Ok(route) => {
                            match filter_repository.list_route_filters(&route.id).await {
                                Ok(route_filters) => {
                                    if !route_filters.is_empty() {
                                        info!(
                                            listener = %built.name,
                                            route_name = %route_name,
                                            count = route_filters.len(),
                                            "Found filters attached to route used by listener"
                                        );
                                        filters.extend(route_filters);
                                    } else {
                                        info!(
                                            listener = %built.name,
                                            route_name = %route_name,
                                            "No filters attached to route"
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        listener = %built.name,
                                        route_name = %route_name,
                                        error = %e,
                                        "Failed to list filters for route"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                listener = %built.name,
                                route_name = %route_name,
                                error = %e,
                                "Failed to look up route by name (or route not found)"
                            );
                        }
                    }
                }
            }

            if filters.is_empty() {
                continue;
            }

            // 3. Process and merge filters
            let mut jwt_configs: Vec<JwtAuthenticationConfig> = Vec::new();
            let mut other_filters: Vec<HttpFilter> = Vec::new();

            for filter_data in &filters {
                let filter_config: FilterConfig =
                    match serde_json::from_str(&filter_data.configuration) {
                        Ok(config) => config,
                        Err(e) => {
                            warn!(
                                listener = %built.name,
                                filter_id = %filter_data.id,
                                filter_name = %filter_data.name,
                                error = %e,
                                "Failed to parse filter configuration, skipping"
                            );
                            continue;
                        }
                    };

                match filter_config {
                    FilterConfig::JwtAuth(jwt_config) => {
                        jwt_configs.push(jwt_config);
                    }
                    FilterConfig::HeaderMutation(_) => {
                        // HeaderMutation is route-level only, skip for listener injection
                        debug!(
                            listener = %built.name,
                            filter_name = %filter_data.name,
                            "HeaderMutation filter is route-level only, skipping listener injection"
                        );
                        continue;
                    }
                }
            }

            // Merge JWT configs if present
            if !jwt_configs.is_empty() {
                let mut merged_config = JwtAuthenticationConfig::default();
                
                // Merge all JWT configs
                for config in jwt_configs {
                    merged_config.providers.extend(config.providers);
                    merged_config.rules.extend(config.rules);
                    merged_config.requirement_map.extend(config.requirement_map);
                    
                    if let Some(rules) = config.filter_state_rules {
                        // Simple last-write-wins for filter state rules for now
                        merged_config.filter_state_rules = Some(rules);
                    }
                    
                    // Merge boolean flags (if any is true, result is true)
                    if config.bypass_cors_preflight.unwrap_or(false) {
                        merged_config.bypass_cors_preflight = Some(true);
                    }
                    if config.strip_failure_response.unwrap_or(false) {
                        merged_config.strip_failure_response = Some(true);
                    }
                    
                    if let Some(prefix) = config.stat_prefix {
                        merged_config.stat_prefix = Some(prefix);
                    }
                }

                // Auto-populate requirement_map if empty but providers exist
                if merged_config.requirement_map.is_empty() && !merged_config.providers.is_empty() {
                    for provider_name in merged_config.providers.keys() {
                        merged_config.requirement_map.insert(
                            provider_name.clone(),
                            JwtRequirementConfig::ProviderName {
                                provider_name: provider_name.clone(),
                            },
                        );
                    }
                    debug!(
                        listener = %built.name,
                        provider_count = merged_config.providers.len(),
                        "Auto-populated requirement_map from providers for merged JWT config"
                    );
                }

                // Auto-create JWKS clusters for remote providers
                let existing_clusters: HashSet<String> = self
                    .cached_resources(CLUSTER_TYPE_URL)
                    .iter()
                    .map(|r| r.name.clone())
                    .collect();

                for (provider_name, provider_config) in &merged_config.providers {
                    if let JwtJwksSourceConfig::Remote(remote) = &provider_config.jwks {
                        let cluster_name = &remote.http_uri.cluster;
                        let jwks_uri = &remote.http_uri.uri;

                        if !existing_clusters.contains(cluster_name) {
                            match create_jwks_cluster(cluster_name, jwks_uri) {
                                Ok(cluster) => {
                                    if self
                                        .apply_built_resources(CLUSTER_TYPE_URL, vec![cluster])
                                        .is_some()
                                    {
                                        info!(
                                            cluster = %cluster_name,
                                            provider = %provider_name,
                                            jwks_uri = %jwks_uri,
                                            "Auto-created JWKS cluster for JWT provider"
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        cluster = %cluster_name,
                                        provider = %provider_name,
                                        error = %e,
                                        "Failed to create JWKS cluster"
                                    );
                                }
                            }
                        }
                    }
                }

                // Create the JWT filter
                match merged_config.to_any() {
                    Ok(any) => {
                        other_filters.push(HttpFilter {
                            name: "envoy.filters.http.jwt_authn".to_string(),
                            config_type: Some(
                                envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType::TypedConfig(any)
                            ),
                            is_optional: false,
                            disabled: false,
                        });
                    }
                    Err(e) => {
                        warn!(
                            listener = %built.name,
                            error = %e,
                            "Failed to convert merged JWT config to protobuf, skipping"
                        );
                    }
                }
            }

            if other_filters.is_empty() {
                continue;
            }

            // 4. Inject filters into listener
            let mut modified = false;

            for filter_chain in listener.filter_chains.iter_mut() {
                for filter in filter_chain.filters.iter_mut() {
                    if filter.name == "envoy.filters.network.http_connection_manager" {
                        if let Some(config_type) = &mut filter.config_type {
                            use envoy_types::pb::envoy::config::listener::v3::filter::ConfigType;

                            if let ConfigType::TypedConfig(typed_config) = config_type {
                                let mut hcm = match HttpConnectionManager::decode(&typed_config.value[..]) {
                                    Ok(hcm) => hcm,
                                    Err(e) => {
                                        warn!("Failed to decode HCM: {}", e);
                                        continue;
                                    }
                                };

                                for http_filter in &other_filters {
                                    // Check if this filter already exists
                                    let existing_filter_idx = hcm
                                        .http_filters
                                        .iter()
                                        .position(|f| f.name == http_filter.name);

                                    match existing_filter_idx {
                                        Some(idx) => {
                                            // For JWT, we always replace because we've merged everything
                                            if http_filter.name == "envoy.filters.http.jwt_authn" {
                                                info!(
                                                    listener = %built.name,
                                                    filter_name = %http_filter.name,
                                                    "Replacing existing JWT filter with merged configuration"
                                                );
                                                hcm.http_filters[idx] = http_filter.clone();
                                                modified = true;
                                            }
                                        }
                                        None => {
                                            // Insert filter BEFORE the router filter
                                            let router_pos = hcm
                                                .http_filters
                                                .iter()
                                                .position(|f| f.name == "envoy.filters.http.router")
                                                .unwrap_or(hcm.http_filters.len());

                                            hcm.http_filters.insert(router_pos, http_filter.clone());
                                            modified = true;

                                            info!(
                                                listener = %built.name,
                                                filter_name = %http_filter.name,
                                                position = router_pos,
                                                "Injected filter into listener HCM"
                                            );
                                        }
                                    }
                                }

                                typed_config.value = hcm.encode_to_vec();
                            }
                        }
                    }
                }
            }

            // If we modified the listener, re-encode it
            if modified {
                built.resource.value = listener.encode_to_vec();
                info!(
                    listener = %built.name,
                    "Re-encoded listener with injected filters"
                );
            }
        }

        Ok(())
    }

    /// Inject ExtProc filter configuration into listeners for active learning sessions
    ///
    /// This method:
    /// 1. Queries active learning sessions from the service
    /// 2. For each active session, decodes the listener protobuf
    /// 3. Injects ExtProc HTTP filter into the listener's filter chains
    /// 4. Re-encodes the modified listener back into the BuiltResource
    ///
    /// This enables dynamic body capture for routes in active learning sessions
    /// without modifying the stored listener configuration.
    ///
    /// The ExtProc filter is configured to:
    /// - Buffer request and response bodies up to 10KB
    /// - Send bodies to the Flowplane ExtProc service
    /// - Fail-open (requests continue even if ExtProc fails)
    #[instrument(skip(self, built_listeners), name = "xds_inject_ext_proc")]
    pub(crate) async fn inject_learning_session_ext_proc(
        &self,
        built_listeners: &mut [BuiltResource],
    ) -> Result<()> {
        use crate::xds::filters::http::ext_proc::{
            ExtProcConfig, GrpcServiceConfig, ProcessingMode,
        };
        use envoy_types::pb::envoy::config::listener::v3::Listener;
        use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter;
        use prost::Message;

        // Get learning session service
        let session_service = match self.get_learning_session_service() {
            Some(service) => service,
            None => {
                debug!("No learning session service available, skipping ExtProc injection");
                return Ok(());
            }
        };

        // Query active learning sessions
        let active_sessions = session_service.list_active_sessions().await?;

        if active_sessions.is_empty() {
            debug!("No active learning sessions, skipping ExtProc injection");
            return Ok(());
        }

        info!(
            session_count = active_sessions.len(),
            "Injecting ExtProc configuration for active learning sessions"
        );

        // For each listener, check if it needs ExtProc injection
        for built in built_listeners.iter_mut() {
            debug!(
                listener = %built.name,
                "Processing listener for ExtProc injection"
            );

            // Decode the listener protobuf
            let mut listener = Listener::decode(&built.resource.value[..]).map_err(|e| {
                crate::Error::internal(format!("Failed to decode listener '{}': {}", built.name, e))
            })?;

            // Track if we modified this listener
            let mut modified = false;

            // Check each active session to see if it applies to this listener
            for session in &active_sessions {
                debug!(
                    listener = %built.name,
                    session_id = %session.id,
                    "Checking session for ExtProc injection"
                );

                // Create ExtProc config for body capture
                let ext_proc_config = ExtProcConfig {
                    grpc_service: GrpcServiceConfig {
                        target_uri: "flowplane_ext_proc_service".to_string(),
                        timeout_seconds: 5,
                    },
                    failure_mode_allow: true, // Fail-open: requests continue even if ExtProc fails
                    processing_mode: Some(ProcessingMode {
                        request_header_mode: Some("SEND".to_string()),
                        response_header_mode: Some("SEND".to_string()),
                        request_body_mode: Some("BUFFERED".to_string()), // Capture request body
                        response_body_mode: Some("BUFFERED".to_string()), // Capture response body
                        request_trailer_mode: Some("SKIP".to_string()),
                        response_trailer_mode: Some("SKIP".to_string()),
                    }),
                    message_timeout_ms: Some(5000), // 5 second timeout per message
                    request_attributes: vec![],
                    response_attributes: vec![],
                };

                let ext_proc_any = ext_proc_config.to_any()?;

                // Create HTTP filter for ExtProc
                let ext_proc_filter = HttpFilter {
                    name: format!("envoy.filters.http.ext_proc.session_{}", session.id),
                    config_type: Some(
                        envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType::TypedConfig(ext_proc_any)
                    ),
                    is_optional: true, // Make it optional so requests continue if filter fails
                    disabled: false,
                };

                // Inject ExtProc filter into each filter chain's HTTP connection manager
                for (fc_idx, filter_chain) in listener.filter_chains.iter_mut().enumerate() {
                    for (f_idx, filter) in filter_chain.filters.iter_mut().enumerate() {
                        // Check if this is an HTTP connection manager
                        if filter.name == "envoy.filters.network.http_connection_manager" {
                            if let Some(config_type) = &mut filter.config_type {
                                use envoy_types::pb::envoy::config::listener::v3::filter::ConfigType;

                                if let ConfigType::TypedConfig(typed_config) = config_type {
                                    // Decode HCM, add ExtProc filter, re-encode
                                    use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpConnectionManager;

                                    let mut hcm =
                                        HttpConnectionManager::decode(&typed_config.value[..])
                                            .map_err(|e| {
                                                crate::Error::internal(format!(
                                                    "Failed to decode HCM for listener '{}': {}",
                                                    built.name, e
                                                ))
                                            })?;

                                    // Check if ExtProc filter already exists for this session
                                    let already_has_ext_proc = hcm
                                        .http_filters
                                        .iter()
                                        .any(|f| f.name.contains(&session.id));

                                    if !already_has_ext_proc {
                                        // Insert ExtProc filter BEFORE the router filter
                                        // Find router filter position
                                        let router_pos = hcm
                                            .http_filters
                                            .iter()
                                            .position(|f| f.name == "envoy.filters.http.router")
                                            .unwrap_or(hcm.http_filters.len());

                                        hcm.http_filters
                                            .insert(router_pos, ext_proc_filter.clone());
                                        modified = true;

                                        debug!(
                                            listener = %built.name,
                                            filter_chain_index = fc_idx,
                                            filter_index = f_idx,
                                            session_id = %session.id,
                                            "Injected ExtProc filter for body capture"
                                        );
                                    }

                                    // Re-encode HCM back into typed_config
                                    typed_config.value = hcm.encode_to_vec();
                                }
                            }
                        }
                    }
                }
            }

            // If we modified the listener, re-encode it
            if modified {
                built.resource.value = listener.encode_to_vec();
                debug!(
                    listener = %built.name,
                    "Re-encoded listener with ExtProc configuration"
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
