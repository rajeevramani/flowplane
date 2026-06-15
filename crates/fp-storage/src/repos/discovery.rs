//! Discovery session repository.

use fp_domain::authz::TeamRef;
use fp_domain::{
    DiscoverySession, DiscoverySessionId, DiscoverySessionSpec, DiscoverySessionStatus,
    DomainError, DomainResult, TeamId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};

const COLUMNS: &str = "id, team_id, name, status, listener_port, upstream_host, upstream_port, \
    upstream_tls, validated_upstream_ip, validated_upstream_port, cluster_name, \
    route_config_name, listener_name, target_sample_count, max_duration_seconds, max_bytes, \
    max_distinct_paths, sample_count, byte_count, path_count, drop_count, started_at, \
    completed_at, cancelled_at, updated_at, created_at";

pub struct DiscoverySessionInsert<'a> {
    pub id: DiscoverySessionId,
    pub name: &'a str,
    pub spec: &'a DiscoverySessionSpec,
    pub validated_upstream_ip: &'a str,
    pub cluster_name: &'a str,
    pub route_config_name: &'a str,
    pub listener_name: &'a str,
}

fn from_row(row: &PgRow) -> DomainResult<DiscoverySession> {
    let status: String = row.get("status");
    Ok(DiscoverySession {
        id: DiscoverySessionId::from(row.get::<uuid::Uuid, _>("id")),
        team_id: TeamId::from(row.get::<uuid::Uuid, _>("team_id")),
        name: row.get("name"),
        status: DiscoverySessionStatus::parse(&status)?,
        listener_port: row.get("listener_port"),
        upstream_host: row.get("upstream_host"),
        upstream_port: row.get("upstream_port"),
        upstream_tls: row.get("upstream_tls"),
        validated_upstream_ip: row.get("validated_upstream_ip"),
        validated_upstream_port: row.get("validated_upstream_port"),
        cluster_name: row.get("cluster_name"),
        route_config_name: row.get("route_config_name"),
        listener_name: row.get("listener_name"),
        target_sample_count: row.get("target_sample_count"),
        max_duration_seconds: row.get("max_duration_seconds"),
        max_bytes: row.get("max_bytes"),
        max_distinct_paths: row.get("max_distinct_paths"),
        sample_count: row.get("sample_count"),
        byte_count: row.get("byte_count"),
        path_count: row.get("path_count"),
        drop_count: row.get("drop_count"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        cancelled_at: row.get("cancelled_at"),
        updated_at: row.get("updated_at"),
        created_at: row.get("created_at"),
    })
}

pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    insert: DiscoverySessionInsert<'_>,
) -> DomainResult<DiscoverySession> {
    let row = sqlx::query(&format!(
        "INSERT INTO discovery_sessions \
         (id, team_id, org_id, name, status, listener_port, upstream_host, upstream_port, \
          upstream_tls, validated_upstream_ip, validated_upstream_port, cluster_name, \
          route_config_name, listener_name, target_sample_count, max_duration_seconds, \
          max_bytes, max_distinct_paths) \
         VALUES ($1, $2, $3, $4, 'capturing', $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) \
         RETURNING {COLUMNS}"
    ))
    .bind(insert.id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(insert.name)
    .bind(insert.spec.listener_port)
    .bind(&insert.spec.upstream_host)
    .bind(insert.spec.upstream_port)
    .bind(insert.spec.upstream_tls)
    .bind(insert.validated_upstream_ip)
    .bind(insert.spec.upstream_port)
    .bind(insert.cluster_name)
    .bind(insert.route_config_name)
    .bind(insert.listener_name)
    .bind(insert.spec.target_sample_count)
    .bind(insert.spec.max_duration_seconds)
    .bind(insert.spec.max_bytes)
    .bind(insert.spec.max_distinct_paths)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict(format!(
                "discovery session \"{}\" already exists in this team",
                insert.name
            ))
        }
        _ => DomainError::internal(format!("create discovery session: {e}")),
    })?;
    from_row(&row)
}

pub async fn get(
    pool: &PgPool,
    team_id: TeamId,
    session: &str,
) -> DomainResult<Option<DiscoverySession>> {
    let id = uuid::Uuid::parse_str(session).ok();
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM discovery_sessions \
         WHERE team_id = $1 AND (name = $2 OR id = $3)"
    ))
    .bind(team_id.as_uuid())
    .bind(session)
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get discovery session: {e}")))?;
    row.as_ref().map(from_row).transpose()
}

pub async fn list(
    pool: &PgPool,
    team_id: TeamId,
    status: Option<DiscoverySessionStatus>,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<DiscoverySession>, i64)> {
    let status = status.map(DiscoverySessionStatus::as_str);
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM discovery_sessions \
         WHERE team_id = $1 AND ($2::text IS NULL OR status = $2) \
         ORDER BY created_at DESC, name LIMIT $3 OFFSET $4"
    ))
    .bind(team_id.as_uuid())
    .bind(status)
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list discovery sessions: {e}")))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM discovery_sessions \
         WHERE team_id = $1 AND ($2::text IS NULL OR status = $2)",
    )
    .bind(team_id.as_uuid())
    .bind(status)
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count discovery sessions: {e}")))?;
    rows.iter()
        .map(from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|items| (items, total))
}

pub async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    session: &str,
) -> DomainResult<DiscoverySession> {
    let id = uuid::Uuid::parse_str(session).ok();
    let row = sqlx::query(&format!(
        "UPDATE discovery_sessions \
         SET status = 'completed', completed_at = now(), updated_at = now() \
         WHERE team_id = $1 AND (name = $2 OR id = $3) AND status = 'capturing' \
         RETURNING {COLUMNS}"
    ))
    .bind(team_id.as_uuid())
    .bind(session)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("complete discovery session: {e}")))?;
    match row {
        Some(row) => from_row(&row),
        None => Err(DomainError::not_found("discovery session", session)),
    }
}
