use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tonic::Status;
use tracing::{error, info, warn};

use crate::xds::state::XdsState;
use envoy_types::pb::envoy::service::discovery::v3::{
    DeltaDiscoveryRequest, DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};

// Removed complex delta state tracking - using PoC-style approach instead

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

/// Run the delta ADS stream loop using PoC-style approach with database persistence
pub fn run_delta_loop<F>(
    state: Arc<XdsState>,
    mut in_stream: tonic::Streaming<DeltaDiscoveryRequest>,
    responder: F,
    label: &str,
) -> ReceiverStream<std::result::Result<DeltaDiscoveryResponse, Status>>
where
    F: Fn(
            Arc<XdsState>,
            DeltaDiscoveryRequest,
        ) -> Pin<Box<dyn Future<Output = crate::Result<DeltaDiscoveryResponse>> + Send>>
        + Send
        + Sync
        + 'static,
{
    let (tx, rx) = mpsc::channel(100);
    let state_clone = state.clone();
    let responder = Arc::new(responder);
    let label = label.to_string();
    let mut update_rx = state.subscribe_updates();

    tokio::spawn(async move {
        let mut last_sent_version = 0u64;
        let mut pending_types: Vec<String> = Vec::new();

        loop {
            tokio::select! {
                result = in_stream.next() => {
                    match result {
                        Some(Ok(delta_request)) => {
                            info!(
                                type_url = %delta_request.type_url,
                                nonce = %delta_request.response_nonce,
                                stream = %label,
                                "Received delta discovery request"
                            );

                            // Check if this is an ACK/NACK (has our previous nonce) or initial request
                            let is_ack_or_nack = !delta_request.response_nonce.is_empty();

                            if is_ack_or_nack {
                                if let Some(error_detail) = &delta_request.error_detail {
                                    warn!(
                                        nonce = %delta_request.response_nonce,
                                        error_code = error_detail.code,
                                        error_message = %error_detail.message,
                                        type_url = %delta_request.type_url,
                                        stream = %label,
                                        "[NACK] Delta request rejected by Envoy"
                                    );
                                } else {
                                    info!(
                                        nonce = %delta_request.response_nonce,
                                        type_url = %delta_request.type_url,
                                        stream = %label,
                                        "[ACK] Delta request acknowledged"
                                    );
                                }
                                // For ACKs/NACKs, just continue listening
                                continue;
                            }

                            // Track what type this client is interested in
                            if !pending_types.contains(&delta_request.type_url) {
                                pending_types.push(delta_request.type_url.clone());
                            }

                            info!(
                                type_url = %delta_request.type_url,
                                stream = %label,
                                "Processing initial delta request, preparing response"
                            );

                            // Get current version and send response for initial request
                            let current_version_num = state_clone.get_version_number();
                            last_sent_version = current_version_num;

                            let state_for_task = state_clone.clone();
                            let responder_for_task = responder.clone();
                            let tx_for_task = tx.clone();
                            let label_for_task = label.clone();

                            tokio::spawn(async move {
                                match responder_for_task(state_for_task, delta_request.clone()).await {
                                    Ok(response) => {
                                        info!(
                                            type_url = %response.type_url,
                                            nonce = %response.nonce,
                                            version = %response.system_version_info,
                                            resource_count = response.resources.len(),
                                            stream = %label_for_task,
                                            "Sending initial delta response"
                                        );
                                        if tx_for_task.send(Ok(response)).await.is_err() {
                                            error!(stream = %label_for_task, "Delta response receiver dropped");
                                        }
                                    }
                                    Err(e) => {
                                        error!(stream = %label_for_task, error = %e, "Failed to create delta response");
                                    }
                                }
                            });
                        }
                        Some(Err(e)) => {
                            warn!(stream = %label, "Error receiving delta discovery request: {}", e);
                            let _ = tx.send(Err(e)).await;
                            break;
                        }
                        None => {
                            info!(stream = %label, "Delta ADS stream ended by client");
                            break;
                        }
                    }
                }
                update = update_rx.recv() => {
                    match update {
                        Ok(_version) => {
                            let current_version_num = state_clone.get_version_number();

                            // Only send updates if version has changed and we have types to update
                            if current_version_num > last_sent_version && !pending_types.is_empty() {
                                info!(
                                    current_version = current_version_num,
                                    last_version = last_sent_version,
                                    type_count = pending_types.len(),
                                    stream = %label,
                                    "Pushing delta resource updates to client"
                                );

                                // Send updates for all types this client is interested in
                                for type_url in &pending_types {
                                    let state_for_task = state_clone.clone();
                                    let responder_for_task = responder.clone();
                                    let tx_for_task = tx.clone();
                                    let label_for_task = label.clone();
                                    let type_url_for_task = type_url.clone();

                                    tokio::spawn(async move {
                                        // Create a delta request for this type
                                        let push_request = DeltaDiscoveryRequest {
                                            type_url: type_url_for_task.clone(),
                                            response_nonce: String::new(), // Empty for push updates
                                            ..Default::default()
                                        };

                                        match responder_for_task(state_for_task, push_request).await {
                                            Ok(response) => {
                                                info!(
                                                    type_url = %response.type_url,
                                                    nonce = %response.nonce,
                                                    version = %response.system_version_info,
                                                    resource_count = response.resources.len(),
                                                    stream = %label_for_task,
                                                    "Sending push update to client"
                                                );
                                                if tx_for_task.send(Ok(response)).await.is_err() {
                                                    error!(stream = %label_for_task, "Push update receiver dropped");
                                                }
                                            }
                                            Err(e) => {
                                                error!(
                                                    type_url = %type_url_for_task,
                                                    stream = %label_for_task,
                                                    error = %e,
                                                    "Failed to create push update"
                                                );
                                            }
                                        }
                                    });
                                }

                                last_sent_version = current_version_num;
                                info!(
                                    version = current_version_num,
                                    type_count = pending_types.len(),
                                    stream = %label,
                                    "All delta push updates sent successfully"
                                );
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!(
                                stream = %label,
                                skipped = skipped,
                                "Missed {} delta update notifications",
                                skipped
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            warn!(stream = %label, "Update notification channel closed");
                            break;
                        }
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!(stream = %label, "Shutting down delta ADS stream");
                    break;
                }
            }
        }
    });

    ReceiverStream::new(rx)
}

// Removed complex process_delta_request function - using PoC-style direct response pattern
