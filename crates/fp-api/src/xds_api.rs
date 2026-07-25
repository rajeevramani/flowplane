//! xDS health surface (S5.5): per-team NACK/quarantine history. Read-only — what a
//! dataplane rejected, when, and which resources are degraded (serving last-good bytes).

use crate::error::{ApiError, ErrorBody};
use crate::resources::resolve_team;
use crate::state::AppState;
use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use fp_core::PrincipalCtx;
use fp_domain::RequestId;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct NackEventView {
    pub id: uuid::Uuid,
    /// Envoy node id of the rejecting dataplane (attribution only).
    pub node_id: String,
    pub type_url: String,
    pub version_rejected: String,
    pub error_message: String,
    /// Resources quarantined by this NACK (now serving their last-good bytes, or held
    /// out of the snapshot when they were new). Empty when attribution was impossible.
    pub quarantined_resources: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<fp_storage::repos::xds_nacks::NackEvent> for NackEventView {
    fn from(e: fp_storage::repos::xds_nacks::NackEvent) -> Self {
        Self {
            id: e.id,
            node_id: e.node_id,
            type_url: e.type_url,
            version_rejected: e.version_rejected,
            error_message: e.error_message,
            quarantined_resources: e.quarantined_resources,
            created_at: e.created_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct XdsStatusView {
    pub health: String,
    pub total_dataplanes: i64,
    pub live_dataplanes: i64,
    pub stale_dataplanes: i64,
    pub config_verified_dataplanes: i64,
    pub total_requests: i64,
    pub total_errors: i64,
    pub warming_failures: i64,
    pub recent_nack_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_nack: Option<NackEventView>,
    pub dataplanes: Vec<DataplaneXdsStatusView>,
}

#[derive(Serialize, ToSchema)]
pub struct DataplaneXdsStatusView {
    pub name: String,
    pub id: String,
    pub live: bool,
    pub version: i64,
    pub last_heartbeat_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_config_verify_at: Option<chrono::DateTime<chrono::Utc>>,
    pub total_requests: i64,
    pub total_errors: i64,
    pub warming_failures: i64,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct TraceParams {
    /// API request id from an error body or response header.
    pub request_id: Option<String>,
    /// W3C trace id to match against persisted outbox trace context.
    pub trace_id: Option<String>,
    /// Resource/path substring to match in persisted audit or outbox rows.
    pub path: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct OpsTraceView {
    pub audit: Vec<AuditTraceView>,
    pub events: Vec<EventTraceView>,
}

#[derive(Serialize, ToSchema)]
pub struct AuditTraceView {
    pub id: uuid::Uuid,
    pub request_id: Option<String>,
    pub actor_label: String,
    pub surface: String,
    pub action: String,
    pub resource: String,
    pub outcome: String,
    pub detail: serde_json::Value,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct EventTraceView {
    pub seq: i64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub trace_context: serde_json::Value,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

/// Query filters for the NACK-history window (fpv2-55x.1). `since`/`until` are RFC-3339 timestamps
/// forming a half-open interval `[since, until)`; `before` is the opaque cursor from a prior page's
/// `next_cursor`; `limit` defaults to 50, clamped to 200.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct NackListParams {
    /// Inclusive lower bound (RFC 3339). Rows with `created_at >= since`.
    pub since: Option<String>,
    /// Exclusive upper bound (RFC 3339). Rows with `created_at < until`.
    pub until: Option<String>,
    /// Page size; default 50, clamped to 1..=200.
    pub limit: Option<i64>,
    /// Opaque cursor from a prior response's `next_cursor` (`<created_at>,<id>`).
    pub before: Option<String>,
}

/// A page of NACK history: the rows, the window-relative total, and the cursor to the next page
/// (absent on the last page).
#[derive(Serialize, ToSchema)]
pub struct NackPage {
    pub items: Vec<NackEventView>,
    /// Count of rows matching `since`/`until` (window-relative, not the whole collection).
    pub window_total: i64,
    /// Cursor to fetch the next (older) page; omitted on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Encode a `(created_at, id)` cursor as `<rfc3339-nanos>,<uuid>`. Nanosecond precision preserves
/// the full stored `TIMESTAMPTZ` so the continuation predicate identifies the exact boundary row.
/// Shared with the MCP `ops_xds_nacks` tool so REST and MCP emit an identical cursor format.
pub(crate) fn encode_nack_cursor(
    created_at: chrono::DateTime<chrono::Utc>,
    id: uuid::Uuid,
) -> String {
    format!(
        "{},{}",
        created_at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        id
    )
}

/// Parse a `before` cursor back into `(created_at, id)`. A malformed cursor is a client error.
/// Shared with the MCP `ops_xds_nacks` tool so REST and MCP parse an identical cursor format.
pub(crate) fn decode_nack_cursor(
    raw: &str,
) -> Result<(chrono::DateTime<chrono::Utc>, uuid::Uuid), fp_domain::DomainError> {
    let (ts, id) = raw.split_once(',').ok_or_else(|| {
        fp_domain::DomainError::validation("before cursor must be '<created_at>,<id>'")
    })?;
    let created_at = chrono::DateTime::parse_from_rfc3339(ts)
        .map_err(|_| fp_domain::DomainError::validation("before cursor timestamp is not RFC 3339"))?
        .with_timezone(&chrono::Utc);
    let id = uuid::Uuid::parse_str(id)
        .map_err(|_| fp_domain::DomainError::validation("before cursor id is not a UUID"))?;
    Ok((created_at, id))
}

fn parse_rfc3339_bound(
    raw: Option<&str>,
    field: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, fp_domain::DomainError> {
    match raw {
        None => Ok(None),
        Some(value) => chrono::DateTime::parse_from_rfc3339(value)
            .map(|dt| Some(dt.with_timezone(&chrono::Utc)))
            .map_err(|_| {
                fp_domain::DomainError::validation(format!("{field} must be an RFC 3339 timestamp"))
            }),
    }
}

/// Configuration rejections (NACKs) from this team's dataplanes, newest first, over a filtered
/// `[since, until)` window with cursor paging.
#[utoipa::path(get, path = "/api/v1/teams/{team}/xds/nacks", tag = "XdsStatus",
    params(("team" = String, Path, description = "Team name or UUID"), NackListParams),
    responses((status = 200, body = NackPage), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub async fn list_nacks(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(params): Query<NackListParams>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<NackPage>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        let query = fp_core::services::xds_status::NackQuery {
            since: parse_rfc3339_bound(params.since.as_deref(), "since")?,
            until: parse_rfc3339_bound(params.until.as_deref(), "until")?,
            before: match params.before.as_deref() {
                Some(raw) => Some(decode_nack_cursor(raw)?),
                None => None,
            },
            limit: params.limit,
        };
        fp_core::services::xds_status::list_nack_window(&state.pool, &ctx, team, query, rid).await
    };
    let window = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(NackPage {
        items: window.events.into_iter().map(Into::into).collect(),
        window_total: window.window_total,
        next_cursor: window
            .next_cursor
            .map(|(created_at, id)| encode_nack_cursor(created_at, id)),
    }))
}

/// Per-team xDS delivery health from persisted dataplane telemetry and NACK history.
#[utoipa::path(get, path = "/api/v1/teams/{team}/xds/status", tag = "XdsStatus",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses((status = 200, body = XdsStatusView), (status = 404, body = ErrorBody)))]
pub async fn status(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<XdsStatusView>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        fp_core::services::xds_status::status(&state.pool, &ctx, team, rid).await
    };
    let status = run.await.map_err(|e| ApiError::new(e, rid))?;
    let health = if status.recent_nack_count > 0 || status.warming_failures > 0 {
        "degraded"
    } else if status.stale_dataplanes > 0 {
        "stale"
    } else {
        "healthy"
    };
    Ok(Json(XdsStatusView {
        health: health.into(),
        total_dataplanes: status.total_dataplanes,
        live_dataplanes: status.live_dataplanes,
        stale_dataplanes: status.stale_dataplanes,
        config_verified_dataplanes: status.config_verified_dataplanes,
        total_requests: status.total_requests,
        total_errors: status.total_errors,
        warming_failures: status.warming_failures,
        recent_nack_count: status.recent_nack_count,
        latest_nack: status.latest_nack.map(Into::into),
        dataplanes: status
            .dataplanes
            .into_iter()
            .map(|item| DataplaneXdsStatusView {
                name: item.dataplane.name,
                id: item.dataplane.id.to_string(),
                live: item.live,
                version: item.dataplane.version,
                last_heartbeat_at: item.dataplane.last_heartbeat_at,
                last_config_verify_at: item.dataplane.last_config_verify_at,
                total_requests: item.dataplane.total_requests,
                total_errors: item.dataplane.total_errors,
                warming_failures: item.dataplane.warming_failures,
            })
            .collect(),
    }))
}

/// Correlate persisted audit and outbox rows by request id, trace id, or resource/path substring.
#[utoipa::path(get, path = "/api/v1/teams/{team}/ops/trace", tag = "Ops",
    params(("team" = String, Path, description = "Team name or UUID"), TraceParams),
    responses((status = 200, body = OpsTraceView), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub async fn trace(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Query(params): Query<TraceParams>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<OpsTraceView>, ApiError> {
    let request_id = match params.request_id.as_deref() {
        Some(value) => Some(RequestId::from_str(value).map_err(|e| ApiError::new(e, rid))?),
        None => None,
    };
    let query = fp_core::services::xds_status::TraceQuery {
        request_id,
        trace_id: params.trace_id,
        path: params.path,
        limit: params.limit.unwrap_or(50),
    };
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        fp_core::services::xds_status::trace(&state.pool, &ctx, team, query, rid).await
    };
    let trace = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(OpsTraceView {
        audit: trace
            .audit
            .into_iter()
            .map(|row| AuditTraceView {
                id: row.id,
                request_id: row.request_id.map(|id| id.to_string()),
                actor_label: row.actor_label,
                surface: row.surface,
                action: row.action,
                resource: row.resource,
                outcome: row.outcome,
                detail: row.detail,
                occurred_at: row.occurred_at,
            })
            .collect(),
        events: trace
            .events
            .into_iter()
            .map(|row| EventTraceView {
                seq: row.seq,
                event_type: row.event_type,
                payload: row.payload,
                trace_context: row.trace_context,
                occurred_at: row.occurred_at,
            })
            .collect(),
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod nack_cursor_tests {
    use super::{decode_nack_cursor, encode_nack_cursor, parse_rfc3339_bound};
    use chrono::{TimeZone, Utc};

    #[test]
    fn cursor_round_trips_at_microsecond_precision() {
        // Postgres TIMESTAMPTZ is microsecond-resolution; a truncated cursor would land on the
        // wrong boundary row. 654_321 micros = 654_321_000 nanos.
        let created_at = Utc
            .timestamp_micros(1_770_000_000_654_321)
            .single()
            .expect("valid micros");
        let id = uuid::Uuid::now_v7();
        let (dt, back) =
            decode_nack_cursor(&encode_nack_cursor(created_at, id)).expect("round-trip");
        assert_eq!(
            dt, created_at,
            "timestamp must survive the round-trip exactly"
        );
        assert_eq!(back, id);
    }

    #[test]
    fn cursor_rejects_malformed_input() {
        assert!(decode_nack_cursor("not-a-cursor").is_err(), "no comma");
        assert!(
            decode_nack_cursor("2026-07-18T00:00:00Z,not-a-uuid").is_err(),
            "bad uuid"
        );
        assert!(
            decode_nack_cursor("garbage,00000000-0000-0000-0000-000000000000").is_err(),
            "bad timestamp"
        );
    }

    #[test]
    fn rfc3339_bounds_parse_and_reject() {
        assert!(parse_rfc3339_bound(None, "since").expect("none").is_none());
        assert_eq!(
            parse_rfc3339_bound(Some("2026-07-18T12:00:00Z"), "since")
                .expect("parsed")
                .expect("some"),
            Utc.with_ymd_and_hms(2026, 7, 18, 12, 0, 0)
                .single()
                .unwrap()
        );
        assert!(parse_rfc3339_bound(Some("18-07-2026"), "until").is_err());
    }
}
