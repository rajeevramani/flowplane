//! AI gateway trace-event repository.
//!
//! Observation-write seam: rows are written by the xDS ExtProc capture path (the same
//! exception class as `ai_usage_events`), best-effort, never on a product-mutation path.
//! One row per AI data-plane request, keyed `(team_id, request_id)`; the listener-side
//! and upstream-side ExtProc streams of one request each upsert their own hops and the
//! merge is order-independent: hop entries are unioned by hop name (an `upstream`-origin
//! entry wins over a `listener`-origin one for the same name), columns fill NULLs only,
//! and `failure_hop` is re-derived from the merged timeline on every write.
//!
//! `expires_at` is resolved at insert from the team's `ai_retention_policies` row,
//! falling back to [`DEFAULT_AI_TRACE_TTL_DAYS`] — the same shape as
//! `raw_observation_ttl_days` in `api_lifecycle.rs`.

use fp_domain::{
    AiProviderId, AiTraceEvent, DomainError, DomainResult, ListenerId, RouteConfigId, TeamId,
};
use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub const DEFAULT_AI_TRACE_TTL_DAYS: i32 = 30;

const TRACE_COLUMNS: &str = "id, team_id, request_id, trace_id, route_config_id, listener_id, \
                             provider_id, model, status_code, failure_hop, hops, created_at, \
                             expires_at";

/// One ExtProc stream's contribution to a trace row. Column ownership is enforced by the
/// capture layer (listener stream: `trace_id`, `listener_id`, `model`, `status_code`;
/// upstream stream: `provider_id`) so the NULL-filling column merge is order-independent.
#[derive(Debug, Clone)]
pub struct AiTraceEventUpsert {
    pub team_id: TeamId,
    pub request_id: String,
    pub trace_id: Option<String>,
    pub route_config_id: RouteConfigId,
    pub listener_id: Option<ListenerId>,
    pub provider_id: Option<AiProviderId>,
    pub model: Option<String>,
    pub status_code: Option<i32>,
    /// JSON array of `{hop, started_at, ended_at, outcome, origin, failed, detail}` entries.
    pub hops: Value,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AiTraceQuery<'a> {
    pub request_id: Option<&'a str>,
    pub trace_id: Option<&'a str>,
    pub limit: i64,
}

fn trace_event_from_row(row: &PgRow) -> AiTraceEvent {
    AiTraceEvent {
        id: row.get("id"),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        request_id: row.get("request_id"),
        trace_id: row.get("trace_id"),
        route_config_id: RouteConfigId::from(row.get::<Uuid, _>("route_config_id")),
        listener_id: row
            .get::<Option<Uuid>, _>("listener_id")
            .map(ListenerId::from),
        provider_id: row
            .get::<Option<Uuid>, _>("provider_id")
            .map(AiProviderId::from),
        model: row.get("model"),
        status_code: row.get("status_code"),
        failure_hop: row.get("failure_hop"),
        hops: row.get("hops"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
    }
}

/// Insert-or-merge one stream's trace contribution. The whole operation runs in one
/// transaction with a row lock, so concurrent listener/upstream writes serialize and
/// converge to the same row regardless of arrival order.
pub async fn upsert_trace_event(pool: &PgPool, event: &AiTraceEventUpsert) -> DomainResult<()> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("upsert AI trace event: begin: {e}")))?;
    let ttl_days: Option<i32> =
        sqlx::query_scalar("SELECT trace_ttl_days FROM ai_retention_policies WHERE team_id = $1")
            .bind(event.team_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| DomainError::internal(format!("resolve AI trace ttl: {e}")))?;
    let ttl_days = resolve_ttl_days(ttl_days);
    // created_at and expires_at come from the same now() so the TTL arithmetic is exact.
    sqlx::query(
        "INSERT INTO ai_trace_events \
         (id, team_id, request_id, route_config_id, hops, created_at, expires_at) \
         VALUES ($1, $2, $3, $4, '[]'::jsonb, now(), now() + make_interval(days => $5)) \
         ON CONFLICT (team_id, request_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(event.team_id.as_uuid())
    .bind(&event.request_id)
    .bind(event.route_config_id.as_uuid())
    .bind(ttl_days)
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("insert AI trace event: {e}")))?;
    let existing: Value = sqlx::query_scalar(
        "SELECT hops FROM ai_trace_events WHERE team_id = $1 AND request_id = $2 FOR UPDATE",
    )
    .bind(event.team_id.as_uuid())
    .bind(&event.request_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("lock AI trace event: {e}")))?;
    let merged = merge_hops(&existing, &event.hops);
    let failure_hop = derive_failure_hop(&merged);
    sqlx::query(
        "UPDATE ai_trace_events SET \
           trace_id = COALESCE(trace_id, $3), \
           listener_id = COALESCE(listener_id, $4), \
           provider_id = COALESCE(provider_id, $5), \
           model = COALESCE(model, $6), \
           status_code = COALESCE(status_code, $7), \
           hops = $8, \
           failure_hop = $9 \
         WHERE team_id = $1 AND request_id = $2",
    )
    .bind(event.team_id.as_uuid())
    .bind(&event.request_id)
    .bind(&event.trace_id)
    .bind(event.listener_id.map(|id| id.as_uuid()))
    .bind(event.provider_id.map(|id| id.as_uuid()))
    .bind(&event.model)
    .bind(event.status_code)
    .bind(&merged)
    .bind(failure_hop)
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("merge AI trace event: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("upsert AI trace event: commit: {e}")))?;
    Ok(())
}

pub async fn list_trace_events(
    pool: &PgPool,
    team_id: TeamId,
    query: AiTraceQuery<'_>,
) -> DomainResult<Vec<AiTraceEvent>> {
    let rows = sqlx::query(&format!(
        "SELECT {TRACE_COLUMNS} FROM ai_trace_events \
         WHERE team_id = $1 \
           AND ($2::TEXT IS NULL OR request_id = $2) \
           AND ($3::TEXT IS NULL OR trace_id = $3) \
         ORDER BY created_at DESC \
         LIMIT $4"
    ))
    .bind(team_id.as_uuid())
    .bind(query.request_id)
    .bind(query.trace_id)
    .bind(query.limit.clamp(1, 500))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list AI trace events: {e}")))?;
    Ok(rows.iter().map(trace_event_from_row).collect())
}

/// Insert-time TTL resolution: the team's `ai_retention_policies.trace_ttl_days` when a
/// policy row exists, the 30-day built-in default otherwise.
fn resolve_ttl_days(policy_ttl_days: Option<i32>) -> i32 {
    policy_ttl_days.unwrap_or(DEFAULT_AI_TRACE_TTL_DAYS)
}

/// Union two hop arrays by hop name. On a name conflict an `upstream`-origin entry beats a
/// `listener`-origin one; same-origin conflicts keep the already-stored entry. The result is
/// sorted by (started_at, hop) so the merged array is identical whichever stream wrote first.
fn merge_hops(existing: &Value, incoming: &Value) -> Value {
    let empty = Vec::new();
    let existing_hops = existing.as_array().unwrap_or(&empty);
    let incoming_hops = incoming.as_array().unwrap_or(&empty);
    let mut merged: Vec<Value> = existing_hops.clone();
    for hop in incoming_hops {
        let name = hop_name(hop);
        match merged.iter_mut().find(|entry| hop_name(entry) == name) {
            Some(entry) => {
                if hop_origin(hop) == "upstream" && hop_origin(entry) != "upstream" {
                    *entry = hop.clone();
                }
            }
            None => merged.push(hop.clone()),
        }
    }
    merged.sort_by(|a, b| {
        hop_started_at(a)
            .cmp(&hop_started_at(b))
            .then_with(|| hop_name(a).cmp(hop_name(b)))
    });
    Value::Array(merged)
}

/// The first hop (in timeline order) flagged as failed, if any.
fn derive_failure_hop(merged: &Value) -> Option<String> {
    merged.as_array()?.iter().find_map(|hop| {
        hop.get("failed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .then(|| hop_name(hop).to_string())
    })
}

fn hop_name(hop: &Value) -> &str {
    hop.get("hop").and_then(Value::as_str).unwrap_or("")
}

fn hop_origin(hop: &Value) -> &str {
    hop.get("origin").and_then(Value::as_str).unwrap_or("")
}

fn hop_started_at(hop: &Value) -> DateTime<Utc> {
    hop.get("started_at")
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|parsed| parsed.with_timezone(&Utc))
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn listener_hops() -> Value {
        json!([
            {"hop": "route_match", "started_at": "2026-07-04T00:00:00.100Z", "ended_at": "2026-07-04T00:00:00.200Z", "outcome": "matched", "origin": "listener", "failed": false, "detail": {}},
            {"hop": "auth", "started_at": "2026-07-04T00:00:00.200Z", "ended_at": "2026-07-04T00:00:00.200Z", "outcome": "not_configured", "origin": "listener", "failed": false, "detail": {}},
            {"hop": "budget", "started_at": "2026-07-04T00:00:00.210Z", "ended_at": "2026-07-04T00:00:00.230Z", "outcome": "allowed", "origin": "listener", "failed": false, "detail": {}},
        ])
    }

    fn upstream_hops() -> Value {
        json!([
            {"hop": "budget", "started_at": "2026-07-04T00:00:00.300Z", "ended_at": "2026-07-04T00:00:00.320Z", "outcome": "allowed", "origin": "upstream", "failed": false, "detail": {}},
            {"hop": "credential_injection", "started_at": "2026-07-04T00:00:00.320Z", "ended_at": "2026-07-04T00:00:00.340Z", "outcome": "injected", "origin": "upstream", "failed": false, "detail": {}},
            {"hop": "upstream", "started_at": "2026-07-04T00:00:00.340Z", "ended_at": "2026-07-04T00:00:00.900Z", "outcome": "ok", "origin": "upstream", "failed": false, "detail": {"status": 200}},
        ])
    }

    #[test]
    fn merge_hops_is_order_independent() {
        let listener_first =
            merge_hops(&merge_hops(&json!([]), &listener_hops()), &upstream_hops());
        let upstream_first =
            merge_hops(&merge_hops(&json!([]), &upstream_hops()), &listener_hops());
        assert_eq!(listener_first, upstream_first);
        let names: Vec<&str> = listener_first
            .as_array()
            .unwrap()
            .iter()
            .map(hop_name)
            .collect();
        assert_eq!(
            names,
            vec![
                "route_match",
                "auth",
                "budget",
                "credential_injection",
                "upstream"
            ]
        );
        // The duplicated budget hop resolved to the upstream-origin entry in both orders.
        let budget = listener_first
            .as_array()
            .unwrap()
            .iter()
            .find(|hop| hop_name(hop) == "budget")
            .unwrap();
        assert_eq!(hop_origin(budget), "upstream");
    }

    #[test]
    fn merge_hops_same_origin_keeps_existing_entry() {
        let stored = json!([{"hop": "auth", "started_at": "2026-07-04T00:00:00Z", "ended_at": "2026-07-04T00:00:00Z", "outcome": "not_configured", "origin": "listener", "failed": false, "detail": {"first": true}}]);
        let retry = json!([{"hop": "auth", "started_at": "2026-07-04T00:00:05Z", "ended_at": "2026-07-04T00:00:05Z", "outcome": "not_configured", "origin": "listener", "failed": false, "detail": {"first": false}}]);
        let merged = merge_hops(&stored, &retry);
        assert_eq!(merged.as_array().unwrap().len(), 1);
        assert_eq!(merged[0]["detail"]["first"], json!(true));
    }

    #[test]
    fn ttl_resolution_falls_back_to_thirty_day_default_without_policy() {
        assert_eq!(resolve_ttl_days(None), DEFAULT_AI_TRACE_TTL_DAYS);
        assert_eq!(resolve_ttl_days(None), 30);
        assert_eq!(resolve_ttl_days(Some(7)), 7);
    }

    #[test]
    fn derive_failure_hop_picks_first_failing_hop_in_timeline_order() {
        assert_eq!(derive_failure_hop(&listener_hops()), None);
        let hops = json!([
            {"hop": "upstream", "started_at": "2026-07-04T00:00:02Z", "ended_at": "2026-07-04T00:00:03Z", "outcome": "error", "origin": "upstream", "failed": true, "detail": {}},
            {"hop": "budget", "started_at": "2026-07-04T00:00:01Z", "ended_at": "2026-07-04T00:00:01Z", "outcome": "rejected", "origin": "listener", "failed": true, "detail": {}},
        ]);
        let merged = merge_hops(&json!([]), &hops);
        assert_eq!(derive_failure_hop(&merged).as_deref(), Some("budget"));
    }
}
