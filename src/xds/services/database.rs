use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::{storage::ClusterRepository, Result};
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
            spawn_database_watcher(state.clone(), repo.clone());
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

        let built = self.build_resources(request.type_url.as_str()).await?;
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
        if let Some(repo) = &self.state.cluster_repository {
            // Try to get clusters from database
            match repo.list(Some(100), None).await {
                Ok(cluster_data_list) => {
                    if cluster_data_list.is_empty() {
                        info!(
                            "No clusters found in database, falling back to config-based cluster"
                        );
                        return self.create_fallback_cluster_resources();
                    }

                    info!(
                        phase = "ads_response",
                        cluster_count = cluster_data_list.len(),
                        "Building cluster resources from database for ADS response"
                    );

                    resources::clusters_from_database_entries(cluster_data_list, "ads_response")
                }
                Err(e) => {
                    warn!(
                        "Failed to load clusters from database: {}, falling back to config",
                        e
                    );
                    self.create_fallback_cluster_resources()
                }
            }
        } else {
            info!("No database repository available, using config-based cluster");
            self.create_fallback_cluster_resources()
        }
    }

    /// Create fallback cluster resources from config
    fn create_fallback_cluster_resources(&self) -> Result<Vec<BuiltResource>> {
        resources::clusters_from_config(&self.state.config)
    }

    async fn build_resources(&self, type_url: &str) -> Result<Vec<BuiltResource>> {
        match type_url {
            "type.googleapis.com/envoy.config.cluster.v3.Cluster" => {
                self.create_cluster_resources_from_db().await
            }
            "type.googleapis.com/envoy.config.route.v3.RouteConfiguration" => {
                resources::routes_from_config(&self.state.config)
            }
            "type.googleapis.com/envoy.config.listener.v3.Listener" => {
                resources::listeners_from_config(&self.state.config)
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
        let built = self.build_resources(&request.type_url).await?;

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

fn spawn_database_watcher(state: Arc<XdsState>, repository: ClusterRepository) {
    tokio::spawn(async move {
        use tokio::time::{sleep, Duration};

        if let Err(error) = state.refresh_clusters_from_repository().await {
            warn!(%error, "Failed to initialize cluster cache from repository");
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
