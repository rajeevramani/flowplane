//! flowplane-rls binary: starts the Envoy-facing gRPC `RateLimitService` and the CP-facing HTTP
//! admin server, sharing one in-RAM policy set and counter store.

use std::sync::Arc;

use envoy_types::pb::envoy::service::ratelimit::v3::rate_limit_service_server::RateLimitServiceServer;
use flowplane_rls::admin::{self, AdminState};
use flowplane_rls::config::RlsConfig;
use flowplane_rls::counter::InMemoryFixedWindow;
use flowplane_rls::grpc::{GrpcAuthMode, RlsService};
use flowplane_rls::policy::PolicyCache;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = RlsConfig::from_env()?;
    let policies = Arc::new(PolicyCache::new());
    let counters = Arc::new(InMemoryFixedWindow::new());
    let service = RlsService::new(
        Arc::clone(&policies),
        counters,
        GrpcAuthMode::InsecureDevOnly,
    );

    let admin_router = admin::router(AdminState {
        policies: Arc::clone(&policies),
        credential: config.admin_credential.clone(),
    });
    let admin_listener = tokio::net::TcpListener::bind(config.admin_listen).await?;

    tracing::info!(
        grpc = %config.grpc_listen,
        admin = %config.admin_listen,
        "flowplane-rls starting"
    );

    let admin_handle = tokio::spawn(async move {
        if let Err(error) = axum::serve(admin_listener, admin_router).await {
            tracing::error!(%error, "admin server stopped");
        }
    });

    let result = flowplane_rls::server::grpc_server(&config)?
        .add_service(RateLimitServiceServer::new(service))
        .serve(config.grpc_listen)
        .await;

    admin_handle.abort();
    result?;
    Ok(())
}
