//! Dataplane + proxy-certificate registry repository (S5.4).
//!
//! `find_active_certificate_exact` is the xDS authentication primitive: exact verified leaf
//! identity lookup with revocation and expiry predicates in SQL. Anything that does not match an
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
	                          warming_failures, retired_at, retired_reason, created_at, updated_at";
const CERT_COLUMNS: &str = "id, team_id, dataplane_id, spiffe_uri, serial_number, issued_at, \
	                            fingerprint_sha256, expires_at, revoked_at, revoked_reason, created_at";
const QUALIFIED_CERT_COLUMNS: &str = "pc.id, pc.team_id, pc.dataplane_id, pc.spiffe_uri, \
	 pc.serial_number, pc.issued_at, pc.fingerprint_sha256, pc.expires_at, pc.revoked_at, \
	 pc.revoked_reason, pc.created_at";

#[derive(Debug, Clone, Copy)]
pub struct TelemetryDelta<'a> {
    pub idempotency_key: &'a str,
    pub requests_delta: i64,
    pub errors_delta: i64,
    pub warming_failures_delta: i64,
    pub config_verified: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct NewProxyCertificate<'a> {
    pub team_id: TeamId,
    pub dataplane_id: DataplaneId,
    pub spiffe_uri: &'a str,
    pub serial_number: &'a str,
    pub fingerprint_sha256: Option<&'a str>,
    pub issued_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub issued_by: Option<UserId>,
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
        retired_at: row.get("retired_at"),
        retired_reason: row.get("retired_reason"),
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
        "SELECT {DP_COLUMNS} FROM dataplanes \
         WHERE team_id = $1 AND name = $2 AND retired_at IS NULL FOR UPDATE"
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
        "SELECT {DP_COLUMNS} FROM dataplanes \
         WHERE team_id = $1 AND id = $2 AND retired_at IS NULL FOR UPDATE"
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
         WHERE team_id = $5 AND id = $6 AND retired_at IS NULL RETURNING {DP_COLUMNS}"
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
            count(*) FILTER (WHERE last_heartbeat_at IS NOT NULL \
                AND clock_timestamp() - last_heartbeat_at <= interval '60 seconds')::bigint \
                AS live_dataplanes, \
            coalesce(sum(total_requests), 0)::bigint AS total_requests, \
            coalesce(sum(total_errors), 0)::bigint AS total_errors, \
            coalesce(sum(warming_failures), 0)::bigint AS warming_failures \
         FROM dataplanes WHERE team_id = $1 AND retired_at IS NULL",
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
        fingerprint_sha256: row.get("fingerprint_sha256"),
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
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23503") => {
            DomainError::conflict("team no longer exists").with_hint("refresh the team and retry")
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
        "SELECT {DP_COLUMNS} FROM dataplanes \
         WHERE team_id = $1 AND name = $2 AND retired_at IS NULL"
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
    list_dataplanes_with_lifecycle(pool, team_id, limit, offset, false).await
}

pub async fn list_dataplanes_with_lifecycle(
    pool: &PgPool,
    team_id: TeamId,
    limit: i64,
    offset: i64,
    include_retired: bool,
) -> DomainResult<(Vec<Dataplane>, i64)> {
    let rows = sqlx::query(&format!(
        "SELECT {DP_COLUMNS} FROM dataplanes \
         WHERE team_id = $1 AND ($4 OR retired_at IS NULL) \
         ORDER BY name, retired_at NULLS FIRST, created_at LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .bind(include_retired)
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list dataplanes: {e}")))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dataplanes WHERE team_id = $1 AND ($2 OR retired_at IS NULL)",
    )
    .bind(team_id.as_uuid())
    .bind(include_retired)
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count dataplanes: {e}")))?;
    Ok((rows.iter().map(dataplane_from_row).collect(), total))
}

pub async fn count_for_team(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM dataplanes WHERE team_id = $1 AND retired_at IS NULL")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count dataplanes: {e}")))
}

/// Serialize credential creation for one dataplane and enforce the bounded-overlap invariant.
/// Expiry is deliberately irrelevant: only explicit revocation releases capacity.
pub async fn lock_certificate_capacity(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    dataplane_id: DataplaneId,
) -> DomainResult<()> {
    let locked: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM dataplanes \
             WHERE team_id = $1 AND id = $2 AND retired_at IS NULL FOR UPDATE",
    )
    .bind(team_id.as_uuid())
    .bind(dataplane_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| DomainError::internal(format!("lock certificate dataplane: {error}")))?;
    if locked.is_none() {
        return Err(DomainError::not_found(
            "dataplane",
            &dataplane_id.as_uuid().to_string(),
        ));
    }

    let unrevoked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM proxy_certificates \
         WHERE team_id = $1 AND dataplane_id = $2 AND revoked_at IS NULL",
    )
    .bind(team_id.as_uuid())
    .bind(dataplane_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| DomainError::internal(format!("count unrevoked certificates: {error}")))?;
    if unrevoked >= 2 {
        return Err(DomainError::conflict(
            "dataplane already has the maximum of two unrevoked certificates",
        )
        .with_hint("revoke an old or abandoned certificate before creating another"));
    }
    Ok(())
}

pub async fn register_certificate(
    tx: &mut Transaction<'_, Postgres>,
    certificate: NewProxyCertificate<'_>,
) -> DomainResult<ProxyCertificate> {
    let row = sqlx::query(&format!(
        "INSERT INTO proxy_certificates \
           (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, issued_at, expires_at, issued_by_user_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {CERT_COLUMNS}"
    ))
    .bind(ProxyCertificateId::generate().as_uuid())
    .bind(certificate.team_id.as_uuid())
    .bind(certificate.dataplane_id.as_uuid())
    .bind(certificate.spiffe_uri)
    .bind(certificate.serial_number)
    .bind(certificate.fingerprint_sha256)
    .bind(certificate.issued_at)
    .bind(certificate.expires_at)
    .bind(certificate.issued_by.map(|user| user.as_uuid()))
    .fetch_one(&mut **tx)
    .await
    .map_err(map_certificate_insert_error)?;
    Ok(cert_from_row(&row))
}

fn map_certificate_insert_error(error: sqlx::Error) -> DomainError {
    let sqlx::Error::Database(database) = &error else {
        return DomainError::internal(format!("register certificate: {error}"));
    };
    if database.code().as_deref() != Some("23505") {
        return DomainError::internal(format!("register certificate: {error}"));
    }
    certificate_unique_conflict(database.constraint().unwrap_or_default())
}

fn certificate_unique_conflict(constraint: &str) -> DomainError {
    match constraint {
        "uq_proxy_certificates_fingerprint_sha256" => {
            DomainError::conflict("a certificate with this leaf fingerprint is already registered")
                .with_hint("register a distinct certificate leaf")
        }
        "proxy_certificates_team_id_serial_number_key" => {
            DomainError::conflict("a certificate with this serial already exists in this team")
                .with_hint("register a certificate with a distinct serial")
        }
        _ => DomainError::conflict("certificate identity conflicts with an existing registry row")
            .with_hint("use a distinct certificate identity"),
    }
}

/// Resolve one active registry row by the exact verified leaf identity.
pub async fn find_active_certificate_exact(
    pool: &PgPool,
    spiffe_uri: &str,
    fingerprint_sha256: &str,
) -> DomainResult<Option<ProxyCertificate>> {
    let row = sqlx::query(&format!(
        "SELECT {QUALIFIED_CERT_COLUMNS} FROM proxy_certificates pc \
         JOIN dataplanes dp ON dp.id = pc.dataplane_id AND dp.team_id = pc.team_id \
         WHERE pc.spiffe_uri = $1 AND pc.fingerprint_sha256 = $2 \
           AND pc.revoked_at IS NULL AND pc.expires_at > now() AND dp.retired_at IS NULL"
    ))
    .bind(spiffe_uri)
    .bind(fingerprint_sha256)
    .fetch_optional(pool)
    .await
    .map_err(|error| DomainError::internal(format!("exact certificate lookup: {error}")))?;
    Ok(row.as_ref().map(cert_from_row))
}

/// Atomically pin an exact leaf fingerprint onto one active legacy registry row. The scalar
/// candidate subquery deliberately errors when more than one row matches; ambiguity must never
/// be resolved by ordering. The final null predicate makes concurrent conflicting pins one-winner.
pub async fn pin_legacy_certificate_fingerprint(
    tx: &mut Transaction<'_, Postgres>,
    spiffe_uri: &str,
    serial_number: &str,
    fingerprint_sha256: &str,
) -> DomainResult<ProxyCertificate> {
    let row = sqlx::query(&format!(
        "UPDATE proxy_certificates SET fingerprint_sha256 = $3 \
         WHERE id = ( \
           SELECT pc.id FROM proxy_certificates pc \
           JOIN dataplanes dp ON dp.id = pc.dataplane_id AND dp.team_id = pc.team_id \
           WHERE pc.spiffe_uri = $1 AND pc.serial_number = $2 \
             AND pc.fingerprint_sha256 IS NULL \
             AND pc.revoked_at IS NULL AND pc.expires_at > now() \
             AND dp.retired_at IS NULL \
           FOR UPDATE \
         ) \
         AND fingerprint_sha256 IS NULL \
         RETURNING {CERT_COLUMNS}"
    ))
    .bind(spiffe_uri)
    .bind(serial_number)
    .bind(fingerprint_sha256)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .and_then(|database_error| database_error.code())
            .as_deref()
            == Some("21000")
        {
            DomainError::conflict(
                "multiple active legacy certificates match the presented URI and serial",
            )
        } else {
            DomainError::internal(format!("pin legacy certificate fingerprint: {error}"))
        }
    })?;
    row.as_ref().map(cert_from_row).ok_or_else(|| {
        DomainError::conflict(
            "no unique active legacy certificate matches the presented URI and serial",
        )
    })
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
    let dataplane_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT dataplane_id FROM proxy_certificates WHERE team_id = $1 AND serial_number = $2",
    )
    .bind(team_id.as_uuid())
    .bind(serial_number)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("revoke certificate: resolve dataplane: {e}")))?;
    let Some(dataplane_id) = dataplane_id else {
        return Err(DomainError::not_found("proxy certificate", serial_number));
    };
    let active: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM dataplanes \
         WHERE team_id = $1 AND id = $2 AND retired_at IS NULL FOR UPDATE",
    )
    .bind(team_id.as_uuid())
    .bind(dataplane_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("revoke certificate: lock dataplane: {e}")))?;
    if active.is_none() {
        return Err(DomainError::conflict("dataplane is retired"));
    }
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

pub async fn retire_dataplane(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
    reason: &str,
) -> DomainResult<(Dataplane, Vec<ProxyCertificate>)> {
    let current = sqlx::query(&format!(
        "SELECT {DP_COLUMNS} FROM dataplanes \
         WHERE team_id = $1 AND name = $2 AND retired_at IS NULL FOR UPDATE"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("retire dataplane: lock: {e}")))?;
    let Some(current) = current else {
        return Err(DomainError::not_found("dataplane", name));
    };
    let current = dataplane_from_row(&current);
    if current.version != expected_version {
        return Err(DomainError::new(
            fp_domain::ErrorCode::RevisionMismatch,
            format!(
                "dataplane \"{name}\" is at revision {}, you supplied {expected_version}",
                current.version
            ),
        )
        .with_hint("re-read the dataplane and retry with the current revision"));
    }

    let retired = sqlx::query(&format!(
        "UPDATE dataplanes SET retired_at = now(), retired_reason = $1, \
             version = version + 1, updated_at = now() \
         WHERE id = $2 AND team_id = $3 AND retired_at IS NULL RETURNING {DP_COLUMNS}"
    ))
    .bind(reason)
    .bind(current.id.as_uuid())
    .bind(team_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("retire dataplane: tombstone: {e}")))?;
    let certificates = sqlx::query(&format!(
        "UPDATE proxy_certificates SET revoked_at = now(), revoked_reason = 'dataplane retired' \
         WHERE team_id = $1 AND dataplane_id = $2 AND revoked_at IS NULL \
         RETURNING {CERT_COLUMNS}"
    ))
    .bind(team_id.as_uuid())
    .bind(current.id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("retire dataplane: revoke credentials: {e}")))?;
    Ok((
        dataplane_from_row(&retired),
        certificates.iter().map(cert_from_row).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::certificate_unique_conflict;
    use fp_domain::ErrorCode;

    #[test]
    fn certificate_unique_constraints_map_to_stable_conflicts() {
        let cases = [
            (
                "uq_proxy_certificates_fingerprint_sha256",
                "leaf fingerprint",
            ),
            (
                "proxy_certificates_team_id_serial_number_key",
                "serial already exists in this team",
            ),
            ("future_unique_constraint", "certificate identity conflicts"),
        ];
        for (constraint, message_fragment) in cases {
            let error = certificate_unique_conflict(constraint);
            assert_eq!(error.code, ErrorCode::Conflict);
            assert!(error.message.contains(message_fragment));
            assert!(error.hint.is_some());
        }
    }
}
