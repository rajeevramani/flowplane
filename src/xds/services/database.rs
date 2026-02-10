use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::{
    storage::repositories::TeamRepository,
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
    ///
    /// # Arguments
    /// - `request`: The discovery request from Envoy
    /// - `mtls_team`: The team extracted from client's mTLS certificate (if mTLS enabled)
    #[tracing::instrument(skip(self, request, mtls_team), fields(type_url = %request.type_url, team, scope_type, resource_count))]
    async fn create_resource_response(
        &self,
        request: &DiscoveryRequest,
        mtls_team: Option<&str>,
    ) -> std::result::Result<DiscoveryResponse, Status> {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        let scope = scope_from_discovery(&request.node, mtls_team)?;

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

        let built = self
            .build_resources(request.type_url.as_str(), &scope)
            .await
            .map_err(|e| Status::internal(format!("Failed to build resources: {}", e)))?;
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
            // Extract teams from scope with fail-safe behavior
            let teams_result = teams_from_scope(scope, "cluster")
                .map_err(|e| crate::errors::Error::internal(e.message()))?;

            // Check if Allowlist scope (None = default only) before moving value
            let is_default_only = teams_result.is_none();

            // Record team filtering in span
            let span = tracing::Span::current();
            let teams = match teams_result {
                Some(t) if t.is_empty() => {
                    span.record("teams", "none");
                    t
                }
                Some(t) => {
                    span.record("teams", format!("{:?}", t).as_str());
                    t
                }
                None => {
                    // Allowlist scope - return only default resources
                    span.record("teams", "default-only");
                    vec![]
                }
            };

            // Resolve team names to UUIDs
            let teams = resolve_teams_for_xds(teams, &self.state)
                .await
                .map_err(|e| crate::errors::Error::internal(e.message()))?;

            // If Allowlist scope, query only default resources
            let cluster_data_list = if is_default_only {
                // For Allowlist on clusters, return only shared/default clusters
                repo.list_default_only(Some(100), None).await
            } else {
                // Normal team-based filtering
                repo.list_by_teams(&teams, true, Some(100), None).await
            };

            match cluster_data_list {
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
            // Extract teams from scope with fail-safe behavior
            let teams_result = teams_from_scope(scope, "route")
                .map_err(|e| crate::errors::Error::internal(e.message()))?;

            // Check if Allowlist scope (None = default only) before moving value
            let is_default_only = teams_result.is_none();

            // Record team filtering in span
            let span = tracing::Span::current();
            let teams = match teams_result {
                Some(t) if t.is_empty() => {
                    span.record("teams", "none");
                    t
                }
                Some(t) => {
                    span.record("teams", format!("{:?}", t).as_str());
                    t
                }
                None => {
                    // Allowlist scope - return only default resources
                    span.record("teams", "default-only");
                    vec![]
                }
            };

            // Resolve team names to UUIDs
            let teams = resolve_teams_for_xds(teams, &self.state)
                .await
                .map_err(|e| crate::errors::Error::internal(e.message()))?;

            // If Allowlist scope, query only default resources
            let route_data_result = if is_default_only {
                repo.list_default_only(Some(100), None).await
            } else {
                repo.list_by_teams(&teams, true, Some(100), None).await
            };

            match route_data_result {
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
            // Extract teams from scope with fail-safe behavior
            let teams_result = teams_from_scope(scope, "listener")
                .map_err(|e| crate::errors::Error::internal(e.message()))?;

            // Check if Allowlist scope (None = default only) before moving value
            let is_default_only = teams_result.is_none();

            // Record team filtering in span
            let span = tracing::Span::current();
            let teams = match teams_result {
                Some(t) if t.is_empty() => {
                    span.record("scope_info", "none");
                    t
                }
                Some(t) => {
                    span.record("scope_info", format!("team:{}", t[0]).as_str());
                    t
                }
                None => {
                    // Allowlist scope - for listeners, we still query default resources
                    // (Allowlist filtering by name happens after retrieval)
                    span.record("scope_info", "default-only");
                    vec![]
                }
            };

            // Resolve team names to UUIDs
            let teams = resolve_teams_for_xds(teams, &self.state)
                .await
                .map_err(|e| crate::errors::Error::internal(e.message()))?;

            // If Allowlist scope, query only default resources
            let listener_data_result = if is_default_only {
                repo.list_default_only(Some(100), None).await
            } else {
                repo.list_by_teams(&teams, true, Some(100), None).await
            };

            match listener_data_result {
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
                        // Team-scoped requests get empty list to enforce explicit listener definition
                        // This prevents port conflicts when multiple teams have no listeners
                        // Note: Scope::All should never reach here after mTLS auth fix
                        match scope {
                            Scope::All => {
                                // SECURITY: This branch should be unreachable after mTLS auth fix
                                tracing::error!(
                                    "SECURITY: Scope::All reached in listener fallback - should be unreachable"
                                );
                                vec![] // Fail closed - return empty, not fallback
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
                    // On database error, return empty (fail closed)
                    match scope {
                        Scope::All => {
                            // SECURITY: This branch should be unreachable after mTLS auth fix
                            tracing::error!(
                                error = %e,
                                "SECURITY: Scope::All reached in listener error handler - should be unreachable"
                            );
                            vec![] // Fail closed - return empty, not fallback
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

        // Extract teams from scope with fail-safe behavior
        let teams_result = teams_from_scope(scope, "secret")
            .map_err(|e| crate::errors::Error::internal(e.message()))?;

        // Check if Allowlist scope (None = default only) before moving value
        let is_default_only = teams_result.is_none();

        // Record team filtering in span
        let span = tracing::Span::current();
        let teams = match teams_result {
            Some(t) if t.is_empty() => {
                span.record("teams", "none");
                t
            }
            Some(t) => {
                span.record("teams", format!("{:?}", t).as_str());
                t
            }
            None => {
                // Allowlist scope - return only default resources (typically empty for secrets)
                span.record("teams", "default-only");
                vec![]
            }
        };

        // Resolve team names to UUIDs
        let teams = resolve_teams_for_xds(teams, &self.state)
            .await
            .map_err(|e| crate::errors::Error::internal(e.message()))?;

        // If Allowlist scope, query only default resources
        let secret_data_result = if is_default_only {
            repo.list_default_only(Some(100), None).await
        } else {
            repo.list_by_teams(&teams, Some(100), None).await
        };

        match secret_data_result {
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
            let Some(backend_type) = backend_str.parse::<SecretBackendType>().ok() else {
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

    /// Create delta discovery response with database-backed resources
    ///
    /// # Arguments
    /// - `request`: The delta discovery request from Envoy
    /// - `mtls_team`: The team extracted from client's mTLS certificate (if mTLS enabled)
    #[tracing::instrument(skip(self, request, mtls_team), fields(type_url = %request.type_url, team, scope_type, resource_count))]
    async fn create_delta_response(
        &self,
        request: &DeltaDiscoveryRequest,
        mtls_team: Option<&str>,
    ) -> std::result::Result<DeltaDiscoveryResponse, Status> {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        // Build all available resources for this type
        // The stream logic will handle proper delta filtering and ACK detection
        let scope = scope_from_discovery(&request.node, mtls_team)?;

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

        let built = self
            .build_resources(&request.type_url, &scope)
            .await
            .map_err(|e| Status::internal(format!("Failed to build resources: {}", e)))?;
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
        // This avoids false positives from polling-based approaches
        let mut last_cluster_state: Option<(i64, Option<String>)> = None;

        loop {
            // Query actual cluster data: count and max updated_at timestamp
            // This only changes when clusters are actually added/removed/modified
            let poll_result = sqlx::query_as::<_, (i64, Option<String>)>(
                "SELECT COUNT(*), MAX(updated_at)::TEXT FROM clusters",
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
        // This avoids false positives from polling-based approaches
        let mut last_route_config_state: Option<(i64, Option<String>)> = None;

        loop {
            // Query actual route config data: count and max updated_at timestamp
            // This only changes when route configs are actually added/removed/modified
            let poll_result = sqlx::query_as::<_, (i64, Option<String>)>(
                "SELECT COUNT(*), MAX(updated_at)::TEXT FROM route_configs",
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
        // This avoids false positives from polling-based approaches
        let mut last_listener_state: Option<(i64, Option<String>)> = None;

        loop {
            // Query actual listener data: count and max updated_at timestamp
            // This only changes when listeners are actually added/removed/modified
            let poll_result = sqlx::query_as::<_, (i64, Option<String>)>(
                "SELECT COUNT(*), MAX(updated_at)::TEXT FROM listeners",
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
                "SELECT COUNT(*), MAX(updated_at)::TEXT FROM secrets",
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
        // SECURITY: This team is cryptographically verified from the client certificate
        let mtls_team_override = mtls_identity.map(|id| id.team);

        let state = self.state.clone();
        // Clone mtls_team for capture in responder closure
        let mtls_team_for_responder = mtls_team_override.clone();
        let responder = move |state: Arc<XdsState>, request: DiscoveryRequest| {
            let service = DatabaseAggregatedDiscoveryService { state };
            // SECURITY: Pass mTLS team to create_resource_response for authorization
            let mtls_team = mtls_team_for_responder.clone();
            Box::pin(async move {
                service
                    .create_resource_response(&request, mtls_team.as_deref())
                    .await
                    .map_err(|status| crate::errors::FlowplaneError::internal(status.message()))
            }) as Pin<Box<dyn Future<Output = Result<DiscoveryResponse>> + Send>>
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
        // SECURITY: This team is cryptographically verified from the client certificate
        let mtls_team_override = mtls_identity.map(|id| id.team);

        // Clone mtls_team for capture in responder closure
        let mtls_team_for_responder = mtls_team_override.clone();
        let responder = move |state: Arc<XdsState>, request: DeltaDiscoveryRequest| {
            let service = DatabaseAggregatedDiscoveryService { state };
            // SECURITY: Pass mTLS team to create_delta_response for authorization
            let mtls_team = mtls_team_for_responder.clone();
            Box::pin(async move {
                service
                    .create_delta_response(&request, mtls_team.as_deref())
                    .await
                    .map_err(|status| crate::errors::FlowplaneError::internal(status.message()))
            }) as Pin<Box<dyn Future<Output = Result<DeltaDiscoveryResponse>> + Send>>
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
/// Scope determines which resources a client can access via xDS.
///
/// # Security Model
/// - `Team`: Client can only access resources belonging to their team (default)
/// - `Allowlist`: Client can only access specifically named listeners (for shared infrastructure)
/// - `All`: Admin access to all resources (DEPRECATED - kept for compatibility but never constructed)
///
/// As of the mTLS authorization security fix, `Scope::All` is never returned from
/// `scope_from_discovery()`. Connections without team identity are rejected.
#[derive(Debug, Clone)]
#[allow(dead_code)] // All variant kept for potential future admin API use
enum Scope {
    /// Admin access - DEPRECATED: No longer constructed, kept for backward compatibility
    All,
    /// Team-scoped access - client sees only their team's resources
    Team { team: String },
    /// Allowlist - client sees only specifically named listeners
    Allowlist { names: Vec<String> },
}

/// Extract teams from scope for repository filtering.
///
/// # Security
/// This function implements **fail-safe** behavior for deprecated/edge case scopes:
/// - `Scope::All`: DEPRECATED - logs security error and returns `Err` (fails closed)
/// - `Scope::Allowlist`: Returns special marker for "default only" access
/// - `Scope::Team`: Returns the team in a Vec
///
/// # Returns
/// - `Ok(Some(teams))` - Team(s) to filter by (may be empty for default-only access)
/// - `Ok(None)` - Use default resources only (for Allowlist on non-listener resources)
/// - `Err` - Scope::All encountered (should never happen after mTLS auth fix)
fn teams_from_scope(
    scope: &Scope,
    resource_type: &str,
) -> std::result::Result<Option<Vec<String>>, tonic::Status> {
    match scope {
        Scope::All => {
            // SECURITY: Scope::All should never be constructed after mTLS authorization fix.
            // If it somehow is, this is a security bug - fail closed.
            tracing::error!(
                resource_type = %resource_type,
                "SECURITY: Scope::All encountered but should be unreachable after mTLS auth fix. \
                 This indicates a potential security vulnerability in scope_from_discovery()."
            );
            Err(tonic::Status::internal(
                "Invalid scope: admin scope is not permitted for xDS connections",
            ))
        }
        Scope::Team { team } => Ok(Some(vec![team.clone()])),
        Scope::Allowlist { .. } => {
            // For Allowlist scope on non-listener resources, return None to signal
            // that only default resources (team IS NULL) should be returned.
            // This prevents Allowlist from granting access to all teams' resources.
            Ok(None)
        }
    }
}

/// Resolve team names to UUIDs for database queries.
/// After FK migration, resource tables store team UUIDs, not names.
async fn resolve_teams_for_xds(
    teams: Vec<String>,
    state: &XdsState,
) -> std::result::Result<Vec<String>, tonic::Status> {
    if teams.is_empty() {
        return Ok(teams);
    }
    if let Some(team_repo) = &state.team_repository {
        team_repo
            .resolve_team_ids(None, &teams)
            .await
            .map_err(|e| tonic::Status::internal(format!("Failed to resolve team IDs: {}", e)))
    } else {
        Ok(teams)
    }
}

/// Determine the scope for xDS resource filtering based on mTLS identity and node metadata.
///
/// # Security
/// - When `mtls_team` is provided (from client certificate), it takes precedence over node metadata
/// - This prevents attackers from using a valid cert for team-A while requesting team-B resources via metadata
/// - If neither mTLS identity nor node metadata provides a team, the connection is REJECTED
///
/// # Arguments
/// - `node`: The Envoy node from the discovery request (contains self-reported metadata)
/// - `mtls_team`: The team extracted from the client's mTLS certificate SPIFFE URI (cryptographically verified)
///
/// # Returns
/// - `Ok(Scope)` - The scope to use for resource filtering
/// - `Err(Status)` - Permission denied if no team identity is available
fn scope_from_discovery(
    node: &Option<envoy_types::pb::envoy::config::core::v3::Node>,
    mtls_team: Option<&str>,
) -> std::result::Result<Scope, Status> {
    // Extract team from node metadata (self-reported, untrusted)
    let metadata_team = node.as_ref().and_then(|n| {
        n.metadata.as_ref().and_then(|meta| {
            if let Some(envoy_types::pb::google::protobuf::value::Kind::StringValue(s)) =
                meta.fields.get("team").and_then(|v| v.kind.as_ref())
            {
                if !s.is_empty() {
                    return Some(s.clone());
                }
            }
            None
        })
    });

    // SECURITY: Prefer mTLS team (cryptographically verified) over node metadata (self-reported)
    let effective_team = mtls_team.map(|t| t.to_string()).or(metadata_team);

    // Log warning if mTLS team differs from metadata team (potential attack or misconfiguration)
    if let (Some(mtls), Some(ref meta)) = (mtls_team, &effective_team) {
        if mtls != meta.as_str() {
            warn!(
                mtls_team = %mtls,
                metadata_team = %meta,
                "mTLS team differs from node.metadata.team - using mTLS team (this may indicate an attack or misconfiguration)"
            );
        }
    }

    // Extract allowlist from node metadata (only valid when no team scope)
    let allowlist = node.as_ref().and_then(|n| {
        n.metadata.as_ref().and_then(|meta| {
            if let Some(envoy_types::pb::google::protobuf::value::Kind::ListValue(lv)) =
                meta.fields.get("listener_allowlist").and_then(|v| v.kind.as_ref())
            {
                let names: Vec<String> = lv
                    .values
                    .iter()
                    .filter_map(|item| {
                        if let Some(envoy_types::pb::google::protobuf::value::Kind::StringValue(
                            s,
                        )) = item.kind.as_ref()
                        {
                            Some(s.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                if !names.is_empty() {
                    return Some(names);
                }
            }
            None
        })
    });

    // Determine scope: allowlist > team > reject
    // Note: Allowlist is only for specific use cases (e.g., shared infrastructure)
    if let Some(names) = allowlist {
        return Ok(Scope::Allowlist { names });
    }

    if let Some(team) = effective_team {
        return Ok(Scope::Team { team });
    }

    // SECURITY: Reject connections without team identity
    // This prevents accidental admin access when team is missing
    Err(Status::permission_denied(
        "Team identity required for xDS connection. Provide either: \
         (1) mTLS certificate with SPIFFE URI containing team, or \
         (2) node.metadata.team in Envoy bootstrap configuration",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoy_types::pb::envoy::config::core::v3::Node;
    use envoy_types::pb::google::protobuf::{value::Kind, ListValue, Struct, Value};
    use std::collections::HashMap;

    fn create_node_with_metadata(team: Option<&str>, allowlist: Option<Vec<&str>>) -> Option<Node> {
        let mut fields = HashMap::new();

        if let Some(t) = team {
            fields
                .insert("team".to_string(), Value { kind: Some(Kind::StringValue(t.to_string())) });
        }

        if let Some(list) = allowlist {
            let values = list
                .into_iter()
                .map(|s| Value { kind: Some(Kind::StringValue(s.to_string())) })
                .collect();
            fields.insert(
                "listener_allowlist".to_string(),
                Value { kind: Some(Kind::ListValue(ListValue { values })) },
            );
        }

        Some(Node {
            id: "test-node".to_string(),
            metadata: Some(Struct { fields }),
            ..Default::default()
        })
    }

    // ==========================================================================
    // Security Tests: mTLS Authorization Fix
    // ==========================================================================

    #[test]
    fn test_mtls_team_takes_precedence_over_metadata() {
        // SECURITY: When mTLS provides team, it should override node.metadata.team
        // This prevents attackers from using valid cert for team-A while requesting team-B
        let node = create_node_with_metadata(Some("attacker-requested-team"), None);
        let mtls_team = Some("actual-cert-team");

        let result = scope_from_discovery(&node, mtls_team);

        assert!(result.is_ok());
        match result.unwrap() {
            Scope::Team { team } => {
                assert_eq!(team, "actual-cert-team");
            }
            _ => panic!("Expected Scope::Team"),
        }
    }

    #[test]
    fn test_connection_rejected_without_team_identity() {
        // SECURITY: Connections without any team identity should be rejected
        // This prevents accidental admin access
        let node = create_node_with_metadata(None, None);
        let mtls_team = None;

        let result = scope_from_discovery(&node, mtls_team);

        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert!(status.message().contains("Team identity required"));
    }

    #[test]
    fn test_metadata_team_used_when_no_mtls() {
        // When mTLS is not enabled, node.metadata.team should be used
        let node = create_node_with_metadata(Some("metadata-team"), None);
        let mtls_team = None;

        let result = scope_from_discovery(&node, mtls_team);

        assert!(result.is_ok());
        match result.unwrap() {
            Scope::Team { team } => {
                assert_eq!(team, "metadata-team");
            }
            _ => panic!("Expected Scope::Team"),
        }
    }

    #[test]
    fn test_empty_metadata_team_rejected() {
        // Empty team string should be treated as missing
        let mut fields = HashMap::new();
        fields.insert("team".to_string(), Value { kind: Some(Kind::StringValue("".to_string())) });
        let node = Some(Node {
            id: "test-node".to_string(),
            metadata: Some(Struct { fields }),
            ..Default::default()
        });

        let result = scope_from_discovery(&node, None);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn test_allowlist_still_works() {
        // Allowlist should still work for shared infrastructure use cases
        let node = create_node_with_metadata(None, Some(vec!["listener-1", "listener-2"]));
        let mtls_team = None;

        let result = scope_from_discovery(&node, mtls_team);

        assert!(result.is_ok());
        match result.unwrap() {
            Scope::Allowlist { names } => {
                assert_eq!(names, vec!["listener-1", "listener-2"]);
            }
            _ => panic!("Expected Scope::Allowlist"),
        }
    }

    #[test]
    fn test_none_node_rejected() {
        // No node at all should be rejected
        let result = scope_from_discovery(&None, None);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn test_mtls_team_with_empty_metadata() {
        // mTLS team should work even when metadata is empty
        let node = Some(Node { id: "test-node".to_string(), metadata: None, ..Default::default() });

        let result = scope_from_discovery(&node, Some("mtls-team"));

        assert!(result.is_ok());
        match result.unwrap() {
            Scope::Team { team } => {
                assert_eq!(team, "mtls-team");
            }
            _ => panic!("Expected Scope::Team"),
        }
    }

    // ==========================================================================
    // Security Tests: teams_from_scope() fail-safe behavior
    // ==========================================================================

    #[test]
    fn test_teams_from_scope_all_returns_error() {
        // SECURITY: Scope::All should NEVER be constructed after mTLS auth fix
        // If it somehow is, teams_from_scope() must fail closed (return error)
        let scope = Scope::All;

        let result = teams_from_scope(&scope, "cluster");

        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Internal);
        assert!(status.message().contains("admin scope is not permitted"));
    }

    #[test]
    fn test_teams_from_scope_team_returns_team() {
        // Normal team scope should return the team
        let scope = Scope::Team { team: "my-team".to_string() };

        let result = teams_from_scope(&scope, "cluster");

        assert!(result.is_ok());
        let teams = result.unwrap();
        assert!(teams.is_some());
        assert_eq!(teams.unwrap(), vec!["my-team"]);
    }

    #[test]
    fn test_teams_from_scope_allowlist_returns_none() {
        // Allowlist scope should return None (meaning default-only resources)
        // This prevents Allowlist from returning ALL resources
        let scope = Scope::Allowlist { names: vec!["listener-1".to_string()] };

        let result = teams_from_scope(&scope, "cluster");

        assert!(result.is_ok());
        let teams = result.unwrap();
        assert!(teams.is_none()); // None means "default only"
    }

    #[test]
    fn test_teams_from_scope_logs_error_for_scope_all() {
        // This test verifies the function doesn't panic and logs appropriately
        // The actual logging is tested by tracing assertions in integration tests
        let scope = Scope::All;

        // Should not panic
        let result = teams_from_scope(&scope, "test_resource");

        // Should return error
        assert!(result.is_err());
    }

    #[test]
    fn test_teams_from_scope_different_resource_types() {
        // Verify the function works correctly for different resource types
        let scope = Scope::Team { team: "engineering".to_string() };

        for resource_type in ["cluster", "route", "listener", "secret"] {
            let result = teams_from_scope(&scope, resource_type);
            assert!(result.is_ok(), "Failed for resource type: {}", resource_type);
            assert_eq!(result.unwrap(), Some(vec!["engineering".to_string()]));
        }
    }
}
