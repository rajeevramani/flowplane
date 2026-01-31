//! Envoy xDS (eXtended Discovery Service) implementation
//!
//! Provides gRPC server implementing Envoy's discovery protocols:
//! - ADS (Aggregated Discovery Service)
//! - CDS (Cluster Discovery Service)
//! - RDS (Route Discovery Service)
//! - LDS (Listener Discovery Service)

pub mod access_log;
pub mod cluster;
mod cluster_spec;
pub mod filters;
pub mod helpers;
pub mod listener;
pub(crate) mod resources;
pub mod route;
pub mod secret;
pub mod services;
mod state;

use crate::observability::GrpcTracingLayer;
use crate::{config::SimpleXdsConfig, storage::DbPool, Result};
use std::future::Future;
use std::sync::Arc;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tracing::info;

use envoy_types::pb::envoy::service::accesslog::v3::access_log_service_server::AccessLogServiceServer;
use envoy_types::pb::envoy::service::discovery::v3::aggregated_discovery_service_server::AggregatedDiscoveryServiceServer;
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::ExternalProcessorServer;

pub use cluster_spec::*;
pub use services::{
    DatabaseAggregatedDiscoveryService, FlowplaneAccessLogService, FlowplaneExtProcService,
    MinimalAggregatedDiscoveryService,
};
pub use state::XdsState;

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
    let ads_service = MinimalAggregatedDiscoveryService::new(state.clone());

    // Create AccessLogService for receiving Envoy access logs
    // Note: log_rx is not used in minimal xDS mode. In full mode (start_database_xds_server),
    // the log receiver is wired to the learning infrastructure for schema inference.
    let (access_log_service, _log_rx) = FlowplaneAccessLogService::new();

    // Create ExtProcService for body capture
    let (ext_proc_service, _ext_proc_rx) = FlowplaneExtProcService::new();

    // Build and start the gRPC server with ADS service, AccessLogService, and ExtProcService
    // This serves actual Envoy resources (clusters, routes, listeners, endpoints)
    let server_builder = configure_server_builder(Server::builder(), &state.config)?;

    // Apply gRPC tracing layer for automatic instrumentation
    let server = server_builder
        .layer(GrpcTracingLayer::new())
        .add_service(AggregatedDiscoveryServiceServer::new(ads_service))
        .add_service(AccessLogServiceServer::new(access_log_service))
        .add_service(ExternalProcessorServer::new(ext_proc_service))
        .serve_with_shutdown(addr, shutdown_signal);

    info!(
        address = %addr,
        "XDS server with AccessLogService, ExtProcService and tracing listening"
    );

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
    let state = Arc::new(XdsState::with_database(simple_config, pool));
    start_database_xds_server_with_state(state, shutdown_signal).await
}

/// Start database-enabled xDS server with a pre-built shared state
pub async fn start_database_xds_server_with_state<F>(
    state: Arc<XdsState>,
    shutdown_signal: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let addr = {
        let cfg = &state.config;
        format!("{}:{}", cfg.bind_address, cfg.port)
            .parse()
            .map_err(|e| crate::Error::config(format!("Invalid xDS address: {}", e)))?
    };

    info!(
        address = %addr,
        "Starting database-enabled Envoy xDS server (Checkpoint 5)"
    );

    let ads_service = DatabaseAggregatedDiscoveryService::new(state.clone());

    let server_builder = configure_server_builder(Server::builder(), &state.config)?;

    // Use the AccessLogService from state if available, otherwise create a new one
    let access_log_service = if let Some(service) = &state.access_log_service {
        Arc::clone(service)
    } else {
        let (service, _log_rx) = FlowplaneAccessLogService::new();
        Arc::new(service)
    };

    // Use the ExtProcService from state if available, otherwise create a new one
    let ext_proc_service = if let Some(service) = &state.ext_proc_service {
        Arc::clone(service)
    } else {
        let (service, _ext_proc_rx) = FlowplaneExtProcService::new();
        Arc::new(service)
    };

    // Apply gRPC tracing layer for automatic instrumentation
    let server = server_builder
        .layer(GrpcTracingLayer::new())
        .add_service(AggregatedDiscoveryServiceServer::new(ads_service))
        .add_service(AccessLogServiceServer::new(
            // Clone the service (shares the inner Arc<RwLock<...>> with learning session service)
            (*access_log_service).clone(),
        ))
        .add_service(ExternalProcessorServer::new(
            // Clone the service (shares the inner Arc<RwLock<...>> for body capture state)
            (*ext_proc_service).clone(),
        ))
        .serve_with_shutdown(addr, shutdown_signal);

    info!(
        address = %addr,
        "Database-enabled XDS server with AccessLogService, ExtProcService and tracing listening"
    );

    server
        .await
        .map_err(|e| {
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

fn configure_server_builder(mut builder: Server, config: &SimpleXdsConfig) -> Result<Server> {
    if let Some(tls_config) = build_server_tls_config(config)? {
        builder = builder.tls_config(tls_config).map_err(|e| {
            crate::Error::transport(format!("Failed to apply xDS TLS configuration: {}", e))
        })?;

        if let Some(tls) = &config.tls {
            info!(
                require_client_cert = tls.require_client_cert,
                has_client_ca = tls.client_ca_path.is_some(),
                "xDS server TLS enabled"
            );
        }
    }

    Ok(builder)
}

fn build_server_tls_config(config: &SimpleXdsConfig) -> Result<Option<ServerTlsConfig>> {
    let tls = match &config.tls {
        Some(tls) => tls,
        None => return Ok(None),
    };

    let cert_bytes = std::fs::read(&tls.cert_path).map_err(|e| {
        crate::Error::config(format!(
            "Failed to read xDS TLS certificate from '{}': {}",
            tls.cert_path, e
        ))
    })?;

    let key_bytes = std::fs::read(&tls.key_path).map_err(|e| {
        crate::Error::config(format!(
            "Failed to read xDS TLS private key from '{}': {}",
            tls.key_path, e
        ))
    })?;

    let identity = Identity::from_pem(cert_bytes, key_bytes);

    let mut server_tls_config = ServerTlsConfig::new().identity(identity);

    if let Some(ca_path) = &tls.client_ca_path {
        let ca_bytes = std::fs::read(ca_path).map_err(|e| {
            crate::Error::config(format!(
                "Failed to read xDS client CA certificate from '{}': {}",
                ca_path, e
            ))
        })?;

        let client_ca = Certificate::from_pem(ca_bytes);

        server_tls_config = server_tls_config.client_ca_root(client_ca);

        if !tls.require_client_cert {
            server_tls_config = server_tls_config.client_auth_optional(true);
        }
    } else if tls.require_client_cert {
        return Err(crate::Error::config(
            "Client certificate verification is enabled but no client CA path is configured",
        ));
    }

    Ok(Some(server_tls_config))
}

/// Legacy function for backward compatibility - kept for existing tests
/// This will be removed in future checkpoints
pub async fn start_minimal_xds_server() -> Result<()> {
    let simple_config = SimpleXdsConfig::default();
    let shutdown_signal = async {
        tokio::signal::ctrl_c().await.expect("Failed to install CTRL+C signal handler");
    };
    start_minimal_xds_server_with_config(simple_config, shutdown_signal).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::XdsConfig;
    use std::sync::Arc;

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

        use crate::xds::resources::BuiltResource;
        use envoy_types::pb::google::protobuf::Any;

        let _ = state.apply_built_resources(
            crate::xds::resources::CLUSTER_TYPE_URL,
            vec![BuiltResource {
                name: "test".to_string(),
                resource: Any {
                    type_url: crate::xds::resources::CLUSTER_TYPE_URL.to_string(),
                    value: vec![1, 2, 3],
                },
            }],
        );
        assert_eq!(state.get_version(), "2");
    }

    #[tokio::test]
    async fn test_minimal_ads_service_creation() {
        let simple_config = SimpleXdsConfig::default();
        let state = Arc::new(XdsState::new(simple_config));
        assert_eq!(Arc::strong_count(&state), 1);
        let _service = MinimalAggregatedDiscoveryService::new(state.clone());

        // Creation should increase the reference count, proving the state is retained.
        assert!(Arc::strong_count(&state) >= 2);
    }
}
