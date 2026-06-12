//! Server startup, observability initialization, graceful shutdown.

use anyhow::Context;
use fp_core::config::{LogFormat, ServerConfig};
use metrics_exporter_prometheus::PrometheusBuilder;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub async fn run() -> anyhow::Result<()> {
    let config = load_config()?;
    init_tracing(&config)?;
    install_crypto_provider()?;

    if config.api_insecure {
        tracing::warn!(
            "API listener is serving PLAINTEXT (FLOWPLANE_API_INSECURE=true) — only acceptable \
             behind a TLS-terminating proxy or in local development"
        );
    }

    let pool = fp_storage::connect(&config.database_url, config.db_max_connections).await?;
    fp_storage::migrate(&pool).await?;
    tracing::info!("database connected and migrations applied");

    let prometheus = PrometheusBuilder::new()
        .install_recorder()
        .context("failed to install Prometheus metrics recorder")?;

    let state = fp_api::AppState {
        pool,
        prometheus,
        version: crate::VERSION,
    };
    let router = fp_api::build_router(state);

    tracing::info!(addr = %config.api_addr, tls = config.api_tls.is_some(), "API listener starting");

    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutdown signal received; draining connections");
        shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
    });

    match &config.api_tls {
        Some(tls) => {
            let rustls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert_path, &tls.key_path)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to load TLS material (cert: {}, key: {})",
                            tls.cert_path.display(),
                            tls.key_path.display()
                        )
                    })?;
            axum_server::bind_rustls(config.api_addr, rustls_config)
                .handle(handle)
                .serve(router.into_make_service())
                .await?;
        }
        None => {
            axum_server::bind(config.api_addr)
                .handle(handle)
                .serve(router.into_make_service())
                .await?;
        }
    }

    tracing::info!("server stopped cleanly");
    Ok(())
}

pub async fn migrate_only() -> anyhow::Result<()> {
    let config = load_config()?;
    init_tracing(&config)?;
    let pool = fp_storage::connect(&config.database_url, 2).await?;
    fp_storage::migrate(&pool).await?;
    tracing::info!("migrations applied");
    Ok(())
}

fn load_config() -> anyhow::Result<ServerConfig> {
    ServerConfig::load().map_err(|e| {
        // Render the config error with its hint — the operator's first contact with our
        // error style is a misconfigured server.
        let hint = e
            .hint
            .clone()
            .map(|h| format!("\n  -> {h}"))
            .unwrap_or_default();
        anyhow::anyhow!("{e}{hint}")
    })
}

fn init_tracing(config: &ServerConfig) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(&config.log_filter)
        .with_context(|| format!("invalid FLOWPLANE_LOG filter: {}", config.log_filter))?;
    let registry = tracing_subscriber::registry().with(filter);
    match config.log_format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .try_init(),
        LogFormat::Pretty => registry
            .with(tracing_subscriber::fmt::layer().pretty())
            .try_init(),
    }
    .map_err(|e| anyhow::anyhow!("failed to initialize tracing: {e}"))
}

fn install_crypto_provider() -> anyhow::Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .map_err(|_| anyhow::anyhow!("failed to install rustls ring crypto provider"))?;
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::error!("failed to listen for ctrl-c; shutdown via signal unavailable");
        }
    };
    #[cfg(unix)]
    {
        let mut sigterm =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("failed to register SIGTERM handler: {e}");
                    ctrl_c.await;
                    return;
                }
            };
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
}
