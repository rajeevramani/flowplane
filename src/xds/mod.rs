//! Envoy xDS (eXtended Discovery Service) implementation
//!
//! Provides gRPC server implementing Envoy's discovery protocols:
//! - ADS (Aggregated Discovery Service)
//! - CDS (Cluster Discovery Service)
//! - RDS (Route Discovery Service)
//! - LDS (Listener Discovery Service)

use crate::{
    config::SimpleXdsConfig,
    storage::{ClusterRepository, DbPool},
    Result,
};
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

// Import Envoy resource types for actual configurations
use envoy_types::pb::envoy::config::cluster::v3::Cluster;
use envoy_types::pb::envoy::config::core::v3::{socket_address, Address, SocketAddress};
use envoy_types::pb::envoy::config::endpoint::v3::{
    lb_endpoint, ClusterLoadAssignment, Endpoint, LbEndpoint, LocalityLbEndpoints,
};
use envoy_types::pb::envoy::config::listener::v3::Listener;
use envoy_types::pb::envoy::config::route::v3::RouteConfiguration;
use envoy_types::pb::google::protobuf::Duration;
use prost::Message;

/// XDS server state with database integration
#[derive(Debug)]
pub struct XdsState {
    pub config: SimpleXdsConfig,
    pub version: Arc<std::sync::atomic::AtomicU64>,
    pub cluster_repository: Option<ClusterRepository>,
}

impl XdsState {
    pub fn new(config: SimpleXdsConfig) -> Self {
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: None,
        }
    }

    pub fn with_database(config: SimpleXdsConfig, pool: DbPool) -> Self {
        Self {
            config,
            version: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cluster_repository: Some(ClusterRepository::new(pool)),
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

/// Database-enabled Aggregated Discovery Service implementation
/// Returns resources from database when available, falls back to config-based resources
#[derive(Debug)]
pub struct DatabaseAggregatedDiscoveryService {
    state: Arc<XdsState>,
}

/// Minimal implementation for config-only scenarios
#[derive(Debug)]
pub struct MinimalAggregatedDiscoveryService {
    state: Arc<XdsState>,
}

impl DatabaseAggregatedDiscoveryService {
    pub fn new(state: Arc<XdsState>) -> Self {
        Self { state }
    }
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
                self.create_cluster_resources()?
            }
            "type.googleapis.com/envoy.config.route.v3.RouteConfiguration" => {
                self.create_route_resources()?
            }
            "type.googleapis.com/envoy.config.listener.v3.Listener" => {
                self.create_listener_resources()?
            }
            "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment" => {
                self.create_endpoint_resources()?
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

    /// Create basic cluster resources for testing (legacy implementation)
    fn create_cluster_resources(&self) -> Result<Vec<Any>> {
        let resources = &self.state.config.resources;
        let cluster = Cluster {
            name: resources.cluster_name.clone(),
            connect_timeout: Some(Duration {
                seconds: 5,
                nanos: 0,
            }),
            // Cluster discovery type is set implicitly based on load_assignment
            load_assignment: Some(ClusterLoadAssignment {
                cluster_name: resources.cluster_name.clone(),
                endpoints: vec![LocalityLbEndpoints {
                    lb_endpoints: vec![LbEndpoint {
                        host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                            address: Some(Address {
                                address: Some(envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                    SocketAddress {
                                        address: resources.backend_address.clone(),
                                        port_specifier: Some(socket_address::PortSpecifier::PortValue(resources.backend_port.into())),
                                        protocol: 0, // TCP protocol
                                        ..Default::default()
                                    }
                                )),
                            }),
                            ..Default::default()
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        // Validate by encoding - this ensures Envoy compatibility
        let encoded = cluster.encode_to_vec();
        info!(
            "Created cluster resource, encoded size: {} bytes",
            encoded.len()
        );

        let any_resource = Any {
            type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
            value: encoded,
        };

        Ok(vec![any_resource])
    }

    /// Create basic route configuration resources
    fn create_route_resources(&self) -> Result<Vec<Any>> {
        let resources = &self.state.config.resources;
        let route_config = RouteConfiguration {
            name: resources.route_name.clone(),
            virtual_hosts: vec![envoy_types::pb::envoy::config::route::v3::VirtualHost {
                name: "test_virtual_host".to_string(),
                domains: vec!["*".to_string()],
                routes: vec![envoy_types::pb::envoy::config::route::v3::Route {
                    name: "test_route_match".to_string(),
                    r#match: Some(envoy_types::pb::envoy::config::route::v3::RouteMatch {
                        path_specifier: Some(envoy_types::pb::envoy::config::route::v3::route_match::PathSpecifier::Prefix("/".to_string())),
                        ..Default::default()
                    }),
                    action: Some(envoy_types::pb::envoy::config::route::v3::route::Action::Route(
                        envoy_types::pb::envoy::config::route::v3::RouteAction {
                            cluster_specifier: Some(envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::Cluster(resources.cluster_name.clone())),
                            ..Default::default()
                        }
                    )),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        // Validate by encoding
        let encoded = route_config.encode_to_vec();
        info!(
            "Created route resource, encoded size: {} bytes",
            encoded.len()
        );

        let any_resource = Any {
            type_url: "type.googleapis.com/envoy.config.route.v3.RouteConfiguration".to_string(),
            value: encoded,
        };

        Ok(vec![any_resource])
    }

    /// Create basic listener resources
    fn create_listener_resources(&self) -> Result<Vec<Any>> {
        use envoy_types::pb::envoy::config::listener::v3::Filter;
        use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpConnectionManager;

        let resources = &self.state.config.resources;

        let http_conn_manager = HttpConnectionManager {
            stat_prefix: "ingress_http".to_string(),
            route_specifier: Some(envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::RouteSpecifier::Rds(
                envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::Rds {
                    config_source: Some(envoy_types::pb::envoy::config::core::v3::ConfigSource {
                        config_source_specifier: Some(envoy_types::pb::envoy::config::core::v3::config_source::ConfigSourceSpecifier::Ads(
                            envoy_types::pb::envoy::config::core::v3::AggregatedConfigSource::default()
                        )),
                        ..Default::default()
                    }),
                    route_config_name: resources.route_name.clone(),
                }
            )),
            ..Default::default()
        };

        let listener = Listener {
            name: resources.listener_name.clone(),
            address: Some(Address {
                address: Some(envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                    SocketAddress {
                        address: "0.0.0.0".to_string(),
                        port_specifier: Some(socket_address::PortSpecifier::PortValue(resources.listener_port.into())),
                        protocol: 0, // TCP protocol
                        ..Default::default()
                    }
                )),
            }),
            filter_chains: vec![envoy_types::pb::envoy::config::listener::v3::FilterChain {
                filters: vec![Filter {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    config_type: Some(envoy_types::pb::envoy::config::listener::v3::filter::ConfigType::TypedConfig(
                        Any {
                            type_url: "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager".to_string(),
                            value: http_conn_manager.encode_to_vec(),
                        }
                    )),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        // Validate by encoding
        let encoded = listener.encode_to_vec();
        info!(
            "Created listener resource, encoded size: {} bytes",
            encoded.len()
        );

        let any_resource = Any {
            type_url: "type.googleapis.com/envoy.config.listener.v3.Listener".to_string(),
            value: encoded,
        };

        Ok(vec![any_resource])
    }

    /// Create basic endpoint resources
    fn create_endpoint_resources(&self) -> Result<Vec<Any>> {
        let resources = &self.state.config.resources;
        let cluster_load_assignment = ClusterLoadAssignment {
            cluster_name: resources.cluster_name.clone(),
            endpoints: vec![LocalityLbEndpoints {
                lb_endpoints: vec![LbEndpoint {
                    host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                        address: Some(Address {
                            address: Some(envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                SocketAddress {
                                    address: resources.backend_address.clone(),
                                    port_specifier: Some(socket_address::PortSpecifier::PortValue(resources.backend_port.into())),
                                    protocol: 0, // TCP protocol
                                    ..Default::default()
                                }
                            )),
                        }),
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        // Validate by encoding
        let encoded = cluster_load_assignment.encode_to_vec();
        info!(
            "Created endpoint resource, encoded size: {} bytes",
            encoded.len()
        );

        let any_resource = Any {
            type_url: "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment"
                .to_string(),
            value: encoded,
        };

        Ok(vec![any_resource])
    }
}

// Database-enabled implementation
impl DatabaseAggregatedDiscoveryService {
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
                self.create_route_resources()? // Still use config-based for now
            }
            "type.googleapis.com/envoy.config.listener.v3.Listener" => {
                self.create_listener_resources()? // Still use config-based for now
            }
            "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment" => {
                self.create_endpoint_resources()? // Still use config-based for now
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
        let resources = &self.state.config.resources;
        let cluster = Cluster {
            name: resources.cluster_name.clone(),
            connect_timeout: Some(Duration {
                seconds: 5,
                nanos: 0,
            }),
            load_assignment: Some(ClusterLoadAssignment {
                cluster_name: resources.cluster_name.clone(),
                endpoints: vec![LocalityLbEndpoints {
                    lb_endpoints: vec![LbEndpoint {
                        host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                            address: Some(Address {
                                address: Some(envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                    SocketAddress {
                                        address: resources.backend_address.clone(),
                                        port_specifier: Some(socket_address::PortSpecifier::PortValue(resources.backend_port.into())),
                                        protocol: 0,
                                        ..Default::default()
                                    }
                                )),
                            }),
                            ..Default::default()
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        let encoded = cluster.encode_to_vec();
        info!(
            cluster_name = %resources.cluster_name,
            "Created fallback cluster resource from config, encoded size: {} bytes",
            encoded.len()
        );

        let any_resource = Any {
            type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
            value: encoded,
        };

        Ok(vec![any_resource])
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
                                    address: Some(envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                        SocketAddress {
                                            address: host,
                                            port_specifier: Some(socket_address::PortSpecifier::PortValue(port)),
                                            protocol: 0,
                                            ..Default::default()
                                        }
                                    )),
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

    /// Create route resources (still config-based for now)
    fn create_route_resources(&self) -> Result<Vec<Any>> {
        let resources = &self.state.config.resources;
        let route_config = RouteConfiguration {
            name: resources.route_name.clone(),
            virtual_hosts: vec![envoy_types::pb::envoy::config::route::v3::VirtualHost {
                name: "test_virtual_host".to_string(),
                domains: vec!["*".to_string()],
                routes: vec![envoy_types::pb::envoy::config::route::v3::Route {
                    name: "test_route_match".to_string(),
                    r#match: Some(envoy_types::pb::envoy::config::route::v3::RouteMatch {
                        path_specifier: Some(envoy_types::pb::envoy::config::route::v3::route_match::PathSpecifier::Prefix("/".to_string())),
                        ..Default::default()
                    }),
                    action: Some(envoy_types::pb::envoy::config::route::v3::route::Action::Route(
                        envoy_types::pb::envoy::config::route::v3::RouteAction {
                            cluster_specifier: Some(envoy_types::pb::envoy::config::route::v3::route_action::ClusterSpecifier::Cluster(resources.cluster_name.clone())),
                            ..Default::default()
                        }
                    )),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let encoded = route_config.encode_to_vec();
        info!(
            "Created route resource, encoded size: {} bytes",
            encoded.len()
        );

        let any_resource = Any {
            type_url: "type.googleapis.com/envoy.config.route.v3.RouteConfiguration".to_string(),
            value: encoded,
        };

        Ok(vec![any_resource])
    }

    /// Create listener resources (still config-based for now)
    fn create_listener_resources(&self) -> Result<Vec<Any>> {
        use envoy_types::pb::envoy::config::listener::v3::Filter;
        use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpConnectionManager;

        let resources = &self.state.config.resources;

        let http_conn_manager = HttpConnectionManager {
            stat_prefix: "ingress_http".to_string(),
            route_specifier: Some(envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::RouteSpecifier::Rds(
                envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::Rds {
                    config_source: Some(envoy_types::pb::envoy::config::core::v3::ConfigSource {
                        config_source_specifier: Some(envoy_types::pb::envoy::config::core::v3::config_source::ConfigSourceSpecifier::Ads(
                            envoy_types::pb::envoy::config::core::v3::AggregatedConfigSource::default()
                        )),
                        ..Default::default()
                    }),
                    route_config_name: resources.route_name.clone(),
                }
            )),
            ..Default::default()
        };

        let listener = Listener {
            name: resources.listener_name.clone(),
            address: Some(Address {
                address: Some(envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                    SocketAddress {
                        address: "0.0.0.0".to_string(),
                        port_specifier: Some(socket_address::PortSpecifier::PortValue(resources.listener_port.into())),
                        protocol: 0,
                        ..Default::default()
                    }
                )),
            }),
            filter_chains: vec![envoy_types::pb::envoy::config::listener::v3::FilterChain {
                filters: vec![Filter {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    config_type: Some(envoy_types::pb::envoy::config::listener::v3::filter::ConfigType::TypedConfig(
                        Any {
                            type_url: "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager".to_string(),
                            value: http_conn_manager.encode_to_vec(),
                        }
                    )),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let encoded = listener.encode_to_vec();
        info!(
            "Created listener resource, encoded size: {} bytes",
            encoded.len()
        );

        let any_resource = Any {
            type_url: "type.googleapis.com/envoy.config.listener.v3.Listener".to_string(),
            value: encoded,
        };

        Ok(vec![any_resource])
    }

    /// Create endpoint resources (still config-based for now)
    fn create_endpoint_resources(&self) -> Result<Vec<Any>> {
        let resources = &self.state.config.resources;
        let cluster_load_assignment = ClusterLoadAssignment {
            cluster_name: resources.cluster_name.clone(),
            endpoints: vec![LocalityLbEndpoints {
                lb_endpoints: vec![LbEndpoint {
                    host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(Endpoint {
                        address: Some(Address {
                            address: Some(envoy_types::pb::envoy::config::core::v3::address::Address::SocketAddress(
                                SocketAddress {
                                    address: resources.backend_address.clone(),
                                    port_specifier: Some(socket_address::PortSpecifier::PortValue(resources.backend_port.into())),
                                    protocol: 0,
                                    ..Default::default()
                                }
                            )),
                        }),
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let encoded = cluster_load_assignment.encode_to_vec();
        info!(
            "Created endpoint resource, encoded size: {} bytes",
            encoded.len()
        );

        let any_resource = Any {
            type_url: "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment"
                .to_string(),
            value: encoded,
        };

        Ok(vec![any_resource])
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

/// Start the minimal xDS gRPC server with configuration and graceful shutdown
/// This implements a basic ADS server that responds with actual Envoy resources
pub async fn start_minimal_xds_server_with_config<F>(
    simple_config: SimpleXdsConfig,
    shutdown_signal: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let addr = format!("{}:{}", simple_config.bind_address, simple_config.port)
        .parse()
        .map_err(|e| crate::Error::config(format!("Invalid xDS address: {}", e)))?;

    let state = Arc::new(XdsState::new(simple_config));

    info!(
        address = %addr,
        "Starting minimal Envoy xDS server (Checkpoint 3)"
    );

    // Create ADS service implementation
    let ads_service = MinimalAggregatedDiscoveryService::new(state);

    // Build and start the gRPC server with ADS service only
    // This serves actual Envoy resources (clusters, routes, listeners, endpoints)
    let server = Server::builder()
        .add_service(AggregatedDiscoveryServiceServer::new(ads_service))
        .serve_with_shutdown(addr, shutdown_signal);

    info!("XDS server listening on {}", addr);

    // Start the server with graceful shutdown
    server
        .await
        .map_err(|e| {
            // Check if this is a port binding error
            let error_msg = e.to_string();
            if error_msg.contains("Address already in use") || error_msg.contains("bind") {
                crate::Error::transport(format!(
                    "XDS server failed to bind to {}: Port {} is already in use. Please use a different port or stop the existing service.",
                    addr, addr.port()
                ))
            } else {
                crate::Error::transport(format!("XDS server failed: {}", e))
            }
        })?;

    Ok(())
}

/// Start database-enabled xDS server
pub async fn start_database_xds_server_with_config<F>(
    simple_config: SimpleXdsConfig,
    pool: DbPool,
    shutdown_signal: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let addr = format!("{}:{}", simple_config.bind_address, simple_config.port)
        .parse()
        .map_err(|e| crate::Error::config(format!("Invalid xDS address: {}", e)))?;

    let state = Arc::new(XdsState::with_database(simple_config, pool));

    info!(
        address = %addr,
        "Starting database-enabled Envoy xDS server (Checkpoint 5)"
    );

    // Create database-enabled ADS service implementation
    let ads_service = DatabaseAggregatedDiscoveryService::new(state);

    // Build and start the gRPC server with database-backed ADS service
    let server = Server::builder()
        .add_service(AggregatedDiscoveryServiceServer::new(ads_service))
        .serve_with_shutdown(addr, shutdown_signal);

    info!("Database-enabled XDS server listening on {}", addr);

    // Start the server with graceful shutdown
    server
        .await
        .map_err(|e| {
            // Check if this is a port binding error
            let error_msg = e.to_string();
            if error_msg.contains("Address already in use") || error_msg.contains("bind") {
                crate::Error::transport(format!(
                    "XDS server failed to bind to {}: Port {} is already in use. Please use a different port or stop the existing service.",
                    addr, addr.port()
                ))
            } else {
                crate::Error::transport(format!("XDS server failed: {}", e))
            }
        })?;

    Ok(())
}

/// Legacy function for backward compatibility - kept for existing tests
/// This will be removed in future checkpoints
pub async fn start_minimal_xds_server() -> Result<()> {
    let simple_config = SimpleXdsConfig::default();
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C signal handler");
    };
    start_minimal_xds_server_with_config(simple_config, shutdown_signal).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::XdsConfig;

    #[test]
    fn test_xds_config_default() {
        let config = XdsConfig::default();
        assert_eq!(config.bind_address(), "0.0.0.0:18000");
        assert_eq!(config.port, 18000);
    }

    #[test]
    fn test_xds_state_versioning() {
        let state = XdsState::new(SimpleXdsConfig::default());
        assert_eq!(state.get_version(), "1");

        state.increment_version();
        assert_eq!(state.get_version(), "2");
    }

    #[tokio::test]
    async fn test_minimal_ads_service_creation() {
        let simple_config = SimpleXdsConfig::default();
        let state = Arc::new(XdsState::new(simple_config));
        let _service = MinimalAggregatedDiscoveryService::new(state);

        // Basic test that service can be created
        assert!(true); // Placeholder - in real tests we'd test the service methods
    }
}
