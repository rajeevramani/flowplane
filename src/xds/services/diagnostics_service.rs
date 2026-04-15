//! EnvoyDiagnosticsService gRPC implementation (fp-hsk MVP).
//!
//! Receives `DiagnosticsReport` streams from `flowplane-agent` sidecars next to
//! Envoy instances. For the MVP the only payload variant persisted is
//! `ListenerStateReport`, which captures listener warming failures observed via
//! Envoy's `/config_dump` — errors that the main xDS stream never surfaces
//! because Envoy ACKs the update before failing the warmup internally.
//!
//! ## Failure isolation
//!
//! This service is strictly advisory. Any stream failure here (dropped agent
//! connection, bad cert, DB hiccup) MUST NEVER affect the xDS stream for the
//! same dataplane. The only shared state with the xDS path is the repository
//! handle (`NackEventRepository`) and the raw DB pool, both used read-only from
//! the xDS side and write-only from here. There is no inter-stream coupling.
//!
//! ## Auth model
//!
//! The agent connects to the same gRPC endpoint as xDS and presents the same
//! SPIFFE client certificate. We reuse `extract_client_identity` from
//! `services::mtls`. The envelope's `dataplane_id` MUST equal the SPIFFE
//! `proxy_id` of the presented cert — agents cannot report on behalf of other
//! dataplanes. Missing cert → gRPC `UNAUTHENTICATED`. Mismatched id →
//! per-report `ACK_STATUS_UNAUTHORIZED` on the stream.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};

use crate::errors::FlowplaneError;
use crate::storage::repositories::{
    CreateNackEventRequest, DataplaneRepository, NackEventRepository, NackSource,
};
use crate::storage::DbPool;
use crate::xds::resources::{CLUSTER_TYPE_URL, LISTENER_TYPE_URL, ROUTE_TYPE_URL};
use crate::xds::services::diagnostics_proto::{
    diagnostics_report, envoy_diagnostics_service_server, Ack, AckStatus, DiagnosticsReport,
    ListenerStateReport, ResourceType,
};
use crate::xds::services::mtls::extract_client_identity;

const SECRET_TYPE_URL: &str =
    "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.Secret";

/// Flowplane implementation of `EnvoyDiagnosticsService`.
#[derive(Clone)]
pub struct FlowplaneDiagnosticsService {
    nack_events: NackEventRepository,
    // Retained in the constructor for call-site stability and future use
    // (e.g. enriching audit log entries with dataplane metadata). The service
    // itself MUST NOT gate report persistence on dataplane row existence —
    // see the `handle_envelope` doc comment for why.
    #[allow(dead_code)]
    dataplanes: DataplaneRepository,
    pool: DbPool,
}

impl std::fmt::Debug for FlowplaneDiagnosticsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowplaneDiagnosticsService").finish()
    }
}

impl FlowplaneDiagnosticsService {
    pub fn new(
        nack_events: NackEventRepository,
        dataplanes: DataplaneRepository,
        pool: DbPool,
    ) -> Self {
        Self { nack_events, dataplanes, pool }
    }
}

/// Outcome of validating a single envelope before payload dispatch.
#[derive(Debug, PartialEq, Eq)]
enum EnvelopeValidation {
    /// Valid envelope — proceed with payload dispatch.
    Ok,
    /// Envelope missing required fields.
    Invalid(&'static str),
    /// The envelope's dataplane_id does not match the authenticated identity.
    Unauthorized,
}

fn validate_envelope(
    envelope: &DiagnosticsReport,
    authenticated_dataplane_id: Option<&str>,
) -> EnvelopeValidation {
    if envelope.dataplane_id.is_empty() {
        return EnvelopeValidation::Invalid("envelope.dataplane_id is empty");
    }
    if envelope.report_id.is_empty() {
        return EnvelopeValidation::Invalid("envelope.report_id is empty");
    }
    if envelope.schema_version == 0 {
        return EnvelopeValidation::Invalid("envelope.schema_version is zero");
    }
    if let Some(authed) = authenticated_dataplane_id {
        if authed != envelope.dataplane_id {
            return EnvelopeValidation::Unauthorized;
        }
    }
    EnvelopeValidation::Ok
}

/// Map a `ResourceType` from the proto to an xDS type URL for NACK persistence.
fn resource_type_to_type_url(rt: ResourceType) -> Option<&'static str> {
    match rt {
        ResourceType::Listener => Some(LISTENER_TYPE_URL),
        ResourceType::Cluster => Some(CLUSTER_TYPE_URL),
        ResourceType::RouteConfig => Some(ROUTE_TYPE_URL),
        ResourceType::Secret => Some(SECRET_TYPE_URL),
        ResourceType::Unspecified => None,
    }
}

/// Compute the deduplication hash used to collapse repeated warming-failure
/// reports for the same (dataplane, resource) into a single NACK row.
///
/// Stability contract: the hash is computed over (dataplane_id, resource_type,
/// resource_name, error_details). `last_update_attempt` is deliberately
/// excluded so that a resource that keeps failing with the same error every
/// warmup cycle produces the same hash and can be deduped at the DB layer via
/// a unique index on `dedup_hash`.
fn compute_dedup_hash(
    dataplane_id: &str,
    resource_type: ResourceType,
    resource_name: &str,
    error_details: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(dataplane_id.as_bytes());
    hasher.update([0u8]);
    hasher.update((resource_type as i32).to_be_bytes());
    hasher.update([0u8]);
    hasher.update(resource_name.as_bytes());
    hasher.update([0u8]);
    hasher.update(error_details.as_bytes());
    hex::encode(hasher.finalize())
}

/// Single-report processing outcome, mapped to an Ack status.
#[derive(Debug)]
struct ReportOutcome {
    status: AckStatus,
    message: Option<String>,
}

impl ReportOutcome {
    fn ok() -> Self {
        Self { status: AckStatus::Ok, message: None }
    }
    fn invalid(msg: impl Into<String>) -> Self {
        Self { status: AckStatus::Invalid, message: Some(msg.into()) }
    }
    fn unauthorized(msg: impl Into<String>) -> Self {
        Self { status: AckStatus::Unauthorized, message: Some(msg.into()) }
    }
    #[allow(dead_code)] // reinstated when a second payload variant ships
    fn unknown_payload() -> Self {
        Self {
            status: AckStatus::UnknownPayload,
            message: Some("payload variant not recognised by this CP".to_string()),
        }
    }
    fn retry(msg: impl Into<String>) -> Self {
        Self { status: AckStatus::Retry, message: Some(msg.into()) }
    }
}

impl FlowplaneDiagnosticsService {
    /// Best-effort update of `dataplanes.last_config_verify`. Failures are
    /// logged and swallowed so that an audit-column failure cannot mask or
    /// fail a NACK persist (or vice versa).
    ///
    /// `team_name` is the team name extracted from the agent's SPIFFE URI
    /// (see `extract_client_identity`). The `dataplanes.team` column stores a
    /// team **id** (FK to `teams.id`) after migration
    /// `20260207000002_switch_team_fk_to_team_id.sql`, so this query joins
    /// `teams` to resolve name → id rather than comparing a name against an
    /// id column (which silently matches zero rows — see decision doc
    /// `specs/decisions/2026-04-14-fp-4n5-dataplanes-team-id-mismatch.md`).
    async fn touch_last_config_verify(&self, dataplane_name: &str, team_name: &str) {
        let now = chrono::Utc::now();
        let res = sqlx::query(
            "UPDATE dataplanes \
             SET last_config_verify = $1, updated_at = $1 \
             FROM teams \
             WHERE dataplanes.team = teams.id \
               AND dataplanes.name = $2 \
               AND teams.name = $3",
        )
        .bind(now)
        .bind(dataplane_name)
        .bind(team_name)
        .execute(&self.pool)
        .await;

        match res {
            Ok(r) if r.rows_affected() > 0 => {
                debug!(
                    dataplane = %dataplane_name,
                    team = %team_name,
                    "Updated last_config_verify"
                );
            }
            Ok(_) => {
                // Zero rows affected means either (a) the dataplane row has
                // not been registered yet for this team (legitimate during
                // agent-first bring-up) or (b) a schema/identity drift we
                // want to surface loudly. We intentionally warn (not debug)
                // so operators see silent-failure modes in `flowplane xds
                // status` — the bug this fix closed was invisible for weeks
                // because the equivalent log line was at debug level.
                warn!(
                    dataplane = %dataplane_name,
                    team = %team_name,
                    "last_config_verify update matched zero rows — dataplane \
                     either not yet registered for this team or SPIFFE team \
                     does not resolve to a known team row"
                );
            }
            Err(e) => {
                warn!(
                    error = %e,
                    dataplane = %dataplane_name,
                    team = %team_name,
                    "Best-effort last_config_verify update failed — continuing"
                );
            }
        }
    }

    /// Persist a `ListenerStateReport` as a `warming_report` NACK event.
    async fn persist_listener_state(
        &self,
        envelope_dataplane_id: &str,
        team: &str,
        report: &ListenerStateReport,
    ) -> ReportOutcome {
        let resource_type =
            ResourceType::try_from(report.resource_type).unwrap_or(ResourceType::Unspecified);
        let type_url = match resource_type_to_type_url(resource_type) {
            Some(url) => url.to_string(),
            None => return ReportOutcome::invalid("resource_type unspecified/unknown"),
        };
        if report.resource_name.is_empty() {
            return ReportOutcome::invalid("resource_name is empty");
        }
        if report.error_details.is_empty() {
            // Informational state snapshots are not errors — we only persist
            // reports that carry an actual failure description.
            debug!(
                dataplane = %envelope_dataplane_id,
                resource = %report.resource_name,
                "Listener state report has no error_details; skipping NACK persist"
            );
            return ReportOutcome::ok();
        }

        // Prefer the agent-provided failed_config_hash when present (it hashes
        // the raw rejected bytes, which is stronger than our field hash).
        // Otherwise compute a stable hash over the logical fields.
        let dedup_hash = if report.failed_config_hash.is_empty() {
            compute_dedup_hash(
                envelope_dataplane_id,
                resource_type,
                &report.resource_name,
                &report.error_details,
            )
        } else {
            report.failed_config_hash.clone()
        };

        let request = CreateNackEventRequest {
            team: team.to_string(),
            dataplane_name: envelope_dataplane_id.to_string(),
            type_url,
            version_rejected: None,
            nonce: None,
            // Envoy's warming path does not surface a numeric error code via
            // /config_dump — 0 is the conventional "no code" value.
            error_code: 0,
            error_message: report.error_details.clone(),
            node_id: None,
            resource_names: Some(format!("[\"{}\"]", report.resource_name)),
            source: NackSource::WarmingReport,
            dedup_hash: Some(dedup_hash),
        };

        match self.nack_events.insert(request).await {
            Ok(_) => ReportOutcome::ok(),
            Err(FlowplaneError::Database { ref source, .. })
                if matches!(
                    source,
                    sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some("23505")
                ) =>
            {
                // Unique-violation on xds_nack_events_dedup_hash_idx is
                // idempotent success: the winning INSERT already persisted
                // the row with the correct data (same dedup_hash, same
                // resource info), and there is no updated_at column to
                // refresh. Race-safe: two concurrent inserts with the same
                // dedup_hash → one wins, the other takes this path.
                debug!(
                    dataplane = %envelope_dataplane_id,
                    "Dedup hit on warming NACK (23505 on dedup_hash) — idempotent OK"
                );
                ReportOutcome::ok()
            }
            Err(e) => {
                error!(error = %e, dataplane = %envelope_dataplane_id, "Failed to persist warming NACK");
                ReportOutcome::retry("failed to persist NACK")
            }
        }
    }

    /// Dispatch a fully-validated envelope to the payload-specific handler.
    ///
    /// The `authed_team` comes from the authenticated SPIFFE identity and is
    /// the authoritative tenancy attribution — we deliberately do NOT look the
    /// dataplane up in the `dataplanes` table to derive it. The cert is the
    /// auth boundary; the dataplanes row is advisory liveness tracking. During
    /// agent-first bring-up (agent connects before the CP row is registered)
    /// warming failures are MOST likely, and silently dropping them because
    /// the row doesn't exist yet would defeat the whole point of this service.
    async fn handle_envelope(
        &self,
        envelope: DiagnosticsReport,
        authed_team: &str,
    ) -> ReportOutcome {
        // Best-effort liveness refresh. Independent from NACK persistence:
        // failure here (e.g. dataplane row not yet registered) MUST NOT fail
        // the report. `touch_last_config_verify` already logs+swallows errors
        // and tolerates zero-row updates.
        self.touch_last_config_verify(&envelope.dataplane_id, authed_team).await;

        // Dispatch. `None` covers two cases: (a) the agent sent an empty
        // envelope (INVALID), or (b) prost dropped an unknown oneof variant
        // from a newer agent (forward-compat path → UNKNOWN_PAYLOAD). The two
        // are indistinguishable after decode, so for MVP we follow the bead
        // spec and return INVALID. When a second variant is added to the
        // oneof, re-introduce the `Some(_) =>` arm mapping to UNKNOWN_PAYLOAD.
        match envelope.payload {
            Some(diagnostics_report::Payload::ListenerState(report)) => {
                self.persist_listener_state(&envelope.dataplane_id, authed_team, &report).await
            }
            None => ReportOutcome::invalid("payload oneof is empty"),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn _unknown_payload_outcome_is_reachable() -> ReportOutcome {
        ReportOutcome::unknown_payload()
    }
}

#[async_trait]
impl envoy_diagnostics_service_server::EnvoyDiagnosticsService for FlowplaneDiagnosticsService {
    type ReportDiagnosticsStream =
        Pin<Box<dyn Stream<Item = Result<Ack, Status>> + Send + 'static>>;

    async fn report_diagnostics(
        &self,
        request: Request<Streaming<DiagnosticsReport>>,
    ) -> Result<Response<Self::ReportDiagnosticsStream>, Status> {
        // Extract SPIFFE identity from peer certs. An agent without a valid
        // cert is rejected at the gRPC layer before any messages are read.
        let peer_certs = request.peer_certs();
        let identity = match peer_certs.as_deref() {
            Some(certs) if !certs.is_empty() => extract_client_identity(certs),
            _ => None,
        };
        let authenticated_dataplane_id = identity.as_ref().map(|i| i.proxy_id.clone());
        let authenticated_team = identity.as_ref().map(|i| i.team.clone());

        if authenticated_dataplane_id.is_none() {
            // If mTLS is not configured we still refuse to blindly trust
            // envelope contents for this service — this stream is strictly
            // advisory and tighter-than-xDS auth is acceptable.
            info!("Diagnostics stream rejected: no authenticated SPIFFE identity");
            return Err(Status::unauthenticated(
                "diagnostics stream requires mTLS SPIFFE identity",
            ));
        }

        info!(
            proxy_id = ?authenticated_dataplane_id,
            team = ?identity.as_ref().map(|i| &i.team),
            "EnvoyDiagnosticsService: new stream"
        );

        let (ack_tx, ack_rx) = mpsc::unbounded_channel::<Result<Ack, Status>>();
        let mut stream = request.into_inner();
        let svc = self.clone();
        let authed = Arc::new(authenticated_dataplane_id);
        let authed_team = Arc::new(authenticated_team.unwrap_or_default());

        tokio::spawn(async move {
            loop {
                match stream.message().await {
                    Ok(Some(envelope)) => {
                        let report_id = envelope.report_id.clone();
                        let outcome = match validate_envelope(&envelope, authed.as_deref()) {
                            EnvelopeValidation::Ok => {
                                svc.handle_envelope(envelope, authed_team.as_str()).await
                            }
                            EnvelopeValidation::Invalid(reason) => {
                                warn!(reason, "Rejecting malformed diagnostics envelope");
                                ReportOutcome::invalid(reason)
                            }
                            EnvelopeValidation::Unauthorized => {
                                warn!(
                                    envelope_dp = %envelope.dataplane_id,
                                    authed_dp = ?authed.as_deref(),
                                    "Rejecting cross-dataplane diagnostics report"
                                );
                                ReportOutcome::unauthorized(
                                    "envelope.dataplane_id does not match SPIFFE identity",
                                )
                            }
                        };

                        let ack = Ack {
                            report_id: vec![report_id],
                            status: outcome.status as i32,
                            message: outcome.message.unwrap_or_default(),
                        };
                        if ack_tx.send(Ok(ack)).is_err() {
                            debug!("Diagnostics ack channel closed — agent disconnected");
                            break;
                        }
                    }
                    Ok(None) => {
                        debug!("Diagnostics stream closed by agent");
                        break;
                    }
                    Err(e) => {
                        warn!(error = %e, "Diagnostics stream recv error");
                        break;
                    }
                }
            }
        });

        let out_stream = UnboundedReceiverStream::new(ack_rx);
        Ok(Response::new(Box::pin(out_stream) as Self::ReportDiagnosticsStream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::Timestamp;

    fn env(dp: &str, report_id: &str) -> DiagnosticsReport {
        DiagnosticsReport {
            schema_version: 1,
            report_id: report_id.to_string(),
            dataplane_id: dp.to_string(),
            observed_at: Some(Timestamp { seconds: 100, nanos: 0 }),
            payload: Some(diagnostics_report::Payload::ListenerState(ListenerStateReport {
                resource_type: ResourceType::Listener as i32,
                resource_name: "my-listener".to_string(),
                error_details: "malformed filter".to_string(),
                last_update_attempt: Some(Timestamp { seconds: 50, nanos: 0 }),
                failed_config_hash: String::new(),
            })),
        }
    }

    #[test]
    fn validate_envelope_happy_path_unauthenticated_stream_accepted() {
        // When the stream is unauthenticated at the envelope layer (caller
        // passes None), validation should still require non-empty fields but
        // skip the identity check. The service entrypoint enforces the
        // mTLS-required check before this function is called.
        let envelope = env("dp-1", "r-1");
        assert_eq!(validate_envelope(&envelope, None), EnvelopeValidation::Ok);
    }

    #[test]
    fn validate_envelope_matches_authenticated_identity() {
        let envelope = env("dp-a", "r-1");
        assert_eq!(validate_envelope(&envelope, Some("dp-a")), EnvelopeValidation::Ok);
    }

    #[test]
    fn validate_envelope_rejects_id_mismatch() {
        let envelope = env("dp-a", "r-1");
        assert_eq!(validate_envelope(&envelope, Some("dp-b")), EnvelopeValidation::Unauthorized);
    }

    #[test]
    fn validate_envelope_rejects_empty_dataplane_id() {
        let envelope = env("", "r-1");
        match validate_envelope(&envelope, None) {
            EnvelopeValidation::Invalid(_) => {}
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_envelope_rejects_empty_report_id() {
        let envelope = env("dp-1", "");
        match validate_envelope(&envelope, None) {
            EnvelopeValidation::Invalid(_) => {}
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn validate_envelope_rejects_zero_schema_version() {
        let mut envelope = env("dp-1", "r-1");
        envelope.schema_version = 0;
        match validate_envelope(&envelope, None) {
            EnvelopeValidation::Invalid(_) => {}
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn dedup_hash_is_stable_across_calls() {
        let a = compute_dedup_hash("dp-1", ResourceType::Listener, "foo", "err");
        let b = compute_dedup_hash("dp-1", ResourceType::Listener, "foo", "err");
        assert_eq!(a, b);
    }

    #[test]
    fn dedup_hash_ignores_last_update_attempt() {
        // `compute_dedup_hash` takes no timestamp argument by design — this
        // test documents the contract that timestamps must not affect the
        // hash so that repeated reports of the same failure dedup at the DB.
        let a = compute_dedup_hash("dp-1", ResourceType::Listener, "foo", "err");
        let b = compute_dedup_hash("dp-1", ResourceType::Listener, "foo", "err");
        assert_eq!(a, b);
    }

    #[test]
    fn dedup_hash_differs_on_dataplane_id() {
        let a = compute_dedup_hash("dp-1", ResourceType::Listener, "foo", "err");
        let b = compute_dedup_hash("dp-2", ResourceType::Listener, "foo", "err");
        assert_ne!(a, b);
    }

    #[test]
    fn dedup_hash_differs_on_resource_name() {
        let a = compute_dedup_hash("dp-1", ResourceType::Listener, "foo", "err");
        let b = compute_dedup_hash("dp-1", ResourceType::Listener, "bar", "err");
        assert_ne!(a, b);
    }

    #[test]
    fn dedup_hash_differs_on_error_details() {
        let a = compute_dedup_hash("dp-1", ResourceType::Listener, "foo", "err-a");
        let b = compute_dedup_hash("dp-1", ResourceType::Listener, "foo", "err-b");
        assert_ne!(a, b);
    }

    #[test]
    fn dedup_hash_differs_on_resource_type() {
        let a = compute_dedup_hash("dp-1", ResourceType::Listener, "foo", "err");
        let b = compute_dedup_hash("dp-1", ResourceType::Cluster, "foo", "err");
        assert_ne!(a, b);
    }

    #[test]
    fn resource_type_to_type_url_covers_known_types() {
        assert_eq!(resource_type_to_type_url(ResourceType::Listener), Some(LISTENER_TYPE_URL));
        assert_eq!(resource_type_to_type_url(ResourceType::Cluster), Some(CLUSTER_TYPE_URL));
        assert_eq!(resource_type_to_type_url(ResourceType::RouteConfig), Some(ROUTE_TYPE_URL));
        assert_eq!(resource_type_to_type_url(ResourceType::Secret), Some(SECRET_TYPE_URL));
        assert!(resource_type_to_type_url(ResourceType::Unspecified).is_none());
    }
}
