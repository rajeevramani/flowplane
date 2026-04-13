//! Polling loop: fetch `/config_dump`, extract errors, enqueue new reports.

use crate::config::AgentConfig;
use crate::config_dump::{extract_error_entries, parse_config_dump, ErrorEntry, ResourceKind};
use crate::dedup::compute_dedup_hash;
use crate::diagnostics_proto::{
    diagnostics_report::Payload, DiagnosticsReport, ListenerStateReport, ResourceType,
};
use crate::queue::BoundedQueue;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};

pub async fn run_poll_loop(
    cfg: Arc<AgentConfig>,
    http: reqwest::Client,
    queue: Arc<BoundedQueue<DiagnosticsReport>>,
) {
    let interval_secs = cfg.poll_interval_secs.max(1);
    let mut tick = tokio::time::interval(Duration::from_secs(interval_secs));
    // First tick fires immediately — skip it so we don't spam a fresh agent
    // with a baseline poll before it has even logged startup.
    tick.tick().await;

    let mut seen_hashes: HashSet<String> = HashSet::new();

    loop {
        tick.tick().await;
        match fetch_config_dump(&http, &cfg.envoy_admin_url).await {
            Ok(body) => match parse_config_dump(&body) {
                Ok(dump) => {
                    let entries = extract_error_entries(&dump);
                    let mut new_seen = HashSet::with_capacity(entries.len());
                    let mut emitted = 0usize;

                    for entry in &entries {
                        let hash = compute_dedup_hash(
                            &cfg.dataplane_id,
                            entry.kind.as_str(),
                            &entry.name,
                            &entry.details,
                        );
                        new_seen.insert(hash.clone());
                        if seen_hashes.contains(&hash) {
                            continue;
                        }
                        let report = build_report(&cfg.dataplane_id, entry, hash);
                        if queue.push(report).await {
                            warn!(
                                queue_cap = cfg.queue_cap,
                                "agent queue full — dropped oldest diagnostic report to make room"
                            );
                        }
                        emitted += 1;
                    }

                    if emitted > 0 {
                        info!(
                            total_errors = entries.len(),
                            new_errors = emitted,
                            "enqueued new diagnostic reports"
                        );
                    } else {
                        debug!(total_errors = entries.len(), "no new errors to report");
                    }

                    seen_hashes = new_seen;
                }
                Err(e) => {
                    warn!(error = %e, "failed to parse /config_dump JSON");
                }
            },
            Err(e) => {
                warn!(error = %e, "failed to fetch /config_dump");
            }
        }
    }
}

async fn fetch_config_dump(
    http: &reqwest::Client,
    admin_url: &str,
) -> Result<String, reqwest::Error> {
    let url = format!("{}/config_dump", admin_url.trim_end_matches('/'));
    http.get(&url).send().await?.error_for_status()?.text().await
}

fn build_report(dataplane_id: &str, entry: &ErrorEntry, hash: String) -> DiagnosticsReport {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let observed_at = Some(prost_types::Timestamp {
        seconds: now.as_secs() as i64,
        nanos: now.subsec_nanos() as i32,
    });
    let resource_type = match entry.kind {
        ResourceKind::Listener => ResourceType::Listener,
        ResourceKind::Cluster => ResourceType::Cluster,
        ResourceKind::RouteConfig => ResourceType::RouteConfig,
    };
    let state = ListenerStateReport {
        resource_type: resource_type as i32,
        resource_name: entry.name.clone(),
        error_details: entry.details.clone(),
        // Intentionally None — the MVP agent does not forward the parsed
        // last_update_attempt timestamp (it is excluded from dedup by
        // design, and skipping it keeps the agent dep footprint small).
        last_update_attempt: None,
        failed_config_hash: hash,
    };
    DiagnosticsReport {
        schema_version: 1,
        report_id: uuid::Uuid::new_v4().to_string(),
        dataplane_id: dataplane_id.to_string(),
        observed_at,
        payload: Some(Payload::ListenerState(state)),
    }
}
