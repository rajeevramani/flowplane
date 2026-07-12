//! Cluster repository: every query carries the team predicate in SQL (spec/10 §4).
//! Updates and deletes require the expected revision — optimistic concurrency on every
//! mutable resource (spec/10 §3.4.4), the fix for v1's lost-update class.

use crate::scope::TeamScope;
use fp_domain::gateway::cluster::{Cluster, ClusterSpec};
use fp_domain::{ClusterId, DomainError, DomainResult, ErrorCode, TeamId};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

fn from_row(row: &PgRow) -> DomainResult<Cluster> {
    let spec: serde_json::Value = row.get("spec");
    Ok(Cluster {
        id: ClusterId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        name: row.get("name"),
        spec: serde_json::from_value::<ClusterSpec>(spec).map_err(|e| {
            DomainError::internal(format!("cluster spec in DB does not parse: {e}"))
        })?,
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

const COLUMNS: &str = "id, team_id, name, spec, version, created_at, updated_at";

/// Insert. The team's org is taken from the TeamRef the caller resolved (the composite FK
/// would reject a mismatch anyway).
pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    team: fp_domain::authz::TeamRef,
    name: &str,
    spec: &ClusterSpec,
) -> DomainResult<Cluster> {
    create_with_owner(tx, team, name, spec, "user", None).await
}

pub async fn create_discovery_owned(
    tx: &mut Transaction<'_, Postgres>,
    team: fp_domain::authz::TeamRef,
    owner_id: Uuid,
    name: &str,
    spec: &ClusterSpec,
) -> DomainResult<Cluster> {
    create_with_owner(tx, team, name, spec, "discovery", Some(owner_id)).await
}

pub async fn create_ai_owned(
    tx: &mut Transaction<'_, Postgres>,
    team: fp_domain::authz::TeamRef,
    owner_id: Uuid,
    name: &str,
    spec: &ClusterSpec,
) -> DomainResult<Cluster> {
    create_with_owner(tx, team, name, spec, "ai", Some(owner_id)).await
}

async fn create_with_owner(
    tx: &mut Transaction<'_, Postgres>,
    team: fp_domain::authz::TeamRef,
    name: &str,
    spec: &ClusterSpec,
    owner_kind: &str,
    owner_id: Option<Uuid>,
) -> DomainResult<Cluster> {
    let spec_json = serde_json::to_value(spec)
        .map_err(|e| DomainError::internal(format!("serialize cluster spec: {e}")))?;
    let row = sqlx::query(&format!(
        "INSERT INTO clusters (id, team_id, org_id, name, spec, owner_kind, owner_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {COLUMNS}"
    ))
    .bind(ClusterId::generate().as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(spec_json)
    .bind(owner_kind)
    .bind(owner_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DomainError::conflict(format!("cluster \"{name}\" already exists in this team"))
                .with_hint("choose a different name or update the existing cluster")
        }
        _ => DomainError::internal(format!("create cluster: {e}")),
    })?;
    from_row(&row)
}

pub async fn get(pool: &PgPool, scope: TeamScope, name: &str) -> DomainResult<Option<Cluster>> {
    let Some(team_id) = scope.team_id() else {
        return Err(DomainError::internal(
            "platform-admin cluster reads are not a supported path (tenant resource)",
        ));
    };
    let row = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM clusters WHERE team_id = $1 AND name = $2 AND owner_kind = 'user'"
    ))
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| DomainError::internal(format!("get cluster: {e}")))?;
    row.as_ref().map(from_row).transpose()
}

pub async fn list(
    pool: &PgPool,
    scope: TeamScope,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<Cluster>, i64)> {
    let Some(team_id) = scope.team_id() else {
        return Err(DomainError::internal(
            "platform-admin cluster reads are not a supported path (tenant resource)",
        ));
    };
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM clusters WHERE team_id = $1 AND owner_kind = 'user' ORDER BY name LIMIT $2 OFFSET $3"
    ))
    .bind(team_id.as_uuid())
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| DomainError::internal(format!("list clusters: {e}")))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM clusters WHERE team_id = $1 AND owner_kind = 'user'",
    )
    .bind(team_id.as_uuid())
    .fetch_one(pool)
    .await
    .map_err(|e| DomainError::internal(format!("count clusters: {e}")))?;
    rows.iter()
        .map(from_row)
        .collect::<DomainResult<Vec<_>>>()
        .map(|items| (items, total))
}

/// Update with optimistic concurrency: succeeds only when the stored version matches.
pub async fn update(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    spec: &ClusterSpec,
    expected_version: i64,
) -> DomainResult<Cluster> {
    let spec_json = serde_json::to_value(spec)
        .map_err(|e| DomainError::internal(format!("serialize cluster spec: {e}")))?;
    let row = sqlx::query(&format!(
        "UPDATE clusters SET spec = $1, version = version + 1, updated_at = now() \
         WHERE team_id = $2 AND name = $3 AND version = $4 AND owner_kind = 'user' RETURNING {COLUMNS}"
    ))
    .bind(spec_json)
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("update cluster: {e}")))?;

    match row {
        Some(row) => from_row(&row),
        None => {
            // Disambiguate: gone vs revision raced.
            let current: Option<i64> =
                sqlx::query_scalar("SELECT version FROM clusters WHERE team_id = $1 AND name = $2 AND owner_kind = 'user'")
                    .bind(team_id.as_uuid())
                    .bind(name)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| DomainError::internal(format!("update cluster: recheck: {e}")))?;
            Err(match current {
                Some(version) => DomainError::new(
                    ErrorCode::RevisionMismatch,
                    format!(
                        "cluster \"{name}\" is at revision {version}, you supplied {expected_version}"
                    ),
                )
                .with_hint("re-read the resource and retry with the current revision"),
                None => DomainError::not_found("cluster", name),
            })
        }
    }
}

/// Rewrite an AI-owned materialized cluster's spec (provider re-materialization). Gated on
/// `owner_kind = 'ai'` so it can never touch user or discovery clusters; the caller supplies
/// names only from `ai_routes.cluster_names` (system-written at materialization). No caller
/// revision check — this is internal projection maintenance, serialized by the per-team AI
/// materialization advisory lock. NotFound means materialized state is corrupt and the whole
/// enclosing transaction must roll back.
pub async fn update_ai_owned_spec(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    spec: &ClusterSpec,
) -> DomainResult<Cluster> {
    let spec_json = serde_json::to_value(spec)
        .map_err(|e| DomainError::internal(format!("serialize cluster spec: {e}")))?;
    let row = sqlx::query(&format!(
        "UPDATE clusters SET spec = $1, version = version + 1, updated_at = now() \
         WHERE team_id = $2 AND name = $3 AND owner_kind = 'ai' RETURNING {COLUMNS}"
    ))
    .bind(spec_json)
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("update AI-owned cluster: {e}")))?;
    row.as_ref()
        .map(from_row)
        .transpose()?
        .ok_or_else(|| DomainError::not_found("AI-owned cluster", name))
}

/// Delete with the same revision contract. Returns the deleted cluster's id.
pub async fn delete(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
    expected_version: i64,
) -> DomainResult<ClusterId> {
    if is_discovery_owned(tx, team_id, name).await? {
        return Err(DomainError::conflict(format!(
            "cluster \"{name}\" is owned by a discovery session"
        ))
        .with_hint("stop the discovery session to remove it"));
    }
    let row = sqlx::query(
        "DELETE FROM clusters WHERE team_id = $1 AND name = $2 AND version = $3 AND owner_kind = 'user' RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .bind(expected_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete cluster: {e}")))?;
    match row {
        Some(row) => Ok(ClusterId::from(row.get::<Uuid, _>("id"))),
        None => {
            let current: Option<i64> =
                sqlx::query_scalar("SELECT version FROM clusters WHERE team_id = $1 AND name = $2 AND owner_kind = 'user'")
                    .bind(team_id.as_uuid())
                    .bind(name)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| DomainError::internal(format!("delete cluster: recheck: {e}")))?;
            Err(match current {
                Some(version) => DomainError::new(
                    ErrorCode::RevisionMismatch,
                    format!("cluster \"{name}\" is at revision {version}, you supplied {expected_version}"),
                )
                .with_hint("re-read the resource and retry with the current revision"),
                None => DomainError::not_found("cluster", name),
            })
        }
    }
}

async fn is_discovery_owned(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
) -> DomainResult<bool> {
    let owner: Option<String> =
        sqlx::query_scalar("SELECT owner_kind FROM clusters WHERE team_id = $1 AND name = $2")
            .bind(team_id.as_uuid())
            .bind(name)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| DomainError::internal(format!("read cluster owner: {e}")))?;
    Ok(owner.as_deref() == Some("discovery"))
}

pub async fn delete_discovery_owned(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    owner_id: Uuid,
    name: &str,
) -> DomainResult<Option<ClusterId>> {
    let row = sqlx::query(
        "DELETE FROM clusters \
         WHERE team_id = $1 AND owner_kind = 'discovery' AND owner_id = $2 AND name = $3 \
         RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(owner_id)
    .bind(name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete discovery cluster: {e}")))?;
    Ok(row.map(|row| ClusterId::from(row.get::<Uuid, _>("id"))))
}

pub async fn delete_ai_owned(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<ClusterId>> {
    let row = sqlx::query(
        "DELETE FROM clusters \
         WHERE team_id = $1 AND owner_kind = 'ai' AND name = $2 \
         RETURNING id",
    )
    .bind(team_id.as_uuid())
    .bind(name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| DomainError::internal(format!("delete AI cluster: {e}")))?;
    Ok(row.map(|row| ClusterId::from(row.get::<Uuid, _>("id"))))
}

/// Per-team resource count for quota enforcement (S3.4).
pub async fn count_for_team(pool: &PgPool, team_id: TeamId) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM clusters WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal(format!("count clusters: {e}")))
}

/// Transaction-executor variant: quota count inside an AI-materialization mutation tx,
/// after the per-team advisory lock (fpv2-8am).
pub async fn count_for_team_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
) -> DomainResult<i64> {
    sqlx::query_scalar("SELECT count(*) FROM clusters WHERE team_id = $1")
        .bind(team_id.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| DomainError::internal(format!("count clusters: {e}")))
}
