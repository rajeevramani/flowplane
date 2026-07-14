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
        Some(setup_dev_mode(&pool, config.dev_token_path.as_deref()).await?)
    } else if let Some(oidc) = &config.oidc {
        tracing::info!(issuer = %oidc.issuer, "OIDC authentication enabled");
        // try_new (not new): a bad operator-supplied CA bundle must fail boot closed,
        // not panic (#171). The `?` propagates through serve::run -> main -> exit non-zero.
        Some(std::sync::Arc::new(fp_core::OidcValidator::try_new(
            fp_core::OidcConfig {
                issuer: oidc.issuer.clone(),
                audience: oidc.audience.clone(),
                jwks_uri: oidc.jwks_uri.clone(),
                ca_bundle_path: oidc.ca_bundle_path.clone(),
            },
        )?))
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
        seed_bootstrap_token(&pool, &config).await?;
    }

    // xDS pipeline: snapshot cache primed from the DB (restart safety), then kept fresh by
    // the outbox consumer, which also feeds certificate revocations to live streams.
    let (xds_shutdown_tx, xds_shutdown_rx) = tokio::sync::watch::channel(false);
    // S6: when an RLS gRPC endpoint is configured, the snapshot cache injects the built-in
    // rate_limit_cluster into every team's CDS. Build its config from ServerConfig (fp-xds stays
    // free of fp-core types) and validate the endpoint once here so a bad URL fails boot, not
    // every per-team rebuild.
    let rls_cluster =
        config
            .rls_grpc_url
            .as_ref()
            .map(|grpc_url| fp_xds::translate::RlsClusterConfig {
                grpc_url: grpc_url.clone(),
                tls: config
                    .dataplane_tls
                    .as_ref()
                    .map(|t| fp_xds::translate::RlsClusterTls {
                        cert_path: t.cert_path.to_string_lossy().into_owned(),
                        key_path: t.key_path.to_string_lossy().into_owned(),
                        client_ca_path: t.client_ca_path.to_string_lossy().into_owned(),
                    }),
            });
    if let Some(rls) = &rls_cluster {
        fp_xds::translate::rls_cluster_to_proto(rls)
            .map_err(|e| anyhow::anyhow!("invalid FLOWPLANE_RLS_GRPC_URL: {e}"))?;
        tracing::info!(
            mtls = config.dataplane_tls.is_some(),
            "built-in rate_limit_cluster will be injected into CDS"
        );
    }
    let snapshot_cache = fp_xds::snapshot::SnapshotCache::with_rls(rls_cluster);
    let xds_consumer_failed = Arc::new(AtomicBool::new(false));
    let primed = snapshot_cache
        .prime_all(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("failed to prime xDS snapshot cache: {e}"))?;
    tracing::info!(teams = primed, "xDS snapshot cache primed from database");
    let (revocation_tx, _) = tokio::sync::broadcast::channel::<uuid::Uuid>(64);
    // Handles for spawned background tasks; awaited (bounded) on shutdown so streams, the
    // outbox consumer, and read-only samplers drain rather than being abandoned mid-flight.
    let mut xds_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    {
        let sampler_pool = pool.clone();
        let sampler_shutdown = xds_shutdown_tx.subscribe();
        let db_max_connections = config.db_max_connections;
        xds_tasks.push(tokio::spawn(async move {
            run_observability_sampler(
                sampler_pool,
                fp_xds::snapshot::XDS_CONSUMER,
                db_max_connections,
                sampler_shutdown,
            )
            .await;
        }));
    }
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

    let discovery_forwarding_policy =
        fp_core::services::discovery::DiscoveryForwardingPolicy::from_server_config(&config).await;

    // Write-time egress advisory (FP-DEC-0008): logs its own startup warning when disabled.
    let egress_advisory =
        fp_core::services::egress_advisory::EgressAdvisoryPolicy::from_server_config(&config).await;

    // CP→RLS policy sync (S5): when the RLS admin URL is set, run the 60 s reconcile worker and
    // expose a force-repush kick. The first reconcile fires immediately at startup.
    let rls_repush = if let Some(admin_url) = config.rls_admin_url.clone() {
        let notify = Arc::new(tokio::sync::Notify::new());
        let worker_pool = pool.clone();
        let worker_notify = Arc::clone(&notify);
        let worker_shutdown = xds_shutdown_tx.subscribe();
        let reconcile_secs = config.rls_reconcile_secs;
        tokio::spawn(run_rls_sync(
            worker_pool,
            admin_url,
            reconcile_secs,
            worker_notify,
            worker_shutdown,
        ));
        tracing::info!(reconcile_secs, "rls_sync worker started");
        Some(notify)
    } else {
        None
    };

    // AI trace retention sweep (ai-gateway-e2e-trace s5): fixed-interval deletion of trace
    // rows whose insert-stamped expires_at has passed. Owned by the CP; no dataplane involved.
    {
        let sweep_pool = pool.clone();
        let sweep_shutdown = xds_shutdown_tx.subscribe();
        tokio::spawn(run_ai_trace_sweep(sweep_pool, sweep_shutdown));
        tracing::info!(
            interval_secs = AI_TRACE_SWEEP_INTERVAL_SECS,
            "ai trace retention sweep started"
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
        discovery_forwarding_policy,
        egress_advisory,
        rls_repush,
        rls_grpc_configured: config.rls_grpc_url.is_some(),
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

async fn run_observability_sampler(
    pool: sqlx::PgPool,
    outbox_consumer: &'static str,
    db_max_connections: u32,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                observe_pool_metrics(&pool, db_max_connections);
                match fp_storage::outbox::consumer_lag_stats(&pool, outbox_consumer).await {
                    Ok(stats) => {
                        metrics::gauge!(
                            "fp_outbox_pending_events",
                            "consumer" => outbox_consumer
                        )
                        .set(stats.pending_count as f64);
                        metrics::gauge!(
                            "fp_outbox_oldest_pending_age_seconds",
                            "consumer" => outbox_consumer
                        )
                        .set(stats.oldest_age_seconds);
                    }
                    Err(e) => {
                        metrics::counter!(
                            "fp_observability_sampler_failures_total",
                            "source" => "outbox"
                        )
                        .increment(1);
                        tracing::warn!("observability sampler failed to read outbox lag: {e}");
                    }
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

/// CP→RLS policy reconcile worker (S5). Pushes the full namespaced policy set to the RLS admin
/// endpoint every 60 s (first tick fires immediately at startup) and on a force-repush kick.
/// Level-triggered: each push is the complete set, so a transient failure self-heals next tick.
async fn run_rls_sync(
    pool: sqlx::PgPool,
    admin_url: String,
    reconcile_secs: u64,
    repush: Arc<tokio::sync::Notify>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let client = reqwest::Client::new();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(reconcile_secs.max(1)));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = repush.notified() => {
                tracing::info!("rls force-repush received; reconciling now");
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
        }
        match fp_core::services::rls_sync::reconcile_once(&pool, &admin_url, &client).await {
            Ok(count) => tracing::debug!(policies = count, "rls policy reconcile pushed"),
            Err(e) => tracing::warn!("rls policy reconcile failed: {e}"),
        }
    }
}

/// Fixed sweep cadence for expired AI trace rows. Hourly is deliberately coarse: expiry
/// precision only needs to be within the sweep interval of a row's `expires_at`, and one
/// bulk DELETE per hour keeps the sweep invisible next to the per-request insert load.
const AI_TRACE_SWEEP_INTERVAL_SECS: u64 = 3600;

/// Expired-trace sweep loop: on every tick, delete all `ai_trace_events` rows whose
/// `expires_at` has passed. The first tick fires immediately at startup so a restarted CP
/// clears backlog without waiting a full interval. Errors are logged and the loop keeps
/// running — retention is eventually consistent, never a boot or request failure.
async fn run_ai_trace_sweep(pool: sqlx::PgPool, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut interval =
        tokio::time::interval(std::time::Duration::from_secs(AI_TRACE_SWEEP_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
        }
        let as_of = sqlx::types::chrono::Utc::now();
        match fp_storage::repos::ai_trace::delete_expired_trace_events(&pool, as_of).await {
            Ok(deleted) if deleted > 0 => {
                tracing::info!(deleted, "ai trace retention sweep removed expired rows");
            }
            Ok(_) => tracing::debug!("ai trace retention sweep found nothing expired"),
            Err(e) => tracing::warn!("ai trace retention sweep failed: {e}"),
        }
    }
}

fn observe_pool_metrics(pool: &sqlx::PgPool, max_connections: u32) {
    let size = pool.size();
    let idle = pool.num_idle() as u32;
    let in_use = size.saturating_sub(idle);

    metrics::gauge!("fp_db_pool_size").set(size as f64);
    metrics::gauge!("fp_db_pool_idle").set(idle as f64);
    metrics::gauge!("fp_db_pool_in_use").set(in_use as f64);
    metrics::gauge!("fp_db_pool_max").set(max_connections as f64);
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
    dev_token_path: Option<&std::path::Path>,
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
    tracing::warn!(
        dev_token = %token,
        "dev bearer token (default 24h, set FLOWPLANE_DEV_TOKEN_TTL to change; this boot only)"
    );
    // Dev-only file sink (#156): a compose `init`/sibling container can't read another
    // container's stdout, so when an operator names a path we also write the raw token there.
    if let Some(path) = dev_token_path {
        write_dev_token(path, &token)?;
        tracing::warn!(path = %path.display(), "dev bearer token also written to file (dev mode)");
    }
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
    _dev_token_path: Option<&std::path::Path>,
) -> anyhow::Result<std::sync::Arc<fp_core::OidcValidator>> {
    Err(anyhow::anyhow!(
        "FLOWPLANE_DEV_MODE=true but this binary was built without the dev-oidc feature \
         (release artifact); use a development build or configure a real OIDC issuer"
    ))
}

/// Persist the minted dev token to an operator-named path so a sibling container can read it.
/// Dev-mode only; a write failure is fatal so a misconfigured eval bundle fails loud, not silent.
/// Written as a private file — 0600, missing parents created — instead of a plain
/// umask-inheriting `fs::write` (FP-DEC-0012).
#[cfg(feature = "dev-oidc")]
fn write_dev_token(path: &std::path::Path, token: &str) -> anyhow::Result<()> {
    crate::paths::write_private_file(path, token)
        .with_context(|| format!("failed to write dev token to {}", path.display()))
}

/// Minimum length of an operator-supplied bootstrap token after trimming (#113). First-platform-
/// admin authority warrants more than "non-empty".
const MIN_BOOTSTRAP_TOKEN_LEN: usize = 32;

/// An operator-supplied bootstrap token. Wraps the secret so it never reaches logs: `Debug` and
/// `Display` redact, and it is not `Serialize`. Only `as_str()` exposes the value, for hashing.
struct BootstrapToken(String);

impl BootstrapToken {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for BootstrapToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BootstrapToken(<redacted>)")
    }
}

impl std::fmt::Display for BootstrapToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Resolve the operator-supplied bootstrap token from the environment, provider-agnostically.
/// `FLOWPLANE_BOOTSTRAP_TOKEN_FILE` (a path) takes precedence over `FLOWPLANE_BOOTSTRAP_TOKEN`.
/// Returns `Ok(None)` when neither is set. Errors never echo the token value or the file path.
fn resolve_operator_bootstrap_token() -> anyhow::Result<Option<BootstrapToken>> {
    let raw = if let Some(path) = std::env::var_os("FLOWPLANE_BOOTSTRAP_TOKEN_FILE") {
        // Deliberately do not include the path or contents in the error.
        match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(_) => {
                anyhow::bail!(
                    "could not read the file named by FLOWPLANE_BOOTSTRAP_TOKEN_FILE \
                     (check the path exists and is readable)"
                )
            }
        }
    } else if let Some(value) = std::env::var_os("FLOWPLANE_BOOTSTRAP_TOKEN") {
        value.to_string_lossy().into_owned()
    } else {
        return Ok(None);
    };

    validate_bootstrap_token(&raw).map(Some)
}

/// Trim and length-validate a bootstrap token from any source. The error never echoes the value.
fn validate_bootstrap_token(raw: &str) -> anyhow::Result<BootstrapToken> {
    let trimmed = raw.trim();
    if trimmed.len() < MIN_BOOTSTRAP_TOKEN_LEN {
        anyhow::bail!(
            "the supplied bootstrap token is too short; it must be at least {MIN_BOOTSTRAP_TOKEN_LEN} \
             characters after trimming whitespace"
        );
    }
    Ok(BootstrapToken(trimmed.to_string()))
}

/// Bootstrap-token handling for an uninitialized, non-dev instance (#113). The operator supplies a
/// token (env / file) that is seeded without ever being logged; with no token the instance fails
/// closed unless the explicit local-only opt-in re-enables the legacy generate-and-log path.
async fn seed_bootstrap_token(pool: &sqlx::PgPool, config: &ServerConfig) -> anyhow::Result<()> {
    use fp_storage::repos::bootstrap::{
        is_initialized, issue_token_if_uninitialized, seed_token_hash_if_uninitialized, SeedOutcome,
    };

    // Already-initialized instances ignore the bootstrap token entirely — do not even resolve it,
    // so a stale/unreadable token file cannot block a restart. (The seed path rechecks under the
    // advisory lock, so this early return is only an optimization, not the correctness boundary.)
    if is_initialized(pool).await? {
        return Ok(());
    }

    match resolve_operator_bootstrap_token()? {
        Some(token) => match seed_token_hash_if_uninitialized(pool, token.as_str()).await? {
            SeedOutcome::Seeded => tracing::info!(
                "bootstrap token accepted from the operator; POST /api/v1/bootstrap/initialize to \
                 complete setup (token not logged)"
            ),
            SeedOutcome::Idempotent => tracing::info!(
                "bootstrap token already seeded for this instance; awaiting \
                 POST /api/v1/bootstrap/initialize"
            ),
            SeedOutcome::AlreadyInitialized => {}
            SeedOutcome::Conflict => anyhow::bail!(
                "a different bootstrap token is already active for this uninitialized instance; \
                 supply the same operator token across all replicas"
            ),
        },
        None => {
            if config.allow_logged_bootstrap_token {
                if let Some(token) = issue_token_if_uninitialized(pool).await? {
                    tracing::warn!(
                        bootstrap_token = %token,
                        "LOCAL-ONLY: FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN is set — generated and \
                         logged a one-shot bootstrap token (valid 24h). Do not use in production."
                    );
                }
            } else if !is_initialized(pool).await? {
                anyhow::bail!(
                    "instance is uninitialized and no bootstrap token was supplied; set \
                     FLOWPLANE_BOOTSTRAP_TOKEN or FLOWPLANE_BOOTSTRAP_TOKEN_FILE (or, for local use \
                     only, FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN=yes-this-is-local-only)"
                );
            }
        }
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod bootstrap_token_tests {
    use super::*;

    #[test]
    fn rejects_short_or_blank_tokens() {
        assert!(validate_bootstrap_token("").is_err(), "empty");
        assert!(
            validate_bootstrap_token("   \n").is_err(),
            "whitespace only"
        );
        assert!(validate_bootstrap_token("short").is_err(), "too short");
        assert!(
            validate_bootstrap_token(&"x".repeat(31)).is_err(),
            "31 chars"
        );
    }

    #[test]
    fn accepts_and_trims_valid_token() {
        let t = validate_bootstrap_token("  abcdefghijklmnopqrstuvwxyz012345  ").expect("valid");
        assert_eq!(t.as_str(), "abcdefghijklmnopqrstuvwxyz012345");
        assert_eq!(t.as_str().len(), 32);
    }

    #[test]
    fn token_never_renders_its_value() {
        let secret = "abcdefghijklmnopqrstuvwxyz012345";
        let t = validate_bootstrap_token(secret).expect("valid");
        assert!(!format!("{t:?}").contains(secret), "Debug must redact");
        assert!(!format!("{t}").contains(secret), "Display must redact");
    }

    #[test]
    fn error_messages_do_not_echo_the_value() {
        let secret = "supersecretbutslightlytooshort"; // 30 chars
        let err = validate_bootstrap_token(secret).unwrap_err().to_string();
        assert!(!err.contains(secret), "error must not echo the token");
    }
}

#[cfg(all(test, feature = "dev-oidc"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod dev_token_tests {
    use super::*;
    use std::process;

    #[test]
    fn write_dev_token_persists_exact_bytes() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("flowplane-dev-token-{}.txt", process::id()));
        let _ = std::fs::remove_file(&path);

        let token = "dev.token.value-123";
        write_dev_token(&path, token).expect("write must succeed");

        let read_back = std::fs::read_to_string(&path).expect("file must be readable");
        assert_eq!(
            read_back, token,
            "file content must match minted token byte-for-byte"
        );

        std::fs::remove_file(&path).expect("cleanup");
    }

    #[test]
    fn write_dev_token_errors_with_context_on_bad_path() {
        // The private write creates missing parent dirs (FP-DEC-0012), so a merely-absent
        // parent no longer fails. A parent path component that is a regular FILE fails
        // deterministically (NotADirectory) — even when running as root — without touching
        // global filesystem state.
        let blocker =
            std::env::temp_dir().join(format!("flowplane-dev-token-blocker-{}", process::id()));
        std::fs::write(&blocker, "not a directory").expect("create blocker file");
        let bad = blocker.join("token.txt");
        let err = write_dev_token(&bad, "x").expect_err("must fail when parent is a file");
        assert!(
            err.to_string().contains("failed to write dev token to"),
            "error must carry write context, got: {err}"
        );
        std::fs::remove_file(&blocker).expect("cleanup");
    }
}
