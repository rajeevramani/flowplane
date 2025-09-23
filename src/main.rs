use std::sync::Arc;

use magaya::{
    api::start_api_server,
    config::{ApiServerConfig, DatabaseConfig, SimpleXdsConfig},
    storage::create_pool,
    xds::{start_database_xds_server_with_state, XdsState},
    Config, Result, APP_NAME, VERSION,
};
use tokio::signal;
use tokio::try_join;
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
        "Starting Magaya Envoy Control Plane - Checkpoint 5: Storage Foundation"
    );

    // Load configuration from environment variables
    let config = Config::from_env()?;
    info!(
        xds_port = config.xds.port,
        xds_bind_address = %config.xds.bind_address,
        "Loaded configuration from environment"
    );

    // Initialize database configuration and pool
    let db_config = DatabaseConfig::from_env();
    let db_kind = if db_config.is_sqlite() {
        "sqlite"
    } else {
        "database"
    };
    info!(database = db_kind, "Creating database connection pool");
    let pool = create_pool(&db_config).await?;

    // Create shutdown signal handler
    let simple_xds_config: SimpleXdsConfig = config.xds.clone();
    let api_config: ApiServerConfig = config.api.clone();

    let state = Arc::new(XdsState::with_database(simple_xds_config.clone(), pool));

    let xds_state = state.clone();
    let xds_task = async move {
        start_database_xds_server_with_state(xds_state, async {
            signal::ctrl_c()
                .await
                .expect("Failed to install CTRL+C signal handler");
            info!("Shutdown signal received for xDS server");
        })
        .await
    };

    let api_state = state.clone();
    let api_task = async move { start_api_server(api_config, api_state).await };

    if let Err(e) = try_join!(xds_task, api_task) {
        error!("Control plane services terminated with error: {}", e);
        std::process::exit(1);
    }

    info!("Control plane shutdown completed");
    Ok(())
}
