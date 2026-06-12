//! Listener and route-config repositories — the clusters pattern (team predicate in SQL,
//! revision-checked writes) plus normalized reference tracking.

use fp_domain::authz::TeamRef;
use fp_domain::gateway::listener::{Listener, ListenerSpec};
use fp_domain::gateway::route_config::{RouteConfig, RouteConfigSpec};
use fp_domain::{DomainError, DomainResult, ErrorCode, ListenerId, RouteConfigId, TeamId};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

const COLUMNS: &str = "id, team_id, name, spec, version, created_at, updated_at";

fn map_unique(e: sqlx::Error, kind: &str, name: &str) -> DomainError {
    if let sqlx::Error::Database(db) = &e {
        if db.code().as_deref() == Some("23505") {
            let constraint = db.constraint().unwrap_or_default();
            if constraint.contains("port") {
                return DomainError::conflict(
                    "the listener port is already bound by another listener in this team",
                )
                .with_hint("choose a different port or delete the listener holding it");
            }
            return DomainError::conflict(format!("{kind} \"{name}\" already exists in this team"))
                .with_hint("choose a different name or update the existing resource");
        }
    }
    DomainError::internal(format!("write {kind}: {e}"))
}

fn stale_or_missing(kind: &str, name: &str, current: Option<i64>, expected: i64) -> DomainError {
    match current {
        Some(version) => DomainError::new(
            ErrorCode::RevisionMismatch,
            format!("{kind} \"{name}\" is at revision {version}, you supplied {expected}"),
        )
        .with_hint("re-read the resource and retry with the current revision"),
        None => DomainError::not_found(kind, name),
    }
}

// ---------------- route configs ----------------

fn rc_from_row(row: &PgRow) -> DomainResult<RouteConfig> {
    Ok(RouteConfig {
        id: RouteConfigId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        spec: serde_json::from_value(row.get::<serde_json::Value, _>("spec")).map_err(|e| {
            DomainError::internal(format!("route-config spec in DB does not parse: {e}"))
        })?,
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

/// Resolve the cluster ids referenced by a spec, by name within the team. Unknown names are
/// a validation error listing exactly what is missing.
async fn resolve_cluster_refs(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    spec: &RouteConfigSpec,
) -> DomainResult<Vec<Uuid>> {
    let names: Vec<String> = spec
        .referenced_clusters()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let rows = sqlx::query("SELECT id, name FROM clusters WHERE team_id = $1 AND name = ANY($2)")
        .bind(team_id.as_uuid())
        .bind(&names)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("resolve cluster refs: {e}")))?;
    if rows.len() != names.len() {
        let found: std::collections::HashSet<String> =
            rows.iter().map(|r| r.get::<String, _>("name")).collect();
        let missing: Vec<&str> = names
            .iter()
            .filter(|n| !found.contains(*n))
            .map(String::as_str)
            .collect();
        return Err(DomainError::validation(format!(
            "route actions reference clusters that do not exist in this team: {}",
            missing.join(", ")
        ))
        .with_hint("create the clusters first, then the route config"));
    }
    Ok(rows.iter().map(|r| r.get::<Uuid, _>("id")).collect())
}

async fn replace_cluster_refs(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    rc_id: Uuid,
    cluster_ids: &[Uuid],
) -> DomainResult<()> {
    sqlx::query("DELETE FROM route_config_cluster_refs WHERE route_config_id = $1")
        .bind(rc_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("clear cluster refs: {e}")))?;
    for cluster_id in cluster_ids {
        sqlx::query(
            "INSERT INTO route_config_cluster_refs (route_config_id, cluster_id, team_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(rc_id)
        .bind(cluster_id)
        .bind(team_id.as_uuid())
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("insert cluster ref: {e}")))?;
    }
    Ok(())
}

pub async fn create_route_config(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &RouteConfigSpec,
) -> DomainResult<RouteConfig> {
    let cluster_ids = resolve_cluster_refs(tx, team.id, spec).await?;
    let spec_json = serde_json::to_value(spec)
        .map_err(|e| DomainError::internal(format!("serialize route-config spec: {e}")))?;
    let row = sqlx::query(&format!(
        "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {COLUMNS}"
    ))
    .bind(RouteConfigId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(spec_json)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_unique(e, "route config", name))?;
    let rc = rc_from_row(&row)?;
    replace_cluster_refs(tx, team.id, rc.id.as_uuid(), &cluster_ids).await?;
    Ok(rc)
}

pub async fn get_route_config(
    pool: &PgPool,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<RouteConfig>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM route_configs WHERE team_id = $1 AND name = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get route config: {e}")))?;
    row.as_ref().map(rc_from_row).transpose()
}

pub async fn list_route_configs(
    pool: &PgPool,
    team_id: TeamId,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<RouteConfig>, i64)> {
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM route_configs WHERE team_id = $1 ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list route configs: {e}")))?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM route_configs WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count route configs: {e}")))?;
    rows.iter()
        .map(rc_from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|items| (items, total))
}

pub async fn update_route_config(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &RouteConfigSpec,
    expected_version: i64,
) -> DomainResult<RouteConfig> {
    let cluster_ids = resolve_cluster_refs(tx, team.id, spec).await?;
    let spec_json = serde_json::to_value(spec)
        .map_err(|e| DomainError::internal(format!("serialize route-config spec: {e}")))?;
    let row = sqlx::query(&format!(
        "UPDATE route_configs SET spec = $1, version = version + 1, updated_at = now() \
         WHERE team_id = $2 AND name = $3 AND version = $4 RETURNING {COLUMNS}"
    ))
    .bind(spec_json)
    .bind(team.id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("update route config: {e}")))?;
    match row {
        Some(row) => {
            let rc = rc_from_row(&row)?;
            replace_cluster_refs(tx, team.id, rc.id.as_uuid(), &cluster_ids).await?;
            Ok(rc)
        }
        None => {
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT version FROM route_configs WHERE team_id = $1 AND name = $2",
            )
            .bind(team.id.as_uuid())
            .bind(name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("update route config: recheck: {e}")))?;
            Err(stale_or_missing(
                "route config",
                name,
                current,
                expected_version,
            ))
        }
    }
}

pub async fn delete_route_config(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainResult<RouteConfigId> {
    // Dependents first: listeners bound to this route config block deletion with names.
    let dependents: Vec<String> = sqlx::query_scalar(
        "SELECT l.name FROM listeners l \
         JOIN listener_route_config_refs r ON r.listener_id = l.id \
         JOIN route_configs rc ON rc.id = r.route_config_id \
         WHERE rc.team_id = $1 AND rc.name = $2 ORDER BY l.name LIMIT 10",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete route config: dependents: {e}")))?;
    if !dependents.is_empty() {
        return Err(DomainError::conflict(format!(
            "route config \"{name}\" is referenced by listeners: {}",
            dependents.join(", ")
        ))
        .with_hint("detach or delete those listeners first"));
    }
    let row = sqlx::query(
        "DELETE FROM route_configs WHERE team_id = $1 AND name = $2 AND version = $3 RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete route config: {e}")))?;
    match row {
        Some(row) => Ok(RouteConfigId::from(row.get::<Uuid, _>("id"))),
        None => {
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT version FROM route_configs WHERE team_id = $1 AND name = $2",
            )
            .bind(team_id.as_uuid())
            .bind(name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("delete route config: recheck: {e}")))?;
            Err(stale_or_missing(
                "route config",
                name,
                current,
                expected_version,
            ))
        }
    }
}

/// Route configs whose actions reference the given cluster (cluster-delete guard).
pub async fn route_configs_referencing_cluster(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    cluster_name: &str,
) -> DomainResult<Vec<String>> {
    sqlx::query_scalar(
        "SELECT rc.name FROM route_configs rc \
         JOIN route_config_cluster_refs r ON r.route_config_id = rc.id \
         JOIN clusters c ON c.id = r.cluster_id \
         WHERE c.team_id = $1 AND c.name = $2 ORDER BY rc.name LIMIT 10",
    )
    .bind(team_id.as_uuid())
    .bind(cluster_name)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("cluster dependents: {e}")))
}

// ---------------- listeners ----------------

fn listener_from_row(row: &PgRow) -> DomainResult<Listener> {
    Ok(Listener {
        id: ListenerId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        spec: serde_json::from_value(row.get::<serde_json::Value, _>("spec")).map_err(|e| {
            DomainError::internal(format!("listener spec in DB does not parse: {e}"))
        })?,
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn resolve_listener_rc_ref(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    spec: &ListenerSpec,
) -> DomainResult<Option<Uuid>> {
    let Some(rc_name) = &spec.route_config else {
        return Ok(None);
    };
    let id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM route_configs WHERE team_id = $1 AND name = $2")
            .bind(team_id.as_uuid())
            .bind(rc_name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("resolve route-config ref: {e}")))?;
    id.map(Some).ok_or_else(|| {
        DomainError::validation(format!(
            "listener references route config \"{rc_name}\" which does not exist in this team"
        ))
        .with_hint("create the route config first, then the listener")
    })
}

async fn replace_listener_rc_ref(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    listener_id: Uuid,
    rc_id: Option<Uuid>,
) -> DomainResult<()> {
    sqlx::query("DELETE FROM listener_route_config_refs WHERE listener_id = $1")
        .bind(listener_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("clear listener refs: {e}")))?;
    if let Some(rc_id) = rc_id {
        sqlx::query(
            "INSERT INTO listener_route_config_refs (listener_id, route_config_id, team_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(listener_id)
        .bind(rc_id)
        .bind(team_id.as_uuid())
        .execute(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("insert listener ref: {e}")))?;
    }
    Ok(())
}

pub async fn create_listener(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &ListenerSpec,
) -> DomainResult<Listener> {
    let rc_id = resolve_listener_rc_ref(tx, team.id, spec).await?;
    let spec_json = serde_json::to_value(spec)
        .map_err(|e| DomainError::internal(format!("serialize listener spec: {e}")))?;
    let row = sqlx::query(&format!(
        "INSERT INTO listeners (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {COLUMNS}"
    ))
    .bind(ListenerId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(spec_json)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_unique(e, "listener", name))?;
    let listener = listener_from_row(&row)?;
    replace_listener_rc_ref(tx, team.id, listener.id.as_uuid(), rc_id).await?;
    Ok(listener)
}

pub async fn get_listener(
    pool: &PgPool,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<Listener>> {
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM listeners WHERE team_id = $1 AND name = $2"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get listener: {e}")))?;
    row.as_ref().map(listener_from_row).transpose()
}

pub async fn list_listeners(
    pool: &PgPool,
    team_id: TeamId,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<Listener>, i64)> {
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM listeners WHERE team_id = $1 ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list listeners: {e}")))?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM listeners WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count listeners: {e}")))?;
    rows.iter()
        .map(listener_from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|i| (i, total))
}

pub async fn update_listener(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    name: &str,
    spec: &ListenerSpec,
    expected_version: i64,
) -> DomainResult<Listener> {
    let rc_id = resolve_listener_rc_ref(tx, team.id, spec).await?;
    let spec_json = serde_json::to_value(spec)
        .map_err(|e| DomainError::internal(format!("serialize listener spec: {e}")))?;
    let row = sqlx::query(&format!(
        "UPDATE listeners SET spec = $1, version = version + 1, updated_at = now() \
         WHERE team_id = $2 AND name = $3 AND version = $4 RETURNING {COLUMNS}"
    ))
    .bind(spec_json)
    .bind(team.id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| map_unique(e, "listener", name))?;
    match row {
        Some(row) => {
            let listener = listener_from_row(&row)?;
            replace_listener_rc_ref(tx, team.id, listener.id.as_uuid(), rc_id).await?;
            Ok(listener)
        }
        None => {
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT version FROM listeners WHERE team_id = $1 AND name = $2",
            )
            .bind(team.id.as_uuid())
            .bind(name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("update listener: recheck: {e}")))?;
            Err(stale_or_missing(
                "listener",
                name,
                current,
                expected_version,
            ))
        }
    }
}

pub async fn delete_listener(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainResult<ListenerId> {
    let row = sqlx::query(
        "DELETE FROM listeners WHERE team_id = $1 AND name = $2 AND version = $3 RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete listener: {e}")))?;
    match row {
        Some(row) => Ok(ListenerId::from(row.get::<Uuid, _>("id"))),
        None => {
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT version FROM listeners WHERE team_id = $1 AND name = $2",
            )
            .bind(team_id.as_uuid())
            .bind(name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("delete listener: recheck: {e}")))?;
            Err(stale_or_missing(
                "listener",
                name,
                current,
                expected_version,
            ))
        }
    }
}

pub async fn count_route_configs(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM route_configs WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count route configs: {e}")))
}

pub async fn count_listeners(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM listeners WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count listeners: {e}")))
}
