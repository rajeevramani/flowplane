use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::{
    storage::{ClusterRepository, ListenerRepository, RouteRepository},
    Result,
};
use envoy_types::pb::envoy::service::discovery::v3::{
    aggregated_discovery_service_server::AggregatedDiscoveryService, DeltaDiscoveryRequest,
    DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse, Resource,
};

use super::super::{
    resources::{self, BuiltResource},
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

        if let Some(repo) = &state.route_repository {
            spawn_route_watcher(state.clone(), repo.clone());
        }

        if let Some(repo) = &state.listener_repository {
            spawn_listener_watcher(state.clone(), repo.clone());
        }

        Self { state }
    }

    /// Create discovery response with database-backed resources
    async fn create_resource_response(
        &self,
        request: &DiscoveryRequest,
    ) -> Result<DiscoveryResponse> {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        let scope = scope_from_discovery(&request.node);
        let built = if request.type_url == "type.googleapis.com/envoy.config.listener.v3.Listener" {
            self.create_listener_resources_from_db_scoped(&scope).await?
        } else {
            self.build_resources(request.type_url.as_str()).await?
        };
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

    /// Create cluster resources from database
    async fn create_cluster_resources_from_db(&self) -> Result<Vec<BuiltResource>> {
        let built = if let Some(repo) = &self.state.cluster_repository {
            match repo.list(Some(100), None).await {
                Ok(cluster_data_list) => {
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

        // NOTE: Platform API clusters are NOT added here to avoid duplicates
        // They are already stored in the database by materialize_native_resources()
        // and loaded above via cluster_repository.list()

        Ok(built)
    }

    /// Create fallback cluster resources from config
    fn create_fallback_cluster_resources(&self) -> Result<Vec<BuiltResource>> {
        resources::clusters_from_config(&self.state.config)
    }

    async fn create_route_resources_from_db(&self) -> Result<Vec<BuiltResource>> {
        let mut built = if let Some(repo) = &self.state.route_repository {
            match repo.list(Some(100), None).await {
                Ok(route_data_list) => {
                    if route_data_list.is_empty() {
                        info!("No routes found in database, falling back to config-based routes");
                        self.create_fallback_route_resources()?
                    } else {
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

        // Merge Platform API virtual hosts into the default gateway routes for non-isolated APIs only
        if let Some(api_repo) = &self.state.api_definition_repository {
            use envoy_types::pb::envoy::config::route::v3::RouteConfiguration;
            use prost::Message;

            let definitions = api_repo.list_definitions().await?;
            let platform_routes = api_repo.list_all_routes().await?;

            if !definitions.is_empty() && !platform_routes.is_empty() {
                let mut default_index: Option<usize> = None;
                for (idx, res) in built.iter().enumerate() {
                    if res.name == crate::openapi::defaults::DEFAULT_GATEWAY_ROUTES {
                        default_index = Some(idx);
                        break;
                    }
                }

                if let Some(idx) = default_index {
                    let mut default_rc = {
                        let any = &built[idx].resource;
                        RouteConfiguration::decode(any.value.as_slice()).map_err(|e| {
                            crate::Error::internal(format!(
                                "Failed to decode default gateway RouteConfiguration: {}",
                                e
                            ))
                        })?
                    };

                    // Build a domain allowlist from non-isolated API definitions
                    let allowed_domains: std::collections::HashSet<String> = definitions
                        .iter()
                        .filter(|d| !d.listener_isolation)
                        .map(|d| d.domain.clone())
                        .collect();

                    let platform_resources =
                        resources::resources_from_api_definitions(definitions.clone(), platform_routes)?;

                    let mut isolated_route_configs = Vec::new();

                    for res in platform_resources.into_iter() {
                        if res.type_url() != resources::ROUTE_TYPE_URL {
                            continue;
                        }
                        let mut rc = RouteConfiguration::decode(res.resource.value.as_slice())
                            .map_err(|e| {
                                crate::Error::internal(format!(
                                    "Failed to decode Platform API RouteConfiguration: {}",
                                    e
                                ))
                            })?;

                        // Check if this route config belongs to an isolated listener
                        let is_isolated = definitions.iter().any(|d|
                            d.listener_isolation &&
                            rc.virtual_hosts.iter().any(|vh| vh.domains.contains(&d.domain))
                        );

                        if is_isolated {
                            // Keep isolated route configs separate - they'll be added to built list
                            let isolated_any = envoy_types::pb::google::protobuf::Any {
                                type_url: resources::ROUTE_TYPE_URL.to_string(),
                                value: rc.encode_to_vec(),
                            };
                            isolated_route_configs.push(resources::BuiltResource {
                                name: rc.name.clone(),
                                resource: isolated_any,
                            });
                        } else {
                            // Merge non-isolated routes into default gateway
                            rc.virtual_hosts
                                .retain(|vh| vh.domains.iter().any(|d| allowed_domains.contains(d)));
                            if !rc.virtual_hosts.is_empty() {
                                default_rc.virtual_hosts.extend(rc.virtual_hosts.into_iter());
                            }
                        }
                    }

                    // Add isolated route configs to the built list
                    built.extend(isolated_route_configs);

                    // Re-encode merged default gateway route config
                    let merged_any = envoy_types::pb::google::protobuf::Any {
                        type_url: resources::ROUTE_TYPE_URL.to_string(),
                        value: default_rc.encode_to_vec(),
                    };

                    built[idx] = resources::BuiltResource {
                        name: crate::openapi::defaults::DEFAULT_GATEWAY_ROUTES.to_string(),
                        resource: merged_any,
                    };
                } else {
                    warn!(
                        "DEFAULT_GATEWAY_ROUTES not found in repository-built routes; skipping Platform API merge"
                    );
                }
            }
        }

        Ok(built)
    }

    fn create_fallback_route_resources(&self) -> Result<Vec<BuiltResource>> {
        resources::routes_from_config(&self.state.config)
    }

    async fn create_listener_resources_from_db_scoped(
        &self,
        scope: &Scope,
    ) -> Result<Vec<BuiltResource>> {
        let built = if let Some(repo) = &self.state.listener_repository {
            match repo.list(Some(100), None).await {
                Ok(listener_data_list) => {
                    if listener_data_list.is_empty() {
                        info!(
                            "No listeners found in database, falling back to config-based listener"
                        );
                        self.create_fallback_listener_resources()?
                    } else {
                        let filtered: Vec<crate::storage::repository::ListenerData> = match scope {
                            Scope::All => listener_data_list,
                            Scope::Team { team, include_default } => {
                                let mut keep = Vec::new();
                                for entry in listener_data_list.into_iter() {
                                    if entry.name
                                        == crate::openapi::defaults::DEFAULT_GATEWAY_LISTENER
                                        && *include_default
                                    {
                                        keep.push(entry);
                                        continue;
                                    }
                                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(
                                        &entry.configuration,
                                    ) {
                                        if let Some(tag_team) = value
                                            .get("flowplaneGateway")
                                            .and_then(|v| v.get("team"))
                                            .and_then(|v| v.as_str())
                                        {
                                            if tag_team == team {
                                                keep.push(entry);
                                            }
                                        }
                                    }
                                }
                                keep
                            }
                            Scope::Allowlist { names } => listener_data_list
                                .into_iter()
                                .filter(|e| names.contains(&e.name))
                                .collect(),
                        };
                        info!(
                            phase = "ads_response",
                            listener_count = filtered.len(),
                            "Building listener resources from database for ADS response"
                        );
                        resources::listeners_from_database_entries(filtered, "ads_response")?
                    }
                }
                Err(e) => {
                    warn!("Failed to load listeners from database: {}, falling back to config", e);
                    self.create_fallback_listener_resources()?
                }
            }
        } else {
            info!("No database repository available, using config-based listener");
            self.create_fallback_listener_resources()?
        };

        // Intentionally do not emit Platform API listeners here to avoid port conflicts

        Ok(built)
    }

    fn create_fallback_listener_resources(&self) -> Result<Vec<BuiltResource>> {
        resources::listeners_from_config(&self.state.config)
    }

    async fn build_resources(&self, type_url: &str) -> Result<Vec<BuiltResource>> {
        match type_url {
            "type.googleapis.com/envoy.config.cluster.v3.Cluster" => {
                self.create_cluster_resources_from_db().await
            }
            "type.googleapis.com/envoy.config.route.v3.RouteConfiguration" => {
                self.create_route_resources_from_db().await
            }
            "type.googleapis.com/envoy.config.listener.v3.Listener" => {
                self.create_listener_resources_from_db_scoped(&Scope::All).await
            }
            "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment" => {
                resources::endpoints_from_config(&self.state.config)
            }
            _ => {
                warn!("Unknown resource type requested: {}", type_url);
                Ok(Vec::new())
            }
        }
    }

    async fn create_delta_response(
        &self,
        request: &DeltaDiscoveryRequest,
    ) -> Result<DeltaDiscoveryResponse> {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        // Build all available resources for this type
        // The stream logic will handle proper delta filtering and ACK detection
        let scope = scope_from_discovery(&request.node);
        let built = if request.type_url == "type.googleapis.com/envoy.config.listener.v3.Listener" {
            self.create_listener_resources_from_db_scoped(&scope).await?
        } else {
            self.build_resources(&request.type_url).await?
        };

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
        if let Err(error) = state.refresh_platform_api_resources().await {
            warn!(%error, "Failed to prime Platform API resources after cluster refresh");
        }

        let mut last_version: Option<i64> = None;

        loop {
            let poll_result = sqlx::query_scalar::<_, i64>("PRAGMA data_version;")
                .fetch_one(repository.pool())
                .await;

            match poll_result {
                Ok(version) => match last_version {
                    Some(previous) if previous == version => {}
                    Some(_) => {
                        last_version = Some(version);
                        if let Err(error) = state.refresh_clusters_from_repository().await {
                            warn!(%error, "Failed to refresh cluster cache from repository");
                        }
                        if let Err(error) = state.refresh_platform_api_resources().await {
                            warn!(%error, "Failed to refresh Platform API resources after cluster change");
                        }
                    }
                    None => {
                        last_version = Some(version);
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Failed to poll SQLite data_version for changes");
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    });
}

fn spawn_route_watcher(state: Arc<XdsState>, repository: RouteRepository) {
    tokio::spawn(async move {
        use tokio::time::{sleep, Duration};

        if let Err(error) = state.refresh_routes_from_repository().await {
            warn!(%error, "Failed to initialize route cache from repository");
        }
        if let Err(error) = state.refresh_platform_api_resources().await {
            warn!(%error, "Failed to prime Platform API resources after route refresh");
        }

        let mut last_version: Option<i64> = None;

        loop {
            let poll_result = sqlx::query_scalar::<_, i64>("PRAGMA data_version;")
                .fetch_one(repository.pool())
                .await;

            match poll_result {
                Ok(version) => match last_version {
                    Some(previous) if previous == version => {}
                    Some(_) => {
                        last_version = Some(version);
                        if let Err(error) = state.refresh_routes_from_repository().await {
                            warn!(%error, "Failed to refresh route cache from repository");
                        }
                        if let Err(error) = state.refresh_platform_api_resources().await {
                            warn!(%error, "Failed to refresh Platform API resources after route change");
                        }
                    }
                    None => {
                        last_version = Some(version);
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Failed to poll SQLite data_version for route changes");
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
        if let Err(error) = state.refresh_platform_api_resources().await {
            warn!(%error, "Failed to prime Platform API resources after listener refresh");
        }

        let mut last_version: Option<i64> = None;

        loop {
            let poll_result = sqlx::query_scalar::<_, i64>("PRAGMA data_version;")
                .fetch_one(repository.pool())
                .await;

            match poll_result {
                Ok(version) => match last_version {
                    Some(previous) if previous == version => {}
                    Some(_) => {
                        last_version = Some(version);
                        if let Err(error) = state.refresh_listeners_from_repository().await {
                            warn!(%error, "Failed to refresh listener cache from repository");
                        }
                        if let Err(error) = state.refresh_platform_api_resources().await {
                            warn!(%error, "Failed to refresh Platform API resources after listener change");
                        }
                    }
                    None => {
                        last_version = Some(version);
                    }
                },
                Err(e) => {
                    warn!(error = %e, "Failed to poll SQLite data_version for listener changes");
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

    async fn stream_aggregated_resources(
        &self,
        request: Request<tonic::Streaming<DiscoveryRequest>>,
    ) -> std::result::Result<Response<Self::StreamAggregatedResourcesStream>, Status> {
        info!("New database-enabled ADS stream connection established");

        let state = self.state.clone();
        let responder = move |state: Arc<XdsState>, request: DiscoveryRequest| {
            let service = DatabaseAggregatedDiscoveryService { state };
            Box::pin(async move { service.create_resource_response(&request).await })
                as Pin<Box<dyn Future<Output = Result<DiscoveryResponse>> + Send>>
        };

        let stream = crate::xds::services::stream::run_stream_loop(
            state,
            request.into_inner(),
            responder,
            "database-enabled",
        );

        Ok(Response::new(Box::pin(stream)))
    }

    async fn delta_aggregated_resources(
        &self,
        request: Request<tonic::Streaming<DeltaDiscoveryRequest>>,
    ) -> std::result::Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        info!("Delta ADS stream connection established (database-enabled)");

        let responder = move |state: Arc<XdsState>, request: DeltaDiscoveryRequest| {
            let service = DatabaseAggregatedDiscoveryService { state };
            Box::pin(async move { service.create_delta_response(&request).await })
                as Pin<Box<dyn Future<Output = Result<DeltaDiscoveryResponse>> + Send>>
        };

        let stream = crate::xds::services::stream::run_delta_loop(
            self.state.clone(),
            request.into_inner(),
            responder,
            "database-enabled",
        );

        Ok(Response::new(Box::pin(stream)))
    }
}
#[derive(Debug, Clone)]
enum Scope {
    All,
    Team { team: String, include_default: bool },
    Allowlist { names: Vec<String> },
}

fn scope_from_discovery(node: &Option<envoy_types::pb::envoy::config::core::v3::Node>) -> Scope {
    if let Some(n) = node {
        if let Some(meta) = &n.metadata {
            let mut team: Option<String> = None;
            let mut include_default = false;
            let mut allow: Vec<String> = Vec::new();

            if let Some(envoy_types::pb::google::protobuf::value::Kind::StringValue(s)) =
                meta.fields.get("team").and_then(|v| v.kind.as_ref())
            {
                if !s.is_empty() {
                    team = Some(s.clone());
                }
            }
            if let Some(envoy_types::pb::google::protobuf::value::Kind::BoolValue(b)) =
                meta.fields.get("include_default").and_then(|v| v.kind.as_ref())
            {
                include_default = *b;
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
                return Scope::Team { team, include_default };
            }
        }
    }
    Scope::All
}
