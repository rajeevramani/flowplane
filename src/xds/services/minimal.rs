use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use envoy_types::pb::envoy::service::discovery::v3::{
    aggregated_discovery_service_server::AggregatedDiscoveryService, DeltaDiscoveryRequest,
    DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};

use crate::Result;

use super::super::{resources, XdsState};

/// Minimal implementation for config-only scenarios
#[derive(Debug)]
pub struct MinimalAggregatedDiscoveryService {
    pub(super) state: Arc<XdsState>,
}

impl MinimalAggregatedDiscoveryService {
    pub fn new(state: Arc<XdsState>) -> Self {
        Self { state }
    }

    /// Create discovery response with actual Envoy resources based on request type (legacy)
    fn create_resource_response(&self, request: &DiscoveryRequest) -> Result<DiscoveryResponse> {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        let resources = match request.type_url.as_str() {
            "type.googleapis.com/envoy.config.cluster.v3.Cluster" => {
                resources::clusters_from_config(&self.state.config)?
            }
            "type.googleapis.com/envoy.config.route.v3.RouteConfiguration" => {
                resources::routes_from_config(&self.state.config)?
            }
            "type.googleapis.com/envoy.config.listener.v3.Listener" => {
                resources::listeners_from_config(&self.state.config)?
            }
            "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment" => {
                resources::endpoints_from_config(&self.state.config)?
            }
            _ => {
                warn!("Unknown resource type requested: {}", request.type_url);
                Vec::new() // Return empty for unknown types
            }
        };

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
}

#[tonic::async_trait]
impl AggregatedDiscoveryService for MinimalAggregatedDiscoveryService {
    type StreamAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = std::result::Result<DiscoveryResponse, Status>> + Send>>;
    type DeltaAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = std::result::Result<DeltaDiscoveryResponse, Status>> + Send>>;

    async fn stream_aggregated_resources(
        &self,
        request: Request<tonic::Streaming<DiscoveryRequest>>,
    ) -> std::result::Result<Response<Self::StreamAggregatedResourcesStream>, Status> {
        info!("New ADS stream connection established");

        let mut in_stream = request.into_inner();
        let (tx, rx) = mpsc::channel(100);
        let state = self.state.clone();

        // Spawn task to handle the bidirectional stream
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Handle incoming requests from Envoy
                    result = in_stream.next() => {
                        match result {
                            Some(Ok(discovery_request)) => {
                                info!(
                                    type_url = %discovery_request.type_url,
                                    version_info = %discovery_request.version_info,
                                    node_id = ?discovery_request.node.as_ref().map(|n| &n.id),
                                    "Received discovery request"
                                );

                                // Create response with actual Envoy resources
                                let service = MinimalAggregatedDiscoveryService { state: state.clone() };
                                let response = match service.create_resource_response(&discovery_request) {
                                    Ok(resp) => resp,
                                    Err(e) => {
                                        error!("Failed to create resource response: {}", e);
                                        continue;
                                    }
                                };

                                info!(
                                    type_url = %response.type_url,
                                    version = %response.version_info,
                                    nonce = %response.nonce,
                                    resource_count = response.resources.len(),
                                    "Sending discovery response with empty resources"
                                );

                                if let Err(e) = tx.send(Ok(response)).await {
                                    error!("Failed to send discovery response: {}", e);
                                    break;
                                }
                            }
                            Some(Err(e)) => {
                                warn!("Error receiving discovery request: {}", e);
                                let _ = tx.send(Err(e)).await;
                                break;
                            }
                            None => {
                                info!("ADS stream ended by client");
                                break;
                            }
                        }
                    }

                    // Handle graceful shutdown
                    _ = tokio::signal::ctrl_c() => {
                        info!("Shutting down ADS stream");
                        break;
                    }
                }
            }
        });

        let out_stream = ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(out_stream) as Self::StreamAggregatedResourcesStream
        ))
    }

    async fn delta_aggregated_resources(
        &self,
        _request: Request<tonic::Streaming<DeltaDiscoveryRequest>>,
    ) -> std::result::Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        info!("Delta ADS stream connection established");

        // For minimal implementation, just return empty stream
        let (_tx, rx) = mpsc::channel(1);
        let out_stream = ReceiverStream::new(rx);

        Ok(Response::new(
            Box::pin(out_stream) as Self::DeltaAggregatedResourcesStream
        ))
    }
}
