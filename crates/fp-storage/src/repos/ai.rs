//! AI gateway repositories.

use fp_domain::authz::TeamRef;
use fp_domain::{
    AiProvider, AiProviderId, AiProviderKind, AiProviderSpec, DomainError, DomainResult, ErrorCode,
    SecretId, TeamId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

const COLUMNS: &str = "id, team_id, name, kind, base_url, path_prefix, credential_secret_id, \
                       models, auth_header, version, created_at, updated_at";

fn provider_from_row(row: &PgRow) -> DomainResult<AiProvider> {
    Ok(AiProvider {
        id: AiProviderId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        spec: AiProviderSpec {
            kind: AiProviderKind::from_str(row.get::<&str, _>("kind"))?,
            base_url: row.get("base_url"),
            path_prefix: row.get("path_prefix"),
            credential_secret_id: SecretId::from(row.get::<Uuid, _>("credential_secret_id")),
            models: row.get("models"),
            auth_header: row.get("auth_header"),
        },
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &AiProviderSpec,
) -> DomainResult<AiProvider> {
    let row = sqlx::query(&format!(
        "INSERT INTO ai_providers \
           (id, team_id, org_id, name, kind, base_url, path_prefix, credential_secret_id, models, auth_header) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {COLUMNS}"
    ))
    .bind(AiProviderId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(spec.kind.as_str())
    .bind(&spec.base_url)
    .bind(&spec.path_prefix)
    .bind(spec.credential_secret_id.as_uuid())
    .bind(&spec.models)
    .bind(&spec.auth_header)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict(format!("AI provider \"{name}\" already exists in this team"))
                .with_hint("choose a different name or update the existing provider")
        }
        _ => DomainError::internal(format!("create AI provider: {e}")),
    })?;
    provider_from_row(&row)
}

pub async fn get(pool: &PgPool, team_id: TeamId, name: &str) -> DomainResult<Option<AiProvider>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM ai_providers WHERE team_id = $1 AND name = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get AI provider: {e}")))?;
    row.as_ref().map(provider_from_row).transpose()
}

pub async fn list(
    pool: &PgPool,
    team_id: TeamId,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<AiProvider>, i64)> {
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM ai_providers WHERE team_id = $1 ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list AI providers: {e}")))?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_providers WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count AI providers: {e}")))?;
    rows.iter()
        .map(provider_from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|items| (items, total))
}

pub async fn update(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    spec: &AiProviderSpec,
    expected_version: i64,
) -> DomainResult<AiProvider> {
    let row = sqlx::query(&format!(
        "UPDATE ai_providers \
         SET kind = $1, base_url = $2, path_prefix = $3, credential_secret_id = $4, \
             models = $5, auth_header = $6, version = version + 1, updated_at = now() \
         WHERE team_id = $7 AND name = $8 AND version = $9 RETURNING {COLUMNS}"
    ))
    .bind(spec.kind.as_str())
    .bind(&spec.base_url)
    .bind(&spec.path_prefix)
    .bind(spec.credential_secret_id.as_uuid())
    .bind(&spec.models)
    .bind(&spec.auth_header)
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("update AI provider: {e}")))?;
    match row {
        Some(row) => provider_from_row(&row),
        None => revision_error(tx, team_id, name, expected_version, "update AI provider").await,
    }
}

pub async fn delete(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainResult<AiProviderId> {
    let row = sqlx::query(
        "DELETE FROM ai_providers WHERE team_id = $1 AND name = $2 AND version = $3 RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete AI provider: {e}")))?;
    match row {
        Some(row) => Ok(AiProviderId::from(row.get::<Uuid, _>("id"))),
        None => revision_error(tx, team_id, name, expected_version, "delete AI provider").await,
    }
}

async fn revision_error<T>(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
    op: &str,
) -> DomainResult<T> {
    let current: Option<i64> =
        sqlx::query_scalar("SELECT version FROM ai_providers WHERE team_id = $1 AND name = $2")
            .bind(team_id.as_uuid())
            .bind(name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("{op}: recheck: {e}")))?;
    Err(match current {
        Some(version) => DomainError::new(
            ErrorCode::RevisionMismatch,
            format!(
                "AI provider \"{name}\" is at revision {version}, you supplied {expected_version}"
            ),
        )
        .with_hint("re-read the provider and retry with the current revision"),
        None => DomainError::not_found("AI provider", name),
    })
}

pub async fn count_for_team(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM ai_providers WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count AI providers: {e}")))
}
