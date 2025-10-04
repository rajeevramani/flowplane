use std::sync::Arc;

use sqlx;

use super::{
    audit, bootstrap,
    validation::{ensure_domain_available, ensure_route_available, ensure_target_listeners_exist},
};
use crate::domain::api_definition::{
    ApiDefinitionSpec as DomainApiDefinitionSpec, AppendRouteOutcome as DomainAppendRouteOutcome,
    CreateDefinitionOutcome as DomainCreateDefinitionOutcome, ListenerConfig, RouteConfig,
};
use crate::errors::{Error, Result};
use crate::storage::{
    ApiDefinitionData, ApiDefinitionRepository, ApiRouteData, AuditLogRepository,
    CreateApiDefinitionRequest, CreateApiRouteRequest,
};
use crate::xds::XdsState;

/// Type alias for backward compatibility
pub type ApiDefinitionSpec = DomainApiDefinitionSpec;

/// Type alias for backward compatibility
pub type RouteSpec = RouteConfig;

/// Type alias for backward compatibility
pub type ListenerInput = ListenerConfig;

/// Type alias for backward compatibility - concrete outcome with storage types
pub type CreateDefinitionOutcome = DomainCreateDefinitionOutcome<ApiDefinitionData, ApiRouteData>;

/// Type alias for backward compatibility - concrete outcome with storage types
pub type AppendRouteOutcome = DomainAppendRouteOutcome<ApiDefinitionData, ApiRouteData>;

impl RouteConfig {
    /// Convert domain RouteConfig to storage CreateApiRouteRequest
    fn into_request(self, definition_id: &str, route_order: i64) -> CreateApiRouteRequest {
        CreateApiRouteRequest {
            api_definition_id: definition_id.to_string(),
            match_type: self.match_type,
            match_value: self.match_value,
            case_sensitive: self.case_sensitive,
            rewrite_prefix: self.rewrite_prefix,
            rewrite_regex: self.rewrite_regex,
            rewrite_substitution: self.rewrite_substitution,
            upstream_targets: self.upstream_targets,
            timeout_seconds: self.timeout_seconds,
            override_config: self.override_config,
            deployment_note: self.deployment_note,
            route_order,
        }
    }
}

pub struct PlatformApiMaterializer {
    state: Arc<XdsState>,
    repository: ApiDefinitionRepository,
    audit_repo: AuditLogRepository,
}

impl PlatformApiMaterializer {
    pub fn new(state: Arc<XdsState>) -> Result<Self> {
        let repository = state
            .api_definition_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("API definition repository is not configured"))?;

        let audit_repo = AuditLogRepository::new(repository.pool());

        Ok(Self { state, repository, audit_repo })
    }

    pub async fn create_definition(
        &self,
        spec: ApiDefinitionSpec,
    ) -> Result<CreateDefinitionOutcome> {
        ensure_domain_available(&self.repository, &spec.team, &spec.domain).await?;

        // Validate target_listeners if provided
        if let Some(ref target_listeners) = spec.target_listeners {
            let listener_repo = self
                .state
                .listener_repository
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::internal("Listener repository is not configured"))?;

            ensure_target_listeners_exist(&listener_repo, target_listeners).await?;
        }

        // Pre-check isolation conflicts to fail fast
        if spec.listener_isolation {
            let listener_repo = self
                .state
                .listener_repository
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::internal("Listener repository is not configured"))?;
            let params = spec.isolation_listener.as_ref().ok_or_else(|| {
                Error::validation("listener parameters are required for isolation mode")
            })?;

            if let Some(name) = &params.name {
                if let Ok(existing) = listener_repo.get_by_name(name).await {
                    if existing.address != params.bind_address
                        || existing.port.unwrap_or_default() as u32 != params.port
                    {
                        return Err(Error::validation(
                            "listener.name reuse must match bindAddress and port",
                        ));
                    }
                }
            } else {
                let current = listener_repo.list(Some(1000), None).await?;
                if current.iter().any(|l| {
                    l.address == params.bind_address
                        && l.port.unwrap_or_default() as u32 == params.port
                }) {
                    return Err(Error::validation(
                        "Requested listener address:port is already in use",
                    ));
                }
            }
        }

        // Create definition and routes (non-transactional), but compensate on listener failure
        let definition = self
            .repository
            .create_definition(CreateApiDefinitionRequest {
                team: spec.team.clone(),
                domain: spec.domain.clone(),
                listener_isolation: spec.listener_isolation,
                target_listeners: spec.target_listeners.clone(),
                tls_config: spec.tls_config.clone(),
                metadata: None,
            })
            .await?;

        let mut created_routes = Vec::with_capacity(spec.routes.len());
        for (idx, route_spec) in spec.routes.iter().cloned().enumerate() {
            ensure_route_available(
                &self.repository,
                &definition.id,
                &route_spec.match_type,
                &route_spec.match_value,
            )
            .await?;
            let order = route_spec.route_order.unwrap_or(idx as i64);
            let created_route = self
                .repository
                .create_route(route_spec.into_request(&definition.id, order))
                .await?;
            created_routes.push(created_route);
        }

        // Compute bootstrap URI without writing files
        let bootstrap_uri = bootstrap::compute_bootstrap_uri(&definition.id);
        let definition = self
            .repository
            .update_bootstrap_metadata(crate::storage::UpdateBootstrapMetadataRequest {
                definition_id: definition.id.clone(),
                bootstrap_uri: Some(bootstrap_uri.clone()),
                bootstrap_revision: definition.bootstrap_revision + 1,
            })
            .await?;

        audit::record_create_event(
            &self.audit_repo,
            &definition.id,
            &definition.team,
            &definition.domain,
        )
        .await?;

        // IMPORTANT: Create clusters BEFORE listeners (clusters must exist for listener validation)
        // Materialize native resources (routes, clusters) with source='platform_api'
        let (mut generated_listener_id, generated_route_ids, generated_cluster_ids) = self
            .materialize_native_resources(
                &definition,
                &created_routes,
                spec.isolation_listener.as_ref(),
            )
            .await?;

        // Trigger xDS updates for clusters first
        tracing::info!("Triggering xDS updates for clusters (before listener creation)");
        self.state.refresh_clusters_from_repository().await?;

        // Now create the isolated listener after clusters exist, or merge routes into shared listeners
        if spec.listener_isolation {
            if let Err(err) = self
                .materialize_isolated_listener(
                    &definition,
                    &created_routes,
                    spec.isolation_listener.as_ref(),
                )
                .await
            {
                // Compensating delete to avoid partial writes
                let _ = self.repository.delete_definition(&definition.id).await;
                return Err(err);
            }

            // Retrieve the generated listener ID
            if let Some(listener_input) = spec.isolation_listener.as_ref() {
                let listener_repo =
                    self.state.listener_repository.as_ref().cloned().ok_or_else(|| {
                        Error::internal("Listener repository is not configured")
                    })?;

                let listener_name = listener_input.name.clone().unwrap_or_else(|| {
                    format!("platform-{}-listener", short_id(&definition.id))
                });

                let listener = listener_repo.get_by_name(&listener_name).await?;
                generated_listener_id = Some(listener.id);
            }
        } else {
            // listenerIsolation=false: merge routes into existing listeners
            if let Err(err) = self
                .materialize_shared_listener_routes(&definition, &created_routes, &spec.target_listeners)
                .await
            {
                // Compensating delete to avoid partial writes
                let _ = self.repository.delete_definition(&definition.id).await;
                return Err(err);
            }
        }

        // Store FK relationships in the database
        if let Some(listener_id) = &generated_listener_id {
            self.repository.update_generated_listener_id(&definition.id, Some(listener_id)).await?;
        }

        // Update each API route with its generated native resource IDs
        for (idx, api_route) in created_routes.iter().enumerate() {
            let route_id = generated_route_ids.get(idx).map(|s| s.as_str());
            let cluster_id = generated_cluster_ids.get(idx).map(|s| s.as_str());
            self.repository
                .update_generated_resource_ids(&api_route.id, route_id, cluster_id)
                .await?;
        }

        // Trigger xDS snapshot updates for newly created native resources
        // Order matters: clusters -> routes (native) -> routes (platform API) -> listeners
        // (clusters were already refreshed before listener creation above)

        tracing::info!("Triggering xDS updates for native routes");
        self.state.refresh_routes_from_repository().await?;

        // Send Platform API route configs BEFORE listeners (listeners reference routes via RDS)
        tracing::info!("Triggering xDS updates for Platform API route configs");
        self.state.refresh_platform_api_resources().await?;

        // Refresh listeners to trigger RDS update in Envoy
        // For isolated mode: creates new listener
        // For shared mode: updates existing listeners to reference updated route configs
        tracing::info!("Triggering xDS updates for listeners");
        self.state.refresh_listeners_from_repository().await?;

        tracing::info!("Platform API CREATE xDS updates complete");

        Ok(CreateDefinitionOutcome {
            definition,
            routes: created_routes,
            bootstrap_uri,
            generated_listener_id,
            generated_route_ids,
            generated_cluster_ids,
        })
    }

    /// Update an existing API definition and its native resources
    pub async fn update_definition(
        &self,
        definition_id: &str,
        updated_routes: Vec<RouteSpec>,
    ) -> Result<CreateDefinitionOutcome> {
        let definition = self.repository.get_definition(definition_id).await?;
        let existing_routes = self.repository.list_routes(definition_id).await?;

        // Get existing native resource IDs to clean up orphaned ones
        let existing_route_ids: Vec<String> =
            existing_routes.iter().filter_map(|r| r.generated_route_id.clone()).collect();
        let existing_cluster_ids: Vec<String> =
            existing_routes.iter().filter_map(|r| r.generated_cluster_id.clone()).collect();

        // Delete existing API routes (will be recreated)
        for existing_route in &existing_routes {
            self.repository.delete_route(&existing_route.id).await?;
        }

        // Create new API routes
        let mut created_routes = Vec::with_capacity(updated_routes.len());
        for (idx, route_spec) in updated_routes.iter().cloned().enumerate() {
            ensure_route_available(
                &self.repository,
                definition_id,
                &route_spec.match_type,
                &route_spec.match_value,
            )
            .await?;
            let order = route_spec.route_order.unwrap_or(idx as i64);
            let created_route =
                self.repository.create_route(route_spec.into_request(definition_id, order)).await?;
            created_routes.push(created_route);
        }

        // Clean up old native resources (clusters and routes)
        let cluster_repo = self
            .state
            .cluster_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Cluster repository not configured"))?;
        let route_repo = self
            .state
            .route_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Route repository not configured"))?;

        for cluster_id in &existing_cluster_ids {
            let _ = cluster_repo.delete(cluster_id).await; // Ignore errors if already deleted
        }
        for route_id in &existing_route_ids {
            let _ = route_repo.delete(route_id).await; // Ignore errors if already deleted
        }

        // Materialize new native resources
        let (generated_listener_id, generated_route_ids, generated_cluster_ids) =
            self.materialize_native_resources(&definition, &created_routes, None).await?;

        // Update FK relationships
        if let Some(listener_id) = &generated_listener_id {
            self.repository.update_generated_listener_id(&definition.id, Some(listener_id)).await?;
        }

        for (idx, api_route) in created_routes.iter().enumerate() {
            let route_id = generated_route_ids.get(idx).map(|s| s.as_str());
            let cluster_id = generated_cluster_ids.get(idx).map(|s| s.as_str());
            self.repository
                .update_generated_resource_ids(&api_route.id, route_id, cluster_id)
                .await?;
        }

        // Compute bootstrap URI without writing files
        let bootstrap_uri = bootstrap::compute_bootstrap_uri(&definition.id);
        let definition = self
            .repository
            .update_bootstrap_metadata(crate::storage::UpdateBootstrapMetadataRequest {
                definition_id: definition.id.clone(),
                bootstrap_uri: Some(bootstrap_uri.clone()),
                bootstrap_revision: definition.bootstrap_revision + 1,
            })
            .await?;

        // Trigger xDS snapshot updates for updated native resources
        if definition.listener_isolation {
            self.state.refresh_listeners_from_repository().await?;
        }
        self.state.refresh_clusters_from_repository().await?;
        self.state.refresh_routes_from_repository().await?;
        self.state.refresh_platform_api_resources().await?;

        Ok(CreateDefinitionOutcome {
            definition,
            routes: created_routes,
            bootstrap_uri,
            generated_listener_id,
            generated_route_ids,
            generated_cluster_ids,
        })
    }

    pub async fn append_route(
        &self,
        definition_id: &str,
        spec: RouteSpec,
    ) -> Result<AppendRouteOutcome> {
        let definition = self.repository.get_definition(definition_id).await?;
        let existing_routes = self.repository.list_routes(definition_id).await?;

        ensure_route_available(
            &self.repository,
            definition_id,
            &spec.match_type,
            &spec.match_value,
        )
        .await?;

        let order = spec.route_order.unwrap_or(existing_routes.len() as i64);

        let route = self.repository.create_route(spec.into_request(definition_id, order)).await?;

        let mut all_routes = existing_routes;
        all_routes.push(route.clone());

        // Compute bootstrap URI without writing files
        let bootstrap_uri = bootstrap::compute_bootstrap_uri(&definition.id);
        let definition = self
            .repository
            .update_bootstrap_metadata(crate::storage::UpdateBootstrapMetadataRequest {
                definition_id: definition.id.clone(),
                bootstrap_uri: Some(bootstrap_uri.clone()),
                bootstrap_revision: definition.bootstrap_revision + 1,
            })
            .await?;

        audit::record_route_appended_event(
            &self.audit_repo,
            &definition.id,
            &route.id,
            &route.match_type,
            &route.match_value,
        )
        .await?;

        // Materialize native resources for the appended route
        let (_, generated_route_ids, generated_cluster_ids) = self
            .materialize_native_resources(&definition, std::slice::from_ref(&route), None)
            .await?;

        // Update FK relationships for the new route
        if let (Some(route_id), Some(cluster_id)) =
            (generated_route_ids.first(), generated_cluster_ids.first())
        {
            self.repository
                .update_generated_resource_ids(&route.id, Some(route_id), Some(cluster_id))
                .await?;
        }

        // If using shared listener mode, merge the new route into shared listeners
        if !definition.listener_isolation {
            self.materialize_shared_listener_routes(
                &definition,
                &all_routes,
                &definition.target_listeners,
            )
            .await?;
        }

        // Trigger xDS snapshot updates for newly created native resources
        self.state.refresh_clusters_from_repository().await?;
        self.state.refresh_routes_from_repository().await?;

        // For shared listener mode, refresh listeners to trigger RDS update in Envoy
        // Routes are only sent to Envoy when referenced by a listener
        if !definition.listener_isolation {
            self.state.refresh_listeners_from_repository().await?;
        }

        self.state.refresh_platform_api_resources().await?;

        Ok(AppendRouteOutcome { definition, route, bootstrap_uri })
    }

    // Note: This method is no longer used - replaced with explicit xDS refresh calls
    // Kept for potential future use
    #[allow(dead_code)]
    async fn refresh_caches(&self) -> Result<()> {
        self.state.refresh_routes_from_repository().await?;
        self.state.refresh_platform_api_resources().await?;
        Ok(())
    }

    async fn materialize_isolated_listener(
        &self,
        definition: &ApiDefinitionData,
        routes: &[ApiRouteData],
        listener: Option<&ListenerInput>,
    ) -> Result<()> {
        use crate::platform_api::filter_overrides::typed_per_filter_config;
        use crate::storage::CreateListenerRequest;
        use crate::xds::listener::ListenerConfig as XListenerConfig;
        use crate::xds::route::{
            PathMatch, RouteActionConfig, RouteMatchConfig, RouteRule,
            VirtualHostConfig,
        };

        let listener_repo = self
            .state
            .listener_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Listener repository is not configured"))?;

        let params = listener.ok_or_else(|| {
            Error::validation("listener parameters are required for isolation mode")
        })?;

        // Build a dedicated route configuration
        let route_config_name = format!("platform-api-{}", short_id(&definition.id));
        tracing::info!(
            route_config_name = %route_config_name,
            definition_id = %definition.id,
            "materialize_isolated_listener: Generated route config name for isolated listener RDS reference"
        );
        let mut vhost = VirtualHostConfig {
            name: format!("{}-vhost", short_id(&definition.id)),
            domains: vec![definition.domain.clone()],
            routes: Vec::with_capacity(routes.len()),
            typed_per_filter_config: Default::default(),
        };

        for route in routes {
            let cluster_name = build_cluster_name(&definition.id, &route.id);
            let action = RouteActionConfig::Cluster {
                name: cluster_name,
                timeout: route.timeout_seconds.map(|v| v as u64),
                prefix_rewrite: route.rewrite_prefix.clone(),
                path_template_rewrite: route.rewrite_regex.clone(),
            };

            let path = match route.match_type.to_lowercase().as_str() {
                "prefix" => PathMatch::Prefix(route.match_value.clone()),
                "path" | "exact" => PathMatch::Exact(route.match_value.clone()),
                other => {
                    return Err(Error::validation(format!(
                        "Unsupported route match type '{}' for isolated listener",
                        other
                    )));
                }
            };

            vhost.routes.push(RouteRule {
                name: Some(format!("platform-api-{}", short_id(&route.id))),
                r#match: RouteMatchConfig { path, headers: None, query_parameters: None },
                action,
                typed_per_filter_config: typed_per_filter_config(&route.override_config)?,
            });
        }

        // Build listener config with RDS (Route Discovery Service)
        // The route config will be sent separately via refresh_platform_api_resources()
        let listener_name = params
            .name
            .clone()
            .unwrap_or_else(|| format!("platform-{}-listener", short_id(&definition.id)));

        let listener_config = XListenerConfig {
            name: listener_name.clone(),
            address: params.bind_address.clone(),
            port: params.port,
            filter_chains: vec![crate::xds::listener::FilterChainConfig {
                name: Some("default".to_string()),
                filters: vec![crate::xds::listener::FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: crate::xds::listener::FilterType::HttpConnectionManager {
                        route_config_name: Some(route_config_name.clone()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: Vec::new(),
                    },
                }],
                tls_context: None,
            }],
        };

        let mut listener_value = serde_json::to_value(&listener_config).map_err(|e| {
            Error::internal(format!("Failed to serialize isolated listener config: {}", e))
        })?;
        if let Some(obj) = listener_value.as_object_mut() {
            obj.insert(
                "flowplaneGateway".to_string(),
                serde_json::json!({ "team": definition.team }),
            );
        }

        if !listener_repo.exists_by_name(&listener_name).await.unwrap_or(false) {
            let created_listener = listener_repo
                .create(CreateListenerRequest {
                    name: listener_name,
                    address: params.bind_address.clone(),
                    port: Some(params.port as i64),
                    protocol: Some(params.protocol.clone()),
                    configuration: listener_value,
                })
                .await?;

            // Tag with source='platform_api'
            sqlx::query("UPDATE listeners SET source = 'platform_api' WHERE id = $1")
                .bind(&created_listener.id)
                .execute(listener_repo.pool())
                .await
                .map_err(|e| {
                    Error::internal(format!("Failed to tag listener with source: {}", e))
                })?;
        }

        Ok(())
    }

    /// Merge Platform API routes into existing shared listeners (listenerIsolation=false mode)
    async fn materialize_shared_listener_routes(
        &self,
        definition: &ApiDefinitionData,
        routes: &[ApiRouteData],
        target_listeners: &Option<Vec<String>>,
    ) -> Result<()> {
        use crate::openapi::defaults::DEFAULT_GATEWAY_LISTENER;
        use crate::platform_api::filter_overrides::typed_per_filter_config;
        use crate::storage::repository::UpdateRouteRequest;
        use crate::xds::route::{PathMatch, RouteActionConfig, RouteMatchConfig, RouteRule, VirtualHostConfig};

        let listener_repo = self
            .state
            .listener_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Listener repository is not configured"))?;

        let route_repo = self
            .state
            .route_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Route repository is not configured"))?;

        // Default to default-gateway-listener if no target_listeners specified
        let target_listener_names = target_listeners
            .as_ref()
            .cloned()
            .unwrap_or_else(|| vec![DEFAULT_GATEWAY_LISTENER.to_string()]);

        for listener_name in target_listener_names {
            // Get the listener to find its route config name
            let listener = match listener_repo.get_by_name(&listener_name).await {
                Ok(l) => l,
                Err(_) => {
                    tracing::warn!(
                        listener_name = %listener_name,
                        definition_id = %definition.id,
                        "Skipping route merge: target listener does not exist"
                    );
                    continue;
                }
            };

            // Parse listener configuration to get route config name
            let listener_config: crate::xds::listener::ListenerConfig =
                serde_json::from_str(&listener.configuration).map_err(|e| {
                    Error::internal(format!("Failed to parse listener configuration: {}", e))
                })?;

            // Extract route config name from the listener's HTTP connection manager filter
            let route_config_name = listener_config
                .filter_chains
                .first()
                .and_then(|fc| fc.filters.first())
                .and_then(|f| match &f.filter_type {
                    crate::xds::listener::FilterType::HttpConnectionManager {
                        route_config_name,
                        ..
                    } => route_config_name.clone(),
                    _ => None,
                })
                .ok_or_else(|| {
                    Error::internal(format!(
                        "Listener '{}' does not have an HTTP connection manager with RDS",
                        listener_name
                    ))
                })?;

            tracing::info!(
                listener_name = %listener_name,
                route_config_name = %route_config_name,
                definition_id = %definition.id,
                "Merging Platform API routes into shared listener route config"
            );

            // Get the existing route configuration
            let existing_route = route_repo.get_by_name(&route_config_name).await?;
            let mut route_config: crate::xds::route::RouteConfig =
                serde_json::from_str(&existing_route.configuration).map_err(|e| {
                    Error::internal(format!("Failed to parse route configuration: {}", e))
                })?;

            // Build a new virtual host for this Platform API definition
            let mut vhost = VirtualHostConfig {
                name: format!("platform-api-{}-vhost", short_id(&definition.id)),
                domains: vec![definition.domain.clone()],
                routes: Vec::with_capacity(routes.len()),
                typed_per_filter_config: Default::default(),
            };

            for route in routes {
                let cluster_name = build_cluster_name(&definition.id, &route.id);
                let action = RouteActionConfig::Cluster {
                    name: cluster_name,
                    timeout: route.timeout_seconds.map(|v| v as u64),
                    prefix_rewrite: route.rewrite_prefix.clone(),
                    path_template_rewrite: route.rewrite_regex.clone(),
                };

                let path = match route.match_type.to_lowercase().as_str() {
                    "prefix" => PathMatch::Prefix(route.match_value.clone()),
                    "path" | "exact" => PathMatch::Exact(route.match_value.clone()),
                    other => {
                        return Err(Error::validation(format!(
                            "Unsupported route match type '{}' for shared listener",
                            other
                        )));
                    }
                };

                vhost.routes.push(RouteRule {
                    name: Some(format!("platform-api-{}", short_id(&route.id))),
                    r#match: RouteMatchConfig { path, headers: None, query_parameters: None },
                    action,
                    typed_per_filter_config: typed_per_filter_config(&route.override_config)?,
                });
            }

            // Remove any existing virtual host for this Platform API definition
            // (This handles the case where append_route is called on an existing definition)
            let vhost_name_pattern = format!("platform-api-{}-vhost", short_id(&definition.id));
            route_config.virtual_hosts.retain(|vh| vh.name != vhost_name_pattern);

            // Add the new virtual host to the route configuration
            route_config.virtual_hosts.push(vhost);

            // Sort virtual hosts for deterministic ordering (prevents config flapping)
            route_config.virtual_hosts.sort_by(|a, b| a.name.cmp(&b.name));

            // Update the route configuration in the database
            let updated_config = serde_json::to_value(&route_config).map_err(|e| {
                Error::internal(format!("Failed to serialize updated route configuration: {}", e))
            })?;

            route_repo
                .update(
                    &existing_route.id,
                    UpdateRouteRequest {
                        path_prefix: None,
                        cluster_name: None,
                        configuration: Some(updated_config),
                    },
                )
                .await?;

            tracing::info!(
                route_config_name = %route_config_name,
                num_virtual_hosts = route_config.virtual_hosts.len(),
                "Updated route config with Platform API virtual host"
            );
        }

        Ok(())
    }

    /// Materialize native resources (listeners, routes, clusters) from Platform API definition
    /// Tags all resources with source='platform_api' and stores FK relationships
    async fn materialize_native_resources(
        &self,
        definition: &ApiDefinitionData,
        api_routes: &[ApiRouteData],
        _listener_spec: Option<&ListenerInput>,
    ) -> Result<(Option<String>, Vec<String>, Vec<String>)> {
        use crate::storage::repository::{CreateClusterRequest, CreateRouteRequest};

        let cluster_repo = self
            .state
            .cluster_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Cluster repository is not configured"))?;

        let route_repo = self
            .state
            .route_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Route repository is not configured"))?;

        let mut generated_cluster_ids = Vec::new();
        let mut generated_route_ids = Vec::new();

        // Create clusters for each API route's upstream targets
        for api_route in api_routes {
            let cluster_name = build_cluster_name(&definition.id, &api_route.id);

            // Convert upstream_targets to endpoints format for ClusterSpec compatibility
            let endpoints = if let Some(targets) = api_route.upstream_targets.get("targets").and_then(|t| t.as_array()) {
                targets
                    .iter()
                    .filter_map(|target| {
                        let endpoint_str = target.get("endpoint").and_then(|e| e.as_str())?;
                        // Parse "host:port" format
                        let parts: Vec<&str> = endpoint_str.split(':').collect();
                        if parts.len() == 2 {
                            let port = parts[1].parse::<u16>().ok()?;
                            Some(serde_json::json!({
                                "host": parts[0],
                                "port": port
                            }))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![]
            };

            // Build cluster configuration in ClusterSpec format
            let cluster_config = serde_json::json!({
                "endpoints": endpoints,
                "connect_timeout_seconds": api_route.timeout_seconds
            });

            let cluster = cluster_repo
                .create(CreateClusterRequest {
                    name: cluster_name.clone(),
                    service_name: cluster_name.clone(),
                    configuration: cluster_config,
                })
                .await?;

            // Tag with source='platform_api'
            sqlx::query("UPDATE clusters SET source = 'platform_api' WHERE id = $1")
                .bind(&cluster.id)
                .execute(cluster_repo.pool())
                .await
                .map_err(|e| {
                    Error::internal(format!("Failed to tag cluster with source: {}", e))
                })?;

            generated_cluster_ids.push(cluster.id.clone());

            // Create native route that references the cluster
            let route_name = format!("platform-api-{}", short_id(&api_route.id));

            // Build virtual host configuration
            let path_match = if api_route.match_type == "exact" {
                serde_json::json!({"Exact": &api_route.match_value})
            } else {
                serde_json::json!({"Prefix": &api_route.match_value})
            };

            let route_config = serde_json::json!({
                "name": route_name,
                "virtual_hosts": [{
                    "name": format!("{}-vhost", route_name),
                    "domains": [&definition.domain],
                    "routes": [{
                        "match": {
                            "path": path_match
                        },
                        "action": {
                            "Cluster": {
                                "name": &cluster_name,
                                "timeout": api_route.timeout_seconds
                            }
                        }
                    }]
                }]
            });

            let route = route_repo
                .create(CreateRouteRequest {
                    name: route_name,
                    path_prefix: api_route.match_value.clone(),
                    cluster_name,
                    configuration: route_config,
                })
                .await?;

            // Tag with source='platform_api'
            sqlx::query("UPDATE routes SET source = 'platform_api' WHERE id = $1")
                .bind(&route.id)
                .execute(route_repo.pool())
                .await
                .map_err(|e| Error::internal(format!("Failed to tag route with source: {}", e)))?;

            generated_route_ids.push(route.id);
        }

        // Listener ID will be retrieved after listener creation in create_definition
        // (listener is created AFTER clusters to ensure proper xDS ordering)
        Ok((None, generated_route_ids, generated_cluster_ids))
    }

    /// Delete a Platform API definition and clean up all associated resources
    pub async fn delete_definition(&self, definition_id: &str) -> Result<()> {
        use crate::openapi::defaults::DEFAULT_GATEWAY_LISTENER;
        use crate::storage::repository::UpdateRouteRequest;

        // Get the definition to determine its configuration
        let definition = self.repository.get_definition(definition_id).await?;
        let routes = self.repository.list_routes(definition_id).await?;

        // Get repository references
        let cluster_repo = self
            .state
            .cluster_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Cluster repository not configured"))?;
        let route_repo = self
            .state
            .route_repository
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::internal("Route repository not configured"))?;

        // If using shared listeners, remove Platform API virtual hosts from route configs
        if !definition.listener_isolation {
            let listener_repo = self
                .state
                .listener_repository
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::internal("Listener repository not configured"))?;

            let target_listener_names = definition
                .target_listeners
                .clone()
                .unwrap_or_else(|| vec![DEFAULT_GATEWAY_LISTENER.to_string()]);

            for listener_name in target_listener_names {
                // Get the listener to find its route config name
                let listener = match listener_repo.get_by_name(&listener_name).await {
                    Ok(l) => l,
                    Err(_) => {
                        tracing::warn!(
                            listener_name = %listener_name,
                            definition_id = %definition_id,
                            "Skipping route removal: target listener does not exist"
                        );
                        continue;
                    }
                };

                // Parse listener configuration to get route config name
                let listener_config: crate::xds::listener::ListenerConfig =
                    serde_json::from_str(&listener.configuration).map_err(|e| {
                        Error::internal(format!("Failed to parse listener configuration: {}", e))
                    })?;

                // Extract route config name from the listener's HTTP connection manager filter
                let route_config_name = listener_config
                    .filter_chains
                    .first()
                    .and_then(|fc| fc.filters.first())
                    .and_then(|f| match &f.filter_type {
                        crate::xds::listener::FilterType::HttpConnectionManager {
                            route_config_name,
                            ..
                        } => route_config_name.clone(),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        Error::internal(format!(
                            "Listener '{}' does not have an HTTP connection manager with RDS",
                            listener_name
                        ))
                    })?;

                tracing::info!(
                    listener_name = %listener_name,
                    route_config_name = %route_config_name,
                    definition_id = %definition_id,
                    "Removing Platform API virtual host from shared listener route config"
                );

                // Get the existing route configuration
                let existing_route = route_repo.get_by_name(&route_config_name).await?;
                let mut route_config: crate::xds::route::RouteConfig =
                    serde_json::from_str(&existing_route.configuration).map_err(|e| {
                        Error::internal(format!("Failed to parse route configuration: {}", e))
                    })?;

                // Remove the Platform API virtual host for this definition
                let vhost_name_to_remove = format!("platform-api-{}-vhost", short_id(&definition.id));
                route_config
                    .virtual_hosts
                    .retain(|vh| vh.name != vhost_name_to_remove);

                tracing::info!(
                    route_config_name = %route_config_name,
                    vhost_removed = %vhost_name_to_remove,
                    num_virtual_hosts_remaining = route_config.virtual_hosts.len(),
                    "Removed Platform API virtual host from route config"
                );

                // Update the route configuration in the database
                let updated_config = serde_json::to_value(&route_config).map_err(|e| {
                    Error::internal(format!("Failed to serialize updated route configuration: {}", e))
                })?;

                route_repo
                    .update(
                        &existing_route.id,
                        UpdateRouteRequest {
                            path_prefix: None,
                            cluster_name: None,
                            configuration: Some(updated_config),
                        },
                    )
                    .await?;
            }
        }

        // Delete generated native resources (clusters and routes)
        for route in &routes {
            if let Some(cluster_id) = &route.generated_cluster_id {
                if let Err(e) = cluster_repo.delete(cluster_id).await {
                    tracing::warn!(
                        cluster_id = %cluster_id,
                        error = %e,
                        "Failed to delete generated cluster (continuing deletion)"
                    );
                }
            }
            if let Some(route_id) = &route.generated_route_id {
                if let Err(e) = route_repo.delete(route_id).await {
                    tracing::warn!(
                        route_id = %route_id,
                        error = %e,
                        "Failed to delete generated route (continuing deletion)"
                    );
                }
            }
        }

        // Delete the isolated listener if one was created
        if let Some(listener_id) = &definition.generated_listener_id {
            let listener_repo = self
                .state
                .listener_repository
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::internal("Listener repository not configured"))?;

            if let Err(e) = listener_repo.delete(listener_id).await {
                tracing::warn!(
                    listener_id = %listener_id,
                    error = %e,
                    "Failed to delete generated listener (continuing deletion)"
                );
            }
        }

        // Delete API routes from database
        for route in &routes {
            self.repository.delete_route(&route.id).await?;
        }

        // Delete the API definition from database
        self.repository.delete_definition(definition_id).await?;

        // Trigger xDS updates
        tracing::info!("Triggering xDS updates after Platform API deletion");
        self.state.refresh_clusters_from_repository().await?;
        self.state.refresh_routes_from_repository().await?;
        self.state.refresh_platform_api_resources().await?;
        if definition.listener_isolation {
            self.state.refresh_listeners_from_repository().await?;
        }

        tracing::info!(
            definition_id = %definition_id,
            "Platform API definition and all associated resources deleted successfully"
        );

        Ok(())
    }
}

fn build_cluster_name(definition_id: &str, route_id: &str) -> String {
    format!("platform-{}-{}", short_id(definition_id), short_id(route_id))
}

fn short_id(id: &str) -> String {
    let candidate: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
    if candidate.is_empty() {
        "platform".into()
    } else {
        candidate.to_lowercase()
    }
}
