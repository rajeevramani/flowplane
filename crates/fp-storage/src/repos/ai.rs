//! AI gateway repositories.

use fp_domain::authz::TeamRef;
use fp_domain::{
    AiProvider, AiProviderId, AiProviderKind, AiProviderSpec, AiRoute, AiRouteId,
    AiRouteMaterializedResources, AiRouteSpec, AiRouteStatus, DomainError, DomainResult, ErrorCode,
    SecretId, TeamId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

const COLUMNS: &str = "id, team_id, name, kind, base_url, path_prefix, credential_secret_id, \
                       models, auth_header, version, created_at, updated_at";
const ROUTE_COLUMNS: &str = "id, team_id, name, spec, status, cluster_names, route_config_name, \
                             listener_name, version, created_at, updated_at";

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

fn route_from_row(row: &PgRow) -> DomainResult<AiRoute> {
    let spec: serde_json::Value = row.get("spec");
    Ok(AiRoute {
        id: AiRouteId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        spec: serde_json::from_value::<AiRouteSpec>(spec).map_err(|e| {
            DomainError::internal(format!("AI route spec in DB does not parse: {e}"))
        })?,
        status: AiRouteStatus::from_str(row.get::<&str, _>("status"))?,
        materialized: AiRouteMaterializedResources {
            cluster_names: row.get("cluster_names"),
            route_config_name: row.get("route_config_name"),
            listener_name: row.get("listener_name"),
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

pub async fn get_provider_by_id(
    pool: &PgPool,
    team_id: TeamId,
    id: AiProviderId,
) -> DomainResult<Option<AiProvider>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM ai_providers WHERE team_id = $1 AND id = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get AI provider by id: {e}")))?;
    row.as_ref().map(provider_from_row).transpose()
}

pub async fn route_names_referencing_provider(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    provider_id: AiProviderId,
) -> DomainResult<Vec<String>> {
    let rows = sqlx::query(
        "SELECT r.name FROM ai_routes r \
         JOIN ai_route_backends b ON b.ai_route_id = r.id AND b.team_id = r.team_id \
         WHERE r.team_id = $1 AND b.provider_id = $2 ORDER BY r.name",
    )
    .bind(team_id.as_uuid())
    .bind(provider_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("list AI routes referencing provider: {e}")))?;
    Ok(rows.into_iter().map(|row| row.get("name")).collect())
}

pub async fn mark_routes_stale_for_provider(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    provider_id: AiProviderId,
) -> DomainResult<u64> {
    let result = sqlx::query(
        "UPDATE ai_routes SET status = 'stale', version = version + 1, updated_at = now() \
         WHERE team_id = $1 AND id IN (\
           SELECT ai_route_id FROM ai_route_backends WHERE team_id = $1 AND provider_id = $2\
         )",
    )
    .bind(team_id.as_uuid())
    .bind(provider_id.as_uuid())
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("mark AI routes stale: {e}")))?;
    Ok(result.rows_affected())
}

pub async fn create_route(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &AiRouteSpec,
    materialized: &AiRouteMaterializedResources,
) -> DomainResult<AiRoute> {
    let id = AiRouteId::generate();
    let spec_json = serde_json::to_value(spec)
        .map_err(|e| DomainError::internal(format!("serialize AI route spec: {e}")))?;
    let row = sqlx::query(&format!(
        "INSERT INTO ai_routes \
           (id, team_id, org_id, name, spec, cluster_names, route_config_name, listener_name) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {ROUTE_COLUMNS}"
    ))
    .bind(id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(spec_json)
    .bind(&materialized.cluster_names)
    .bind(&materialized.route_config_name)
    .bind(&materialized.listener_name)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict(format!("AI route \"{name}\" already exists in this team"))
                .with_hint("choose a different name or update the existing route")
        }
        _ => DomainError::internal(format!("create AI route: {e}")),
    })?;
    insert_route_backends(tx, id, team.id, spec).await?;
    route_from_row(&row)
}

pub async fn get_route(
    pool: &PgPool,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<AiRoute>> {
    let row = sqlx::query(&format!(
        "SELECT {ROUTE_COLUMNS} FROM ai_routes WHERE team_id = $1 AND name = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get AI route: {e}")))?;
    row.as_ref().map(route_from_row).transpose()
}

pub async fn list_routes(
    pool: &PgPool,
    team_id: TeamId,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<AiRoute>, i64)> {
    let rows = sqlx::query(&format!(
        "SELECT {ROUTE_COLUMNS} FROM ai_routes WHERE team_id = $1 ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list AI routes: {e}")))?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_routes WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count AI routes: {e}")))?;
    rows.iter()
        .map(route_from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|items| (items, total))
}

pub async fn update_route(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    spec: &AiRouteSpec,
    materialized: &AiRouteMaterializedResources,
    expected_version: i64,
) -> DomainResult<AiRoute> {
    let spec_json = serde_json::to_value(spec)
        .map_err(|e| DomainError::internal(format!("serialize AI route spec: {e}")))?;
    let row = sqlx::query(&format!(
        "UPDATE ai_routes \
         SET spec = $1, status = 'active', cluster_names = $2, route_config_name = $3, \
             listener_name = $4, version = version + 1, updated_at = now() \
         WHERE team_id = $5 AND name = $6 AND version = $7 RETURNING {ROUTE_COLUMNS}"
    ))
    .bind(spec_json)
    .bind(&materialized.cluster_names)
    .bind(&materialized.route_config_name)
    .bind(&materialized.listener_name)
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("update AI route: {e}")))?;
    let route = match row {
        Some(row) => route_from_row(&row)?,
        None => {
            return route_revision_error(tx, team_id, name, expected_version, "update AI route")
                .await
        }
    };
    sqlx::query("DELETE FROM ai_route_backends WHERE ai_route_id = $1 AND team_id = $2")
        .bind(route.id.as_uuid())
        .bind(team_id.as_uuid())
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("replace AI route backends: {e}")))?;
    insert_route_backends(tx, route.id, team_id, spec).await?;
    Ok(route)
}

pub async fn delete_route(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainResult<AiRoute> {
    let row = sqlx::query(&format!(
        "DELETE FROM ai_routes WHERE team_id = $1 AND name = $2 AND version = $3 RETURNING {ROUTE_COLUMNS}"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete AI route: {e}")))?;
    match row {
        Some(row) => route_from_row(&row),
        None => route_revision_error(tx, team_id, name, expected_version, "delete AI route").await,
    }
}

pub async fn count_routes_for_team(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM ai_routes WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count AI routes: {e}")))
}

async fn insert_route_backends(
    tx: &mut Transaction<'_, Postgres>,
    ai_route_id: AiRouteId,
    team_id: TeamId,
    spec: &AiRouteSpec,
) -> DomainResult<()> {
    for (position, backend) in spec.backends.iter().enumerate() {
        sqlx::query(
            "INSERT INTO ai_route_backends (ai_route_id, team_id, provider_id, position) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(ai_route_id.as_uuid())
        .bind(team_id.as_uuid())
        .bind(backend.provider_id.as_uuid())
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("insert AI route backend: {e}")))?;
    }
    Ok(())
}

async fn route_revision_error<T>(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
    op: &str,
) -> DomainResult<T> {
    let current: Option<i64> =
        sqlx::query_scalar("SELECT version FROM ai_routes WHERE team_id = $1 AND name = $2")
            .bind(team_id.as_uuid())
            .bind(name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("{op}: recheck: {e}")))?;
    Err(match current {
        Some(version) => DomainError::new(
            ErrorCode::RevisionMismatch,
            format!(
                "AI route \"{name}\" is at revision {version}, you supplied {expected_version}"
            ),
        )
        .with_hint("re-read the route and retry with the current revision"),
        None => DomainError::not_found("AI route", name),
    })
}
