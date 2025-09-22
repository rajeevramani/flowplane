use magaya::{Config, Result, APP_NAME, VERSION};
use tokio::signal;
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
        "Starting Magaya Envoy Control Plane - Checkpoint 3: Basic Envoy Resource Types"
    );

    // Load configuration from environment variables
    let config = Config::from_env()?;
    info!(
        xds_port = config.xds.port,
        xds_bind_address = %config.xds.bind_address,
        "Loaded configuration from environment"
    );

    // Create shutdown signal handler
    let shutdown_signal = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C signal handler");
        info!("Shutdown signal received");
    };

    // Start the XDS server with configuration and graceful shutdown
    if let Err(e) =
        magaya::xds::start_minimal_xds_server_with_config(config.xds, shutdown_signal).await
    {
        error!("Failed to start XDS server: {}", e);
        std::process::exit(1);
    }

    info!("XDS server shutdown completed");
    Ok(())
}
