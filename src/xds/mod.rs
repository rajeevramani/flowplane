//! Envoy xDS (eXtended Discovery Service) implementation
//!
//! Provides gRPC server implementing Envoy's discovery protocols:
//! - ADS (Aggregated Discovery Service)
//! - CDS (Cluster Discovery Service)
//! - RDS (Route Discovery Service)
//! - LDS (Listener Discovery Service)

use crate::{config::XdsConfig, Result};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tonic::{transport::Server, Request, Response, Status};
use tracing::{error, info, warn};

// Import envoy-types for proper Envoy protobuf types
use envoy_types::pb::envoy::service::discovery::v3::{
    aggregated_discovery_service_server::{
        AggregatedDiscoveryService, AggregatedDiscoveryServiceServer,
    },
    DeltaDiscoveryRequest, DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};
use envoy_types::pb::google::protobuf::Any;

/// Minimal XDS server state
#[derive(Debug)]
pub struct XdsState {
    pub config: XdsConfig,
    pub version: Arc<std::sync::atomic::AtomicU64>,
}

impl XdsState {
    pub fn new(config: XdsConfig) -> Self {
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    pub fn get_version(&self) -> String {
        self.version
            .load(std::sync::atomic::Ordering::Relaxed)
            .to_string()
    }

    pub fn increment_version(&self) {
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Minimal Aggregated Discovery Service implementation
/// Returns empty resources for all requests to pass basic connectivity tests
#[derive(Debug)]
pub struct MinimalAggregatedDiscoveryService {
    state: Arc<XdsState>,
}

impl MinimalAggregatedDiscoveryService {
    pub fn new(state: Arc<XdsState>) -> Self {
        Self { state }
    }

    /// Create an empty discovery response for any resource type
    fn create_empty_response(&self, request: &DiscoveryRequest) -> DiscoveryResponse {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        DiscoveryResponse {
            version_info: version.clone(),
            resources: Vec::<Any>::new(), // Empty resources for minimal implementation
            canary: false,
            type_url: request.type_url.clone(),
            nonce: nonce.clone(),
            control_plane: None,
            resource_errors: Vec::new(), // No resource errors for empty responses
        }
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

                                // Create empty response for this request
                                let response = MinimalAggregatedDiscoveryService { state: state.clone() }
                                    .create_empty_response(&discovery_request);

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

/// Start the minimal xDS gRPC server with configuration and graceful shutdown
/// This implements a basic ADS server that responds with empty resources
pub async fn start_minimal_xds_server_with_config<F>(
    xds_config: XdsConfig,
    shutdown_signal: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let addr = format!("{}:{}", xds_config.bind_address, xds_config.port)
        .parse()
        .map_err(|e| crate::Error::config(format!("Invalid xDS address: {}", e)))?;

    let state = Arc::new(XdsState::new(xds_config));

    info!(
        address = %addr,
        "Starting minimal Envoy xDS server (Checkpoint 2)"
    );

    // Create ADS service implementation
    let ads_service = MinimalAggregatedDiscoveryService::new(state);

    // Build and start the gRPC server with ADS service only
    // This is sufficient for Envoy to connect and receive empty responses
    let server = Server::builder()
        .add_service(AggregatedDiscoveryServiceServer::new(ads_service))
        .serve_with_shutdown(addr, shutdown_signal);

    info!("XDS server listening on {}", addr);

    // Start the server with graceful shutdown
    server
        .await
        .map_err(|e| crate::Error::transport(format!("XDS server failed: {}", e)))?;

    Ok(())
}

/// Legacy function for backward compatibility - kept for existing tests
/// This will be removed in future checkpoints
pub async fn start_minimal_xds_server() -> Result<()> {
    let xds_config = XdsConfig::default();
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C signal handler");
    };
    start_minimal_xds_server_with_config(xds_config, shutdown_signal).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xds_config_default() {
        let config = XdsConfig::default();
        assert_eq!(config.bind_address, "0.0.0.0");
        assert_eq!(config.port, 18000);
    }

    #[test]
    fn test_xds_state_versioning() {
        let state = XdsState::new(XdsConfig::default());
        assert_eq!(state.get_version(), "1");

        state.increment_version();
        assert_eq!(state.get_version(), "2");
    }

    #[tokio::test]
    async fn test_minimal_ads_service_creation() {
        let state = Arc::new(XdsState::new(XdsConfig::default()));
        let _service = MinimalAggregatedDiscoveryService::new(state);

        // Basic test that service can be created
        assert!(true); // Placeholder - in real tests we'd test the service methods
    }
}
