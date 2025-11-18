use std::sync::Arc;

use sqlx;
use tracing::info;

use super::{
    audit, bootstrap,
    validation::{ensure_domain_available, ensure_route_available},
};
use crate::domain::api_definition::{
    ApiDefinitionSpec as DomainApiDefinitionSpec, AppendRouteOutcome as DomainAppendRouteOutcome,
    CreateDefinitionOutcome as DomainCreateDefinitionOutcome, ListenerConfig, RouteConfig,
};
use crate::errors::{Error, Result};
use crate::observability::http_tracing::create_operation_span;
use crate::storage::{
    ApiDefinitionData, ApiDefinitionRepository, ApiRouteData, AuditLogRepository,
    CreateApiDefinitionRequest, CreateApiRouteRequest,
};
use crate::xds::XdsState;
use opentelemetry::{
    trace::{Span, SpanKind},
    KeyValue,
};

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
        // Convert headers from Vec<HeaderMatchConfig> to JSON
        let headers = self.headers.and_then(|h| serde_json::to_value(h).ok());

        CreateApiRouteRequest {
            api_definition_id: definition_id.to_string(),
            match_type: self.match_type,
            match_value: self.match_value,
            case_sensitive: self.case_sensitive,
            headers,
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
        use opentelemetry::trace::{FutureExt, TraceContextExt};

        let mut span = create_operation_span("platform_api.create_definition", SpanKind::Internal);
        span.set_attribute(KeyValue::new("team", spec.team.clone()));
        span.set_attribute(KeyValue::new("domain", spec.domain.clone()));

        let cx = opentelemetry::Context::current().with_span(span);

        async move {
            ensure_domain_available(&self.repository, &spec.team, &spec.domain).await?;

            // Pre-check listener conflicts to fail fast
            let listener_repo = self
                .state
                .listener_repository
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::internal("Listener repository is not configured"))?;

            if let Some(name) = &spec.listener.name {
                if let Ok(existing) = listener_repo.get_by_name(name).await {
                    if existing.address != spec.listener.bind_address
                        || existing.port.unwrap_or_default() as u32 != spec.listener.port
                    {
                        return Err(Error::validation(
                            "listener.name reuse must match bindAddress and port",
                        ));
                    }
                }
            } else {
                let current = listener_repo.list(Some(1000), None).await?;
                if current.iter().any(|l| {
                    l.address == spec.listener.bind_address
                        && l.port.unwrap_or_default() as u32 == spec.listener.port
                }) {
                    return Err(Error::validation(
                        "Requested listener address:port is already in use",
                    ));
                }
            }

            // Create definition and routes (non-transactional), but compensate on listener failure
            let mut db_span = create_operation_span("db.api_definition.insert", SpanKind::Client);
            db_span.set_attribute(KeyValue::new("db.operation", "INSERT"));
            db_span.set_attribute(KeyValue::new("db.table", "api_definitions"));
            let definition = self
                .repository
                .create_definition(CreateApiDefinitionRequest {
                    team: spec.team.clone(),
                    domain: spec.domain.clone(),
                    tls_config: spec.tls_config.clone(),
                    metadata: None,
                })
                .await?;
            drop(db_span);

            let mut created_routes = Vec::with_capacity(spec.routes.len());
            for (idx, route_spec) in spec.routes.iter().cloned().enumerate() {
                let headers_json =
                    route_spec.headers.as_ref().and_then(|h| serde_json::to_value(h).ok());
                ensure_route_available(
                    &self.repository,
                    definition.id.as_str(),
                    &route_spec.match_type,
                    &route_spec.match_value,
                    headers_json.as_ref(),
                )
                .await?;
                let order = route_spec.route_order.unwrap_or(idx as i64);
                let created_route = self
                    .repository
                    .create_route(route_spec.into_request(definition.id.as_str(), order))
                    .await?;
                created_routes.push(created_route);
            }

            // Compute bootstrap URI without writing files (team-scoped)
            let bootstrap_uri = bootstrap::compute_bootstrap_uri(&definition.team);
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
                definition.id.as_str(),
                &definition.team,
                &definition.domain,
            )
            .await?;

            // IMPORTANT: Create clusters BEFORE listeners (clusters must exist for listener validation)
            // Materialize native resources (routes, clusters) with source='platform_api'
            let (_, generated_route_ids, generated_cluster_ids) = self
                .materialize_native_resources(
                    &definition,
                    &created_routes,
                    Some(&spec.listener),
                )
                .await?;

            // Trigger xDS updates for clusters first
            info!("Triggering xDS updates for clusters (before listener creation)");
            let xds_span = create_operation_span("xds.refresh_clusters", SpanKind::Internal);
            self.state.refresh_clusters_from_repository().await?;
            drop(xds_span);

            // Now create the isolated listener after clusters exist, or merge routes into shared listeners
            // Materialize the listener for this API definition
            if let Err(err) = self
                .materialize_isolated_listener(
                    &definition,
                    &created_routes,
                    Some(&spec.listener),
                )
                .await
            {
                // Compensating delete to avoid partial writes
                let _ = self.repository.delete_definition(&definition.id).await;
                return Err(err);
            }

            // Retrieve the generated listener ID
            let listener_repo = self
                .state
                .listener_repository
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::internal("Listener repository is not configured"))?;

            let listener_name = spec.listener.name.clone().unwrap_or_else(|| {
                format!("platform-{}-listener", short_id(definition.id.as_str()))
            });

            let listener = listener_repo.get_by_name(&listener_name).await?;
            let generated_listener_id = Some(listener.id.to_string());

            // Store FK relationships in the database
            if let Some(listener_id) = &generated_listener_id {
                self.repository
                    .update_generated_listener_id(&definition.id, Some(listener_id))
                    .await?;
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

            info!("Triggering xDS updates for native routes");
            let xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
            self.state.refresh_routes_from_repository().await?;
            drop(xds_span);

            // Send Platform API route configs BEFORE listeners (listeners reference routes via RDS)
            info!("Triggering xDS updates for Platform API route configs");
            let xds_span =
                create_operation_span("xds.refresh_platform_api_resources", SpanKind::Internal);
            self.state.refresh_platform_api_resources().await?;
            drop(xds_span);

            // Refresh listeners to trigger RDS update in Envoy
            // For isolated mode: creates new listener
            // For shared mode: updates existing listeners to reference updated route configs
            info!("Triggering xDS updates for listeners");
            let xds_span = create_operation_span("xds.refresh_listeners", SpanKind::Internal);
            self.state.refresh_listeners_from_repository().await?;
            drop(xds_span);

            info!("Platform API CREATE xDS updates complete");

            Ok(CreateDefinitionOutcome {
                definition,
                routes: created_routes,
                bootstrap_uri,
                generated_listener_id,
                generated_route_ids,
                generated_cluster_ids,
            })
        }
        .with_context(cx)
        .await
    }

    /// Update an existing API definition and its native resources
    pub async fn update_definition(
        &self,
        definition_id: &str,
        updated_routes: Vec<RouteSpec>,
    ) -> Result<CreateDefinitionOutcome> {
        let definition = self
            .repository
            .get_definition(&crate::domain::ApiDefinitionId::from_str_unchecked(definition_id))
            .await?;
        let existing_routes = self
            .repository
            .list_routes(&crate::domain::ApiDefinitionId::from_str_unchecked(definition_id))
            .await?;

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
            let headers_json =
                route_spec.headers.as_ref().and_then(|h| serde_json::to_value(h).ok());
            ensure_route_available(
                &self.repository,
                definition_id,
                &route_spec.match_type,
                &route_spec.match_value,
                headers_json.as_ref(),
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
            let cluster_id_typed = crate::domain::ClusterId::from_string(cluster_id.clone());
            let _ = cluster_repo.delete(&cluster_id_typed).await; // Ignore errors if already deleted
        }
        for route_id in &existing_route_ids {
            let route_id_typed = crate::domain::RouteId::from_string(route_id.clone());
            let _ = route_repo.delete(&route_id_typed).await; // Ignore errors if already deleted
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

        // Compute bootstrap URI without writing files (team-scoped)
        let bootstrap_uri = bootstrap::compute_bootstrap_uri(&definition.team);
        let definition = self
            .repository
            .update_bootstrap_metadata(crate::storage::UpdateBootstrapMetadataRequest {
                definition_id: definition.id.clone(),
                bootstrap_uri: Some(bootstrap_uri.clone()),
                bootstrap_revision: definition.bootstrap_revision + 1,
            })
            .await?;

        // Update listener virtual host configurations with new domain/routes
        // This is critical - without this, domain changes won't propagate to Envoy!
        tracing::info!("Route config will be updated via xDS refresh");

        // Trigger xDS snapshot updates for updated native resources
        // Order matters: clusters -> routes -> listeners (to avoid NACK errors)
        let xds_span = create_operation_span("xds.refresh_clusters", SpanKind::Internal);
        self.state.refresh_clusters_from_repository().await?;
        drop(xds_span);

        let xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
        self.state.refresh_routes_from_repository().await?;
        drop(xds_span);

        // Refresh platform API resources and listeners
        let xds_span =
            create_operation_span("xds.refresh_platform_api_resources", SpanKind::Internal);
        self.state.refresh_platform_api_resources().await?;
        drop(xds_span);

        let xds_span = create_operation_span("xds.refresh_listeners", SpanKind::Internal);
        self.state.refresh_listeners_from_repository().await?;
        drop(xds_span);

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
        let definition = self
            .repository
            .get_definition(&crate::domain::ApiDefinitionId::from_str_unchecked(definition_id))
            .await?;
        let existing_routes = self
            .repository
            .list_routes(&crate::domain::ApiDefinitionId::from_str_unchecked(definition_id))
            .await?;

        let headers_json = spec.headers.as_ref().and_then(|h| serde_json::to_value(h).ok());
        ensure_route_available(
            &self.repository,
            definition_id,
            &spec.match_type,
            &spec.match_value,
            headers_json.as_ref(),
        )
        .await?;

        let order = spec.route_order.unwrap_or(existing_routes.len() as i64);

        let route = self.repository.create_route(spec.into_request(definition_id, order)).await?;

        let mut all_routes = existing_routes;
        all_routes.push(route.clone());

        // Compute bootstrap URI without writing files (team-scoped)
        let bootstrap_uri = bootstrap::compute_bootstrap_uri(&definition.team);
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
            definition.id.as_str(),
            route.id.as_str(),
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

        // Trigger xDS snapshot updates for newly created native resources
        let xds_span = create_operation_span("xds.refresh_clusters", SpanKind::Internal);
        self.state.refresh_clusters_from_repository().await?;
        drop(xds_span);

        let xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
        self.state.refresh_routes_from_repository().await?;
        drop(xds_span);

        // Refresh listeners to trigger RDS update in Envoy
        // Routes are only sent to Envoy when referenced by a listener
        let xds_span = create_operation_span("xds.refresh_listeners", SpanKind::Internal);
        self.state.refresh_listeners_from_repository().await?;
        drop(xds_span);

        let xds_span =
            create_operation_span("xds.refresh_platform_api_resources", SpanKind::Internal);
        self.state.refresh_platform_api_resources().await?;
        drop(xds_span);

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
            PathMatch, RouteActionConfig, RouteMatchConfig, RouteRule, VirtualHostConfig,
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
        let route_config_name = format!("platform-api-{}", short_id(definition.id.as_str()));
        tracing::info!(
            route_config_name = %route_config_name,
            definition_id = %definition.id,
            "materialize_isolated_listener: Generated route config name for isolated listener RDS reference"
        );
        let mut vhost = VirtualHostConfig {
            name: format!("{}-vhost", short_id(definition.id.as_str())),
            domains: vec![definition.domain.clone()],
            routes: Vec::with_capacity(routes.len()),
            typed_per_filter_config: Default::default(),
        };

        for route in routes {
            let cluster_name = build_cluster_name(definition.id.as_str(), route.id.as_str());
            let action = RouteActionConfig::Cluster {
                name: cluster_name,
                timeout: route.timeout_seconds.map(|v| v as u64),
                prefix_rewrite: route.rewrite_prefix.clone(),
                path_template_rewrite: route.rewrite_regex.clone(),
            };

            let path = match route.match_type.to_lowercase().as_str() {
                "prefix" => PathMatch::Prefix(route.match_value.clone()),
                "path" | "exact" => PathMatch::Exact(route.match_value.clone()),
                "template" => PathMatch::Template(route.match_value.clone()),
                other => {
                    return Err(Error::validation(format!(
                        "Unsupported route match type '{}' for isolated listener",
                        other
                    )));
                }
            };

            // Deserialize headers from JSON storage to HeaderMatchConfig
            let headers =
                route.headers.as_ref().and_then(|h| serde_json::from_value(h.clone()).ok());

            vhost.routes.push(RouteRule {
                name: Some(format!("platform-api-{}", short_id(route.id.as_str()))),
                r#match: RouteMatchConfig { path, headers, query_parameters: None },
                action,
                typed_per_filter_config: typed_per_filter_config(&route.override_config)?,
            });
        }

        // Build listener config with RDS (Route Discovery Service)
        // The route config will be sent separately via refresh_platform_api_resources()
        let listener_name = params
            .name
            .clone()
            .unwrap_or_else(|| format!("platform-{}-listener", short_id(definition.id.as_str())));

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
                        http_filters: listener
                            .and_then(|l| l.http_filters.clone())
                            .unwrap_or_default(),
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
                    team: Some(definition.team.clone()),
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


    /// Materialize native resources (listeners, routes, clusters) from Platform API definition
    /// Tags all resources with source='platform_api' and stores FK relationships
    ///
    /// # Route Creation Strategy
    ///
    /// - **Isolated listeners** (listenerIsolation=true): Creates only clusters, NO native routes.
    ///   Route configurations are built dynamically by `resources_from_api_definitions` in xds/resources.rs
    /// - **Shared listeners** (listenerIsolation=false): Creates both clusters and native routes.
    ///   Native routes are merged into shared listener route configs via `materialize_shared_listener_routes`
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

        // Step 1: Deduplicate upstream targets across all routes
        // Extract unique upstream configurations (keyed by endpoint)
        use std::collections::HashMap;
        let mut unique_upstreams: HashMap<String, (serde_json::Value, Option<i64>)> =
            HashMap::new();

        for api_route in api_routes {
            // Extract endpoint string as deduplication key
            if let Some(targets) =
                api_route.upstream_targets.get("targets").and_then(|t| t.as_array())
            {
                for target in targets {
                    if let Some(endpoint_str) = target.get("endpoint").and_then(|e| e.as_str()) {
                        // Store the full upstream config and timeout for this endpoint
                        unique_upstreams
                            .entry(endpoint_str.to_string())
                            .or_insert((target.clone(), api_route.timeout_seconds));
                    }
                }
            }
        }

        // Step 2: Create ONE cluster per unique upstream endpoint
        let mut endpoint_to_cluster: HashMap<String, (String, String)> = HashMap::new();

        for (endpoint_str, (_target_config, timeout)) in unique_upstreams {
            // Parse "host:port" format for cluster configuration
            let parts: Vec<&str> = endpoint_str.split(':').collect();
            if parts.len() != 2 {
                continue; // Skip invalid endpoint formats
            }

            let port = match parts[1].parse::<u16>() {
                Ok(p) => p,
                Err(_) => continue, // Skip invalid ports
            };

            let endpoints = vec![serde_json::json!({
                "host": parts[0],
                "port": port
            })];

            // Build cluster name based on upstream endpoint instead of route ID
            // This ensures cluster name is deterministic and tied to the upstream
            let cluster_name = format!(
                "platform-{}-{}",
                short_id(definition.id.as_str()),
                endpoint_str.replace(":", "-").replace(".", "-")
            );

            // Build cluster configuration
            let cluster_config = serde_json::json!({
                "endpoints": endpoints,
                "connect_timeout_seconds": timeout
            });

            // Check if cluster already exists (for append_route operations)
            let cluster =
                if let Ok(existing_cluster) = cluster_repo.get_by_name(&cluster_name).await {
                    // Reuse existing cluster
                    info!(
                        endpoint = %endpoint_str,
                        cluster_name = %cluster_name,
                        cluster_id = %existing_cluster.id,
                        "Reusing existing cluster for upstream endpoint"
                    );
                    existing_cluster
                } else {
                    // Create new cluster
                    let new_cluster = cluster_repo
                        .create(CreateClusterRequest {
                            name: cluster_name.clone(),
                            service_name: cluster_name.clone(),
                            configuration: cluster_config,
                            team: Some(definition.team.clone()),
                        })
                        .await?;

                    // Tag with source='platform_api'
                    sqlx::query("UPDATE clusters SET source = 'platform_api' WHERE id = $1")
                        .bind(&new_cluster.id)
                        .execute(cluster_repo.pool())
                        .await
                        .map_err(|e| {
                            Error::internal(format!("Failed to tag cluster with source: {}", e))
                        })?;

                    info!(
                        endpoint = %endpoint_str,
                        cluster_name = %new_cluster.name,
                        cluster_id = %new_cluster.id,
                        "Created new cluster for upstream endpoint"
                    );
                    new_cluster
                };

            // Store mapping: endpoint -> (cluster_name, cluster_id)
            endpoint_to_cluster
                .insert(endpoint_str.clone(), (cluster_name, cluster.id.to_string()));
        }

        // Step 3: Populate cluster IDs for FK relationships
        // Note: Native routes are not created here - they are built dynamically
        // by resources_from_api_definitions() in xds/resources.rs and sent via refresh_platform_api_resources()
        for api_route in api_routes {
            let cluster_info = if let Some(targets) =
                api_route.upstream_targets.get("targets").and_then(|t| t.as_array())
            {
                targets
                    .iter()
                    .find_map(|target| {
                        let endpoint_str = target.get("endpoint").and_then(|e| e.as_str())?;
                        endpoint_to_cluster.get(endpoint_str)
                    })
                    .cloned()
            } else {
                None
            };

            let (_cluster_name, cluster_id) = cluster_info.ok_or_else(|| {
                Error::internal(format!("No cluster found for route {}", api_route.id))
            })?;

            // Track cluster ID for FK relationship (no route ID since routes are dynamic)
            generated_cluster_ids.push(cluster_id.clone());

            info!(
                api_route_id = %api_route.id,
                native_cluster_id = %cluster_id,
                "Cluster created, native route skipped (route config via xDS)"
            );
        }

        let unique_clusters = endpoint_to_cluster.len();
        info!(
            total_routes = generated_route_ids.len(),
            unique_clusters = unique_clusters,
            total_cluster_references = generated_cluster_ids.len(),
            "Completed native resource materialization with cluster deduplication"
        );

        // Listener ID will be retrieved after listener creation in create_definition
        // (listener is created AFTER clusters to ensure proper xDS ordering)
        Ok((None, generated_route_ids, generated_cluster_ids))
    }

    /// Delete a Platform API definition and clean up all associated resources
    pub async fn delete_definition(&self, definition_id: &str) -> Result<()> {
        use crate::openapi::defaults::DEFAULT_GATEWAY_LISTENER;
        use crate::storage::repository::UpdateRouteRequest;

        // Get the definition to determine its configuration
        let definition = self
            .repository
            .get_definition(&crate::domain::ApiDefinitionId::from_str_unchecked(definition_id))
            .await?;
        let routes = self
            .repository
            .list_routes(&crate::domain::ApiDefinitionId::from_str_unchecked(definition_id))
            .await?;

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

        // Delete generated native resources (clusters and routes)
        for route in &routes {
            if let Some(cluster_id) = &route.generated_cluster_id {
                let cluster_id_typed = crate::domain::ClusterId::from_string(cluster_id.clone());
                if let Err(e) = cluster_repo.delete(&cluster_id_typed).await {
                    tracing::warn!(
                        cluster_id = %cluster_id,
                        error = %e,
                        "Failed to delete generated cluster (continuing deletion)"
                    );
                }
            }
            if let Some(route_id) = &route.generated_route_id {
                let route_id_typed = crate::domain::RouteId::from_string(route_id.clone());
                if let Err(e) = route_repo.delete(&route_id_typed).await {
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

            let listener_id_typed = crate::domain::ListenerId::from_string(listener_id.clone());
            if let Err(e) = listener_repo.delete(&listener_id_typed).await {
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
        self.repository
            .delete_definition(&crate::domain::ApiDefinitionId::from_str_unchecked(definition_id))
            .await?;

        // Trigger xDS updates
        info!("Triggering xDS updates after Platform API deletion");
        let xds_span = create_operation_span("xds.refresh_clusters", SpanKind::Internal);
        self.state.refresh_clusters_from_repository().await?;
        drop(xds_span);

        let xds_span = create_operation_span("xds.refresh_routes", SpanKind::Internal);
        self.state.refresh_routes_from_repository().await?;
        drop(xds_span);

        let xds_span =
            create_operation_span("xds.refresh_platform_api_resources", SpanKind::Internal);
        self.state.refresh_platform_api_resources().await?;
        drop(xds_span);

        let xds_span = create_operation_span("xds.refresh_listeners", SpanKind::Internal);
        self.state.refresh_listeners_from_repository().await?;
        drop(xds_span);

        info!(
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
