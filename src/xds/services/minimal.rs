use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use envoy_types::pb::envoy::service::discovery::v3::{
    aggregated_discovery_service_server::AggregatedDiscoveryService, DeltaDiscoveryRequest,
    DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse, Resource,
};

use crate::Result;

use super::super::{
    resources::{self, BuiltResource},
    XdsState,
};

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

        let built = self.build_resources(request.type_url.as_str())?;
        let resources = built.into_iter().map(BuiltResource::into_any).collect();

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

impl MinimalAggregatedDiscoveryService {
    fn build_resources(&self, type_url: &str) -> Result<Vec<BuiltResource>> {
        match type_url {
            "type.googleapis.com/envoy.config.cluster.v3.Cluster" => {
                resources::clusters_from_config(&self.state.config)
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

    fn create_delta_response(
        &self,
        request: &DeltaDiscoveryRequest,
    ) -> Result<DeltaDiscoveryResponse> {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        // Build all available resources for this type
        // The stream logic will handle proper delta filtering and ACK detection
        let built = self.build_resources(&request.type_url)?;

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

        // Extract trace context from gRPC metadata for distributed tracing
        let parent_context =
            crate::xds::services::stream::extract_trace_context(request.metadata());

        let responder = move |state: Arc<XdsState>, request: DiscoveryRequest| {
            let service = MinimalAggregatedDiscoveryService { state };
            Box::pin(async move { service.create_resource_response(&request) })
                as Pin<Box<dyn Future<Output = Result<DiscoveryResponse>> + Send>>
        };

        let stream = crate::xds::services::stream::run_stream_loop(
            self.state.clone(),
            request.into_inner(),
            responder,
            "minimal",
            Some(parent_context),
        );

        Ok(Response::new(Box::pin(stream)))
    }

    async fn delta_aggregated_resources(
        &self,
        request: Request<tonic::Streaming<DeltaDiscoveryRequest>>,
    ) -> std::result::Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        info!("Delta ADS stream connection established");

        // Extract trace context from gRPC metadata for distributed tracing
        let parent_context =
            crate::xds::services::stream::extract_trace_context(request.metadata());

        let responder = move |state: Arc<XdsState>, request: DeltaDiscoveryRequest| {
            let service = MinimalAggregatedDiscoveryService { state };
            Box::pin(async move { service.create_delta_response(&request) })
                as Pin<Box<dyn Future<Output = Result<DeltaDiscoveryResponse>> + Send>>
        };

        let stream = crate::xds::services::stream::run_delta_loop(
            self.state.clone(),
            request.into_inner(),
            responder,
            "minimal",
            Some(parent_context),
        );

        Ok(Response::new(Box::pin(stream)))
    }
}
