//! Encrypted SDS secret repository. Read paths return metadata only; callers cannot
//! accidentally echo plaintext because this module has no API that returns decrypted values.

use fp_domain::authz::TeamRef;
use fp_domain::{DomainError, DomainResult, ErrorCode, Secret, SecretId, SecretType, TeamId};
use sqlx::postgres::PgRow;
use sqlx::types::chrono;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

const COLUMNS: &str = "id, team_id, name, description, secret_type, version, encryption_key_id, \
                       expires_at, created_at, updated_at";
const DEPENDENCY_NAME_LIMIT: i64 = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretDependencyGroup {
    pub names: Vec<String>,
    pub total: i64,
}

impl SecretDependencyGroup {
    pub fn truncated(&self) -> bool {
        self.total > self.names.len() as i64
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretDependencies {
    pub listeners: SecretDependencyGroup,
    pub clusters: SecretDependencyGroup,
    pub ai_providers: SecretDependencyGroup,
}

impl SecretDependencies {
    pub fn is_empty(&self) -> bool {
        self.listeners.total == 0 && self.clusters.total == 0 && self.ai_providers.total == 0
    }
}

fn secret_from_row(row: &PgRow) -> DomainResult<Secret> {
    Ok(Secret {
        id: SecretId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        description: row.get("description"),
        secret_type: SecretType::from_str(row.get::<&str, _>("secret_type"))?,
        version: row.get("version"),
        encryption_key_id: row.get("encryption_key_id"),
        expires_at: row.get("expires_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn create_secret(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    description: &str,
    secret_type: SecretType,
    ciphertext: &[u8],
    nonce: &[u8],
    encryption_key_id: &str,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> DomainResult<Secret> {
    let row = sqlx::query(&format!(
        "INSERT INTO secrets \
           (id, team_id, org_id, name, description, secret_type, configuration_encrypted, nonce, \
            encryption_key_id, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {COLUMNS}"
    ))
    .bind(SecretId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(description)
    .bind(secret_type.as_str())
    .bind(ciphertext)
    .bind(nonce)
    .bind(encryption_key_id)
    .bind(expires_at)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict(format!("secret \"{name}\" already exists in this team"))
                .with_hint("choose a different name or rotate the existing secret")
        }
        _ => DomainError::internal(format!("create secret: {e}")),
    })?;
    secret_from_row(&row)
}

pub async fn list_secrets(
    pool: &PgPool,
    team_id: TeamId,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<Secret>, i64)> {
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM secrets WHERE team_id = $1 ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list secrets: {e}")))?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM secrets WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count secrets: {e}")))?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(secret_from_row(&row)?);
    }
    Ok((items, total))
}

pub async fn count_for_team(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM secrets WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count secrets: {e}")))
}

pub async fn get_secret(
    pool: &PgPool,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<Secret>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM secrets WHERE team_id = $1 AND name = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get secret: {e}")))?;
    row.as_ref().map(secret_from_row).transpose()
}

pub async fn get_secret_for_update(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<Secret>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM secrets WHERE team_id = $1 AND name = $2 FOR UPDATE"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("lock secret metadata: {e}")))?;
    row.as_ref().map(secret_from_row).transpose()
}

fn dependency_group(rows: &[PgRow]) -> SecretDependencyGroup {
    SecretDependencyGroup {
        names: rows.iter().map(|row| row.get("name")).collect(),
        total: rows.first().map_or(0, |row| row.get("total")),
    }
}

pub async fn dependencies_for_secret(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    secret_id: SecretId,
) -> DomainResult<SecretDependencies> {
    let listener_rows = sqlx::query(
        "SELECT name, count(*) OVER() AS total FROM ( \
           SELECT DISTINCT l.name FROM listener_secret_refs r \
           JOIN listeners l ON l.id = r.listener_id AND l.team_id = r.team_id \
           WHERE r.team_id = $1 AND r.secret_id = $2 \
         ) dependencies ORDER BY name LIMIT $3",
    )
    .bind(team_id.as_uuid())
    .bind(secret_id.as_uuid())
    .bind(DEPENDENCY_NAME_LIMIT)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("read listener secret dependants: {e}")))?;
    let cluster_rows = sqlx::query(
        "SELECT name, count(*) OVER() AS total FROM ( \
           SELECT DISTINCT c.name FROM cluster_secret_refs r \
           JOIN clusters c ON c.id = r.cluster_id AND c.team_id = r.team_id \
           WHERE r.team_id = $1 AND r.secret_id = $2 \
         ) dependencies ORDER BY name LIMIT $3",
    )
    .bind(team_id.as_uuid())
    .bind(secret_id.as_uuid())
    .bind(DEPENDENCY_NAME_LIMIT)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("read cluster secret dependants: {e}")))?;
    let ai_provider_rows = sqlx::query(
        "SELECT name, count(*) OVER() AS total FROM ( \
           SELECT name FROM ai_providers \
           WHERE team_id = $1 AND credential_secret_id = $2 \
         ) dependencies ORDER BY name LIMIT $3",
    )
    .bind(team_id.as_uuid())
    .bind(secret_id.as_uuid())
    .bind(DEPENDENCY_NAME_LIMIT)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("read AI provider secret dependants: {e}")))?;
    Ok(SecretDependencies {
        listeners: dependency_group(&listener_rows),
        clusters: dependency_group(&cluster_rows),
        ai_providers: dependency_group(&ai_provider_rows),
    })
}

pub async fn get_secret_by_id(
    pool: &PgPool,
    team_id: TeamId,
    id: SecretId,
) -> DomainResult<Option<Secret>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM secrets WHERE team_id = $1 AND id = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get secret by id: {e}")))?;
    row.as_ref().map(secret_from_row).transpose()
}

pub async fn get_encrypted_secret_by_id(
    pool: &PgPool,
    team_id: TeamId,
    id: SecretId,
) -> DomainResult<Option<EncryptedSecret>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS}, configuration_encrypted, nonce FROM secrets \
         WHERE team_id = $1 AND id = $2 AND (expires_at IS NULL OR expires_at > now())"
    ))
    .bind(team_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get encrypted secret by id: {e}")))?;
    row.map(|row| {
        Ok(EncryptedSecret {
            metadata: secret_from_row(&row)?,
            ciphertext: row.get("configuration_encrypted"),
            nonce: row.get("nonce"),
        })
    })
    .transpose()
}

#[allow(clippy::too_many_arguments)]
pub async fn rotate_secret(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
    ciphertext: &[u8],
    nonce: &[u8],
    encryption_key_id: &str,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> DomainResult<Secret> {
    let row = sqlx::query(&format!(
        "UPDATE secrets SET configuration_encrypted = $1, nonce = $2, encryption_key_id = $3, \
            expires_at = $4, version = version + 1, updated_at = now() \
         WHERE team_id = $5 AND name = $6 AND version = $7 RETURNING {COLUMNS}"
    ))
    .bind(ciphertext)
    .bind(nonce)
    .bind(encryption_key_id)
    .bind(expires_at)
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("rotate secret: {e}")))?;
    match row {
        Some(row) => secret_from_row(&row),
        None => {
            let current: Option<i64> =
                sqlx::query_scalar("SELECT version FROM secrets WHERE team_id = $1 AND name = $2")
                    .bind(team_id.as_uuid())
                    .bind(name)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| DomainError::internal(format!("rotate secret: recheck: {e}")))?;
            Err(match current {
                Some(version) => DomainError::new(
                    ErrorCode::RevisionMismatch,
                    format!(
                        "secret \"{name}\" is at revision {version}, you supplied {expected_version}"
                    ),
                )
                .with_hint("re-read the secret metadata and retry with the current revision"),
                None => DomainError::not_found("secret", name),
            })
        }
    }
}

pub async fn delete_secret(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainResult<SecretId> {
    let row = sqlx::query(
        "DELETE FROM secrets WHERE team_id = $1 AND name = $2 AND version = $3 RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23503") => {
            DomainError::conflict(format!("secret \"{name}\" is still referenced"))
                .with_hint("update or delete the dependent resources first")
        }
        _ => DomainError::internal(format!("delete secret: {e}")),
    })?;
    match row {
        Some(row) => Ok(SecretId::from(row.get::<Uuid, _>("id"))),
        None => {
            let current: Option<i64> =
                sqlx::query_scalar("SELECT version FROM secrets WHERE team_id = $1 AND name = $2")
                    .bind(team_id.as_uuid())
                    .bind(name)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| DomainError::internal(format!("delete secret: recheck: {e}")))?;
            Err(match current {
                Some(version) => DomainError::new(
                    ErrorCode::RevisionMismatch,
                    format!(
                        "secret \"{name}\" is at revision {version}, you supplied {expected_version}"
                    ),
                )
                .with_hint("re-read the secret metadata and retry with the current revision"),
                None => DomainError::not_found("secret", name),
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct EncryptedSecret {
    pub metadata: Secret,
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
}

pub async fn list_encrypted_secrets(
    pool: &PgPool,
    team_id: TeamId,
) -> DomainResult<Vec<EncryptedSecret>> {
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS}, configuration_encrypted, nonce FROM secrets \
         WHERE team_id = $1 AND (expires_at IS NULL OR expires_at > now()) ORDER BY name"
    ))
    .bind(team_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list encrypted secrets: {e}")))?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(EncryptedSecret {
            metadata: secret_from_row(&row)?,
            ciphertext: row.get("configuration_encrypted"),
            nonce: row.get("nonce"),
        });
    }
    Ok(items)
}
