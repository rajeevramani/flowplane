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
    let mut total_requests = 0;
    let mut total_errors = 0;
    let mut warming_failures = 0;
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
            total_requests += dataplane.total_requests;
            total_errors += dataplane.total_errors;
            warming_failures += dataplane.warming_failures;
            DataplaneXdsStatus { dataplane, live }
        })
        .collect::<Vec<_>>();
    let total_dataplanes = dataplanes.len() as i64;

    Ok(XdsStatus {
        total_dataplanes,
        live_dataplanes,
        stale_dataplanes: total_dataplanes - live_dataplanes,
        config_verified_dataplanes,
        total_requests,
        total_errors,
        warming_failures,
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
    let limit = query.limit.clamp(1, 200);
    let audit = fp_storage::repos::audit::trace_rows(
        pool,
        team.id,
        query.request_id,
        query.path.as_deref(),
        limit,
    )
    .await?;
    let events = fp_storage::outbox::trace_rows(
        pool,
        team.id,
        query.trace_id.as_deref(),
        query.path.as_deref(),
        limit,
    )
    .await?;
    Ok(OpsTrace { audit, events })
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
