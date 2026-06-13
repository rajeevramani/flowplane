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

/// Recent configuration rejections (NACKs) from this team's dataplanes, newest first.
#[utoipa::path(get, path = "/api/v1/teams/{team}/xds/nacks", tag = "XdsStatus",
    params(("team" = String, Path, description = "Team name or UUID")),
    responses((status = 200, body = [NackEventView]), (status = 404, body = ErrorBody)))]
pub async fn list_nacks(
    State(state): State<AppState>,
    Path(team): Path<String>,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
) -> Result<Json<Vec<NackEventView>>, ApiError> {
    let run = async {
        let team = resolve_team(&state, &ctx, &team).await?;
        fp_core::services::xds_status::list_nack_events(&state.pool, &ctx, team, 100, rid).await
    };
    let events = run.await.map_err(|e| ApiError::new(e, rid))?;
    Ok(Json(events.into_iter().map(Into::into).collect()))
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
