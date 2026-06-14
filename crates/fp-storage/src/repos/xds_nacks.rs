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
