use std::sync::Arc;

use flowplane::{
    api::start_api_server,
    config::{ApiServerConfig, DatabaseConfig, ObservabilityConfig, SimpleXdsConfig},
    observability::init_observability,
    openapi::defaults::ensure_default_gateway_resources,
    services::LearningSessionService,
    storage::{create_pool, repositories::LearningSessionRepository},
    xds::{
        services::access_log_service::FlowplaneAccessLogService,
        start_database_xds_server_with_state, XdsState,
    },
    Config, Result, APP_NAME, VERSION,
};
use tokio::signal;
use tokio::try_join;
use tracing::{error, info};

fn install_rustls_provider() {
    use rustls::crypto::{ring, CryptoProvider};

    if CryptoProvider::get_default().is_none() {
        ring::default_provider().install_default().expect("install ring crypto provider");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    install_rustls_provider();

    // Load .env file if it exists (optional - won't fail if missing)
    // This must happen before any config is read from environment
    if let Err(e) = dotenvy::dotenv() {
        // Only warn if the error is NOT "file not found"
        if !e.to_string().contains("not found") {
            eprintln!("Warning: Error loading .env file: {}", e);
        }
    }

    let observability_config = ObservabilityConfig::from_env();
    let (_health_checker, tracer_provider) = init_observability(&observability_config).await?;

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

    // Create Access Log Service for learning sessions
    let (access_log_service, _log_rx) = FlowplaneAccessLogService::new();
    let access_log_service = Arc::new(access_log_service);

    // Create Learning Session Service with Access Log Service integration
    let learning_session_repo = LearningSessionRepository::new(pool.clone());
    let learning_session_service = LearningSessionService::new(learning_session_repo)
        .with_access_log_service(access_log_service.clone());
    let learning_session_service = Arc::new(learning_session_service);

    // Create XdsState with services
    let state = Arc::new(
        XdsState::with_database(simple_xds_config.clone(), pool)
            .with_services(access_log_service.clone(), learning_session_service.clone()),
    );

    // Spawn background worker for learning session auto-completion
    let learning_session_service_bg = learning_session_service.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            match learning_session_service_bg.check_all_active_sessions().await {
                Ok(completed) if !completed.is_empty() => {
                    info!(
                        count = completed.len(),
                        sessions = ?completed,
                        "Background worker completed learning sessions"
                    );
                }
                Ok(_) => {} // No sessions completed
                Err(e) => {
                    error!(
                        error = %e,
                        "Background worker failed to check active sessions"
                    );
                }
            }
        }
    });

    // Sync existing active sessions with Access Log Service on startup
    if let Err(e) = learning_session_service.sync_active_sessions_with_access_log_service().await {
        error!(error = %e, "Failed to sync active sessions with Access Log Service on startup");
    }

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

    // Shutdown OpenTelemetry tracer provider to flush any pending spans
    // Must use spawn_blocking to avoid deadlock with tokio runtime
    if let Some(provider) = tracer_provider {
        info!("Flushing OpenTelemetry traces before shutdown");
        if let Err(e) = tokio::task::spawn_blocking(move || provider.shutdown()).await {
            error!("Error shutting down OpenTelemetry tracer provider: {}", e);
        } else {
            info!("OpenTelemetry tracer provider shutdown completed");
        }
    }

    info!("Control plane shutdown completed");
    Ok(())
}
