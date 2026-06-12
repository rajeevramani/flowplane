//! Dataplane + proxy-certificate registry repository (S5.4).
//!
//! `find_active_certificate` is the xDS authentication primitive: full-SPIFFE-URI lookup
//! with the revocation and expiry predicates in SQL. Anything that does not match an
//! active row authenticates nothing — fail closed is the only mode.

use fp_domain::authz::TeamRef;
use fp_domain::dataplane::{Dataplane, ProxyCertificate};
use fp_domain::{DataplaneId, DomainError, DomainResult, ProxyCertificateId, TeamId, UserId};
use sqlx::postgres::PgRow;
use sqlx::types::chrono;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

const DP_COLUMNS: &str = "id, team_id, name, description, version, created_at, updated_at";
const CERT_COLUMNS: &str = "id, team_id, dataplane_id, spiffe_uri, serial_number, issued_at, \
                            expires_at, revoked_at, revoked_reason, created_at";

fn dataplane_from_row(row: &PgRow) -> Dataplane {
    Dataplane {
        id: DataplaneId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        description: row.get("description"),
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
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
