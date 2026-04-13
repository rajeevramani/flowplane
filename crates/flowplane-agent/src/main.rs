//! `flowplane-agent` — dataplane-side diagnostics agent.
//!
//! Runs alongside each Envoy instance, reads `/config_dump` over loopback,
//! extracts listener / cluster / route_config warming failures, and streams
//! `DiagnosticsReport` messages to the Flowplane control plane over the
//! `EnvoyDiagnosticsService` bidi stream.
//!
//! MVP scope: warming failure reporting only. Cert rotation, heartbeats,
//! telemetry relay, and traffic capture are explicitly out of scope (see
//! fp-hsk epic notes) — the proto reserves field numbers for each of those
//! so they can be added as additive payload variants later without a new
//! agent binary release.

mod agent;
mod backoff;
mod client;
mod config;
mod config_dump;
mod dedup;
mod queue;

/// Generated tonic/prost bindings for `flowplane.diagnostics.v1`.
#[allow(clippy::doc_overindented_list_items)]
#[allow(missing_docs)]
pub mod diagnostics_proto {
    tonic::include_proto!("flowplane.diagnostics.v1");
}

use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tonic::transport::{Certificate, ClientTlsConfig, Identity};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cfg = config::AgentConfig::parse();

    if !config::admin_is_loopback(&cfg.envoy_admin_url) {
        warn!(
            url = %cfg.envoy_admin_url,
            "Envoy admin URL is NOT loopback — exposing Envoy admin beyond 127.0.0.1 is a security risk; continuing anyway"
        );
    }

    let tls = build_tls_config(&cfg)?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

    let queue = Arc::new(queue::BoundedQueue::<diagnostics_proto::DiagnosticsReport>::new(
        cfg.queue_cap.max(1),
    ));
    let cfg_arc = Arc::new(cfg.clone());

    info!(
        dataplane_id = %cfg.dataplane_id,
        envoy_admin = %cfg.envoy_admin_url,
        cp_endpoint = %cfg.cp_endpoint,
        poll_interval_secs = cfg.poll_interval_secs,
        queue_cap = cfg.queue_cap,
        "flowplane-agent starting"
    );

    let poll_task = tokio::spawn(agent::run_poll_loop(cfg_arc.clone(), http, queue.clone()));
    let stream_task = {
        let q = queue.clone();
        let endpoint = cfg.cp_endpoint.clone();
        tokio::spawn(async move { client::run_stream_loop(endpoint, tls, q).await })
    };

    tokio::select! {
        _ = signal::ctrl_c() => info!("SIGINT received, shutting down"),
        res = poll_task => warn!(?res, "poll loop exited unexpectedly"),
        res = stream_task => warn!(?res, "stream loop exited unexpectedly"),
    }

    Ok(())
}

fn init_tracing() {
    // Writer is stderr so operators can redirect agent logs via stderr (Unix
    // daemon convention) while leaving stdout clean for any future structured
    // output. Initialized before any other code in main() so the first
    // `warn!` / `info!` call is guaranteed to have a live subscriber.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
}

/// Build the client-side TLS config for the CP diagnostics stream.
///
/// Both `FLOWPLANE_AGENT_TLS_CERT_PATH` and `FLOWPLANE_AGENT_TLS_KEY_PATH`
/// must be set together, or both unset. The both-unset case is the
/// *explicit* "agent-started-before-cert-issued" fallback path — it logs
/// a loud WARN rather than silently degrading. A half-configured pair is
/// a hard error because it almost always indicates a misconfiguration.
fn build_tls_config(cfg: &config::AgentConfig) -> anyhow::Result<Option<ClientTlsConfig>> {
    match (&cfg.tls_cert_path, &cfg.tls_key_path) {
        (Some(cert_path), Some(key_path)) => {
            let cert = std::fs::read(cert_path)
                .map_err(|e| anyhow::anyhow!("reading TLS cert {cert_path}: {e}"))?;
            let key = std::fs::read(key_path)
                .map_err(|e| anyhow::anyhow!("reading TLS key {key_path}: {e}"))?;
            let mut tls = ClientTlsConfig::new().identity(Identity::from_pem(cert, key));
            if let Some(ca_path) = &cfg.tls_ca_path {
                let ca = std::fs::read(ca_path)
                    .map_err(|e| anyhow::anyhow!("reading TLS CA {ca_path}: {e}"))?;
                tls = tls.ca_certificate(Certificate::from_pem(ca));
            }
            Ok(Some(tls))
        }
        (None, None) => {
            warn!(
                "flowplane-agent started WITHOUT TLS credentials (fallback path). \
                 This is expected if the agent starts before the SPIFFE cert is issued. \
                 Configure FLOWPLANE_AGENT_TLS_CERT_PATH / FLOWPLANE_AGENT_TLS_KEY_PATH \
                 for prod deployments."
            );
            Ok(None)
        }
        _ => Err(anyhow::anyhow!(
            "FLOWPLANE_AGENT_TLS_CERT_PATH and FLOWPLANE_AGENT_TLS_KEY_PATH must both be set or both unset"
        )),
    }
}
