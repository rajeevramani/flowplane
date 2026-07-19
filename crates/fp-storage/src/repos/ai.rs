//! AI gateway repositories.

use fp_domain::authz::TeamRef;
use fp_domain::{
    AiBudget, AiBudgetId, AiBudgetMode, AiBudgetSpec, AiBudgetState, AiProvider, AiProviderId,
    AiProviderKind, AiProviderSpec, AiRoute, AiRouteBackend, AiRouteId,
    AiRouteMaterializedResources, AiRouteSpec, AiRouteStatus, AiUsageSummary, DomainError,
    DomainResult, ErrorCode, OpenAiTokenUsage, RouteConfigId, SecretId, TeamId,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

const COLUMNS: &str = "id, team_id, name, kind, base_url, path_prefix, credential_secret_id, \
                       models, auth_header, auth_scheme, version, created_at, updated_at";
const PROVIDER_COLUMNS: &str = "p.id, p.team_id, p.name, p.kind, p.base_url, p.path_prefix, \
                                p.credential_secret_id, p.models, p.auth_header, p.auth_scheme, \
                                p.version, p.created_at, p.updated_at";
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
    /// Half-open window lower bound: only events with `created_at >= since` count.
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    /// Half-open window upper bound: only events with `created_at < until` count.
    pub until: Option<chrono::DateTime<chrono::Utc>>,
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
            auth_scheme: row.get("auth_scheme"),
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
           (id, team_id, org_id, name, kind, base_url, path_prefix, credential_secret_id, models, auth_header, auth_scheme) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING {COLUMNS}"
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
    .bind(&spec.auth_scheme)
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
             models = $5, auth_header = $6, auth_scheme = $7, version = version + 1, \
             updated_at = now() \
         WHERE team_id = $8 AND name = $9 AND version = $10 RETURNING {COLUMNS}"
    ))
    .bind(spec.kind.as_str())
    .bind(&spec.base_url)
    .bind(&spec.path_prefix)
    .bind(spec.credential_secret_id.as_uuid())
    .bind(&spec.models)
    .bind(&spec.auth_header)
    .bind(&spec.auth_scheme)
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

/// Transaction-executor variant: provider read inside an AI-materialization mutation tx,
/// after the per-team advisory lock (fpv2-8am — the read must not precede the lock).
pub async fn get_provider_by_id_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    id: AiProviderId,
) -> DomainResult<Option<AiProvider>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM ai_providers WHERE team_id = $1 AND id = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(id.as_uuid())
    .fetch_optional(&mut **tx)
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

/// Budgets pinning a route's materialized route config (FK `ai_budgets.route_config_id`
/// ON DELETE RESTRICT, 0027). Route update/delete must refuse with a conflict while these
/// exist — the pre-fix cleanup silently leaked the route config instead (fpv2-8am).
pub async fn budget_names_referencing_route_config_name(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    route_config_name: &str,
) -> DomainResult<Vec<String>> {
    let rows = sqlx::query(
        "SELECT b.name FROM ai_budgets b \
         JOIN route_configs rc ON rc.id = b.route_config_id AND rc.team_id = b.team_id \
         WHERE b.team_id = $1 AND rc.name = $2 ORDER BY b.name",
    )
    .bind(team_id.as_uuid())
    .bind(route_config_name)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("list AI budgets referencing route config: {e}")))?;
    Ok(rows.into_iter().map(|row| row.get("name")).collect())
}

/// Namespace half of the per-team AI-materialization advisory-lock key ("flai"). Distinct
/// from `BOOTSTRAP_LOCK_KEY` (bootstrap.rs) so the two lock families can never collide.
const AI_MATERIALIZATION_LOCK_NS: u32 = 0x666c_6169;

/// FNV-1a 32-bit over the team UUID bytes. Deterministic per team; a cross-team collision
/// only over-serializes two teams' AI materialization writes (harmless), never under-locks.
fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in bytes {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// The advisory-lock key serializing all AI materialization mutations for one team.
pub fn ai_materialization_lock_key(team_id: TeamId) -> i64 {
    ((AI_MATERIALIZATION_LOCK_NS as i64) << 32) | i64::from(fnv1a_32(team_id.as_uuid().as_bytes()))
}

/// Serialize AI materialization mutations per team. Must be the FIRST statement of the
/// caller's transaction, before any materialization-sensitive read or write; the lock is
/// released automatically at commit/rollback (transaction-scoped, per the
/// `pg_advisory_xact_lock` precedent in bootstrap.rs).
pub async fn acquire_materialization_lock(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
) -> DomainResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ai_materialization_lock_key(team_id))
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("AI materialization lock: {e}")))?;
    Ok(())
}

/// Dependent AI routes of a provider, each returned once even when the provider occupies
/// multiple backend positions (EXISTS, not JOIN).
pub async fn routes_referencing_provider(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    provider_id: AiProviderId,
) -> DomainResult<Vec<AiRoute>> {
    let rows = sqlx::query(&format!(
        "SELECT {ROUTE_COLUMNS} FROM ai_routes r \
         WHERE r.team_id = $1 AND EXISTS (\
           SELECT 1 FROM ai_route_backends b \
           WHERE b.ai_route_id = r.id AND b.team_id = $1 AND b.provider_id = $2\
         ) ORDER BY r.name"
    ))
    .bind(team_id.as_uuid())
    .bind(provider_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("list AI routes referencing provider: {e}")))?;
    rows.iter().map(route_from_row).collect()
}

/// Bump dependent routes' revision after a provider update: the conflict signal that fails a
/// racing route writer's OCC check instead of letting it materialize from a superseded
/// provider spec. Deliberately does NOT touch `status` — nothing produces `'stale'` anymore.
pub async fn bump_routes_version_for_provider(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    provider_id: AiProviderId,
) -> DomainResult<u64> {
    let result = sqlx::query(
        "UPDATE ai_routes SET version = version + 1, updated_at = now() \
         WHERE team_id = $1 AND id IN (\
           SELECT ai_route_id FROM ai_route_backends WHERE team_id = $1 AND provider_id = $2\
         )",
    )
    .bind(team_id.as_uuid())
    .bind(provider_id.as_uuid())
    .execute(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("bump AI route versions: {e}")))?;
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

/// Transaction-executor variant: route read + revision check inside an AI-materialization
/// mutation tx, after the per-team advisory lock (fpv2-8am — a pre-lock revision check can
/// pass, then lose to a provider update's version bump mid-mutation).
pub async fn get_route_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<AiRoute>> {
    let row = sqlx::query(&format!(
        "SELECT {ROUTE_COLUMNS} FROM ai_routes WHERE team_id = $1 AND name = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(&mut **tx)
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

/// Current-window state for a set of the team's budgets, in ONE batched query: LEFT JOIN
/// `ai_budget_counters` on the per-budget server-aligned window (the same alignment
/// formula enforcement uses in [`exhausted_enforcing_budget`]); a missing counter row
/// yields `used_units = 0` with the server-computed aligned `window_start`. Operator-facing
/// derived read — never writes.
pub async fn budget_window_states(
    pool: &PgPool,
    team_id: TeamId,
    budget_ids: &[Uuid],
) -> DomainResult<Vec<(AiBudgetId, AiBudgetState)>> {
    let rows = sqlx::query(
        "SELECT b.id, \
                COALESCE(c.used_units, 0)::BIGINT AS used_units, \
                to_timestamp(floor(extract(epoch FROM now()) / b.window_seconds) * b.window_seconds) AS window_start, \
                b.limit_units, b.window_seconds \
         FROM ai_budgets b \
         LEFT JOIN ai_budget_counters c \
           ON c.budget_id = b.id \
          AND c.window_start = to_timestamp(floor(extract(epoch FROM now()) / b.window_seconds) * b.window_seconds) \
         WHERE b.team_id = $1 AND b.id = ANY($2)",
    )
    .bind(team_id.as_uuid())
    .bind(budget_ids)
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("read AI budget window states: {e}")))?;
    rows.iter()
        .map(|row| {
            Ok((
                AiBudgetId::from(row.get::<Uuid, _>("id")),
                AiBudgetState {
                    used_units: u64::try_from(row.get::<i64, _>("used_units")).map_err(|_| {
                        DomainError::internal("AI budget used_units is outside domain range")
                    })?,
                    window_start: row.get("window_start"),
                    limit_units: u64::try_from(row.get::<i64, _>("limit_units")).map_err(|_| {
                        DomainError::internal("AI budget limit_units is outside domain range")
                    })?,
                    window_seconds: u32::try_from(row.get::<i32, _>("window_seconds")).map_err(
                        |_| {
                            DomainError::internal(
                                "AI budget window_seconds is outside domain range",
                            )
                        },
                    )?,
                },
            ))
        })
        .collect()
}

pub async fn count_budgets_for_team(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM ai_budgets WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count AI budgets: {e}")))
}

/// Windowed usage summary plus the total number of grouped `(route_config_id, provider_id)`
/// summary rows matching the same filters (the `Page.total`, NOT the raw event count). The
/// items and the total are two reads without a shared snapshot — acceptable drift for an
/// observability read.
pub async fn usage_summary(
    pool: &PgPool,
    team_id: TeamId,
    query: AiUsageQuery,
) -> DomainResult<(Vec<AiUsageSummary>, i64)> {
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ( \
            SELECT 1 FROM ai_usage_events \
            WHERE team_id = $1 \
              AND ($2::UUID IS NULL OR route_config_id = $2) \
              AND ($3::UUID IS NULL OR provider_id = $3) \
              AND ($4::TIMESTAMPTZ IS NULL OR created_at >= $4) \
              AND ($5::TIMESTAMPTZ IS NULL OR created_at < $5) \
            GROUP BY route_config_id, provider_id \
         ) grouped",
    )
    .bind(team_id.as_uuid())
    .bind(query.route_config_id.map(|id| id.as_uuid()))
    .bind(query.provider_id.map(|id| id.as_uuid()))
    .bind(query.since)
    .bind(query.until)
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count AI usage summary groups: {e}")))?;
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
           AND ($4::TIMESTAMPTZ IS NULL OR created_at >= $4) \
           AND ($5::TIMESTAMPTZ IS NULL OR created_at < $5) \
         GROUP BY route_config_id, provider_id \
         ORDER BY total_tokens DESC, route_config_id, provider_id \
         LIMIT $6 OFFSET $7",
    )
    .bind(team_id.as_uuid())
    .bind(query.route_config_id.map(|id| id.as_uuid()))
    .bind(query.provider_id.map(|id| id.as_uuid()))
    .bind(query.since)
    .bind(query.until)
    .bind(query.limit.clamp(1, 500))
    .bind(query.offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("query AI usage summary: {e}")))?;
    let items: DomainResult<Vec<AiUsageSummary>> = rows
        .into_iter()
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
        .collect();
    Ok((items?, total))
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod lock_key_tests {
    use super::*;

    #[test]
    fn lock_key_is_stable_per_team() {
        let team = TeamId::from(Uuid::parse_str("00000000-0000-000f-1071-000000000002").unwrap());
        let key = ai_materialization_lock_key(team);
        // Same team, same key — every time (the correctness property of the lock).
        assert_eq!(key, ai_materialization_lock_key(team));
        // Namespace occupies the high 32 bits.
        assert_eq!((key >> 32) as u32, 0x666c_6169);
    }

    #[test]
    fn lock_key_differs_across_teams_and_from_bootstrap() {
        let a = TeamId::from(Uuid::parse_str("00000000-0000-000f-1071-000000000002").unwrap());
        let b = TeamId::from(Uuid::parse_str("00000000-0000-000f-1071-000000000003").unwrap());
        assert_ne!(
            ai_materialization_lock_key(a),
            ai_materialization_lock_key(b)
        );
        // Never collides with the bootstrap lock family ("flowboot"): different high bits.
        assert_ne!(
            (ai_materialization_lock_key(a) >> 32) as u32,
            0x666c_6f77_u32
        );
    }

    #[test]
    fn fnv1a_matches_reference_vectors() {
        // Published FNV-1a 32-bit vectors.
        assert_eq!(fnv1a_32(b""), 0x811c_9dc5);
        assert_eq!(fnv1a_32(b"a"), 0xe40c_292c);
        assert_eq!(fnv1a_32(b"foobar"), 0xbf9c_f968);
    }
}
