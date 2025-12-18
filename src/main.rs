use std::sync::Arc;

use flowplane::{
    api::start_api_server,
    auth::scope_registry::init_scope_registry,
    config::{ApiServerConfig, DatabaseConfig, ObservabilityConfig, SimpleXdsConfig},
    observability::init_observability,
    secrets::SecretBackendRegistry,
    services::{LearningSessionService, SchemaAggregator, WebhookService},
    storage::{
        create_pool,
        repositories::{
            AggregatedSchemaRepository, InferredSchemaRepository, LearningSessionRepository,
        },
    },
    xds::{
        services::{
            access_log_service::FlowplaneAccessLogService,
            ext_proc_service::FlowplaneExtProcService,
        },
        start_database_xds_server_with_state, XdsState,
    },
    Config, Result, APP_NAME, VERSION,
};
use tokio::signal;
use tokio::try_join;
use tracing::{error, info, warn};

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

    // Handle first-time startup: auto-generate setup token if needed
    flowplane::startup::handle_first_time_startup(pool.clone()).await?;

    // Initialize scope registry for scope validation
    init_scope_registry(pool.clone()).await?;
    info!("Scope registry initialized");

    // Check mTLS configuration status
    if flowplane::secrets::PkiConfig::is_mtls_enabled() {
        let pki_config = flowplane::secrets::PkiConfig::from_env()
            .expect("PKI config should be available when mTLS is enabled");
        info!(
            pki_mount = %pki_config.mount_path,
            pki_role = %pki_config.role_name,
            trust_domain = %pki_config.trust_domain,
            "mTLS enabled - proxies will be authenticated via client certificates"
        );
    } else {
        tracing::warn!(
            "mTLS disabled - FLOWPLANE_VAULT_PKI_MOUNT_PATH not configured. \
             Proxies will not be authenticated. This is insecure for production use."
        );
    }

    // Create shutdown signal handler
    let simple_xds_config: SimpleXdsConfig = config.xds.clone();
    let api_config: ApiServerConfig = config.api.clone();

    // Create Access Log Service for learning sessions and wire repository + processor
    let (access_log_service, log_rx) = FlowplaneAccessLogService::new();
    // Attach repository so ALS can increment sample counts for sessions
    let als_session_repo = LearningSessionRepository::new(pool.clone());
    let access_log_service = Arc::new(access_log_service.with_repository(als_session_repo));

    // Create External Processor Service for request/response body capture during learning sessions
    let (ext_proc_service, ext_proc_rx) = FlowplaneExtProcService::new();
    let ext_proc_service = Arc::new(ext_proc_service);

    // Create Webhook Service for learning session event notifications
    let (webhook_service, _webhook_rx) = WebhookService::new();
    let webhook_service = Arc::new(webhook_service);

    // Create Schema Aggregator for learning session completion
    let inferred_schema_repo = InferredSchemaRepository::new(pool.clone());
    let aggregated_schema_repo = AggregatedSchemaRepository::new(pool.clone());
    let schema_aggregator =
        Arc::new(SchemaAggregator::new(inferred_schema_repo, aggregated_schema_repo));

    // Create XdsState first (without LearningSessionService) to resolve circular dependency
    // We initialize with access log and ext proc services directly
    let mut state_struct = XdsState::with_database(simple_xds_config.clone(), pool.clone());
    state_struct.access_log_service = Some(access_log_service.clone());
    state_struct.ext_proc_service = Some(ext_proc_service.clone());

    // Initialize secret backend registry for external secrets (Vault, AWS, GCP)
    if let Some(encryption) = state_struct.encryption_service.clone() {
        let cache_ttl =
            std::env::var("FLOWPLANE_SECRET_CACHE_TTL_SECS").ok().and_then(|s| s.parse().ok());

        match SecretBackendRegistry::from_env(pool.clone(), Some(encryption), cache_ttl).await {
            Ok(registry) => {
                info!(
                    backends = ?registry.registered_backends(),
                    "Initialized secret backend registry"
                );
                state_struct.set_secret_backend_registry(registry);
            }
            Err(e) => {
                warn!(error = %e, "Failed to initialize secret backend registry - external secrets will not work");
            }
        }
    }

    let state = Arc::new(state_struct);

    // Create LearningSessionService with XdsState (Weak reference)
    let learning_session_repo = LearningSessionRepository::new(pool.clone());
    let learning_session_service = LearningSessionService::new(learning_session_repo)
        .with_access_log_service(access_log_service.clone())
        .with_ext_proc_service(ext_proc_service.clone())
        .with_webhook_service(webhook_service.clone())
        .with_schema_aggregator(schema_aggregator.clone())
        .with_xds_state(state.clone());
    let learning_session_service = Arc::new(learning_session_service);

    // Update XdsState with LearningSessionService using safe interior mutability
    state.set_learning_session_service(learning_session_service.clone());

    // Start background Access Log Processor to handle inference + persistence
    // Wire ExtProc body channel for request/response body capture (Task 12.3)
    {
        use flowplane::services::AccessLogProcessor;
        let db_pool = Some(pool.clone());
        let processor = AccessLogProcessor::new(log_rx, Some(ext_proc_rx), db_pool, None);
        // Spawn workers (handles internal batching and metrics). We don't need the handle here.
        let _handle = processor.spawn_workers();
    }

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
