use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::{debug, info, warn};

use crate::{
    storage::{ClusterRepository, ListenerRepository, RouteConfigRepository, SecretRepository},
    Result,
};
use envoy_types::pb::envoy::service::discovery::v3::{
    aggregated_discovery_service_server::AggregatedDiscoveryService, DeltaDiscoveryRequest,
    DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse, Resource,
};

use super::super::{
    resources::{self, BuiltResource},
    secret::{secrets_from_database_entries, SECRET_TYPE_URL},
    XdsState,
};

/// Database-enabled Aggregated Discovery Service implementation
/// Returns resources from database when available, falls back to config-based resources
#[derive(Debug)]
pub struct DatabaseAggregatedDiscoveryService {
    pub(super) state: Arc<XdsState>,
}

impl DatabaseAggregatedDiscoveryService {
    pub fn new(state: Arc<XdsState>) -> Self {
        if let Some(repo) = &state.cluster_repository {
            spawn_cluster_watcher(state.clone(), repo.clone());
        }

        if let Some(repo) = &state.route_config_repository {
            spawn_route_config_watcher(state.clone(), repo.clone());
        }

        if let Some(repo) = &state.listener_repository {
            spawn_listener_watcher(state.clone(), repo.clone());
        }

        if let Some(repo) = &state.secret_repository {
            spawn_secret_watcher(state.clone(), repo.clone());
        }

        Self { state }
    }

    /// Create discovery response with database-backed resources
    #[tracing::instrument(skip(self, request), fields(type_url = %request.type_url, team, scope_type, resource_count))]
    async fn create_resource_response(
        &self,
        request: &DiscoveryRequest,
    ) -> Result<DiscoveryResponse> {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        let scope = scope_from_discovery(&request.node);

        // Record team and scope information in the span
        let span = tracing::Span::current();
        match &scope {
            Scope::All => {
                span.record("scope_type", "all");
                span.record("team", "admin");
            }
            Scope::Team { team } => {
                span.record("scope_type", "team");
                span.record("team", team.as_str());
                // NOTE: Team connection metric is now tracked at stream lifecycle level
                // in stream.rs (increment on stream start, decrement on stream close)
            }
            Scope::Allowlist { names } => {
                span.record("scope_type", "allowlist");
                span.record("team", format!("allowlist:{}", names.len()).as_str());
            }
        }

        let built = self.build_resources(request.type_url.as_str(), &scope).await?;
        span.record("resource_count", built.len());

        let resources = built.iter().map(|r| r.resource.clone()).collect();

        Ok(DiscoveryResponse {
            version_info: version.clone(),
            resources,
            canary: false,
            type_url: request.type_url.clone(),
            nonce: nonce.clone(),
            control_plane: None,
            resource_errors: Vec::new(),
        })
    }

    /// Create cluster resources from database with team-based filtering
    #[tracing::instrument(skip(self, scope), fields(teams, filtered_count))]
    async fn create_cluster_resources_from_db(&self, scope: &Scope) -> Result<Vec<BuiltResource>> {
        let mut built = if let Some(repo) = &self.state.cluster_repository {
            // Extract teams from scope for filtering
            // Default resources (team IS NULL) are always included
            let teams = match scope {
                Scope::All => vec![], // Admin access includes all resources
                Scope::Team { team } => vec![team.clone()],
                Scope::Allowlist { .. } => vec![], // Allowlist doesn't apply to clusters
            };

            // Record team filtering in span
            let span = tracing::Span::current();
            if teams.is_empty() {
                span.record("teams", "all");
            } else {
                span.record("teams", format!("{:?}", teams).as_str());
            }

            // Always include default resources (include_default = true)
            match repo.list_by_teams(&teams, true, Some(100), None).await {
                Ok(cluster_data_list) => {
                    span.record("filtered_count", cluster_data_list.len());

                    // Record team-scoped resource count metrics
                    if let Some(team) = teams.first() {
                        crate::observability::metrics::update_team_resource_count(
                            "cluster",
                            team,
                            cluster_data_list.len(),
                        )
                        .await;
                    }

                    if cluster_data_list.is_empty() {
                        info!(
                            "No clusters found in database, falling back to config-based cluster"
                        );
                        self.create_fallback_cluster_resources()?
                    } else {
                        info!(
                            phase = "ads_response",
                            cluster_count = cluster_data_list.len(),
                            "Building cluster resources from database for ADS response"
                        );
                        resources::clusters_from_database_entries(
                            cluster_data_list,
                            "ads_response",
                        )?
                    }
                }
                Err(e) => {
                    warn!("Failed to load clusters from database: {}, falling back to config", e);
                    self.create_fallback_cluster_resources()?
                }
            }
        } else {
            info!("No database repository available, using config-based cluster");
            self.create_fallback_cluster_resources()?
        };

        // Always include internal gRPC clusters used by Envoy filters
        // These are not stored in the database and must be emitted with CDS
        if let Ok(ext_proc) = crate::xds::resources::create_ext_proc_cluster(
            &self.state.config.bind_address,
            self.state.config.port,
        ) {
            built.push(ext_proc);
        }

        if let Ok(als) = crate::xds::resources::create_access_log_cluster(
            &self.state.config.bind_address,
            self.state.config.port,
        ) {
            built.push(als);
        }

        // Include dynamically created JWKS clusters from cache
        // These are created by inject_listener_auto_filters for JWT auth providers
        let existing_names: std::collections::HashSet<String> =
            built.iter().map(|r| r.name.clone()).collect();
        for cached in self.state.cached_resources(resources::CLUSTER_TYPE_URL) {
            if cached.name.contains("jwks") && !existing_names.contains(&cached.name) {
                built.push(resources::BuiltResource { name: cached.name, resource: cached.body });
            }
        }

        // NOTE: Platform API clusters are NOT added here to avoid duplicates
        // They are already stored in the database by materialize_native_resources()
        // and loaded above via cluster_repository.list()

        Ok(built)
    }

    /// Create fallback cluster resources from config
    fn create_fallback_cluster_resources(&self) -> Result<Vec<BuiltResource>> {
        resources::clusters_from_config(&self.state.config)
    }

    #[tracing::instrument(skip(self, scope), fields(teams, filtered_count))]
    async fn create_route_resources_from_db(&self, scope: &Scope) -> Result<Vec<BuiltResource>> {
        let built = if let Some(repo) = &self.state.route_config_repository {
            // Extract teams from scope for filtering
            // Default resources (team IS NULL) are always included
            let teams = match scope {
                Scope::All => vec![], // Admin access includes all resources
                Scope::Team { team } => vec![team.clone()],
                Scope::Allowlist { .. } => vec![], // Allowlist doesn't apply to routes
            };

            // Record team filtering in span
            let span = tracing::Span::current();
            if teams.is_empty() {
                span.record("teams", "all");
            } else {
                span.record("teams", format!("{:?}", teams).as_str());
            }

            // Always include default resources (include_default = true)
            match repo.list_by_teams(&teams, true, Some(100), None).await {
                Ok(mut route_data_list) => {
                    span.record("filtered_count", route_data_list.len());

                    // Record team-scoped resource count metrics
                    if let Some(team) = teams.first() {
                        crate::observability::metrics::update_team_resource_count(
                            "route",
                            team,
                            route_data_list.len(),
                        )
                        .await;
                    }

                    if route_data_list.is_empty() {
                        info!("No route configs found in database, falling back to config-based routes");
                        self.create_fallback_route_resources()?
                    } else {
                        // Inject attached filters into route configurations
                        if let Err(e) =
                            self.state.inject_route_config_filters(&mut route_data_list).await
                        {
                            warn!(error = %e, "Failed to inject filters into route configs for ADS response");
                        }

                        info!(
                            phase = "ads_response",
                            route_count = route_data_list.len(),
                            "Building route resources from database for ADS response"
                        );
                        resources::routes_from_database_entries(route_data_list, "ads_response")?
                    }
                }
                Err(e) => {
                    warn!("Failed to load routes from database: {}, falling back to config", e);
                    self.create_fallback_route_resources()?
                }
            }
        } else {
            info!("No database repository available, using config-based routes");
            self.create_fallback_route_resources()?
        };

        Ok(built)
    }

    fn create_fallback_route_resources(&self) -> Result<Vec<BuiltResource>> {
        resources::routes_from_config(&self.state.config)
    }

    #[tracing::instrument(skip(self, scope), fields(scope_info, filtered_count))]
    async fn create_listener_resources_from_db_scoped(
        &self,
        scope: &Scope,
    ) -> Result<Vec<BuiltResource>> {
        let mut built = if let Some(repo) = &self.state.listener_repository {
            // Extract teams from scope for filtering
            // Default resources (team IS NULL) are always included
            let teams = match scope {
                Scope::All => vec![], // Admin access includes all resources
                Scope::Team { team } => vec![team.clone()],
                Scope::Allowlist { .. } => vec![], // Allowlist doesn't apply to listeners
            };

            // Record team filtering in span
            let span = tracing::Span::current();
            if teams.is_empty() {
                span.record("scope_info", "all");
            } else {
                span.record("scope_info", format!("team:{}", teams[0]).as_str());
            }

            // Always include default resources (include_default = true)
            match repo.list_by_teams(&teams, true, Some(100), None).await {
                Ok(listener_data_list) => {
                    span.record("filtered_count", listener_data_list.len());

                    // Record team-scoped listener count metrics
                    if let Some(team) = teams.first() {
                        crate::observability::metrics::update_team_resource_count(
                            "listener",
                            team,
                            listener_data_list.len(),
                        )
                        .await;
                    }

                    if listener_data_list.is_empty() {
                        // Only provide fallback listener for admin scope (Scope::All)
                        // Team-scoped requests get empty list to enforce explicit listener definition
                        // This prevents port conflicts when multiple teams have no listeners
                        match scope {
                            Scope::All => {
                                info!(
                                    "No listeners in database, providing config-based fallback for admin scope"
                                );
                                self.create_fallback_listener_resources()?
                            }
                            Scope::Team { team, .. } => {
                                info!(
                                    team = %team,
                                    "No listeners found for team, returning empty list (teams must define listeners explicitly)"
                                );
                                vec![]
                            }
                            Scope::Allowlist { .. } => {
                                info!(
                                    "No listeners found for allowlist scope, returning empty list"
                                );
                                vec![]
                            }
                        }
                    } else {
                        info!(
                            phase = "ads_response",
                            listener_count = listener_data_list.len(),
                            "Building listener resources from database for ADS response"
                        );
                        resources::listeners_from_database_entries(
                            listener_data_list,
                            "ads_response",
                        )?
                    }
                }
                Err(e) => {
                    // On database error, only provide fallback for admin scope
                    match scope {
                        Scope::All => {
                            warn!("Failed to load listeners from database: {}, falling back to config", e);
                            self.create_fallback_listener_resources()?
                        }
                        Scope::Team { team, .. } => {
                            warn!(team = %team, error = %e, "Failed to load listeners from database for team, returning empty list");
                            vec![]
                        }
                        Scope::Allowlist { .. } => {
                            warn!(error = %e, "Failed to load listeners from database for allowlist, returning empty list");
                            vec![]
                        }
                    }
                }
            }
        } else {
            // No database repository - only provide fallback for admin scope
            match scope {
                Scope::All => {
                    info!("No database repository available, using config-based listener");
                    self.create_fallback_listener_resources()?
                }
                Scope::Team { team, .. } => {
                    info!(team = %team, "No database repository available for team, returning empty list");
                    vec![]
                }
                Scope::Allowlist { .. } => {
                    info!("No database repository available for allowlist, returning empty list");
                    vec![]
                }
            }
        };

        // Intentionally do not emit Platform API listeners here to avoid port conflicts

        // Inject listener-attached filters (e.g., JWT authentication)
        // This ensures Envoy receives listeners with user-defined filters attached
        if let Err(e) = self.state.inject_listener_auto_filters(&mut built).await {
            warn!(
                error = %e,
                "Failed to inject listener-attached filters in ADS response"
            );
        }

        // Inject access log configuration for active learning sessions
        // This ensures Envoy receives listeners with access log config when sessions are active
        if let Err(e) = self.state.inject_learning_session_access_logs(&mut built).await {
            warn!(
                error = %e,
                "Failed to inject access log configuration in ADS response"
            );
        }

        // Inject ExtProc filter for request/response body capture during learning sessions
        // Required for schema inference to produce inferred_schemas rows
        if let Err(e) = self.state.inject_learning_session_ext_proc(&mut built).await {
            warn!(
                error = %e,
                "Failed to inject ExtProc configuration in ADS response"
            );
        }

        Ok(built)
    }

    fn create_fallback_listener_resources(&self) -> Result<Vec<BuiltResource>> {
        resources::listeners_from_config(&self.state.config)
    }

    /// Create secret resources from database for SDS (Secret Discovery Service)
    ///
    /// Supports both:
    /// - Legacy database-stored secrets (encrypted configuration in DB)
    /// - Reference-based secrets (backend/reference fields, fetched on-demand)
    #[tracing::instrument(skip(self, scope), fields(teams, filtered_count))]
    async fn create_secret_resources_from_db(&self, scope: &Scope) -> Result<Vec<BuiltResource>> {
        let Some(repo) = &self.state.secret_repository else {
            info!("No secret repository available, returning empty secrets");
            return Ok(Vec::new());
        };

        // Extract teams from scope for filtering
        let teams = match scope {
            Scope::All => vec![],
            Scope::Team { team } => vec![team.clone()],
            Scope::Allowlist { .. } => vec![],
        };

        // Record team filtering in span
        let span = tracing::Span::current();
        if teams.is_empty() {
            span.record("teams", "all");
        } else {
            span.record("teams", format!("{:?}", teams).as_str());
        }

        match repo.list_by_teams(&teams, Some(100), None).await {
            Ok(mut secret_data_list) => {
                span.record("filtered_count", secret_data_list.len());

                // Record team-scoped resource count metrics
                if let Some(team) = teams.first() {
                    crate::observability::metrics::update_team_resource_count(
                        "secret",
                        team,
                        secret_data_list.len(),
                    )
                    .await;
                }

                if secret_data_list.is_empty() {
                    info!("No secrets found in database for scope");
                    return Ok(Vec::new());
                }

                // Resolve reference-based secrets from external backends
                if let Some(registry) = &self.state.secret_backend_registry {
                    info!(
                        secret_count = secret_data_list.len(),
                        "Resolving reference-based secrets from external backends"
                    );
                    self.resolve_reference_secrets(&mut secret_data_list, registry).await;
                } else {
                    warn!("No secret backend registry available, reference-based secrets will not be resolved");
                }

                info!(
                    phase = "ads_response",
                    secret_count = secret_data_list.len(),
                    "Building secret resources from database for SDS response"
                );

                secrets_from_database_entries(secret_data_list, "ads_response")
            }
            Err(e) => {
                warn!("Failed to load secrets from database: {}", e);
                Ok(Vec::new())
            }
        }
    }

    /// Resolve reference-based secrets by fetching from external backends
    ///
    /// For secrets with `backend` and `reference` fields set, this method
    /// fetches the actual secret from the external backend (Vault, AWS, GCP)
    /// and populates the `configuration` field with the JSON serialization.
    async fn resolve_reference_secrets(
        &self,
        secrets: &mut [crate::storage::SecretData],
        registry: &crate::secrets::backends::SecretBackendRegistry,
    ) {
        use crate::secrets::backends::SecretBackendType;

        for secret in secrets.iter_mut() {
            // Skip if not a reference-based secret
            let (Some(backend_str), Some(reference)) = (&secret.backend, &secret.reference) else {
                continue;
            };

            // Parse backend type
            let Some(backend_type) = SecretBackendType::from_str(backend_str) else {
                warn!(
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
                "Fetching reference-based secret from backend"
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
                                "Resolved reference-based secret from backend"
                            );
                            secret.configuration = json;
                        }
                        Err(e) => {
                            warn!(
                                secret_name = %secret.name,
                                error = %e,
                                "Failed to serialize secret spec"
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
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

    async fn build_resources(&self, type_url: &str, scope: &Scope) -> Result<Vec<BuiltResource>> {
        match type_url {
            "type.googleapis.com/envoy.config.cluster.v3.Cluster" => {
                self.create_cluster_resources_from_db(scope).await
            }
            "type.googleapis.com/envoy.config.route.v3.RouteConfiguration" => {
                self.create_route_resources_from_db(scope).await
            }
            "type.googleapis.com/envoy.config.listener.v3.Listener" => {
                self.create_listener_resources_from_db_scoped(scope).await
            }
            "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment" => {
                resources::endpoints_from_config(&self.state.config)
            }
            SECRET_TYPE_URL => self.create_secret_resources_from_db(scope).await,
            _ => {
                warn!("Unknown resource type requested: {}", type_url);
                Ok(Vec::new())
            }
        }
    }

    #[tracing::instrument(skip(self, request), fields(type_url = %request.type_url, team, scope_type, resource_count))]
    async fn create_delta_response(
        &self,
        request: &DeltaDiscoveryRequest,
    ) -> Result<DeltaDiscoveryResponse> {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        // Build all available resources for this type
        // The stream logic will handle proper delta filtering and ACK detection
        let scope = scope_from_discovery(&request.node);

        // Record team and scope information in the span
        let span = tracing::Span::current();
        match &scope {
            Scope::All => {
                span.record("scope_type", "all");
                span.record("team", "admin");
            }
            Scope::Team { team } => {
                span.record("scope_type", "team");
                span.record("team", team.as_str());
            }
            Scope::Allowlist { names } => {
                span.record("scope_type", "allowlist");
                span.record("team", format!("allowlist:{}", names.len()).as_str());
            }
        }

        let built = self.build_resources(&request.type_url, &scope).await?;
        span.record("resource_count", built.len());

        let resources: Vec<Resource> = built
            .into_iter()
            .map(|r| Resource {
                name: r.name,
                version: version.clone(),
                resource: Some(r.resource),
                ..Default::default()
            })
            .collect();

        Ok(DeltaDiscoveryResponse {
            system_version_info: version.clone(),
            type_url: request.type_url.clone(),
            nonce,
            resources,
            removed_resources: request.resource_names_unsubscribe.clone(),
            ..Default::default()
        })
    }
}

fn spawn_cluster_watcher(state: Arc<XdsState>, repository: ClusterRepository) {
    tokio::spawn(async move {
        use tokio::time::{sleep, Duration};

        if let Err(error) = state.refresh_clusters_from_repository().await {
            warn!(%error, "Failed to initialize cluster cache from repository");
        }

        // Track cluster state using count + last modification timestamp
        // This avoids false positives from PRAGMA data_version which can change
        // due to SQLite internal operations (WAL checkpoints, vacuum, etc.)
        let mut last_cluster_state: Option<(i64, Option<String>)> = None;

        loop {
            // Query actual cluster data: count and max updated_at timestamp
            // This only changes when clusters are actually added/removed/modified
            let poll_result = sqlx::query_as::<_, (i64, Option<String>)>(
                "SELECT COUNT(*), MAX(updated_at) FROM clusters",
            )
            .fetch_one(repository.pool())
            .await;

            match poll_result {
                Ok(current_state) => match &last_cluster_state {
                    Some(previous_state) if previous_state == &current_state => {
                        // No actual cluster changes, skip update
                    }
                    Some(_) => {
                        last_cluster_state = Some(current_state.clone());
                        info!(
                            cluster_count = current_state.0,
                            last_updated = ?current_state.1,
                            "Cluster data changed, refreshing cluster cache"
                        );
                        if let Err(error) = state.refresh_clusters_from_repository().await {
                            warn!(%error, "Failed to refresh cluster cache from repository");
                        }
                    }
                    None => {
                        // First poll, just record the state without triggering update
                        last_cluster_state = Some(current_state);
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Failed to poll cluster state for change detection");
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    });
}

fn spawn_route_config_watcher(state: Arc<XdsState>, repository: RouteConfigRepository) {
    tokio::spawn(async move {
        use tokio::time::{sleep, Duration};

        if let Err(error) = state.refresh_routes_from_repository().await {
            warn!(%error, "Failed to initialize route config cache from repository");
        }

        // Track route config state using count + last modification timestamp
        // This avoids false positives from PRAGMA data_version which can change
        // due to SQLite internal operations (WAL checkpoints, vacuum, etc.)
        let mut last_route_config_state: Option<(i64, Option<String>)> = None;

        loop {
            // Query actual route config data: count and max updated_at timestamp
            // This only changes when route configs are actually added/removed/modified
            let poll_result = sqlx::query_as::<_, (i64, Option<String>)>(
                "SELECT COUNT(*), MAX(updated_at) FROM route_configs",
            )
            .fetch_one(repository.pool())
            .await;

            match poll_result {
                Ok(current_state) => match &last_route_config_state {
                    Some(previous_state) if previous_state == &current_state => {
                        // No actual route config changes, skip update
                    }
                    Some(_) => {
                        last_route_config_state = Some(current_state.clone());
                        info!(
                            route_config_count = current_state.0,
                            last_updated = ?current_state.1,
                            "Route config data changed, refreshing route cache"
                        );
                        if let Err(error) = state.refresh_routes_from_repository().await {
                            warn!(%error, "Failed to refresh route config cache from repository");
                        }
                    }
                    None => {
                        // First poll, just record the state without triggering update
                        last_route_config_state = Some(current_state);
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Failed to poll route config state for change detection");
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    });
}

fn spawn_listener_watcher(state: Arc<XdsState>, repository: ListenerRepository) {
    tokio::spawn(async move {
        use tokio::time::{sleep, Duration};

        if let Err(error) = state.refresh_listeners_from_repository().await {
            warn!(%error, "Failed to initialize listener cache from repository");
        }

        // Track listener state using count + last modification timestamp
        // This avoids false positives from PRAGMA data_version which can change
        // due to SQLite internal operations (WAL checkpoints, vacuum, etc.)
        let mut last_listener_state: Option<(i64, Option<String>)> = None;

        loop {
            // Query actual listener data: count and max updated_at timestamp
            // This only changes when listeners are actually added/removed/modified
            let poll_result = sqlx::query_as::<_, (i64, Option<String>)>(
                "SELECT COUNT(*), MAX(updated_at) FROM listeners",
            )
            .fetch_one(repository.pool())
            .await;

            match poll_result {
                Ok(current_state) => match &last_listener_state {
                    Some(previous_state) if previous_state == &current_state => {
                        // No actual listener changes, skip update
                    }
                    Some(_) => {
                        last_listener_state = Some(current_state.clone());
                        info!(
                            listener_count = current_state.0,
                            last_updated = ?current_state.1,
                            "Listener data changed, refreshing listener cache"
                        );
                        if let Err(error) = state.refresh_listeners_from_repository().await {
                            warn!(%error, "Failed to refresh listener cache from repository");
                        }
                    }
                    None => {
                        // First poll, just record the state without triggering update
                        last_listener_state = Some(current_state);
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Failed to poll listener state for change detection");
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    });
}

fn spawn_secret_watcher(state: Arc<XdsState>, repository: SecretRepository) {
    tokio::spawn(async move {
        use tokio::time::{sleep, Duration};

        if let Err(error) = state.refresh_secrets_from_repository().await {
            warn!(%error, "Failed to initialize secret cache from repository");
        }

        // Track secret state using count + last modification timestamp
        let mut last_secret_state: Option<(i64, Option<String>)> = None;

        loop {
            // Query actual secret data: count and max updated_at timestamp
            let poll_result = sqlx::query_as::<_, (i64, Option<String>)>(
                "SELECT COUNT(*), MAX(updated_at) FROM secrets",
            )
            .fetch_one(repository.pool())
            .await;

            match poll_result {
                Ok(current_state) => match &last_secret_state {
                    Some(previous_state) if previous_state == &current_state => {
                        // No actual secret changes, skip update
                    }
                    Some(_) => {
                        last_secret_state = Some(current_state.clone());
                        info!(
                            secret_count = current_state.0,
                            last_updated = ?current_state.1,
                            "Secret data changed, refreshing secret cache"
                        );
                        if let Err(error) = state.refresh_secrets_from_repository().await {
                            warn!(%error, "Failed to refresh secret cache from repository");
                        }
                    }
                    None => {
                        // First poll, just record the state without triggering update
                        last_secret_state = Some(current_state);
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Failed to poll secret state for change detection");
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    });
}

#[tonic::async_trait]
impl AggregatedDiscoveryService for DatabaseAggregatedDiscoveryService {
    type StreamAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = std::result::Result<DiscoveryResponse, Status>> + Send>>;
    type DeltaAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = std::result::Result<DeltaDiscoveryResponse, Status>> + Send>>;

    #[tracing::instrument(skip(self, request), fields(stream_type = "SOTW_ADS"))]
    async fn stream_aggregated_resources(
        &self,
        request: Request<tonic::Streaming<DiscoveryRequest>>,
    ) -> std::result::Result<Response<Self::StreamAggregatedResourcesStream>, Status> {
        info!("New database-enabled ADS stream connection established");

        // Extract trace context from gRPC metadata for distributed tracing
        let parent_context =
            crate::xds::services::stream::extract_trace_context(request.metadata());

        // Extract mTLS identity from client certificate (if mTLS is enabled)
        let mtls_identity = if super::mtls::is_xds_mtls_enabled() {
            if let Some(peer_certs) = request.peer_certs() {
                match super::mtls::extract_client_identity(peer_certs.as_slice()) {
                    Some(identity) => {
                        info!(
                            team = %identity.team,
                            proxy_id = %identity.proxy_id,
                            serial = %identity.serial_number,
                            "Client authenticated via mTLS"
                        );
                        Some(identity)
                    }
                    None => {
                        warn!("mTLS enabled but no valid SPIFFE identity in client certificate");
                        return Err(Status::unauthenticated(
                            "Client certificate does not contain valid SPIFFE identity",
                        ));
                    }
                }
            } else {
                warn!("mTLS enabled but client did not present certificate");
                return Err(Status::unauthenticated(
                    "Client certificate required for mTLS authentication",
                ));
            }
        } else {
            // mTLS not enabled, will use node metadata for team identity
            None
        };

        // Extract team from node metadata for connection tracking (fallback when mTLS disabled)
        let metadata = request.metadata();
        if let Some(node_id) = metadata.get("node-id").and_then(|v| v.to_str().ok()) {
            tracing::debug!(node_id, "xDS stream established");
        }

        // Store mTLS team override for use in stream processing
        let mtls_team_override = mtls_identity.map(|id| id.team);

        let state = self.state.clone();
        let responder = move |state: Arc<XdsState>, request: DiscoveryRequest| {
            let service = DatabaseAggregatedDiscoveryService { state };
            Box::pin(async move { service.create_resource_response(&request).await })
                as Pin<Box<dyn Future<Output = Result<DiscoveryResponse>> + Send>>
        };

        let stream = crate::xds::services::stream::run_stream_loop_with_mtls(
            state,
            request.into_inner(),
            responder,
            "database-enabled",
            Some(parent_context),
            mtls_team_override,
        );

        Ok(Response::new(Box::pin(stream)))
    }

    #[tracing::instrument(skip(self, request), fields(stream_type = "Delta_ADS"))]
    async fn delta_aggregated_resources(
        &self,
        request: Request<tonic::Streaming<DeltaDiscoveryRequest>>,
    ) -> std::result::Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        info!("Delta ADS stream connection established (database-enabled)");

        // Extract trace context from gRPC metadata for distributed tracing
        let parent_context =
            crate::xds::services::stream::extract_trace_context(request.metadata());

        // Extract mTLS identity from client certificate (if mTLS is enabled)
        let mtls_identity = if super::mtls::is_xds_mtls_enabled() {
            if let Some(peer_certs) = request.peer_certs() {
                match super::mtls::extract_client_identity(peer_certs.as_slice()) {
                    Some(identity) => {
                        info!(
                            team = %identity.team,
                            proxy_id = %identity.proxy_id,
                            serial = %identity.serial_number,
                            "Client authenticated via mTLS (Delta)"
                        );
                        Some(identity)
                    }
                    None => {
                        warn!("mTLS enabled but no valid SPIFFE identity in client certificate");
                        return Err(Status::unauthenticated(
                            "Client certificate does not contain valid SPIFFE identity",
                        ));
                    }
                }
            } else {
                warn!("mTLS enabled but client did not present certificate");
                return Err(Status::unauthenticated(
                    "Client certificate required for mTLS authentication",
                ));
            }
        } else {
            None
        };

        // Store mTLS team override for use in stream processing
        let mtls_team_override = mtls_identity.map(|id| id.team);

        let responder = move |state: Arc<XdsState>, request: DeltaDiscoveryRequest| {
            let service = DatabaseAggregatedDiscoveryService { state };
            Box::pin(async move { service.create_delta_response(&request).await })
                as Pin<Box<dyn Future<Output = Result<DeltaDiscoveryResponse>> + Send>>
        };

        let stream = crate::xds::services::stream::run_delta_loop_with_mtls(
            self.state.clone(),
            request.into_inner(),
            responder,
            "database-enabled",
            Some(parent_context),
            mtls_team_override,
        );

        Ok(Response::new(Box::pin(stream)))
    }
}
#[derive(Debug, Clone)]
enum Scope {
    All,
    Team { team: String },
    Allowlist { names: Vec<String> },
}

fn scope_from_discovery(node: &Option<envoy_types::pb::envoy::config::core::v3::Node>) -> Scope {
    if let Some(n) = node {
        if let Some(meta) = &n.metadata {
            let mut team: Option<String> = None;
            let mut allow: Vec<String> = Vec::new();

            if let Some(envoy_types::pb::google::protobuf::value::Kind::StringValue(s)) =
                meta.fields.get("team").and_then(|v| v.kind.as_ref())
            {
                if !s.is_empty() {
                    team = Some(s.clone());
                }
            }
            if let Some(envoy_types::pb::google::protobuf::value::Kind::ListValue(lv)) =
                meta.fields.get("listener_allowlist").and_then(|v| v.kind.as_ref())
            {
                for item in &lv.values {
                    if let Some(envoy_types::pb::google::protobuf::value::Kind::StringValue(s)) =
                        item.kind.as_ref()
                    {
                        allow.push(s.clone());
                    }
                }
            }

            if !allow.is_empty() {
                return Scope::Allowlist { names: allow };
            }
            if let Some(team) = team {
                return Scope::Team { team };
            }
        }
    }
    Scope::All
}
