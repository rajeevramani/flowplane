//! Flowplane dataplane agent. The v2 shape is intentionally small: one outbound
//! diagnostics stream, one bounded report queue, and a local health endpoint.

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use fp_xds::diagnostics::{
    diagnostics_report, AckStatus, DiagnosticsAck, DiagnosticsReport,
    EnvoyDiagnosticsServiceClient, HeartbeatReport,
};
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

const DEFAULT_QUEUE_CAP: usize = 256;
const MIN_ATTEMPT_DEADLINE: Duration = Duration::from_secs(10);
const MAX_ATTEMPT_DEADLINE: Duration = Duration::from_secs(30);
const INITIAL_RETRY_DELAY_MS: u64 = 250;
const MAX_RETRY_DELAY_MS: u64 = 5_000;

fn attempt_deadline(poll_interval: Duration) -> Duration {
    poll_interval
        .saturating_mul(2)
        .max(MIN_ATTEMPT_DEADLINE)
        .min(MAX_ATTEMPT_DEADLINE)
}

fn retry_delay(dataplane_id: uuid::Uuid, attempt: u32) -> Duration {
    let exponent = attempt.min(5);
    let base_ms = INITIAL_RETRY_DELAY_MS
        .saturating_mul(1_u64 << exponent)
        .min(MAX_RETRY_DELAY_MS);

    // Stable FNV-1a over dataplane identity and attempt gives deterministic ±20% jitter.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in dataplane_id
        .as_bytes()
        .iter()
        .copied()
        .chain(attempt.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let jitter_per_mille = 800 + (hash % 401);
    Duration::from_millis(base_ms.saturating_mul(jitter_per_mille) / 1_000)
}

#[derive(Parser, Debug)]
#[command(
    name = "flowplane-agent",
    version,
    about = "Flowplane dataplane telemetry relay"
)]
struct Args {
    /// Envoy admin base URL, usually loopback-only.
    #[arg(
        long,
        env = "FLOWPLANE_AGENT_ENVOY_ADMIN_URL",
        default_value = "http://127.0.0.1:9901"
    )]
    envoy_admin_url: String,
    /// Control-plane diagnostics gRPC endpoint, e.g. https://localhost:18000.
    #[arg(long, env = "FLOWPLANE_AGENT_CP_ENDPOINT")]
    cp_endpoint: String,
    /// Dataplane UUID registered in Flowplane.
    #[arg(long, env = "FLOWPLANE_AGENT_DATAPLANE_ID")]
    dataplane_id: uuid::Uuid,
    /// Poll interval for Envoy admin stats.
    #[arg(long, env = "FLOWPLANE_AGENT_POLL_INTERVAL_SECS", default_value_t = 10)]
    poll_interval_secs: u64,
    /// Maximum queued reports before backpressure is applied.
    #[arg(long, env = "FLOWPLANE_AGENT_QUEUE_CAP", default_value_t = DEFAULT_QUEUE_CAP)]
    queue_cap: usize,
    /// Client certificate PEM for mTLS.
    #[arg(long, env = "FLOWPLANE_AGENT_TLS_CERT_PATH")]
    tls_cert_path: Option<PathBuf>,
    /// Client key PEM for mTLS.
    #[arg(long, env = "FLOWPLANE_AGENT_TLS_KEY_PATH")]
    tls_key_path: Option<PathBuf>,
    /// CP/server CA PEM.
    #[arg(long, env = "FLOWPLANE_AGENT_TLS_CA_PATH")]
    tls_ca_path: Option<PathBuf>,
    /// Server name used for TLS verification.
    #[arg(
        long,
        env = "FLOWPLANE_AGENT_TLS_SERVER_NAME",
        default_value = "localhost"
    )]
    tls_server_name: String,
    /// Local health endpoint bind address.
    #[arg(
        long,
        env = "FLOWPLANE_AGENT_HEALTH_BIND_ADDR",
        default_value = "127.0.0.1:19902"
    )]
    health_bind_addr: SocketAddr,
    /// Scrape and report once, then exit.
    #[arg(long)]
    once: bool,
}

#[derive(Debug, Clone)]
struct Config {
    envoy_admin_url: String,
    cp_endpoint: String,
    dataplane_id: uuid::Uuid,
    poll_interval: Duration,
    queue_cap: usize,
    tls: Option<TlsConfig>,
    health_bind_addr: SocketAddr,
    once: bool,
}

#[derive(Debug, Clone)]
struct TlsConfig {
    cert_path: PathBuf,
    key_path: PathBuf,
    ca_path: PathBuf,
    server_name: String,
}

impl TryFrom<Args> for Config {
    type Error = anyhow::Error;

    fn try_from(args: Args) -> Result<Self> {
        let poll_interval = Duration::from_secs(args.poll_interval_secs.max(1));
        let queue_cap = args.queue_cap.clamp(1, 16_384);
        warn_if_admin_url_is_not_loopback(&args.envoy_admin_url);
        let tls = match (args.tls_cert_path, args.tls_key_path, args.tls_ca_path) {
            (Some(cert_path), Some(key_path), Some(ca_path)) => Some(TlsConfig {
                cert_path,
                key_path,
                ca_path,
                server_name: args.tls_server_name,
            }),
            (None, None, None) => {
                tracing::warn!("diagnostics connection has no TLS material; plaintext is dev-only");
                None
            }
            _ => anyhow::bail!(
                "FLOWPLANE_AGENT_TLS_CERT_PATH, _KEY_PATH, and _CA_PATH are all-or-none"
            ),
        };
        validate_cp_transport(&args.cp_endpoint, tls.is_some())?;
        Ok(Self {
            envoy_admin_url: args.envoy_admin_url,
            cp_endpoint: args.cp_endpoint,
            dataplane_id: args.dataplane_id,
            poll_interval,
            queue_cap,
            tls,
            health_bind_addr: args.health_bind_addr,
            once: args.once,
        })
    }
}

fn validate_cp_transport(endpoint: &str, tls_configured: bool) -> Result<()> {
    let parsed = reqwest::Url::parse(endpoint).context("parse FLOWPLANE_AGENT_CP_ENDPOINT")?;
    let Some(host) = parsed.host_str() else {
        anyhow::bail!("FLOWPLANE_AGENT_CP_ENDPOINT must include a host");
    };
    let loopback =
        host == "localhost" || host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback());
    if parsed.scheme() != "https" && !tls_configured && !loopback {
        anyhow::bail!(
            "plaintext diagnostics is allowed only for loopback control-plane endpoints; configure TLS for {host}"
        );
    }
    Ok(())
}

fn warn_if_admin_url_is_not_loopback(url: &str) {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return;
    };
    let Some(host) = parsed.host_str() else {
        return;
    };
    let loopback =
        host == "localhost" || host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback());
    if !loopback {
        tracing::warn!(%host, "Envoy admin URL is not loopback; production should bind admin locally");
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StatsSnapshot {
    requests: i64,
    errors: i64,
}

impl StatsSnapshot {
    fn delta_from(self, previous: Option<Self>) -> Self {
        let Some(previous) = previous else {
            return self;
        };
        Self {
            requests: (self.requests - previous.requests).max(0),
            errors: (self.errors - previous.errors).max(0),
        }
    }
}

#[derive(Debug, Default)]
struct HealthState {
    last_admin_poll: Option<Instant>,
    last_ack: Option<Instant>,
    diagnostics_connected: bool,
}

type SharedHealth = Arc<RwLock<HealthState>>;

#[derive(Deserialize)]
struct EnvoyStats {
    stats: Vec<serde_json::Value>,
}

fn parse_envoy_stats(body: &str) -> Result<StatsSnapshot> {
    let parsed: EnvoyStats =
        serde_json::from_str(body).context("parse Envoy /stats?format=json response")?;
    let mut snapshot = StatsSnapshot::default();
    for entry in &parsed.stats {
        // Envoy's `/stats?format=json` mixes scalar `{name, value}` objects with at
        // least one non-scalar element (the histograms object, `{"histograms": {...}}`).
        // Pick out only `{name, numeric value}` entries and skip everything else, so a
        // single non-conforming element does not fail deserialization of the whole
        // payload. Shape-based and version-agnostic (works across Envoy releases).
        let (Some(name), Some(value)) = (
            entry.get("name").and_then(|n| n.as_str()),
            entry.get("value").and_then(stat_value),
        ) else {
            continue;
        };
        if name.ends_with(".downstream_rq_total") {
            snapshot.requests += value;
        } else if name.ends_with(".downstream_rq_5xx") {
            snapshot.errors += value;
        }
    }
    Ok(snapshot)
}

fn stat_value(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok()))
}

async fn scrape_stats(client: &reqwest::Client, admin_url: &str) -> Result<StatsSnapshot> {
    let url = format!("{}/stats?format=json", admin_url.trim_end_matches('/'));
    let body = client
        .get(url)
        .send()
        .await
        .context("request Envoy stats")?
        .error_for_status()
        .context("Envoy stats returned an error")?
        .text()
        .await
        .context("read Envoy stats body")?;
    parse_envoy_stats(&body)
}

async fn diagnostics_channel(config: &Config) -> Result<Channel> {
    let mut endpoint = Endpoint::from_shared(config.cp_endpoint.clone())
        .context("parse FLOWPLANE_AGENT_CP_ENDPOINT")?
        .connect_timeout(Duration::from_secs(5));
    if let Some(tls) = &config.tls {
        let identity = Identity::from_pem(
            tokio::fs::read(&tls.cert_path).await?,
            tokio::fs::read(&tls.key_path).await?,
        );
        let tls_config = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(tokio::fs::read(&tls.ca_path).await?))
            .identity(identity)
            .domain_name(tls.server_name.clone());
        endpoint = endpoint
            .tls_config(tls_config)
            .context("configure diagnostics mTLS")?;
    }
    endpoint.connect().await.context("connect diagnostics gRPC")
}

fn heartbeat_report(config: &Config, delta: StatsSnapshot) -> DiagnosticsReport {
    DiagnosticsReport {
        schema_version: 1,
        report_id: uuid::Uuid::now_v7().to_string(),
        dataplane_id: config.dataplane_id.to_string(),
        observed_at: None,
        payload: Some(diagnostics_report::Payload::Heartbeat(HeartbeatReport {
            requests_delta: delta.requests,
            errors_delta: delta.errors,
            warming_failures_delta: 0,
            config_verified: true,
        })),
    }
}

async fn poll_loop(
    config: Config,
    http: reqwest::Client,
    tx: mpsc::Sender<DiagnosticsReport>,
    health: SharedHealth,
) -> Result<()> {
    let mut previous = None;
    loop {
        let snapshot = match scrape_stats(&http, &config.envoy_admin_url).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(%error, "Envoy admin scrape failed; retrying");
                if config.once {
                    return Err(error);
                }
                tokio::time::sleep(config.poll_interval).await;
                continue;
            }
        };
        health.write().await.last_admin_poll = Some(Instant::now());
        let delta = snapshot.delta_from(previous);
        tx.send(heartbeat_report(&config, delta))
            .await
            .context("diagnostics queue closed")?;
        previous = Some(snapshot);
        if config.once {
            return Ok(());
        }
        tokio::time::sleep(config.poll_interval).await;
    }
}

async fn stream_loop_once(
    config: Config,
    rx: mpsc::Receiver<DiagnosticsReport>,
    health: SharedHealth,
) -> Result<()> {
    let channel = diagnostics_channel(&config).await?;
    let mut client = EnvoyDiagnosticsServiceClient::new(channel);
    let mut responses = client
        .report_diagnostics(ReceiverStream::new(rx))
        .await
        .context("open diagnostics stream")?
        .into_inner();
    while let Some(ack) = responses
        .message()
        .await
        .context("receive diagnostics ack")?
    {
        if ack.status != AckStatus::Ok as i32 {
            anyhow::bail!(
                "diagnostics reports {:?} rejected: {}",
                ack.report_ids,
                ack.message
            );
        }
        health.write().await.last_ack = Some(Instant::now());
        if config.once {
            return Ok(());
        }
    }
    anyhow::bail!("diagnostics stream closed")
}

struct DiagnosticsConnection {
    requests: mpsc::Sender<DiagnosticsReport>,
    responses: tonic::Streaming<DiagnosticsAck>,
}

#[derive(Debug)]
enum AttemptFailure {
    Retryable(anyhow::Error),
    Fatal(anyhow::Error),
}

async fn open_diagnostics_stream(config: &Config) -> Result<DiagnosticsConnection> {
    let channel = diagnostics_channel(config).await?;
    let mut client = EnvoyDiagnosticsServiceClient::new(channel);
    let (requests, receiver) = mpsc::channel(1);
    let responses = client
        .report_diagnostics(ReceiverStream::new(receiver))
        .await
        .context("open diagnostics stream")?
        .into_inner();
    Ok(DiagnosticsConnection {
        requests,
        responses,
    })
}

async fn send_report_attempt(
    config: &Config,
    connection: &mut Option<DiagnosticsConnection>,
    report: &DiagnosticsReport,
) -> std::result::Result<(), AttemptFailure> {
    if connection.is_none() {
        *connection = Some(
            open_diagnostics_stream(config)
                .await
                .map_err(AttemptFailure::Retryable)?,
        );
    }
    let Some(active) = connection.as_mut() else {
        return Err(AttemptFailure::Retryable(anyhow::anyhow!(
            "diagnostics connection was not initialized"
        )));
    };
    active.requests.send(report.clone()).await.map_err(|_| {
        AttemptFailure::Retryable(anyhow::anyhow!("diagnostics request stream closed"))
    })?;
    let ack = active
        .responses
        .message()
        .await
        .map_err(|error| {
            AttemptFailure::Retryable(anyhow::Error::new(error).context("receive diagnostics ack"))
        })?
        .ok_or_else(|| {
            AttemptFailure::Retryable(anyhow::anyhow!("diagnostics response stream closed"))
        })?;

    if ack.status != AckStatus::Ok as i32 {
        return Err(AttemptFailure::Fatal(anyhow::anyhow!(
            "diagnostics reports {:?} rejected: {}",
            ack.report_ids,
            ack.message
        )));
    }
    if !ack.report_ids.contains(&report.report_id) {
        return Err(AttemptFailure::Fatal(anyhow::anyhow!(
            "diagnostics acknowledgment {:?} omitted in-flight report {}",
            ack.report_ids,
            report.report_id
        )));
    }
    Ok(())
}

async fn diagnostics_supervisor(
    config: Config,
    mut rx: mpsc::Receiver<DiagnosticsReport>,
    health: SharedHealth,
) -> Result<()> {
    let mut connection = None;
    let mut in_flight = None;
    let mut retry_attempt = 0_u32;

    loop {
        if in_flight.is_none() {
            in_flight = Some(rx.recv().await.context("diagnostics source queue closed")?);
        }
        let Some(report) = in_flight.as_ref() else {
            continue;
        };
        let deadline = attempt_deadline(config.poll_interval);
        let outcome = tokio::time::timeout(
            deadline,
            send_report_attempt(&config, &mut connection, report),
        )
        .await;

        match outcome {
            Ok(Ok(())) => {
                let mut state = health.write().await;
                state.last_ack = Some(Instant::now());
                state.diagnostics_connected = true;
                drop(state);
                in_flight = None;
                retry_attempt = 0;
            }
            Ok(Err(AttemptFailure::Fatal(error))) => return Err(error),
            Ok(Err(AttemptFailure::Retryable(error))) => {
                connection = None;
                health.write().await.diagnostics_connected = false;
                let delay = retry_delay(config.dataplane_id, retry_attempt);
                tracing::warn!(
                    %error,
                    attempt = retry_attempt + 1,
                    timeout_ms = deadline.as_millis(),
                    next_delay_ms = delay.as_millis(),
                    "diagnostics disconnected; retrying"
                );
                retry_attempt = retry_attempt.saturating_add(1);
                tokio::time::sleep(delay).await;
            }
            Err(_) => {
                connection = None;
                health.write().await.diagnostics_connected = false;
                let delay = retry_delay(config.dataplane_id, retry_attempt);
                tracing::warn!(
                    attempt = retry_attempt + 1,
                    timeout_ms = deadline.as_millis(),
                    next_delay_ms = delay.as_millis(),
                    "diagnostics report attempt timed out; retrying"
                );
                retry_attempt = retry_attempt.saturating_add(1);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn healthz(
    State((health, stale_after)): State<(SharedHealth, Duration)>,
) -> (StatusCode, String) {
    let state = health.read().await;
    let now = Instant::now();
    let Some(last_admin_poll) = state.last_admin_poll else {
        return (StatusCode::SERVICE_UNAVAILABLE, "never polled".to_string());
    };
    let admin_age = now.saturating_duration_since(last_admin_poll);
    if admin_age > stale_after {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("admin poll stale {}s", admin_age.as_secs()),
        );
    }
    if !state.diagnostics_connected {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "diagnostics disconnected; retrying".to_string(),
        );
    }
    let Some(last_ack) = state.last_ack else {
        return (StatusCode::SERVICE_UNAVAILABLE, "never acked".to_string());
    };
    let ack_age = now.saturating_duration_since(last_ack);
    if ack_age > stale_after {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("diagnostics ack stale {}s", ack_age.as_secs()),
        );
    }
    (StatusCode::OK, "ok".to_string())
}

async fn serve_health(config: Config, health: SharedHealth) -> Result<()> {
    let stale_after = config
        .poll_interval
        .saturating_mul(2)
        .max(Duration::from_secs(2));
    let app = Router::new()
        .route("/healthz", get(healthz))
        .with_state((health, stale_after));
    let listener = tokio::net::TcpListener::bind(config.health_bind_addr)
        .await
        .with_context(|| format!("bind health endpoint {}", config.health_bind_addr))?;
    axum::serve(listener, app)
        .await
        .context("serve health endpoint")
}

async fn run(config: Config) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build HTTP client")?;
    let (tx, rx) = mpsc::channel::<DiagnosticsReport>(config.queue_cap);
    let health = Arc::new(RwLock::new(HealthState::default()));

    if config.once {
        let poll = poll_loop(config.clone(), http, tx, health.clone());
        let stream = stream_loop_once(config, rx, health);
        let (poll, stream) = tokio::join!(poll, stream);
        poll?;
        stream?;
        return Ok(());
    }

    tokio::select! {
        result = poll_loop(config.clone(), http, tx, health.clone()) => result,
        result = diagnostics_supervisor(config.clone(), rx, health.clone()) => result,
        result = serve_health(config, health) => result,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "fp_agent=info,warn".to_string()),
        )
        .init();
    run(Config::try_from(Args::parse())?).await
}

#[cfg(test)]
mod tests {
    use super::{attempt_deadline, parse_envoy_stats, retry_delay, Config, StatsSnapshot};
    use std::collections::HashSet;
    use std::time::Duration;

    #[test]
    fn envoy_stats_parser_sums_http_downstream_counters() -> anyhow::Result<()> {
        let parsed = parse_envoy_stats(
            r#"{"stats":[
                {"name":"http.ingress.downstream_rq_total","value":7},
                {"name":"http.other.downstream_rq_total","value":2},
                {"name":"http.ingress.downstream_rq_5xx","value":1},
                {"name":"server.live","value":1}
            ]}"#,
        )?;
        assert_eq!(
            parsed,
            StatsSnapshot {
                requests: 9,
                errors: 1
            }
        );
        Ok(())
    }

    // Regression for #170. Real `/stats?format=json` body captured from
    // `envoyproxy/envoy:v1.37-latest` (the D-013 pinned image) after driving traffic
    // through an ingress listener (7×200, 3×503-to-dead-cluster). Its `stats` array
    // contains the non-`{name,value}` histograms element that made the old strict
    // `Vec<EnvoyStat>` deserialize fail for the whole payload — so no heartbeat was
    // sent and `last_heartbeat_at` was never populated. Expected sums computed
    // independently from the captured file: Σ `*.downstream_rq_total` = 12,
    // Σ `*.downstream_rq_5xx` = 6.
    const ENVOY_1_37_STATS: &str = include_str!("testdata/envoy_1_37_stats.json");

    #[test]
    fn parses_real_envoy_1_37_stats_including_histograms_element() -> anyhow::Result<()> {
        // Sanity: the captured fixture really does carry the offending element.
        assert!(
            ENVOY_1_37_STATS.contains("\"histograms\""),
            "fixture must include the histograms element this regression guards against"
        );
        let parsed = parse_envoy_stats(ENVOY_1_37_STATS)?;
        assert_eq!(
            parsed,
            StatsSnapshot {
                requests: 12,
                errors: 6
            }
        );
        Ok(())
    }

    #[test]
    fn parser_skips_histograms_string_and_non_scalar_elements() -> anyhow::Result<()> {
        // The histograms object, a string-valued stat, a null-valued stat, and a
        // non-integer (float) value must all be skipped rather than fail the payload;
        // only integer counters are summed (`stat_value` is integer-only).
        let parsed = parse_envoy_stats(
            r#"{"stats":[
                {"name":"http.ingress.downstream_rq_total","value":5},
                {"name":"server.version","value":"d3a3..."},
                {"name":"http.ingress.downstream_rq_5xx","value":2},
                {"name":"some.gauge","value":null},
                {"name":"http.float.downstream_rq_total","value":1.5},
                {"histograms":{"supported_quantiles":[0,25,50],"computed_quantiles":[]}}
            ]}"#,
        )?;
        assert_eq!(
            parsed,
            StatsSnapshot {
                requests: 5,
                errors: 2
            }
        );
        Ok(())
    }

    #[test]
    fn parser_returns_zero_on_empty_stats_array() -> anyhow::Result<()> {
        // The valid-but-empty boundary: a well-formed body with no entries parses to a
        // zero snapshot (distinct from the malformed cases below, which must error).
        assert_eq!(
            parse_envoy_stats(r#"{"stats":[]}"#)?,
            StatsSnapshot::default()
        );
        Ok(())
    }

    #[test]
    fn parser_rejects_malformed_body() {
        // Tolerance is for unexpected *elements*, not a broken scrape: invalid JSON,
        // a missing `stats` key, a non-array `stats`, and a null `stats` all still error.
        assert!(parse_envoy_stats("not json at all").is_err());
        assert!(parse_envoy_stats(r#"{"stats":"nope"}"#).is_err());
        assert!(parse_envoy_stats(r#"{"stats":null}"#).is_err());
        assert!(parse_envoy_stats(r#"{"other":[]}"#).is_err());
    }

    #[test]
    fn first_delta_uses_snapshot_and_next_delta_is_positive_difference() {
        let first = StatsSnapshot {
            requests: 10,
            errors: 3,
        };
        assert_eq!(first.delta_from(None), first);
        let next = StatsSnapshot {
            requests: 12,
            errors: 2,
        };
        assert_eq!(
            next.delta_from(Some(first)),
            StatsSnapshot {
                requests: 2,
                errors: 0
            }
        );
    }

    #[test]
    fn config_clamps_queue_and_poll_interval() -> anyhow::Result<()> {
        let args = super::Args {
            envoy_admin_url: "http://127.0.0.1:9901".to_string(),
            cp_endpoint: "http://127.0.0.1:18000".to_string(),
            dataplane_id: uuid::Uuid::now_v7(),
            poll_interval_secs: 0,
            queue_cap: 0,
            tls_cert_path: None,
            tls_key_path: None,
            tls_ca_path: None,
            tls_server_name: "localhost".to_string(),
            health_bind_addr: "127.0.0.1:0".parse()?,
            once: true,
        };
        let config = Config::try_from(args)?;
        assert_eq!(config.poll_interval, Duration::from_secs(1));
        assert_eq!(config.queue_cap, 1);
        Ok(())
    }

    #[test]
    fn config_rejects_plaintext_cp_endpoint_off_loopback() -> anyhow::Result<()> {
        let args = super::Args {
            envoy_admin_url: "http://127.0.0.1:9901".to_string(),
            cp_endpoint: "http://10.0.0.10:18000".to_string(),
            dataplane_id: uuid::Uuid::now_v7(),
            poll_interval_secs: 10,
            queue_cap: 10,
            tls_cert_path: None,
            tls_key_path: None,
            tls_ca_path: None,
            tls_server_name: "localhost".to_string(),
            health_bind_addr: "127.0.0.1:0".parse()?,
            once: false,
        };
        let err = Config::try_from(args)
            .err()
            .ok_or_else(|| anyhow::anyhow!("plaintext remote CP was accepted"))?;
        assert!(
            err.to_string().contains("plaintext diagnostics"),
            "unexpected error: {err:#}"
        );
        Ok(())
    }

    #[test]
    fn config_allows_plaintext_cp_endpoint_on_loopback() -> anyhow::Result<()> {
        let args = super::Args {
            envoy_admin_url: "http://127.0.0.1:9901".to_string(),
            cp_endpoint: "http://localhost:18000".to_string(),
            dataplane_id: uuid::Uuid::now_v7(),
            poll_interval_secs: 10,
            queue_cap: 10,
            tls_cert_path: None,
            tls_key_path: None,
            tls_ca_path: None,
            tls_server_name: "localhost".to_string(),
            health_bind_addr: "127.0.0.1:0".parse()?,
            once: false,
        };
        Config::try_from(args)?;
        Ok(())
    }

    #[test]
    fn report_attempt_deadline_has_connect_margin_and_thirty_second_ceiling() {
        assert_eq!(
            attempt_deadline(Duration::from_secs(1)),
            Duration::from_secs(10)
        );
        assert_eq!(
            attempt_deadline(Duration::from_secs(10)),
            Duration::from_secs(20)
        );
        assert_eq!(
            attempt_deadline(Duration::from_secs(60)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn retry_delay_is_deterministic_bounded_and_varies_by_attempt() {
        let dataplane_id = uuid::Uuid::from_bytes([
            0x01, 0x8f, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ]);
        let delays: Vec<_> = (0..12)
            .map(|attempt| retry_delay(dataplane_id, attempt))
            .collect();
        assert_eq!(delays[4], retry_delay(dataplane_id, 4));
        assert!(delays
            .iter()
            .all(|delay| *delay >= Duration::from_millis(200)));
        assert!(delays.iter().all(|delay| *delay <= Duration::from_secs(6)));
        assert!(
            delays.iter().copied().collect::<HashSet<_>>().len() > 4,
            "attempt-derived jitter/backoff should not collapse to one delay"
        );
    }
}
