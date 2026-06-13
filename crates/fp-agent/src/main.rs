//! Minimal Flowplane dataplane agent. It scrapes Envoy admin stats and relays heartbeat
//! telemetry to the CP diagnostics gRPC service over the dataplane certificate identity.

use anyhow::{Context, Result};
use clap::Parser;
use fp_xds::diagnostics::{
    diagnostics_report, AckStatus, DiagnosticsReport, EnvoyDiagnosticsServiceClient,
    HeartbeatReport,
};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

#[derive(Parser, Debug)]
#[command(
    name = "fp-agent",
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
    dataplane_id: String,
    /// Poll interval for daemon mode.
    #[arg(long, env = "FLOWPLANE_AGENT_POLL_INTERVAL_SECS", default_value_t = 10)]
    poll_interval_secs: u64,
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
    /// Scrape and report once, then exit.
    #[arg(long)]
    once: bool,
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

#[derive(Deserialize)]
struct EnvoyStats {
    stats: Vec<EnvoyStat>,
}

#[derive(Deserialize)]
struct EnvoyStat {
    name: String,
    value: serde_json::Value,
}

fn parse_envoy_stats(body: &str) -> Result<StatsSnapshot> {
    let parsed: EnvoyStats =
        serde_json::from_str(body).context("parse Envoy /stats?format=json response")?;
    let mut snapshot = StatsSnapshot::default();
    for stat in parsed.stats {
        let Some(value) = stat_value(&stat.value) else {
            continue;
        };
        if stat.name.ends_with(".downstream_rq_total") {
            snapshot.requests += value;
        } else if stat.name.ends_with(".downstream_rq_5xx") {
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

async fn diagnostics_channel(args: &Args) -> Result<Channel> {
    let mut endpoint = Endpoint::from_shared(args.cp_endpoint.clone())
        .context("parse FLOWPLANE_AGENT_CP_ENDPOINT")?
        .connect_timeout(Duration::from_secs(5));
    match (&args.tls_cert_path, &args.tls_key_path, &args.tls_ca_path) {
        (Some(cert), Some(key), Some(ca)) => {
            let identity =
                Identity::from_pem(tokio::fs::read(cert).await?, tokio::fs::read(key).await?);
            let tls = ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(tokio::fs::read(ca).await?))
                .identity(identity)
                .domain_name(args.tls_server_name.clone());
            endpoint = endpoint
                .tls_config(tls)
                .context("configure diagnostics mTLS")?;
        }
        (None, None, None) => {
            tracing::warn!("diagnostics connection has no TLS material; plaintext is dev-only");
        }
        _ => {
            anyhow::bail!("FLOWPLANE_AGENT_TLS_CERT_PATH, _KEY_PATH, and _CA_PATH are all-or-none")
        }
    }
    endpoint.connect().await.context("connect diagnostics gRPC")
}

async fn send_heartbeat(args: &Args, delta: StatsSnapshot) -> Result<()> {
    let channel = diagnostics_channel(args).await?;
    let mut client = EnvoyDiagnosticsServiceClient::new(channel);
    let report_id = uuid::Uuid::now_v7().to_string();
    let report = DiagnosticsReport {
        schema_version: 1,
        report_id: report_id.clone(),
        dataplane_id: args.dataplane_id.clone(),
        observed_at: None,
        payload: Some(diagnostics_report::Payload::Heartbeat(HeartbeatReport {
            requests_delta: delta.requests,
            errors_delta: delta.errors,
            warming_failures_delta: 0,
            config_verified: true,
        })),
    };
    let mut responses = client
        .report_diagnostics(tokio_stream::iter([report]))
        .await
        .context("open diagnostics stream")?
        .into_inner();
    let ack = tokio::time::timeout(Duration::from_secs(10), responses.message())
        .await
        .context("timed out waiting for diagnostics ack")?
        .context("receive diagnostics ack")?
        .context("diagnostics stream closed before ack")?;
    if ack.status != AckStatus::Ok as i32 {
        anyhow::bail!("diagnostics report {report_id} rejected: {}", ack.message);
    }
    Ok(())
}

async fn run(args: Args) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build HTTP client")?;
    let mut previous = None;
    loop {
        let snapshot = scrape_stats(&http, &args.envoy_admin_url).await?;
        let delta = snapshot.delta_from(previous);
        send_heartbeat(&args, delta).await?;
        previous = Some(snapshot);
        if args.once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(args.poll_interval_secs.max(1))).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "fp_agent=info,warn".to_string()),
        )
        .init();
    run(Args::parse()).await
}

#[cfg(test)]
mod tests {
    use super::{parse_envoy_stats, StatsSnapshot};

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
}
