use std::sync::Arc;

use flowplane::{
    api::start_api_server,
    config::{ApiServerConfig, DatabaseConfig, ObservabilityConfig, SimpleXdsConfig},
    observability::init_observability,
    openapi::defaults::ensure_default_gateway_resources,
    storage::create_pool,
    xds::{start_database_xds_server_with_state, XdsState},
    Config, Result, APP_NAME, VERSION,
};
use tokio::signal;
use tokio::try_join;
use tracing::{error, info};

fn install_rustls_provider() {
    use rustls::crypto::{ring, CryptoProvider};

    if CryptoProvider::get_default().is_none() {
        ring::default_provider()
            .install_default()
            .expect("install ring crypto provider");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    install_rustls_provider();

    let observability_config = ObservabilityConfig::from_env();
    let _health_checker = init_observability(&observability_config).await?;

    info!(
        app_name = APP_NAME,
        version = VERSION,
        "Starting Flowplane Envoy Control Plane - Checkpoint 5: Storage Foundation"
    );

    // Load configuration from environment variables
    let config = Config::from_env()?;
    info!(
        xds_port = config.xds.port,
        xds_bind_address = %config.xds.bind_address,
        metrics_enabled = %observability_config.enable_metrics,
        tracing_enabled = %observability_config.enable_tracing,
        "Loaded configuration from environment"
    );

    // Initialize database configuration and pool
    let db_config = DatabaseConfig::from_env();
    let db_kind = if db_config.is_sqlite() { "sqlite" } else { "database" };
    info!(database = db_kind, "Creating database connection pool");
    let pool = create_pool(&db_config).await?;

    // Create shutdown signal handler
    let simple_xds_config: SimpleXdsConfig = config.xds.clone();
    let api_config: ApiServerConfig = config.api.clone();

    let state = Arc::new(XdsState::with_database(simple_xds_config.clone(), pool));

    ensure_default_gateway_resources(&state).await?;

    let xds_state = state.clone();
    let xds_task = async move {
        start_database_xds_server_with_state(xds_state, async {
            signal::ctrl_c().await.expect("Failed to install CTRL+C signal handler");
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
