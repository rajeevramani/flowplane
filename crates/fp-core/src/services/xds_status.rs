//! xDS health surfacing (S5.5): per-team NACK history. Read-only; quarantine state lives
//! in the xDS snapshot cache and reaches operators through these persisted events.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{deny_to_error, record_authz_denial};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::dataplane::Dataplane;
use fp_domain::{DomainError, DomainResult, RequestId};
use fp_storage::outbox::EventTraceRow;
use fp_storage::repos::audit::AuditTraceRow;
use fp_storage::repos::xds_nacks::NackEvent;
use sqlx::PgPool;

const LIVE_HEARTBEAT_SECONDS: i64 = 60;
const RECENT_NACK_MINUTES: i64 = 15;
const MIN_TRACE_PATH_QUERY_LEN: usize = 3;
const MAX_TRACE_PATH_QUERY_LEN: usize = 256;
const MIN_TRACE_ID_QUERY_LEN: usize = 3;
const MAX_TRACE_ID_QUERY_LEN: usize = 256;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct XdsCounterTotals {
    total_requests: i64,
    total_errors: i64,
    warming_failures: i64,
}

impl XdsCounterTotals {
    fn add_dataplane(&mut self, dataplane: &Dataplane) {
        self.total_requests = self.total_requests.saturating_add(dataplane.total_requests);
        self.total_errors = self.total_errors.saturating_add(dataplane.total_errors);
        self.warming_failures = self
            .warming_failures
            .saturating_add(dataplane.warming_failures);
    }
}

#[derive(Debug, Clone)]
pub struct XdsStatus {
    pub total_dataplanes: i64,
    pub live_dataplanes: i64,
    pub stale_dataplanes: i64,
    pub config_verified_dataplanes: i64,
    pub total_requests: i64,
    pub total_errors: i64,
    pub warming_failures: i64,
    pub recent_nack_count: i64,
    pub latest_nack: Option<NackEvent>,
    pub dataplanes: Vec<DataplaneXdsStatus>,
}

#[derive(Debug, Clone)]
pub struct DataplaneXdsStatus {
    pub dataplane: Dataplane,
    pub live: bool,
}

#[derive(Debug, Clone)]
pub struct TraceQuery {
    pub request_id: Option<RequestId>,
    pub trace_id: Option<String>,
    pub path: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct OpsTrace {
    pub audit: Vec<AuditTraceRow>,
    pub events: Vec<EventTraceRow>,
}

pub async fn list_nack_events(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    request_id: RequestId,
) -> DomainResult<Vec<NackEvent>> {
    match check_resource_access(ctx, Resource::Stats, Action::Read, Some(team)) {
        Decision::Allow(_) => {}
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::Stats,
                Action::Read,
                Some(team),
                reason,
            )
            .await;
            return Err(deny_to_error(Resource::Stats, Action::Read, reason));
        }
    }
    fp_storage::repos::xds_nacks::list(pool, team.id, limit).await
}

pub async fn status(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<XdsStatus> {
    authorize_read(pool, ctx, team, request_id).await?;
    let (dataplanes, _total) =
        fp_storage::repos::dataplanes::list_dataplanes(pool, team.id, 500, 0).await?;
    let recent_nack_count =
        fp_storage::repos::xds_nacks::count_recent(pool, team.id, RECENT_NACK_MINUTES).await?;
    let latest_nack = fp_storage::repos::xds_nacks::list(pool, team.id, 1)
        .await?
        .into_iter()
        .next();

    let now = chrono::Utc::now();
    let mut live_dataplanes = 0;
    let mut config_verified_dataplanes = 0;
    let mut counters = XdsCounterTotals::default();
    let dataplanes = dataplanes
        .into_iter()
        .map(|dataplane| {
            let live = dataplane
                .last_heartbeat_at
                .map(|ts| (now - ts).num_seconds() <= LIVE_HEARTBEAT_SECONDS)
                .unwrap_or(false);
            if live {
                live_dataplanes += 1;
            }
            if dataplane.last_config_verify_at.is_some() {
                config_verified_dataplanes += 1;
            }
            counters.add_dataplane(&dataplane);
            DataplaneXdsStatus { dataplane, live }
        })
        .collect::<Vec<_>>();
    let total_dataplanes = dataplanes.len() as i64;

    Ok(XdsStatus {
        total_dataplanes,
        live_dataplanes,
        stale_dataplanes: total_dataplanes - live_dataplanes,
        config_verified_dataplanes,
        total_requests: counters.total_requests,
        total_errors: counters.total_errors,
        warming_failures: counters.warming_failures,
        recent_nack_count,
        latest_nack,
        dataplanes,
    })
}

pub async fn trace(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    query: TraceQuery,
    request_id: RequestId,
) -> DomainResult<OpsTrace> {
    authorize_read(pool, ctx, team, request_id).await?;
    if query.request_id.is_none() && query.trace_id.is_none() && query.path.is_none() {
        return Err(
            DomainError::validation("provide one of request_id, trace_id, or path").with_hint(
                "try --request-id from an error, --trace-id from logs, or --path clusters/name",
            ),
        );
    }
    let path = bounded_trace_path(query.path.as_deref())?;
    let trace_id = bounded_trace_id(query.trace_id.as_deref())?;
    let limit = query.limit.clamp(1, 200);
    let audit = fp_storage::repos::audit::trace_rows(
        pool,
        team.id,
        query.request_id,
        path.as_deref(),
        limit,
    )
    .await?;
    let events =
        fp_storage::outbox::trace_rows(pool, team.id, trace_id.as_deref(), path.as_deref(), limit)
            .await?;
    Ok(OpsTrace { audit, events })
}

fn bounded_trace_path(path: Option<&str>) -> DomainResult<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if path.chars().any(char::is_control) {
        return Err(DomainError::validation(
            "trace path search must not contain control characters",
        ));
    }
    let path = path.trim();
    if path.len() < MIN_TRACE_PATH_QUERY_LEN || path.len() > MAX_TRACE_PATH_QUERY_LEN {
        return Err(DomainError::validation(format!(
            "trace path search must be {MIN_TRACE_PATH_QUERY_LEN}-{MAX_TRACE_PATH_QUERY_LEN} characters"
        ))
        .with_hint("use a request_id or trace_id for exact correlation, or a longer resource/path fragment"));
    }
    Ok(Some(path.to_string()))
}

fn bounded_trace_id(trace_id: Option<&str>) -> DomainResult<Option<String>> {
    let Some(trace_id) = trace_id else {
        return Ok(None);
    };
    if trace_id.chars().any(char::is_control) {
        return Err(DomainError::validation(
            "trace id search must not contain control characters",
        ));
    }
    let trace_id = trace_id.trim();
    if trace_id.len() < MIN_TRACE_ID_QUERY_LEN || trace_id.len() > MAX_TRACE_ID_QUERY_LEN {
        return Err(DomainError::validation(format!(
            "trace id search must be {MIN_TRACE_ID_QUERY_LEN}-{MAX_TRACE_ID_QUERY_LEN} characters"
        ))
        .with_hint("use a request_id for exact correlation, or a longer trace id fragment"));
    }
    Ok(Some(trace_id.to_string()))
}

async fn authorize_read(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::Stats, Action::Read, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::Stats,
                Action::Read,
                Some(team),
                reason,
            )
            .await;
            Err(deny_to_error(Resource::Stats, Action::Read, reason))
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn dataplane_counters(
        total_requests: i64,
        total_errors: i64,
        warming_failures: i64,
    ) -> Dataplane {
        let now = chrono::Utc::now();
        Dataplane {
            id: fp_domain::DataplaneId::generate(),
            team_id: fp_domain::TeamId::generate(),
            name: "dp".into(),
            description: String::new(),
            version: 1,
            last_heartbeat_at: None,
            last_config_verify_at: None,
            total_requests,
            total_errors,
            warming_failures,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn xds_counter_totals_saturate_instead_of_overflowing() {
        let mut counters = XdsCounterTotals::default();
        counters.add_dataplane(&dataplane_counters(i64::MAX - 1, i64::MAX - 2, 7));
        counters.add_dataplane(&dataplane_counters(10, 20, i64::MAX));

        assert_eq!(
            counters,
            XdsCounterTotals {
                total_requests: i64::MAX,
                total_errors: i64::MAX,
                warming_failures: i64::MAX,
            }
        );
    }

    #[test]
    fn trace_path_search_is_bounded() {
        assert_eq!(
            bounded_trace_path(Some("  clusters/orders  "))
                .expect("valid")
                .as_deref(),
            Some("clusters/orders")
        );
        assert!(bounded_trace_path(Some("/")).is_err());
        assert!(bounded_trace_path(Some(&"x".repeat(MAX_TRACE_PATH_QUERY_LEN + 1))).is_err());
        assert!(bounded_trace_path(Some("abc\n")).is_err());
        assert_eq!(bounded_trace_path(None).expect("none"), None);
    }

    #[test]
    fn trace_id_search_is_bounded() {
        assert_eq!(
            bounded_trace_id(Some("  abc123  "))
                .expect("valid")
                .as_deref(),
            Some("abc123")
        );
        assert!(bounded_trace_id(Some("%")).is_err());
        assert!(bounded_trace_id(Some(&"x".repeat(MAX_TRACE_ID_QUERY_LEN + 1))).is_err());
        assert!(bounded_trace_id(Some("abc\n")).is_err());
        assert_eq!(bounded_trace_id(None).expect("none"), None);
    }
}
