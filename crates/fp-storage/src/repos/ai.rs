//! AI gateway repositories.

use fp_domain::authz::TeamRef;
use fp_domain::{
    AiBudget, AiBudgetId, AiBudgetMode, AiBudgetSpec, AiProvider, AiProviderId, AiProviderKind,
    AiProviderSpec, AiRoute, AiRouteBackend, AiRouteId, AiRouteMaterializedResources, AiRouteSpec,
    AiRouteStatus, AiUsageSummary, DomainError, DomainResult, ErrorCode, OpenAiTokenUsage,
    RouteConfigId, SecretId, TeamId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

const COLUMNS: &str = "id, team_id, name, kind, base_url, path_prefix, credential_secret_id, \
                       models, auth_header, version, created_at, updated_at";
const PROVIDER_COLUMNS: &str = "p.id, p.team_id, p.name, p.kind, p.base_url, p.path_prefix, \
                                p.credential_secret_id, p.models, p.auth_header, p.version, \
                                p.created_at, p.updated_at";
const ROUTE_COLUMNS: &str = "id, team_id, name, spec, status, cluster_names, route_config_name, \
                             listener_name, version, created_at, updated_at";
const BUDGET_COLUMNS: &str = "id, team_id, name, mode, limit_units, window_seconds, provider_id, \
                              route_config_id, prompt_token_weight, completion_token_weight, \
                              version, created_at, updated_at";

#[derive(Debug, Clone)]
pub struct SelectedAiBackend {
    pub provider: AiProvider,
    pub backend: AiRouteBackend,
}

pub struct AiUsageEventInsert {
    pub team_id: TeamId,
    pub route_config_id: RouteConfigId,
    pub provider_id: AiProviderId,
    pub backend_position: Option<i32>,
    pub usage: OpenAiTokenUsage,
}

#[derive(Debug, Clone, Copy)]
pub struct AiUsageQuery {
    pub route_config_id: Option<RouteConfigId>,
    pub provider_id: Option<AiProviderId>,
    pub limit: i64,
    pub offset: i64,
}

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

fn budget_from_row(row: &PgRow) -> DomainResult<AiBudget> {
    let limit_units: i64 = row.get("limit_units");
    let window_seconds: i32 = row.get("window_seconds");
    let prompt_token_weight: i32 = row.get("prompt_token_weight");
    let completion_token_weight: i32 = row.get("completion_token_weight");
    Ok(AiBudget {
        id: AiBudgetId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        spec: AiBudgetSpec {
            mode: AiBudgetMode::from_str(row.get::<&str, _>("mode"))?,
            limit_units: u64::try_from(limit_units).map_err(|_| {
                DomainError::internal("AI budget limit_units in DB is outside domain range")
            })?,
            window_seconds: u32::try_from(window_seconds).map_err(|_| {
                DomainError::internal("AI budget window_seconds in DB is outside domain range")
            })?,
            provider_id: row
                .try_get::<Option<Uuid>, _>("provider_id")
                .map_err(|e| DomainError::internal(format!("read AI budget provider_id: {e}")))?
                .map(AiProviderId::from),
            route_config_id: row
                .try_get::<Option<Uuid>, _>("route_config_id")
                .map_err(|e| DomainError::internal(format!("read AI budget route_config_id: {e}")))?
                .map(RouteConfigId::from),
            prompt_token_weight: u32::try_from(prompt_token_weight).map_err(|_| {
                DomainError::internal("AI budget prompt_token_weight in DB is outside domain range")
            })?,
            completion_token_weight: u32::try_from(completion_token_weight).map_err(|_| {
                DomainError::internal(
                    "AI budget completion_token_weight in DB is outside domain range",
                )
            })?,
        },
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub async fn record_usage_event(pool: &PgPool, event: AiUsageEventInsert) -> DomainResult<()> {
    let prompt_tokens = i64::try_from(event.usage.prompt_tokens)
        .map_err(|_| DomainError::validation("AI prompt token count exceeds storage range"))?;
    let completion_tokens = i64::try_from(event.usage.completion_tokens)
        .map_err(|_| DomainError::validation("AI completion token count exceeds storage range"))?;
    let total_tokens = i64::try_from(event.usage.total_tokens)
        .map_err(|_| DomainError::validation("AI total token count exceeds storage range"))?;
    sqlx::query(
        "INSERT INTO ai_usage_events \
         (id, team_id, route_config_id, provider_id, backend_position, \
          prompt_tokens, completion_tokens, total_tokens) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(event.team_id.as_uuid())
    .bind(event.route_config_id.as_uuid())
    .bind(event.provider_id.as_uuid())
    .bind(event.backend_position)
    .bind(prompt_tokens)
    .bind(completion_tokens)
    .bind(total_tokens)
    .execute(pool)
    .await
    .map_err(|e| DomainError::internal(format!("record AI usage event: {e}")))?;
    Ok(())
}

pub async fn record_usage_event_and_settle_budgets(
    pool: &PgPool,
    event: AiUsageEventInsert,
) -> DomainResult<()> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("record AI usage event: begin: {e}")))?;
    insert_usage_event_in_tx(&mut tx, &event).await?;
    settle_budgets_in_tx(&mut tx, &event).await?;
    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("record AI usage event: commit: {e}")))?;
    Ok(())
}

pub async fn exhausted_enforcing_budget(
    pool: &PgPool,
    team_id: TeamId,
    route_config_id: RouteConfigId,
    provider_id: AiProviderId,
) -> DomainResult<Option<String>> {
    sqlx::query_scalar(
        "SELECT b.name \
         FROM ai_budgets b \
         LEFT JOIN ai_budget_counters c \
           ON c.budget_id = b.id \
          AND c.window_start = to_timestamp(floor(extract(epoch FROM now()) / b.window_seconds) * b.window_seconds) \
         WHERE b.team_id = $1 \
           AND b.mode = 'enforcing' \
           AND (b.provider_id IS NULL OR b.provider_id = $2) \
           AND (b.route_config_id IS NULL OR b.route_config_id = $3) \
           AND COALESCE(c.used_units, 0) >= b.limit_units \
         ORDER BY b.name \
         LIMIT 1",
    )
    .bind(team_id.as_uuid())
    .bind(provider_id.as_uuid())
    .bind(route_config_id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("check AI budget capacity: {e}")))
}

/// One matching shadow budget's current standing, read for trace annotation only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowBudgetEvaluation {
    pub name: String,
    pub used_units: i64,
    pub limit_units: i64,
}

/// Read-only shadow-budget evaluation for the trace `budget` hop (design AC 3): returns
/// every `mode='shadow'` budget matching this (team, route config, provider) scope with
/// its current-window usage, so capture can record `would_reject` without ever gating the
/// request. Never writes: counters are settled only by usage settlement.
pub async fn evaluate_shadow_budgets(
    pool: &PgPool,
    team_id: TeamId,
    route_config_id: RouteConfigId,
    provider_id: AiProviderId,
) -> DomainResult<Vec<ShadowBudgetEvaluation>> {
    let rows = sqlx::query(
        "SELECT b.name, COALESCE(c.used_units, 0) AS used_units, b.limit_units \
         FROM ai_budgets b \
         LEFT JOIN ai_budget_counters c \
           ON c.budget_id = b.id \
          AND c.window_start = to_timestamp(floor(extract(epoch FROM now()) / b.window_seconds) * b.window_seconds) \
         WHERE b.team_id = $1 \
           AND b.mode = 'shadow' \
           AND (b.provider_id IS NULL OR b.provider_id = $2) \
           AND (b.route_config_id IS NULL OR b.route_config_id = $3) \
         ORDER BY b.name",
    )
    .bind(team_id.as_uuid())
    .bind(provider_id.as_uuid())
    .bind(route_config_id.as_uuid())
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("evaluate AI shadow budgets: {e}")))?;
    Ok(rows
        .iter()
        .map(|row| ShadowBudgetEvaluation {
            name: row.get("name"),
            used_units: row.get("used_units"),
            limit_units: row.get("limit_units"),
        })
        .collect())
}

async fn insert_usage_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: &AiUsageEventInsert,
) -> DomainResult<()> {
    let prompt_tokens = i64::try_from(event.usage.prompt_tokens)
        .map_err(|_| DomainError::validation("AI prompt token count exceeds storage range"))?;
    let completion_tokens = i64::try_from(event.usage.completion_tokens)
        .map_err(|_| DomainError::validation("AI completion token count exceeds storage range"))?;
    let total_tokens = i64::try_from(event.usage.total_tokens)
        .map_err(|_| DomainError::validation("AI total token count exceeds storage range"))?;
    sqlx::query(
        "INSERT INTO ai_usage_events \
         (id, team_id, route_config_id, provider_id, backend_position, \
          prompt_tokens, completion_tokens, total_tokens) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(event.team_id.as_uuid())
    .bind(event.route_config_id.as_uuid())
    .bind(event.provider_id.as_uuid())
    .bind(event.backend_position)
    .bind(prompt_tokens)
    .bind(completion_tokens)
    .bind(total_tokens)
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("record AI usage event: {e}")))?;
    Ok(())
}

async fn settle_budgets_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: &AiUsageEventInsert,
) -> DomainResult<()> {
    let rows = sqlx::query(&format!(
        "SELECT {BUDGET_COLUMNS} FROM ai_budgets \
         WHERE team_id = $1 \
           AND (provider_id IS NULL OR provider_id = $2) \
           AND (route_config_id IS NULL OR route_config_id = $3) \
         ORDER BY name \
         FOR UPDATE"
    ))
    .bind(event.team_id.as_uuid())
    .bind(event.provider_id.as_uuid())
    .bind(event.route_config_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("load AI budgets for settlement: {e}")))?;
    for row in rows {
        let budget = budget_from_row(&row)?;
        let units = budget.spec.units_for_usage(event.usage)?;
        let units = i64::try_from(units)
            .map_err(|_| DomainError::validation("AI budget units exceed storage range"))?;
        sqlx::query(
            "INSERT INTO ai_budget_counters (budget_id, team_id, window_start, used_units) \
             VALUES ($1, $2, to_timestamp(floor(extract(epoch FROM now()) / $3) * $3), $4) \
             ON CONFLICT (budget_id, window_start) DO UPDATE \
             SET used_units = ai_budget_counters.used_units + EXCLUDED.used_units, updated_at = now()",
        )
        .bind(budget.id.as_uuid())
        .bind(event.team_id.as_uuid())
        .bind(i64::from(budget.spec.window_seconds))
        .bind(units)
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("settle AI budget counter: {e}")))?;
    }
    Ok(())
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

pub async fn get_provider_for_route_config(
    pool: &PgPool,
    team_id: TeamId,
    route_config_id: RouteConfigId,
    provider_id: AiProviderId,
) -> DomainResult<Option<AiProvider>> {
    Ok(
        get_backend_for_route_config(pool, team_id, route_config_id, provider_id, None)
            .await?
            .map(|selected| selected.provider),
    )
}

pub async fn get_backend_for_route_config(
    pool: &PgPool,
    team_id: TeamId,
    route_config_id: RouteConfigId,
    provider_id: AiProviderId,
    position: Option<i32>,
) -> DomainResult<Option<SelectedAiBackend>> {
    let row = sqlx::query(&format!(
        "SELECT {PROVIDER_COLUMNS}, r.spec AS route_spec, b.position AS backend_position \
         FROM ai_providers p \
         JOIN ai_route_backends b ON b.team_id = p.team_id AND b.provider_id = p.id \
         JOIN ai_routes r ON r.team_id = b.team_id AND r.id = b.ai_route_id \
         JOIN route_configs rc ON rc.team_id = r.team_id AND rc.name = r.route_config_name \
         WHERE p.team_id = $1 AND rc.id = $2 AND p.id = $3 AND ($4::INT IS NULL OR b.position = $4) \
         LIMIT 1"
    ))
    .bind(team_id.as_uuid())
    .bind(route_config_id.as_uuid())
    .bind(provider_id.as_uuid())
    .bind(position)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get AI backend for route config: {e}")))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let spec: AiRouteSpec = serde_json::from_value(row.get("route_spec"))
        .map_err(|e| DomainError::internal(format!("AI route spec in DB does not parse: {e}")))?;
    let backend_position: i32 = row.get("backend_position");
    let backend = spec
        .backends
        .get(usize::try_from(backend_position).unwrap_or(usize::MAX))
        .cloned()
        .ok_or_else(|| DomainError::internal("AI backend position is outside route spec"))?;
    Ok(Some(SelectedAiBackend {
        provider: provider_from_row(&row)?,
        backend,
    }))
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

pub async fn budget_names_referencing_provider(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    provider_id: AiProviderId,
) -> DomainResult<Vec<String>> {
    let rows = sqlx::query(
        "SELECT name FROM ai_budgets \
         WHERE team_id = $1 AND provider_id = $2 ORDER BY name",
    )
    .bind(team_id.as_uuid())
    .bind(provider_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("list AI budgets referencing provider: {e}")))?;
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

pub async fn create_budget(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &AiBudgetSpec,
) -> DomainResult<AiBudget> {
    let limit_units = i64::try_from(spec.limit_units)
        .map_err(|_| DomainError::validation("AI budget limit_units exceeds storage range"))?;
    let row = sqlx::query(&format!(
        "INSERT INTO ai_budgets \
           (id, team_id, org_id, name, mode, limit_units, window_seconds, provider_id, \
            route_config_id, prompt_token_weight, completion_token_weight) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING {BUDGET_COLUMNS}"
    ))
    .bind(AiBudgetId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(spec.mode.as_str())
    .bind(limit_units)
    .bind(i32::try_from(spec.window_seconds).unwrap_or(i32::MAX))
    .bind(spec.provider_id.map(|id| id.as_uuid()))
    .bind(spec.route_config_id.map(|id| id.as_uuid()))
    .bind(i32::try_from(spec.prompt_token_weight).unwrap_or(i32::MAX))
    .bind(i32::try_from(spec.completion_token_weight).unwrap_or(i32::MAX))
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict(format!("AI budget \"{name}\" already exists in this team"))
                .with_hint("choose a different name or update the existing budget")
        }
        _ => DomainError::internal(format!("create AI budget: {e}")),
    })?;
    budget_from_row(&row)
}

pub async fn get_budget(
    pool: &PgPool,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<AiBudget>> {
    let row = sqlx::query(&format!(
        "SELECT {BUDGET_COLUMNS} FROM ai_budgets WHERE team_id = $1 AND name = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get AI budget: {e}")))?;
    row.as_ref().map(budget_from_row).transpose()
}

pub async fn list_budgets(
    pool: &PgPool,
    team_id: TeamId,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<AiBudget>, i64)> {
    let rows = sqlx::query(&format!(
        "SELECT {BUDGET_COLUMNS} FROM ai_budgets WHERE team_id = $1 ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list AI budgets: {e}")))?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_budgets WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count AI budgets: {e}")))?;
    rows.iter()
        .map(budget_from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|items| (items, total))
}

pub async fn update_budget(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    spec: &AiBudgetSpec,
    expected_version: i64,
) -> DomainResult<AiBudget> {
    let limit_units = i64::try_from(spec.limit_units)
        .map_err(|_| DomainError::validation("AI budget limit_units exceeds storage range"))?;
    let row = sqlx::query(&format!(
        "UPDATE ai_budgets \
         SET mode = $1, limit_units = $2, window_seconds = $3, provider_id = $4, \
             route_config_id = $5, prompt_token_weight = $6, completion_token_weight = $7, \
             version = version + 1, updated_at = now() \
         WHERE team_id = $8 AND name = $9 AND version = $10 RETURNING {BUDGET_COLUMNS}"
    ))
    .bind(spec.mode.as_str())
    .bind(limit_units)
    .bind(i32::try_from(spec.window_seconds).unwrap_or(i32::MAX))
    .bind(spec.provider_id.map(|id| id.as_uuid()))
    .bind(spec.route_config_id.map(|id| id.as_uuid()))
    .bind(i32::try_from(spec.prompt_token_weight).unwrap_or(i32::MAX))
    .bind(i32::try_from(spec.completion_token_weight).unwrap_or(i32::MAX))
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("update AI budget: {e}")))?;
    match row {
        Some(row) => budget_from_row(&row),
        None => {
            budget_revision_error(tx, team_id, name, expected_version, "update AI budget").await
        }
    }
}

pub async fn delete_budget(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainResult<AiBudget> {
    let row = sqlx::query(&format!(
        "DELETE FROM ai_budgets WHERE team_id = $1 AND name = $2 AND version = $3 RETURNING {BUDGET_COLUMNS}"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete AI budget: {e}")))?;
    match row {
        Some(row) => budget_from_row(&row),
        None => {
            budget_revision_error(tx, team_id, name, expected_version, "delete AI budget").await
        }
    }
}

pub async fn count_budgets_for_team(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM ai_budgets WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count AI budgets: {e}")))
}

pub async fn usage_summary(
    pool: &PgPool,
    team_id: TeamId,
    query: AiUsageQuery,
) -> DomainResult<Vec<AiUsageSummary>> {
    let rows = sqlx::query(
        "SELECT route_config_id, provider_id, \
                COALESCE(sum(prompt_tokens), 0)::BIGINT AS prompt_tokens, \
                COALESCE(sum(completion_tokens), 0)::BIGINT AS completion_tokens, \
                COALESCE(sum(total_tokens), 0)::BIGINT AS total_tokens, \
                count(*)::BIGINT AS event_count \
         FROM ai_usage_events \
         WHERE team_id = $1 \
           AND ($2::UUID IS NULL OR route_config_id = $2) \
           AND ($3::UUID IS NULL OR provider_id = $3) \
         GROUP BY route_config_id, provider_id \
         ORDER BY total_tokens DESC, route_config_id, provider_id \
         LIMIT $4 OFFSET $5",
    )
    .bind(team_id.as_uuid())
    .bind(query.route_config_id.map(|id| id.as_uuid()))
    .bind(query.provider_id.map(|id| id.as_uuid()))
    .bind(query.limit.clamp(1, 500))
    .bind(query.offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("query AI usage summary: {e}")))?;
    rows.into_iter()
        .map(|row| {
            Ok(AiUsageSummary {
                route_config_id: row
                    .get::<Option<Uuid>, _>("route_config_id")
                    .map(RouteConfigId::from),
                provider_id: row
                    .get::<Option<Uuid>, _>("provider_id")
                    .map(AiProviderId::from),
                prompt_tokens: u64::try_from(row.get::<i64, _>("prompt_tokens")).map_err(|_| {
                    DomainError::internal(
                        "AI usage prompt_tokens aggregate is outside domain range",
                    )
                })?,
                completion_tokens: u64::try_from(row.get::<i64, _>("completion_tokens")).map_err(
                    |_| {
                        DomainError::internal(
                            "AI usage completion_tokens aggregate is outside domain range",
                        )
                    },
                )?,
                total_tokens: u64::try_from(row.get::<i64, _>("total_tokens")).map_err(|_| {
                    DomainError::internal("AI usage total_tokens aggregate is outside domain range")
                })?,
                event_count: u64::try_from(row.get::<i64, _>("event_count")).map_err(|_| {
                    DomainError::internal("AI usage event_count aggregate is outside domain range")
                })?,
            })
        })
        .collect()
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

async fn budget_revision_error<T>(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
    op: &str,
) -> DomainResult<T> {
    let current: Option<i64> =
        sqlx::query_scalar("SELECT version FROM ai_budgets WHERE team_id = $1 AND name = $2")
            .bind(team_id.as_uuid())
            .bind(name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("{op}: recheck: {e}")))?;
    Err(match current {
        Some(version) => DomainError::new(
            ErrorCode::RevisionMismatch,
            format!(
                "AI budget \"{name}\" is at revision {version}, you supplied {expected_version}"
            ),
        )
        .with_hint("re-read the budget and retry with the current revision"),
        None => DomainError::not_found("AI budget", name),
    })
}
