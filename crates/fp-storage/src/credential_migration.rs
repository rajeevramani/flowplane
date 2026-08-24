//! Read-only operator preflight for the dataplane credential lifecycle migrations.

use fp_domain::{DomainError, DomainResult};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

pub const SERIAL_MALFORMED: &str = "FP_CERT_SERIAL_MALFORMED";
pub const SERIAL_CANONICAL_COLLISION: &str = "FP_CERT_SERIAL_CANONICAL_COLLISION";
pub const UNREVOKED_CAP_EXCEEDED: &str = "FP_CERT_UNREVOKED_CAP_EXCEEDED";

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct CredentialMigrationBlocker {
    pub code: String,
    pub dataplane_ids: Vec<Uuid>,
    pub certificate_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct CredentialMigrationPreflight {
    pub schema_version: u8,
    pub status: &'static str,
    pub blockers: Vec<CredentialMigrationBlocker>,
}

impl CredentialMigrationPreflight {
    pub fn is_ready(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// Inspect the pre-3.1.3 credential population without applying migrations or returning
/// certificate identity/material. The stable blocker codes match migrations 0034 and 0035.
pub async fn preflight(pool: &PgPool) -> DomainResult<CredentialMigrationPreflight> {
    let malformed: Vec<(Uuid, Vec<Uuid>)> = sqlx::query_as(
        "SELECT dataplane_id, array_agg(id ORDER BY id) \
         FROM proxy_certificates \
         WHERE serial_number = '' OR serial_number !~ '^[0-9A-Fa-f]+$' \
         GROUP BY dataplane_id ORDER BY dataplane_id",
    )
    .fetch_all(pool)
    .await
    .map_err(map_preflight_error)?;

    let collisions: Vec<(Vec<Uuid>, Vec<Uuid>)> = sqlx::query_as(
        "WITH canonical AS ( \
             SELECT id, team_id, dataplane_id, \
                    CASE WHEN ltrim(lower(serial_number), '0') = '' THEN '0' \
                         ELSE ltrim(lower(serial_number), '0') END AS canonical_serial \
             FROM proxy_certificates \
             WHERE serial_number <> '' AND serial_number ~ '^[0-9A-Fa-f]+$' \
         ) \
         SELECT array_agg(DISTINCT dataplane_id ORDER BY dataplane_id), \
                array_agg(id ORDER BY id) \
         FROM canonical \
         GROUP BY team_id, canonical_serial \
         HAVING count(*) > 1 \
         ORDER BY array_agg(id ORDER BY id)",
    )
    .fetch_all(pool)
    .await
    .map_err(map_preflight_error)?;

    let over_cap: Vec<(Uuid, Vec<Uuid>)> = sqlx::query_as(
        "SELECT dataplane_id, array_agg(id ORDER BY id) \
         FROM proxy_certificates \
         WHERE revoked_at IS NULL \
         GROUP BY dataplane_id \
         HAVING count(*) > 2 \
         ORDER BY dataplane_id",
    )
    .fetch_all(pool)
    .await
    .map_err(map_preflight_error)?;

    let mut blockers = Vec::with_capacity(malformed.len() + collisions.len() + over_cap.len());
    blockers.extend(
        malformed.into_iter().map(
            |(dataplane_id, certificate_ids)| CredentialMigrationBlocker {
                code: SERIAL_MALFORMED.to_owned(),
                dataplane_ids: vec![dataplane_id],
                certificate_ids,
            },
        ),
    );
    blockers.extend(
        collisions.into_iter().map(
            |(dataplane_ids, certificate_ids)| CredentialMigrationBlocker {
                code: SERIAL_CANONICAL_COLLISION.to_owned(),
                dataplane_ids,
                certificate_ids,
            },
        ),
    );
    blockers.extend(over_cap.into_iter().map(|(dataplane_id, certificate_ids)| {
        CredentialMigrationBlocker {
            code: UNREVOKED_CAP_EXCEEDED.to_owned(),
            dataplane_ids: vec![dataplane_id],
            certificate_ids,
        }
    }));

    Ok(CredentialMigrationPreflight {
        schema_version: 1,
        status: if blockers.is_empty() {
            "ready"
        } else {
            "blocked"
        },
        blockers,
    })
}

fn map_preflight_error(error: sqlx::Error) -> DomainError {
    DomainError::internal(format!("credential migration preflight failed: {error}"))
        .with_hint("verify the database is a Flowplane 3.1.2 or later database")
}
