use magaya::{Result, APP_NAME, VERSION};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing for logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "magaya=info,tonic=info".into()),
        )
        .init();

    info!(
        app_name = APP_NAME,
        version = VERSION,
        "Starting Magaya Envoy Control Plane - Checkpoint 1: Minimal XDS Server"
    );

    // Start the XDS server
    if let Err(e) = magaya::xds::start_minimal_xds_server().await {
        error!("Failed to start XDS server: {}", e);
        std::process::exit(1);
    }

    Ok(())
}
