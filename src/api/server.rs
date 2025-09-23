use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::{config::ApiServerConfig, errors::Error, xds::XdsState};

use super::routes::build_router;

pub async fn start_api_server(config: ApiServerConfig, state: Arc<XdsState>) -> crate::Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.bind_address, config.port)
        .parse()
        .map_err(|e| Error::config(format!("Invalid API address: {}", e)))?;

    let router: Router = build_router(state);

    info!(address = %addr, "Starting HTTP API server");

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| Error::transport(format!("Failed to bind API server: {}", e)))?;

    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            if let Err(e) = tokio::signal::ctrl_c().await {
                warn!(error = %e, "API server shutdown listener failed");
            }
        })
        .await
        .map_err(|e| Error::transport(format!("API server error: {}", e)))?;

    info!("API server shutdown completed");
    Ok(())
}
