//! Server startup, observability initialization, graceful shutdown.

use anyhow::Context;
use fp_core::config::{LogFormat, ServerConfig};
use metrics_exporter_prometheus::PrometheusBuilder;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub async fn run() -> anyhow::Result<()> {
    let config = load_config()?;
    let otel_provider = init_tracing(&config)?;
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

    if let Some(provider) = otel_provider {
        if let Err(e) = provider.shutdown() {
            tracing::warn!("OTel provider shutdown reported an error: {e}");
        }
    }
    tracing::info!("server stopped cleanly");
    Ok(())
}

pub async fn migrate_only() -> anyhow::Result<()> {
    let config = load_config()?;
    let _otel_provider = init_tracing(&config)?;
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

/// Initialize logging + tracing per spec/10 §8a. Returns the OTel provider (if exporting)
/// so shutdown can flush spans.
fn init_tracing(
    config: &ServerConfig,
) -> anyhow::Result<Option<opentelemetry_sdk::trace::SdkTracerProvider>> {
    use tracing_subscriber::Layer;

    // W3C trace-context propagation is always on: inbound traceparent headers are honored
    // even when span export is disabled.
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    let filter = EnvFilter::try_new(&config.log_filter)
        .with_context(|| format!("invalid FLOWPLANE_LOG filter: {}", config.log_filter))?;

    // The OTel layer is always installed so every request gets a real trace context
    // (trace_id in logs, caller traceparent honored even without a collector). Without an
    // OTLP endpoint the provider has no exporter: spans are recorded locally and dropped.
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig;
    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name("flowplane")
        .build();
    let builder = opentelemetry_sdk::trace::SdkTracerProvider::builder().with_resource(resource);
    let provider = match &config.otlp_endpoint {
        Some(endpoint) => {
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint.clone())
                .build()
                .with_context(|| format!("failed to build OTLP exporter for {endpoint}"))?;
            builder.with_batch_exporter(exporter).build()
        }
        None => builder.build(),
    };
    let tracer = provider.tracer("flowplane");
    opentelemetry::global::set_tracer_provider(provider.clone());
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let provider = Some(provider);

    let fmt_layer = match config.log_format {
        LogFormat::Json => tracing_subscriber::fmt::layer().json().boxed(),
        LogFormat::Pretty => tracing_subscriber::fmt::layer().pretty().boxed(),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(otel_layer)
        .with(fmt_layer)
        .try_init()
        .map_err(|e| anyhow::anyhow!("failed to initialize tracing: {e}"))?;

    if let Some(endpoint) = &config.otlp_endpoint {
        tracing::info!(endpoint = %endpoint, "OTLP trace export enabled");
    }
    Ok(provider)
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
