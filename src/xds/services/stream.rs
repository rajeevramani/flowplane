//! xDS Streaming Protocol Implementation
//!
//! This module provides the core streaming protocol implementation for Envoy's
//! xDS (eXtended Discovery Service) APIs, supporting both State of the World (SOTW)
//! and Delta protocols for CDS, RDS, LDS, and EDS resource types.
//!
//! # Architecture
//!
//! The implementation uses a shared stream loop pattern that handles:
//! - **ACK/NACK Processing**: Detects and handles Envoy acknowledgments and rejections
//! - **Version Tracking**: Tracks sent versions and nonces per resource type
//! - **Subscription Management**: Maintains per-client subscription state
//! - **Push Updates**: Proactively sends updates when resources change
//! - **Error Recovery**: Handles stream errors, timeouts, and lagged notifications
//! - **Concurrent Safety**: Uses Arc/Mutex for thread-safe state management
//!
//! # Protocol Flow
//!
//! ## SOTW (State of the World) Protocol
//!
//! 1. **Initial Request**: Envoy sends DiscoveryRequest with empty version
//! 2. **Response**: Server sends all resources with version and nonce
//! 3. **ACK/NACK**: Envoy sends request with version and nonce
//!    - ACK: Same version/nonce, no error_detail (we skip duplicate)
//!    - NACK: Same nonce, but error_detail present (we log and resend)
//! 4. **Push Updates**: When resources change, server pushes new snapshot
//! 5. **Subscription**: Client subscriptions tracked per type_url
//!
//! ## Delta Protocol
//!
//! 1. **Initial Request**: Envoy sends DeltaDiscoveryRequest with subscribed resources
//! 2. **Response**: Server sends only changed resources (added/updated/removed)
//! 3. **ACK/NACK**: Envoy sends request with response_nonce
//! 4. **Push Updates**: Server pushes only deltas when resources change
//!
//! # Concurrency
//!
//! - Each incoming request spawns a tokio task for parallel processing
//! - Version tracking uses `Arc<Mutex<HashMap>>` for thread-safe updates
//! - Subscription state uses `Arc<Mutex<HashSet>>` for concurrent access
//! - Resource updates broadcast via tokio::sync::broadcast channel
//!
//! # Error Handling
//!
//! - **Stream Errors**: Logged and stream terminated gracefully
//! - **Lagged Notifications**: Warns when broadcast channel falls behind
//! - **Response Failures**: Logged with context, stream continues
//! - **Channel Closed**: Detects disconnection and exits cleanly

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tonic::Status;
use tracing::{debug, error, info, warn};

use crate::xds::state::{ResourceDelta, XdsState};
use envoy_types::pb::envoy::service::discovery::v3::Resource;
use envoy_types::pb::envoy::service::discovery::v3::{
    DeltaDiscoveryRequest, DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};
use uuid::Uuid;

/// Tracks the last sent version and nonce for ACK/NACK detection
#[derive(Clone, Debug)]
struct LastDiscoverySnapshot {
    version: Arc<str>,
    nonce: Arc<str>,
}

/// Extract team identifier from Envoy node metadata
///
/// # Arguments
///
/// * `node` - Optional Envoy node metadata from discovery request
///
/// # Returns
///
/// `Some(team)` if team metadata is present, `None` otherwise
fn extract_team_from_node(
    node: &Option<envoy_types::pb::envoy::config::core::v3::Node>,
) -> Option<String> {
    node.as_ref().and_then(|n| {
        n.metadata.as_ref().and_then(|meta| {
            meta.fields.get("team").and_then(|v| {
                if let Some(envoy_types::pb::google::protobuf::value::Kind::StringValue(s)) =
                    v.kind.as_ref()
                {
                    if !s.is_empty() {
                        Some(s.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        })
    })
}

/// Run the shared SOTW (State of the World) ADS stream loop.
///
/// This function implements the core xDS streaming protocol for CDS, RDS, LDS, and EDS.
/// It handles both client-initiated requests and server-initiated push updates.
///
/// # Protocol Behavior
///
/// - **ACK Detection**: Skips duplicate requests when client ACKs with matching version/nonce
/// - **NACK Handling**: Logs errors when client rejects configuration but continues operation
/// - **Subscription Tracking**: Remembers which resource types each client has requested
/// - **Push Updates**: Proactively sends updates when XdsState broadcasts changes
/// - **Concurrent Processing**: Spawns tasks for each request to maximize throughput
///
/// # Arguments
///
/// * `state` - Shared xDS state containing resource cache and repositories
/// * `in_stream` - Incoming stream of DiscoveryRequest messages from Envoy
/// * `responder` - Async function that builds DiscoveryResponse for a given request
/// * `label` - Human-readable label for logging (e.g., "ADS", "CDS")
///
/// # Returns
///
/// A `ReceiverStream` that produces DiscoveryResponse messages to send to Envoy
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
    let label: Arc<str> = Arc::from(label);
    let last_sent = Arc::new(Mutex::new(HashMap::<String, LastDiscoverySnapshot>::new()));
    let mut update_rx = state.subscribe_updates();
    let subscribed_types = Arc::new(Mutex::new(std::collections::HashSet::<String>::new()));

    // Track the team for this stream to decrement connection metric on stream close
    let team_for_stream: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let team_for_cleanup = team_for_stream.clone();

    tokio::spawn(async move {
        // Run the stream loop and ensure cleanup happens
        async {
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

                            // Extract and track team from node metadata on first request
                            let team_extracted = extract_team_from_node(&discovery_request.node);
                            {
                                let mut team_guard = team_for_stream.lock().await;
                                if team_guard.is_none() && team_extracted.is_some() {
                                    *team_guard = team_extracted.clone();
                                    // Increment connection metric when we first track a team for this stream
                                    if let Some(ref team) = team_extracted {
                                        crate::observability::metrics::record_team_xds_connection(team, true).await;
                                        info!(stream = %label, team = %team, "New xDS stream established, incrementing connection gauge");
                                    }
                                }
                            }

                            let state = state_clone.clone();
                            let responder = responder.clone();
                            let tx = tx.clone();
                            let label_for_task = label.clone();
                            let tracker = last_sent.clone();
                            let subscribed_for_task = subscribed_types.clone();

                            tokio::spawn(async move {
                                let node_id = discovery_request
                                    .node
                                    .as_ref()
                                    .map(|n| n.id.clone());

                                // Create a span for this discovery request processing
                                let span = tracing::info_span!(
                                    "xds_discovery_request",
                                    type_url = %discovery_request.type_url,
                                    node_id = ?node_id,
                                    stream = %label_for_task
                                );
                                let _enter = span.enter();

                                let tracker_guard = tracker.lock().await;
                                let last_snapshot = tracker_guard
                                    .get(&discovery_request.type_url)
                                    .cloned();

                                let current_version = state.get_version();

                                let is_ack = last_snapshot
                                    .as_ref()
                                    .map(|snapshot| {
                                        !discovery_request.response_nonce.is_empty()
                                            && discovery_request.response_nonce.as_str() == snapshot.nonce.as_ref()
                                            && discovery_request.version_info.as_str() == snapshot.version.as_ref()
                                            && discovery_request.error_detail.is_none()
                                            && snapshot.version.as_ref() == current_version
                                    })
                                    .unwrap_or(false);

                                if is_ack {
                                    debug!(
                                        type_url = %discovery_request.type_url,
                                        version = %discovery_request.version_info,
                                        nonce = %discovery_request.response_nonce,
                                        node_id = ?node_id,
                                        stream = %label_for_task,
                                        "[ACK] Skipping duplicate discovery request"
                                    );
                                    return;
                                }

                                if let Some(error_detail) = discovery_request.error_detail.as_ref() {
                                    warn!(
                                        type_url = %discovery_request.type_url,
                                        nonce = %discovery_request.response_nonce,
                                        error_code = error_detail.code,
                                        error_message = %error_detail.message,
                                        node_id = ?node_id,
                                        stream = %label_for_task,
                                        "[NACK] Envoy rejected previous response"
                                    );
                                }

                                drop(tracker_guard);

                                // Track this type_url as subscribed by the client
                                {
                                    let mut guard = subscribed_for_task.lock().await;
                                    guard.insert(discovery_request.type_url.clone());
                                }

                                match responder(state, discovery_request).await {
                                    Ok(response) => {
                                        info!(
                                            type_url = %response.type_url,
                                            version = %response.version_info,
                                            nonce = %response.nonce,
                                            resource_count = response.resources.len(),
                                            stream = %label_for_task,
                                            "Sending discovery response"
                                        );

                                        let version: Arc<str> = Arc::from(response.version_info.clone());
                                        let nonce: Arc<str> = Arc::from(response.nonce.clone());
                                        let type_url = response.type_url.clone();

                                        {
                                            let mut tracker_guard = tracker.lock().await;
                                            tracker_guard.insert(
                                                type_url,
                                                LastDiscoverySnapshot { version, nonce },
                                            );
                                        }

                                        if tx.send(Ok(response)).await.is_err() {
                                            error!(stream = %label_for_task, "Discovery response receiver dropped");
                                        }
                                    }
                                    Err(e) => {
                                        error!(stream = %label_for_task, error = %e, "Failed to create resource response");
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
                update = update_rx.recv() => {
                    match update {
                        Ok(update) => {
                            // For SOTW, push a fresh snapshot for each type this client has requested
                            let interested: Vec<String> = {
                                let guard = subscribed_types.lock().await;
                                guard.iter().cloned().collect()
                            };
                            if interested.is_empty() { continue; }

                            for delta in &update.deltas {
                                if !interested.contains(&delta.type_url) { continue; }

                                let state_for_task = state_clone.clone();
                                let responder_for_task = responder.clone();
                                let tx_for_task = tx.clone();
                                let label_for_task = label.clone();
                                let tracker_for_task = last_sent.clone();
                                let type_url_for_task = delta.type_url.clone();

                                tokio::spawn(async move {
                                    // Create a span for SOTW push updates
                                    let span = tracing::info_span!(
                                        "xds_sotw_push_update",
                                        type_url = %type_url_for_task,
                                        stream = %label_for_task
                                    );
                                    let _enter = span.enter();

                                    // Build a minimal request for this type
                                    let request = DiscoveryRequest { type_url: type_url_for_task.clone(), ..Default::default() };
                                    match responder_for_task(state_for_task, request).await {
                                        Ok(response) => {
                                            info!(
                                                type_url = %response.type_url,
                                                version = %response.version_info,
                                                nonce = %response.nonce,
                                                resource_count = response.resources.len(),
                                                stream = %label_for_task,
                                                "Pushing SOTW update response"
                                            );

                                            let version: Arc<str> = Arc::from(response.version_info.clone());
                                            let nonce: Arc<str> = Arc::from(response.nonce.clone());
                                            let type_url = response.type_url.clone();
                                            {
                                                let mut guard = tracker_for_task.lock().await;
                                                guard.insert(type_url, LastDiscoverySnapshot { version, nonce });
                                            }

                                            if tx_for_task.send(Ok(response)).await.is_err() {
                                                error!(stream = %label_for_task, "Discovery response receiver dropped");
                                            }
                                        }
                                        Err(e) => {
                                            error!(stream = %label_for_task, error = %e, "Failed to create SOTW push response");
                                        }
                                    }
                                });
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!(stream = %label, skipped = skipped, "Missed {} update notifications", skipped);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            warn!(stream = %label, "Update notification channel closed");
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
        }
        .await;

        // Stream cleanup: decrement the connection gauge for this team
        let team_option = team_for_cleanup.lock().await;
        if let Some(team) = team_option.as_ref() {
            info!(stream = %label, team = %team, "xDS stream closing, decrementing connection gauge");
            crate::observability::metrics::record_team_xds_connection(team, false).await;
        } else {
            debug!(stream = %label, "xDS stream closing, but no team was tracked");
        }
    });

    ReceiverStream::new(rx)
}

/// Run the shared Delta xDS ADS stream loop.
///
/// This function implements the Delta variant of the xDS protocol, which sends only
/// incremental changes instead of full snapshots. This is more efficient for large
/// resource sets where only a few items change at a time.
///
/// # Protocol Behavior
///
/// - **Initial Request**: Client sends empty nonce, server responds with all resources
/// - **ACK Detection**: Non-empty nonce with no error_detail indicates acknowledgment
/// - **NACK Handling**: Non-empty nonce with error_detail indicates rejection
/// - **Incremental Updates**: Only changed/added/removed resources are sent
/// - **Subscription Tracking**: Tracks subscribed resource types per client
/// - **Push Updates**: Sends only deltas when XdsState broadcasts changes
///
/// # Delta Protocol Advantages
///
/// - **Bandwidth Efficiency**: Only sends changed resources, not full snapshots
/// - **Reduced Envoy Processing**: Envoy only processes changed configs
/// - **Better for Large Deployments**: Scales better with many routes/clusters
///
/// # Arguments
///
/// * `state` - Shared xDS state containing resource cache and repositories
/// * `in_stream` - Incoming stream of DeltaDiscoveryRequest messages from Envoy
/// * `responder` - Async function that builds DeltaDiscoveryResponse for a given request
/// * `label` - Human-readable label for logging (e.g., "Delta-ADS")
///
/// # Returns
///
/// A `ReceiverStream` that produces DeltaDiscoveryResponse messages to send to Envoy
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
    let label: Arc<str> = Arc::from(label);
    let mut update_rx = state.subscribe_updates();

    // Track the team for this stream to decrement connection metric on stream close
    let team_for_stream: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let team_for_cleanup = team_for_stream.clone();

    tokio::spawn(async move {
        let mut pending_types: HashSet<String> = HashSet::new();

        async {
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

                                // Extract and track team from node metadata on first request
                                let team_extracted = extract_team_from_node(&delta_request.node);
                                {
                                    let mut team_guard = team_for_stream.lock().await;
                                    if team_guard.is_none() && team_extracted.is_some() {
                                        *team_guard = team_extracted.clone();
                                        // Increment connection metric when we first track a team for this stream
                                        if let Some(ref team) = team_extracted {
                                            crate::observability::metrics::record_team_xds_connection(team, true).await;
                                            info!(stream = %label, team = %team, "New Delta xDS stream established, incrementing connection gauge");
                                        }
                                    }
                                }

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
                            pending_types.insert(delta_request.type_url.clone());

                            info!(
                                type_url = %delta_request.type_url,
                                stream = %label,
                                "Processing initial delta request, preparing response"
                            );

                            let state_for_task = state_clone.clone();
                            let responder_for_task = responder.clone();
                            let tx_for_task = tx.clone();
                            let label_for_task = label.clone();

                            tokio::spawn(async move {
                                // Create a span for delta discovery request processing
                                let span = tracing::info_span!(
                                    "xds_delta_discovery_request",
                                    type_url = %delta_request.type_url,
                                    stream = %label_for_task
                                );
                                let _enter = span.enter();

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
                        Ok(update) => {
                            if pending_types.is_empty() {
                                continue;
                            }

                            for delta in &update.deltas {
                                if !pending_types.contains(&delta.type_url) {
                                    continue;
                                }

                                if delta.added_or_updated.is_empty() && delta.removed.is_empty() {
                                    continue;
                                }

                                let response = build_delta_response(update.version, delta);
                                let tx_for_task = tx.clone();
                                let label_for_task = label.clone();

                                info!(
                                    type_url = %delta.type_url,
                                    added = delta.added_or_updated.len(),
                                    removed = delta.removed.len(),
                                    version = update.version,
                                    stream = %label,
                                    "Sending delta push update to client"
                                );

                                tokio::spawn(async move {
                                    // Create a span for delta push updates
                                    let span = tracing::info_span!(
                                        "xds_delta_push_update",
                                        type_url = %response.type_url,
                                        stream = %label_for_task
                                    );
                                    let _enter = span.enter();

                                    if tx_for_task.send(Ok(response)).await.is_err() {
                                        error!(stream = %label_for_task, "Delta response receiver dropped");
                                    }
                                });
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
        }
        .await;

        // Stream cleanup: decrement the connection gauge for this team
        let team_option = team_for_cleanup.lock().await;
        if let Some(team) = team_option.as_ref() {
            info!(stream = %label, team = %team, "Delta xDS stream closing, decrementing connection gauge");
            crate::observability::metrics::record_team_xds_connection(team, false).await;
        } else {
            debug!(stream = %label, "Delta xDS stream closing, but no team was tracked");
        }
    });

    ReceiverStream::new(rx)
}

/// Build a DeltaDiscoveryResponse from a ResourceDelta.
///
/// Converts internal ResourceDelta representation into the Envoy protobuf format
/// with added/updated resources and removed resource names.
///
/// # Arguments
///
/// * `update_version` - Global version number for this update
/// * `delta` - Resource changes (added, updated, removed)
///
/// # Returns
///
/// A `DeltaDiscoveryResponse` ready to send to Envoy
fn build_delta_response(update_version: u64, delta: &ResourceDelta) -> DeltaDiscoveryResponse {
    let resources: Vec<Resource> = delta
        .added_or_updated
        .iter()
        .map(|cached| Resource {
            name: cached.name.clone(),
            version: cached.version.to_string(),
            resource: Some(cached.body.clone()),
            ..Default::default()
        })
        .collect();

    DeltaDiscoveryResponse {
        system_version_info: update_version.to_string(),
        type_url: delta.type_url.clone(),
        nonce: Uuid::new_v4().to_string(),
        resources,
        removed_resources: delta.removed.clone(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoy_types::pb::envoy::config::core::v3::Node;
    use envoy_types::pb::google::protobuf::value::Kind;
    use envoy_types::pb::google::protobuf::{Struct, Value};
    use std::collections::HashMap;

    /// Test extracting team from node metadata with valid team field
    #[test]
    fn test_extract_team_from_node_with_team() {
        let mut fields = HashMap::new();
        fields.insert(
            "team".to_string(),
            Value { kind: Some(Kind::StringValue("payments".to_string())) },
        );

        let node = Some(Node {
            id: "test-node".to_string(),
            metadata: Some(Struct { fields }),
            ..Default::default()
        });

        let result = extract_team_from_node(&node);
        assert_eq!(result, Some("payments".to_string()));
    }

    /// Test extracting team from node metadata with empty team string
    #[test]
    fn test_extract_team_from_node_with_empty_team() {
        let mut fields = HashMap::new();
        fields.insert("team".to_string(), Value { kind: Some(Kind::StringValue("".to_string())) });

        let node = Some(Node {
            id: "test-node".to_string(),
            metadata: Some(Struct { fields }),
            ..Default::default()
        });

        let result = extract_team_from_node(&node);
        assert_eq!(result, None);
    }

    /// Test extracting team from node metadata without team field
    #[test]
    fn test_extract_team_from_node_without_team() {
        let fields = HashMap::new();

        let node = Some(Node {
            id: "test-node".to_string(),
            metadata: Some(Struct { fields }),
            ..Default::default()
        });

        let result = extract_team_from_node(&node);
        assert_eq!(result, None);
    }

    /// Test extracting team from node without metadata
    #[test]
    fn test_extract_team_from_node_without_metadata() {
        let node = Some(Node { id: "test-node".to_string(), metadata: None, ..Default::default() });

        let result = extract_team_from_node(&node);
        assert_eq!(result, None);
    }

    /// Test extracting team when node is None
    #[test]
    fn test_extract_team_from_none_node() {
        let result = extract_team_from_node(&None);
        assert_eq!(result, None);
    }

    /// Test extracting team with non-string metadata value
    #[test]
    fn test_extract_team_from_node_with_non_string_team() {
        let mut fields = HashMap::new();
        fields.insert("team".to_string(), Value { kind: Some(Kind::NumberValue(123.0)) });

        let node = Some(Node {
            id: "test-node".to_string(),
            metadata: Some(Struct { fields }),
            ..Default::default()
        });

        let result = extract_team_from_node(&node);
        assert_eq!(result, None);
    }

    /// Test extracting team with multiple metadata fields
    #[test]
    fn test_extract_team_from_node_with_multiple_fields() {
        let mut fields = HashMap::new();
        fields.insert(
            "team".to_string(),
            Value { kind: Some(Kind::StringValue("platform".to_string())) },
        );
        fields.insert(
            "cluster".to_string(),
            Value { kind: Some(Kind::StringValue("prod".to_string())) },
        );
        fields.insert(
            "region".to_string(),
            Value { kind: Some(Kind::StringValue("us-west-2".to_string())) },
        );

        let node = Some(Node {
            id: "test-node".to_string(),
            metadata: Some(Struct { fields }),
            ..Default::default()
        });

        let result = extract_team_from_node(&node);
        assert_eq!(result, Some("platform".to_string()));
    }
}
