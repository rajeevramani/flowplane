use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tonic::Status;
use tracing::{error, info, warn};

use crate::xds::state::XdsState;
use envoy_types::pb::envoy::service::discovery::v3::{
    DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};

/// Run the shared ADS stream loop for both minimal and database-backed services.
pub fn run_stream_loop<F>(
    state: Arc<XdsState>,
    mut in_stream: tonic::Streaming<DiscoveryRequest>,
    responder: F,
    label: &str,
) -> ReceiverStream<std::result::Result<DiscoveryResponse, Status>>
where
    F: Fn(
            Arc<XdsState>,
            DiscoveryRequest,
        ) -> Pin<Box<dyn Future<Output = crate::Result<DiscoveryResponse>> + Send>>
        + Send
        + Sync
        + 'static,
{
    let (tx, rx) = mpsc::channel(100);
    let state_clone = state.clone();
    let responder = Arc::new(responder);
    let label = label.to_string();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = in_stream.next() => {
                    match result {
                        Some(Ok(discovery_request)) => {
                            info!(
                                type_url = %discovery_request.type_url,
                                version_info = %discovery_request.version_info,
                                node_id = ?discovery_request.node.as_ref().map(|n| &n.id),
                                stream = %label,
                                "Received discovery request"
                            );

                            let state = state_clone.clone();
                            let responder = responder.clone();
                            let tx = tx.clone();
                            let label_clone = label.clone();

                            tokio::spawn(async move {
                                match responder(state, discovery_request).await {
                                    Ok(response) => {
                                        info!(
                                            type_url = %response.type_url,
                                            version = %response.version_info,
                                            nonce = %response.nonce,
                                            resource_count = response.resources.len(),
                                            stream = %label_clone,
                                            "Sending discovery response"
                                        );
                                        if tx.send(Ok(response)).await.is_err() {
                                            error!(stream = %label_clone, "Discovery response receiver dropped");
                                        }
                                    }
                                    Err(e) => {
                                        error!(stream = %label_clone, error = %e, "Failed to create resource response");
                                    }
                                }
                            });
                        }
                        Some(Err(e)) => {
                            warn!(stream = %label, "Error receiving discovery request: {}", e);
                            let _ = tx.send(Err(e)).await;
                            break;
                        }
                        None => {
                            info!(stream = %label, "ADS stream ended by client");
                            break;
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!(stream = %label, "Shutting down ADS stream");
                    break;
                }
            }
        }
    });

    ReceiverStream::new(rx)
}

/// Placeholder delta stream (returns empty stream for now).
pub fn empty_delta_stream() -> ReceiverStream<std::result::Result<DeltaDiscoveryResponse, Status>> {
    let (_tx, rx) = mpsc::channel(1);
    ReceiverStream::new(rx)
}
