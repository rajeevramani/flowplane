//! Encrypted SDS secret repository. Read paths return metadata only; callers cannot
//! accidentally echo plaintext because this module has no API that returns decrypted values.

use fp_domain::authz::TeamRef;
use fp_domain::{DomainError, DomainResult, Secret, SecretId, SecretType, TeamId};
use sqlx::postgres::PgRow;
use sqlx::types::chrono;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

const COLUMNS: &str = "id, team_id, name, description, secret_type, version, encryption_key_id, \
                       expires_at, created_at, updated_at";

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

pub async fn rotate_secret(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    secret_type: SecretType,
    ciphertext: &[u8],
    nonce: &[u8],
    encryption_key_id: &str,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> DomainResult<Secret> {
    let row = sqlx::query(&format!(
        "UPDATE secrets SET configuration_encrypted = $1, nonce = $2, encryption_key_id = $3, \
            secret_type = $4, expires_at = $5, version = version + 1, updated_at = now() \
         WHERE team_id = $6 AND name = $7 RETURNING {COLUMNS}"
    ))
    .bind(ciphertext)
    .bind(nonce)
    .bind(encryption_key_id)
    .bind(secret_type.as_str())
    .bind(expires_at)
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("rotate secret: {e}")))?;
    match row {
        Some(row) => secret_from_row(&row),
        None => Err(DomainError::not_found("secret", name)),
    }
}
