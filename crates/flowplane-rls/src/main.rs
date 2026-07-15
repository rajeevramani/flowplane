//! flowplane-rls binary: starts the Envoy-facing gRPC `RateLimitService` and the CP-facing HTTP
//! admin server, sharing one in-RAM policy set and counter store.

use std::sync::Arc;

use envoy_types::pb::envoy::service::ratelimit::v3::rate_limit_service_server::RateLimitServiceServer;
use flowplane_rls::admin::{self, AdminState};
use flowplane_rls::config::RlsConfig;
use flowplane_rls::counter::InMemoryFixedWindow;
use flowplane_rls::grpc::RlsService;
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
    let service = RlsService::new(Arc::clone(&policies), counters);

    let admin_router = admin::router(AdminState {
        policies: Arc::clone(&policies),
        credential: config.admin_credential.clone().map(Arc::new),
    });

    tracing::info!(
        grpc = %config.grpc_listen,
        grpc_security = if config.grpc_tls.is_some() { "mtls" } else { "plaintext (loopback dev)" },
        admin = %config.admin_listen,
        admin_security = if config.admin_tls.is_some() { "https + bearer" } else { "plaintext, unauthenticated (loopback dev)" },
        "flowplane-rls starting"
    );

    let admin_handle = match &config.admin_tls {
        Some(tls) => {
            let rustls_config = flowplane_rls::server::admin_rustls_config(tls).await?;
            let listener = std::net::TcpListener::bind(config.admin_listen)?;
            listener.set_nonblocking(true)?;
            tokio::spawn(async move {
                if let Err(error) = axum_server::from_tcp_rustls(listener, rustls_config)
                    .serve(admin_router.into_make_service())
                    .await
                {
                    tracing::error!(%error, "admin server stopped");
                }
            })
        }
        None => {
            let listener = tokio::net::TcpListener::bind(config.admin_listen).await?;
            tokio::spawn(async move {
                if let Err(error) = axum::serve(listener, admin_router).await {
                    tracing::error!(%error, "admin server stopped");
                }
            })
        }
    };

    let result = flowplane_rls::server::grpc_server(&config)?
        .add_service(RateLimitServiceServer::new(service))
        .serve(config.grpc_listen)
        .await;

    admin_handle.abort();
    result?;
    Ok(())
}
