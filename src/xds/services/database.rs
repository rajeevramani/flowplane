use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use envoy_types::pb::envoy::config::cluster::v3::Cluster;
use envoy_types::pb::envoy::config::core::v3::{socket_address, Address, SocketAddress};
use envoy_types::pb::envoy::config::endpoint::v3::{
    lb_endpoint, ClusterLoadAssignment, Endpoint, LbEndpoint, LocalityLbEndpoints,
};
use envoy_types::pb::envoy::service::discovery::v3::{
    aggregated_discovery_service_server::AggregatedDiscoveryService, DeltaDiscoveryRequest,
    DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};
use envoy_types::pb::google::protobuf::{Any, Duration};
use prost::Message;

use crate::Result;

use super::super::{resources, XdsState};

/// Database-enabled Aggregated Discovery Service implementation
/// Returns resources from database when available, falls back to config-based resources
#[derive(Debug)]
pub struct DatabaseAggregatedDiscoveryService {
    pub(super) state: Arc<XdsState>,
}

impl DatabaseAggregatedDiscoveryService {
    pub fn new(state: Arc<XdsState>) -> Self {
        Self { state }
    }

    /// Create discovery response with database-backed resources
    async fn create_resource_response(
        &self,
        request: &DiscoveryRequest,
    ) -> Result<DiscoveryResponse> {
        let version = self.state.get_version();
        let nonce = uuid::Uuid::new_v4().to_string();

        let resources = match request.type_url.as_str() {
            "type.googleapis.com/envoy.config.cluster.v3.Cluster" => {
                self.create_cluster_resources_from_db().await?
            }
            "type.googleapis.com/envoy.config.route.v3.RouteConfiguration" => {
                resources::routes_from_config(&self.state.config)? // Still use config-based for now
            }
            "type.googleapis.com/envoy.config.listener.v3.Listener" => {
                resources::listeners_from_config(&self.state.config)? // Still use config-based for now
            }
            "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment" => {
                resources::endpoints_from_config(&self.state.config)? // Still use config-based for now
            }
            _ => {
                warn!("Unknown resource type requested: {}", request.type_url);
                Vec::new()
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

    /// Create cluster resources from database
    async fn create_cluster_resources_from_db(&self) -> Result<Vec<Any>> {
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
                        "Creating {} cluster resources from database",
                        cluster_data_list.len()
                    );
                    let mut resources = Vec::new();

                    for cluster_data in cluster_data_list {
                        // Parse the stored JSON configuration
                        let config: serde_json::Value =
                            serde_json::from_str(&cluster_data.configuration).map_err(|e| {
                                crate::Error::config(format!(
                                    "Invalid cluster configuration JSON: {}",
                                    e
                                ))
                            })?;

                        // Create Envoy cluster from stored configuration
                        let cluster = self
                            .create_envoy_cluster_from_config(&cluster_data.name, &config)
                            .await?;

                        let encoded = cluster.encode_to_vec();
                        info!(
                            cluster_name = %cluster_data.name,
                            service_name = %cluster_data.service_name,
                            version = cluster_data.version,
                            encoded_size = encoded.len(),
                            "Created cluster resource from database"
                        );

                        let any_resource = Any {
                            type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster"
                                .to_string(),
                            value: encoded,
                        };
                        resources.push(any_resource);
                    }

                    Ok(resources)
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
    fn create_fallback_cluster_resources(&self) -> Result<Vec<Any>> {
        resources::clusters_from_config(&self.state.config)
    }

    /// Create Envoy cluster from JSON configuration
    async fn create_envoy_cluster_from_config(
        &self,
        name: &str,
        config: &serde_json::Value,
    ) -> Result<Cluster> {
        // Parse endpoints from configuration
        let endpoints = config
            .get("endpoints")
            .and_then(|e| e.as_array())
            .ok_or_else(|| {
                crate::Error::config("Cluster configuration missing 'endpoints' array".to_string())
            })?;

        let mut lb_endpoints = Vec::new();
        for endpoint in endpoints {
            if let Some(endpoint_str) = endpoint.as_str() {
                // Parse "host:port" format
                let parts: Vec<&str> = endpoint_str.split(':').collect();
                if parts.len() == 2 {
                    let host = parts[0].to_string();
                    if let Ok(port) = parts[1].parse::<u32>() {
                        lb_endpoints.push(LbEndpoint {
                            host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                                address: Some(Address {
                                    address: Some(
                                        envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                            SocketAddress {
                                                address: host,
                                                port_specifier: Some(
                                                    socket_address::PortSpecifier::PortValue(port),
                                                ),
                                                protocol: 0,
                                                ..Default::default()
                                            },
                                        ),
                                    ),
                                }),
                                ..Default::default()
                            })),
                            ..Default::default()
                        });
                    }
                }
            }
        }

        if lb_endpoints.is_empty() {
            return Err(crate::Error::config(
                "No valid endpoints found in cluster configuration".to_string(),
            ));
        }

        Ok(Cluster {
            name: name.to_string(),
            connect_timeout: Some(Duration {
                seconds: config
                    .get("connect_timeout_seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(5) as i64,
                nanos: 0,
            }),
            load_assignment: Some(ClusterLoadAssignment {
                cluster_name: name.to_string(),
                endpoints: vec![LocalityLbEndpoints {
                    lb_endpoints,
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        })
    }
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
                                    "Received discovery request (database-enabled)"
                                );

                                // Create response with database-backed resources
                                let service = DatabaseAggregatedDiscoveryService { state: state.clone() };
                                let response = match service.create_resource_response(&discovery_request).await {
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
                                    "Sending discovery response with database-backed resources"
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
        info!("Delta ADS stream connection established (database-enabled)");

        // For minimal implementation, just return empty stream
        let (_tx, rx) = mpsc::channel(1);
        let out_stream = ReceiverStream::new(rx);

        Ok(Response::new(
            Box::pin(out_stream) as Self::DeltaAggregatedResourcesStream
        ))
    }
}
