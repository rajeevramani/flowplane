//! Persisted dataplane NACKs (S5.5). Inserted from the xDS stream path (best-effort,
//! never blocking the stream); read per team by the status API.

use fp_domain::{DomainError, DomainResult, TeamId};
use sqlx::postgres::PgRow;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NackEvent {
    pub id: Uuid,
    pub team_id: TeamId,
    pub node_id: String,
    pub type_url: String,
    pub version_rejected: String,
    pub error_message: String,
    pub quarantined_resources: Vec<String>,
    pub created_at: DateTime<Utc>,
}

fn from_row(row: &PgRow) -> NackEvent {
    let quarantined: serde_json::Value = row.get("quarantined_resources");
    NackEvent {
        id: row.get("id"),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        node_id: row.get("node_id"),
        type_url: row.get("type_url"),
        version_rejected: row.get("version_rejected"),
        error_message: row.get("error_message"),
        quarantined_resources: serde_json::from_value(quarantined).unwrap_or_default(),
        created_at: row.get("created_at"),
    }
}

/// What a stream records about one NACK.
#[derive(Debug, Clone)]
pub struct NackRecord {
    pub team_id: TeamId,
    pub node_id: String,
    pub type_url: String,
    pub version_rejected: String,
    pub error_message: String,
    pub quarantined_resources: Vec<String>,
}

/// Insert one NACK row. The team's org is resolved in SQL (the stream only knows the
/// team); a vanished team makes this a no-op rather than an error.
pub async fn record(pool: &PgPool, record: &NackRecord) -> DomainResult<()> {
    sqlx::query(
        "INSERT INTO xds_nack_events \
           (id, team_id, org_id, node_id, type_url, version_rejected, error_message, \
            quarantined_resources) \
         SELECT $1, t.id, t.org_id, $3, $4, $5, $6, $7 FROM teams t WHERE t.id = $2",
    )
    .bind(Uuid::now_v7())
    .bind(record.team_id.as_uuid())
    .bind(&record.node_id)
    .bind(&record.type_url)
    .bind(&record.version_rejected)
    .bind(&record.error_message)
    .bind(serde_json::json!(record.quarantined_resources))
    .execute(pool)
    .await
    .map_err(|e| DomainError::internal(format!("record nack: {e}")))?;
    Ok(())
}

pub async fn list(pool: &PgPool, team_id: TeamId, limit: i64) -> DomainResult<Vec<NackEvent>> {
    let rows = sqlx::query(
        "SELECT id, team_id, node_id, type_url, version_rejected, error_message, \
                quarantined_resources, created_at \
         FROM xds_nack_events WHERE team_id = $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list nacks: {e}")))?;
    Ok(rows.iter().map(from_row).collect())
}

/// A filtered, cursor-paged window over a team's NACK history (S5.5 read path, fpv2-55x.1).
/// `since`/`until` form a half-open interval `[since, until)`; `before` is a total-order cursor
/// `(created_at, id)` matching `ORDER BY created_at DESC, id DESC`. `limit` is the number of rows
/// the caller wants — callers pass `limit + 1` here to detect a further page.
#[derive(Debug, Clone)]
pub struct NackWindowQuery {
    pub team_id: TeamId,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub before: Option<(DateTime<Utc>, Uuid)>,
    pub limit: i64,
}

/// Rows in `[since, until)` for one team, newest first, after the `before` cursor, capped at
/// `limit`. The total order `(created_at DESC, id DESC)` is deterministic under equal `created_at`
/// (ties broken by the UUIDv7 `id`) and stable under interleaved retention deletes (a cursor names
/// a point in the order, not an offset).
pub async fn list_window(pool: &PgPool, query: &NackWindowQuery) -> DomainResult<Vec<NackEvent>> {
    let (before_created_at, before_id) = match query.before {
        Some((ts, id)) => (Some(ts), Some(id)),
        None => (None, None),
    };
    let rows = sqlx::query(
        "SELECT id, team_id, node_id, type_url, version_rejected, error_message, \
                quarantined_resources, created_at \
         FROM xds_nack_events \
         WHERE team_id = $1 \
           AND ($2::timestamptz IS NULL OR created_at >= $2) \
           AND ($3::timestamptz IS NULL OR created_at < $3) \
           AND ($4::timestamptz IS NULL OR (created_at, id) < ($4, $5)) \
         ORDER BY created_at DESC, id DESC \
         LIMIT $6",
    )
    .bind(query.team_id.as_uuid())
    .bind(query.since)
    .bind(query.until)
    .bind(before_created_at)
    .bind(before_id)
    .bind(query.limit.clamp(1, 501))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list nack window: {e}")))?;
    Ok(rows.iter().map(from_row).collect())
}

/// Count of rows matching the same `[since, until)` window for one team (the `window_total`).
pub async fn count_window(
    pool: &PgPool,
    team_id: TeamId,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> DomainResult<i64> {
    sqlx::query_scalar(
        "SELECT count(*)::bigint FROM xds_nack_events \
         WHERE team_id = $1 \
           AND ($2::timestamptz IS NULL OR created_at >= $2) \
           AND ($3::timestamptz IS NULL OR created_at < $3)",
    )
    .bind(team_id.as_uuid())
    .bind(since)
    .bind(until)
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count nack window: {e}")))
}

pub async fn count_recent(pool: &PgPool, team_id: TeamId, minutes: i64) -> DomainResult<i64> {
    sqlx::query_scalar(
        "SELECT count(*)::bigint FROM xds_nack_events \
         WHERE team_id = $1 AND created_at > now() - ($2::text || ' minutes')::interval",
    )
    .bind(team_id.as_uuid())
    .bind(minutes.clamp(1, 1440))
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count recent nacks: {e}")))
}

pub async fn delete_older_than_for_team(
    pool: &PgPool,
    team_id: TeamId,
    older_than: DateTime<Utc>,
) -> DomainResult<u64> {
    let result = sqlx::query("DELETE FROM xds_nack_events WHERE team_id = $1 AND created_at < $2")
        .bind(team_id.as_uuid())
        .bind(older_than)
        .execute(pool)
        .await
        .map_err(|e| DomainError::internal(format!("delete old xds nacks: {e}")))?;
    Ok(result.rows_affected())
}
