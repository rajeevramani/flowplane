//! Server startup, observability initialization, graceful shutdown.

use anyhow::Context;
use fp_core::config::{LogFormat, ServerConfig};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

    let validator: Option<std::sync::Arc<fp_core::OidcValidator>> = if config.dev_mode {
        Some(setup_dev_mode(&pool).await?)
    } else if let Some(oidc) = &config.oidc {
        tracing::info!(issuer = %oidc.issuer, "OIDC authentication enabled");
        Some(std::sync::Arc::new(fp_core::OidcValidator::new(
            fp_core::OidcConfig {
                issuer: oidc.issuer.clone(),
                audience: oidc.audience.clone(),
                jwks_uri: oidc.jwks_uri.clone(),
            },
        )))
    } else {
        tracing::warn!(
            "no OIDC issuer configured and dev mode off — authenticated endpoints return 503"
        );
        None
    };

    let prometheus = PrometheusBuilder::new()
        .install_recorder()
        .context("failed to install Prometheus metrics recorder")?;

    if !config.dev_mode {
        if let Some(token) =
            fp_storage::repos::bootstrap::issue_token_if_uninitialized(&pool).await?
        {
            tracing::warn!(
                bootstrap_token = %token,
                "instance is uninitialized — POST /api/v1/bootstrap/initialize with this \
                 one-shot token (valid 24h, logged only once)"
            );
        }
    }

    // xDS pipeline: snapshot cache primed from the DB (restart safety), then kept fresh by
    // the outbox consumer, which also feeds certificate revocations to live streams.
    let (xds_shutdown_tx, xds_shutdown_rx) = tokio::sync::watch::channel(false);
    let snapshot_cache = fp_xds::snapshot::SnapshotCache::new();
    let xds_consumer_failed = Arc::new(AtomicBool::new(false));
    let primed = snapshot_cache
        .prime_all(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("failed to prime xDS snapshot cache: {e}"))?;
    tracing::info!(teams = primed, "xDS snapshot cache primed from database");
    let (revocation_tx, _) = tokio::sync::broadcast::channel::<uuid::Uuid>(64);
    // Handles for every spawned xDS task; awaited (bounded) on shutdown so streams and the
    // outbox consumer drain rather than being abandoned mid-flight.
    let mut xds_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    {
        let cache = snapshot_cache.clone();
        let consumer_pool = pool.clone();
        let revocations = revocation_tx.clone();
        let consumer_failed = xds_consumer_failed.clone();
        xds_tasks.push(tokio::spawn(async move {
            let handler_pool = consumer_pool.clone();
            let result = fp_storage::outbox::run_consumer(
                consumer_pool,
                fp_xds::snapshot::XDS_CONSUMER,
                move |events| {
                    let cache = cache.clone();
                    let pool = handler_pool.clone();
                    let revocations = revocations.clone();
                    async move {
                        fp_xds::ads::publish_revocations(&revocations, &events);
                        fp_xds::snapshot::handle_events(&cache, &pool, events).await
                    }
                },
                xds_shutdown_rx,
            )
            .await;
            if let Err(e) = result {
                consumer_failed.store(true, Ordering::SeqCst);
                tracing::error!("xds outbox consumer exited with error: {e}");
            }
        }));
    }
    if let Some(xds_tls) = &config.xds_tls {
        // Production path: mandatory mTLS, team identity from the certificate registry.
        let cache = snapshot_cache.clone();
        let xds_addr = config.xds_addr;
        let tls = fp_xds::server::XdsTlsPaths {
            cert_path: xds_tls.cert_path.clone(),
            key_path: xds_tls.key_path.clone(),
            client_ca_path: xds_tls.client_ca_path.clone(),
        };
        let resolver = std::sync::Arc::new(fp_xds::ads::CertRegistryResolver::new(pool.clone()));
        let revocations = revocation_tx.clone();
        let nack_pool = pool.clone();
        let shutdown = xds_shutdown_signal(&xds_shutdown_tx);
        xds_tasks.push(tokio::spawn(async move {
            if let Err(e) = fp_xds::server::serve_mtls(
                xds_addr,
                cache,
                resolver,
                revocations,
                nack_pool,
                &tls,
                shutdown,
            )
            .await
            {
                tracing::error!("xds server exited: {e}");
            }
        }));
    } else if config.dev_mode {
        let cache = snapshot_cache.clone();
        let xds_addr = config.xds_addr;
        let nack_pool = pool.clone();
        let shutdown = xds_shutdown_signal(&xds_shutdown_tx);
        xds_tasks.push(tokio::spawn(async move {
            if let Err(e) = fp_xds::server::serve_plaintext(
                xds_addr,
                cache,
                std::sync::Arc::new(fp_xds::ads::NodeIdTeamResolver),
                Some(nack_pool),
                shutdown,
            )
            .await
            {
                tracing::error!("xds server exited: {e}");
            }
        }));
    } else {
        tracing::warn!(
            "xDS listener disabled: mTLS is mandatory for xDS — set FLOWPLANE_XDS_TLS_CERT, \
             FLOWPLANE_XDS_TLS_KEY, and FLOWPLANE_XDS_TLS_CLIENT_CA to enable it"
        );
    }

    let state = fp_api::AppState {
        pool,
        prometheus,
        version: crate::VERSION,
        validator,
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(
            config.tenant_write_limit_per_minute,
        )),
        xds_readiness: Some(fp_api::state::XdsReadiness {
            consumer: fp_xds::snapshot::XDS_CONSUMER,
            max_lag: 0,
            failed: xds_consumer_failed,
        }),
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

    // Signal the xDS tasks and let them drain (bounded). serve_with_shutdown finishes the
    // in-flight gRPC streams; the outbox consumer exits its loop on the watch flag.
    let _ = xds_shutdown_tx.send(true);
    for task in xds_tasks {
        if tokio::time::timeout(std::time::Duration::from_secs(10), task)
            .await
            .is_err()
        {
            tracing::warn!("an xDS task did not drain within 10s; abandoning it");
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

/// A future that resolves when the xDS shutdown flag flips to `true`. Each xDS server task
/// gets its own receiver so `serve_with_shutdown` can drain its streams on shutdown.
fn xds_shutdown_signal(
    tx: &tokio::sync::watch::Sender<bool>,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    let mut rx = tx.subscribe();
    async move {
        // Wakes on the first `true`; treats a dropped sender as shutdown too.
        let _ = rx.wait_for(|flag| *flag).await;
    }
}

pub async fn migrate_only() -> anyhow::Result<()> {
    let config = load_config()?;
    let _otel_provider = init_tracing(&config)?;
    let pool = fp_storage::connect(&config.database_url, 2).await?;
    fp_storage::migrate(&pool).await?;
    tracing::info!("migrations applied");
    Ok(())
}

/// Dev-mode startup: triple-gated (config flag + build feature + release ack), then seeds
/// dev resources and boots the in-process issuer (spec/10 §4a).
#[cfg(feature = "dev-oidc")]
async fn setup_dev_mode(
    pool: &sqlx::PgPool,
) -> anyhow::Result<std::sync::Arc<fp_core::OidcValidator>> {
    if !cfg!(debug_assertions) {
        let ack = std::env::var("FLOWPLANE_DEV_MODE_ACK").unwrap_or_default();
        if ack != "yes-this-is-not-production" {
            return Err(anyhow::anyhow!(
                "FLOWPLANE_DEV_MODE=true in a release build requires \
                 FLOWPLANE_DEV_MODE_ACK=yes-this-is-not-production"
            ));
        }
    }
    tracing::warn!("DEV MODE: in-process identity, seeded resources — never production");
    fp_storage::seed::seed_dev(pool).await?;
    let issuer = fp_core::dev::DevIssuer::generate()?;
    let token = issuer.mint_dev_user()?;
    // Dev-only by triple gate: the token grants access to the seeded local instance only
    // and dies with this process (per-boot key).
    tracing::warn!(dev_token = %token, "dev bearer token (valid 1h, this boot only)");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .map_err(|e| anyhow::anyhow!("dev jwks load: {e}"))?;
    Ok(std::sync::Arc::new(validator))
}

#[cfg(not(feature = "dev-oidc"))]
async fn setup_dev_mode(
    _pool: &sqlx::PgPool,
) -> anyhow::Result<std::sync::Arc<fp_core::OidcValidator>> {
    Err(anyhow::anyhow!(
        "FLOWPLANE_DEV_MODE=true but this binary was built without the dev-oidc feature \
         (release artifact); use a development build or configure a real OIDC issuer"
    ))
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
