//! gRPC stream loop: connect to the CP `EnvoyDiagnosticsService`, stream
//! `DiagnosticsReport` messages from the local queue, log `Ack`s.
//!
//! If the connection drops (TCP reset, TLS failure, CP restart, etc.) the
//! loop reconnects with exponential backoff. While disconnected, the
//! polling loop keeps filling the bounded queue — if the queue saturates,
//! the bounded queue evicts oldest-first and logs a WARN per drop.

use crate::backoff::Backoff;
use crate::diagnostics_proto::envoy_diagnostics_service_client::EnvoyDiagnosticsServiceClient;
use crate::diagnostics_proto::DiagnosticsReport;
use crate::queue::BoundedQueue;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tracing::{debug, info, warn};

pub async fn run_stream_loop(
    endpoint: String,
    tls: Option<ClientTlsConfig>,
    queue: Arc<BoundedQueue<DiagnosticsReport>>,
) {
    let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(30));
    loop {
        match connect(&endpoint, tls.clone()).await {
            Ok(channel) => {
                info!("connected to CP diagnostics service");
                backoff.reset();
                match stream_once(channel, queue.clone()).await {
                    Ok(()) => {
                        warn!("diagnostics stream closed cleanly by server, reconnecting");
                    }
                    Err(e) => {
                        warn!(error = %e, "diagnostics stream error, reconnecting");
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to connect to CP diagnostics service");
            }
        }
        let delay = backoff.next_delay();
        debug!(?delay, "backing off before reconnect");
        tokio::time::sleep(delay).await;
    }
}

async fn connect(endpoint: &str, tls: Option<ClientTlsConfig>) -> anyhow::Result<Channel> {
    let mut ep = Endpoint::from_shared(endpoint.to_string())
        .map_err(|e| anyhow::anyhow!("invalid CP endpoint {endpoint}: {e}"))?
        // HTTP/2 keepalive: PING frames detect dead peers when there is
        // no application traffic. Without these, an idle bidi stream
        // (which is exactly what we get after dedup suppresses repeat
        // reports) can stay pending forever after the CP dies, and the
        // reconnect loop never fires. With keepalive, hyper errors the
        // stream within `keep_alive_timeout` of the peer going dark.
        .http2_keep_alive_interval(Duration::from_secs(2))
        .keep_alive_timeout(Duration::from_secs(4))
        .keep_alive_while_idle(true);
    if let Some(t) = tls {
        ep = ep
            .tls_config(t)
            .map_err(|e| anyhow::anyhow!("configuring TLS for CP endpoint: {e}"))?;
    }
    ep.connect().await.map_err(|e| anyhow::anyhow!("connecting to CP endpoint: {e}"))
}

async fn stream_once(
    channel: Channel,
    queue: Arc<BoundedQueue<DiagnosticsReport>>,
) -> anyhow::Result<()> {
    let mut client = EnvoyDiagnosticsServiceClient::new(channel);
    let (tx, rx) = mpsc::channel::<DiagnosticsReport>(32);
    let outbound = ReceiverStream::new(rx);
    let response = client
        .report_diagnostics(tonic::Request::new(outbound))
        .await
        .map_err(|e| anyhow::anyhow!("opening ReportDiagnostics stream: {e}"))?;
    let mut inbound = response.into_inner();

    loop {
        tokio::select! {
            biased;

            // Watchdog: fires when tonic drops the outbound ReceiverStream,
            // which happens when the HTTP/2 stream is torn down by the
            // server (crash, graceful shutdown, TCP reset). Without this
            // arm, `inbound.message()` can stay pending indefinitely on
            // certain failure modes while we busy-loop on the sleep arm —
            // that was the "agent never reconnects after CP drops" bug.
            _ = tx.closed() => {
                return Err(anyhow::anyhow!(
                    "outbound stream closed by peer (server disconnected)"
                ));
            }

            incoming = inbound.message() => {
                match incoming {
                    Ok(Some(ack)) => {
                        debug!(
                            report_ids = ?ack.report_id,
                            status = ack.status,
                            message = %ack.message,
                            "ack from CP"
                        );
                    }
                    Ok(None) => return Ok(()),
                    Err(status) => {
                        return Err(anyhow::anyhow!("stream recv: {status}"));
                    }
                }
            }

            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                while let Some(report) = queue.pop().await {
                    if tx.send(report).await.is_err() {
                        return Err(anyhow::anyhow!("outbound channel closed (server hung up)"));
                    }
                }
            }
        }
    }
}
