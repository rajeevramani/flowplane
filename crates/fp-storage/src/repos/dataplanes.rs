//! Dataplane + proxy-certificate registry repository (S5.4).
//!
//! `find_active_certificate` is the xDS authentication primitive: full-SPIFFE-URI lookup
//! with the revocation and expiry predicates in SQL. Anything that does not match an
//! active row authenticates nothing — fail closed is the only mode.

use fp_domain::authz::TeamRef;
use fp_domain::dataplane::{Dataplane, ProxyCertificate};
use fp_domain::{
    DataplaneId, DomainError, DomainResult, ProxyCertificateId, TeamId, TeamStatsOverview, UserId,
};
use sqlx::postgres::PgRow;
use sqlx::types::chrono;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

const DP_COLUMNS: &str = "id, team_id, name, description, version, last_heartbeat_at, \
	                          last_config_verify_at, total_requests, total_errors, \
	                          warming_failures, created_at, updated_at";
const CERT_COLUMNS: &str = "id, team_id, dataplane_id, spiffe_uri, serial_number, issued_at, \
	                            expires_at, revoked_at, revoked_reason, created_at";

#[derive(Debug, Clone, Copy)]
pub struct TelemetryDelta<'a> {
    pub idempotency_key: &'a str,
    pub requests_delta: i64,
    pub errors_delta: i64,
    pub warming_failures_delta: i64,
    pub config_verified: bool,
}

fn dataplane_from_row(row: &PgRow) -> Dataplane {
    Dataplane {
        id: DataplaneId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        description: row.get("description"),
        version: row.get("version"),
        last_heartbeat_at: row.get("last_heartbeat_at"),
        last_config_verify_at: row.get("last_config_verify_at"),
        total_requests: row.get("total_requests"),
        total_errors: row.get("total_errors"),
        warming_failures: row.get("warming_failures"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

pub async fn record_telemetry(
    pool: &PgPool,
    team_id: TeamId,
    name: &str,
    telemetry: TelemetryDelta<'_>,
) -> DomainResult<Dataplane> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("record dataplane telemetry: begin: {e}")))?;
    let row = sqlx::query(&format!(
        "SELECT {DP_COLUMNS} FROM dataplanes WHERE team_id = $1 AND name = $2 FOR UPDATE"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("lock dataplane telemetry target: {e}")))?;
    let Some(row) = row else {
        return Err(DomainError::not_found("dataplane", name));
    };
    let current = dataplane_from_row(&row);
    let dataplane = apply_telemetry_delta(&mut tx, team_id, current, telemetry).await?;
    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("record dataplane telemetry: commit: {e}")))?;
    Ok(dataplane)
}

pub async fn record_telemetry_by_id(
    pool: &PgPool,
    team_id: TeamId,
    dataplane_id: DataplaneId,
    telemetry: TelemetryDelta<'_>,
) -> DomainResult<Dataplane> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("record dataplane telemetry: begin: {e}")))?;
    let row = sqlx::query(&format!(
        "SELECT {DP_COLUMNS} FROM dataplanes WHERE team_id = $1 AND id = $2 FOR UPDATE"
    ))
    .bind(team_id.as_uuid())
    .bind(dataplane_id.as_uuid())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("lock dataplane telemetry target: {e}")))?;
    let Some(row) = row else {
        let handle = dataplane_id.as_uuid().to_string();
        return Err(DomainError::not_found("dataplane", &handle));
    };
    let current = dataplane_from_row(&row);
    let dataplane = apply_telemetry_delta(&mut tx, team_id, current, telemetry).await?;
    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("record dataplane telemetry: commit: {e}")))?;
    Ok(dataplane)
}

async fn apply_telemetry_delta(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    current: Dataplane,
    telemetry: TelemetryDelta<'_>,
) -> DomainResult<Dataplane> {
    let requests_delta = telemetry.requests_delta.max(0);
    let errors_delta = telemetry.errors_delta.max(0);
    let warming_failures_delta = telemetry.warming_failures_delta.max(0);
    let inserted = sqlx::query(
        "INSERT INTO dataplane_telemetry_reports \
         (id, team_id, dataplane_id, idempotency_key, requests_delta, errors_delta, \
          warming_failures_delta, config_verified) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         ON CONFLICT (team_id, dataplane_id, idempotency_key) DO NOTHING",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(team_id.as_uuid())
    .bind(current.id.as_uuid())
    .bind(telemetry.idempotency_key)
    .bind(requests_delta)
    .bind(errors_delta)
    .bind(warming_failures_delta)
    .bind(telemetry.config_verified)
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("record dataplane telemetry idempotency: {e}")))?;
    if inserted.rows_affected() == 0 {
        return Ok(current);
    }

    let row = sqlx::query(&format!(
        "UPDATE dataplanes SET \
            last_heartbeat_at = now(), \
            last_config_verify_at = CASE WHEN $1 THEN now() ELSE last_config_verify_at END, \
            total_requests = total_requests + $2, \
            total_errors = total_errors + $3, \
            warming_failures = warming_failures + $4, \
            updated_at = now() \
         WHERE team_id = $5 AND id = $6 RETURNING {DP_COLUMNS}"
    ))
    .bind(telemetry.config_verified)
    .bind(requests_delta)
    .bind(errors_delta)
    .bind(warming_failures_delta)
    .bind(team_id.as_uuid())
    .bind(current.id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("record dataplane telemetry: {e}")))?;
    Ok(dataplane_from_row(&row))
}

pub async fn stats_overview(pool: &PgPool, team_id: TeamId) -> DomainResult<TeamStatsOverview> {
    let row = sqlx::query(
        "SELECT \
            count(*)::bigint AS total_dataplanes, \
            count(*) FILTER (WHERE last_heartbeat_at > now() - interval '60 seconds')::bigint \
                AS live_dataplanes, \
            coalesce(sum(total_requests), 0)::bigint AS total_requests, \
            coalesce(sum(total_errors), 0)::bigint AS total_errors, \
            coalesce(sum(warming_failures), 0)::bigint AS warming_failures \
         FROM dataplanes WHERE team_id = $1",
    )
    .bind(team_id.as_uuid())
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("dataplane stats overview: {e}")))?;
    let total_dataplanes = row.get("total_dataplanes");
    let live_dataplanes = row.get("live_dataplanes");
    Ok(TeamStatsOverview {
        total_dataplanes,
        live_dataplanes,
        stale_dataplanes: total_dataplanes - live_dataplanes,
        total_requests: row.get("total_requests"),
        total_errors: row.get("total_errors"),
        warming_failures: row.get("warming_failures"),
    })
}

fn cert_from_row(row: &PgRow) -> ProxyCertificate {
    ProxyCertificate {
        id: ProxyCertificateId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        dataplane_id: DataplaneId::from(row.get::<Uuid, _>("dataplane_id")),
        spiffe_uri: row.get("spiffe_uri"),
        serial_number: row.get("serial_number"),
        issued_at: row.get("issued_at"),
        expires_at: row.get("expires_at"),
        revoked_at: row.get("revoked_at"),
        revoked_reason: row.get("revoked_reason"),
        created_at: row.get("created_at"),
    }
}

pub async fn create_dataplane(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    description: &str,
) -> DomainResult<Dataplane> {
    let row = sqlx::query(&format!(
        "INSERT INTO dataplanes (id, team_id, org_id, name, description) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {DP_COLUMNS}"
    ))
    .bind(DataplaneId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(description)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict(format!("dataplane \"{name}\" already exists in this team"))
                .with_hint("choose a different name")
        }
        _ => DomainError::internal(format!("create dataplane: {e}")),
    })?;
    Ok(dataplane_from_row(&row))
}

pub async fn get_dataplane(
    pool: &PgPool,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<Dataplane>> {
    let row = sqlx::query(&format!(
        "SELECT {DP_COLUMNS} FROM dataplanes WHERE team_id = $1 AND name = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get dataplane: {e}")))?;
    Ok(row.as_ref().map(dataplane_from_row))
}

pub async fn list_dataplanes(
    pool: &PgPool,
    team_id: TeamId,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<Dataplane>, i64)> {
    let rows = sqlx::query(&format!(
        "SELECT {DP_COLUMNS} FROM dataplanes WHERE team_id = $1 ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list dataplanes: {e}")))?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM dataplanes WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count dataplanes: {e}")))?;
    Ok((rows.iter().map(dataplane_from_row).collect(), total))
}

pub async fn count_for_team(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM dataplanes WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count dataplanes: {e}")))
}

pub async fn register_certificate(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    dataplane_id: DataplaneId,
    spiffe_uri: &str,
    serial_number: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
    issued_by: Option<UserId>,
) -> DomainResult<ProxyCertificate> {
    let row = sqlx::query(&format!(
        "INSERT INTO proxy_certificates \
           (id, team_id, dataplane_id, spiffe_uri, serial_number, expires_at, issued_by_user_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {CERT_COLUMNS}"
    ))
    .bind(ProxyCertificateId::generate().as_uuid())
    .bind(team_id.as_uuid())
    .bind(dataplane_id.as_uuid())
    .bind(spiffe_uri)
    .bind(serial_number)
    .bind(expires_at)
    .bind(issued_by.map(|u| u.as_uuid()))
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict("a certificate with this SPIFFE URI or serial already exists")
                .with_hint("each certificate must have a unique SPIFFE URI and serial number")
        }
        _ => DomainError::internal(format!("register certificate: {e}")),
    })?;
    Ok(cert_from_row(&row))
}

/// The xDS authentication lookup: the certificate row for `spiffe_uri` that is neither
/// revoked nor expired. The predicates live in SQL so there is no code path that can see
/// (and mistakenly trust) an inactive row.
pub async fn find_active_certificate(
    pool: &PgPool,
    spiffe_uri: &str,
) -> DomainResult<Option<ProxyCertificate>> {
    let row = sqlx::query(&format!(
        "SELECT {CERT_COLUMNS} FROM proxy_certificates \
         WHERE spiffe_uri = $1 AND revoked_at IS NULL AND expires_at > now()"
    ))
    .bind(spiffe_uri)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("certificate lookup: {e}")))?;
    Ok(row.as_ref().map(cert_from_row))
}

pub async fn list_certificates(
    pool: &PgPool,
    team_id: TeamId,
) -> DomainResult<Vec<ProxyCertificate>> {
    let rows = sqlx::query(&format!(
        "SELECT {CERT_COLUMNS} FROM proxy_certificates WHERE team_id = $1 ORDER BY created_at"
    ))
    .bind(team_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list certificates: {e}")))?;
    Ok(rows.iter().map(cert_from_row).collect())
}

/// Revoke by serial within the team. Idempotence is rejected loudly: revoking an already
/// revoked certificate is a conflict, not a silent success (audit clarity).
pub async fn revoke_certificate(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    serial_number: &str,
    reason: &str,
) -> DomainResult<ProxyCertificate> {
    let row = sqlx::query(&format!(
        "UPDATE proxy_certificates SET revoked_at = now(), revoked_reason = $1 \
         WHERE team_id = $2 AND serial_number = $3 AND revoked_at IS NULL \
         RETURNING {CERT_COLUMNS}"
    ))
    .bind(reason)
    .bind(team_id.as_uuid())
    .bind(serial_number)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("revoke certificate: {e}")))?;
    match row {
        Some(row) => Ok(cert_from_row(&row)),
        None => {
            let exists: Option<bool> = sqlx::query_scalar(
                "SELECT revoked_at IS NOT NULL FROM proxy_certificates \
                 WHERE team_id = $1 AND serial_number = $2",
            )
            .bind(team_id.as_uuid())
            .bind(serial_number)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("revoke certificate: recheck: {e}")))?;
            Err(match exists {
                Some(true) => DomainError::conflict(format!(
                    "certificate with serial \"{serial_number}\" is already revoked"
                )),
                _ => DomainError::not_found("proxy certificate", serial_number),
            })
        }
    }
}
