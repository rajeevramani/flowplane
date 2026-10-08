//! Team-scoped shortcut association and transactional dependency readers.
use fp_domain::authz::{Resource, TeamRef};
use fp_domain::gateway::exposure::Exposure;
use fp_domain::{
    ClusterId, DomainError, DomainResult, ExposureId, ListenerId, OrgId, RouteConfigId, TeamId,
};
use sqlx::{postgres::PgRow, Postgres, Row, Transaction};
use uuid::Uuid;

fn from_row(row: &PgRow) -> Exposure {
    Exposure {
        id: ExposureId::from(row.get::<Uuid, _>("id")),
        team_id: TeamId::from(row.get::<Uuid, _>("team_id")),
        org_id: OrgId::from(row.get::<Uuid, _>("org_id")),
        name: row.get("name"),
        cluster_id: ClusterId::from(row.get::<Uuid, _>("cluster_id")),
        route_config_id: RouteConfigId::from(row.get::<Uuid, _>("route_config_id")),
        listener_id: ListenerId::from(row.get::<Uuid, _>("listener_id")),
        virtual_host: row.get("virtual_host"),
        route_name: row.get("route_name"),
        cleanup_listener: row.get("cleanup_listener"),
        cleanup_route_config: row.get("cleanup_route_config"),
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

/// Only shortcut callers use this namespace; ordinary creates retain their existing quota race.
pub async fn lock_team(tx: &mut Transaction<'_, Postgres>, team_id: TeamId) -> DomainResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("exposure:{}", team_id))
        .execute(&mut **tx)
        .await
        .map_err(|e| db_error("exposure team lock", e))?;
    Ok(())
}

pub async fn get_for_update(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    name: &str,
) -> DomainResult<Option<Exposure>> {
    let row = sqlx::query("SELECT * FROM exposures WHERE team_id=$1 AND name=$2 FOR UPDATE")
        .bind(team_id.as_uuid())
        .bind(name)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| db_error("read exposure", e))?;
    Ok(row.as_ref().map(from_row))
}

pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    value: &Exposure,
) -> DomainResult<Exposure> {
    if value.team_id != team.id || value.org_id != team.org_id {
        return Err(DomainError::validation(
            "exposure scope does not match its team",
        ));
    }
    let row=sqlx::query("INSERT INTO exposures (id,team_id,org_id,name,cluster_id,route_config_id,listener_id,virtual_host,route_name,cleanup_listener,cleanup_route_config) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING *")
        .bind(value.id.as_uuid()).bind(team.id.as_uuid()).bind(team.org_id.as_uuid()).bind(&value.name)
        .bind(value.cluster_id.as_uuid()).bind(value.route_config_id.as_uuid()).bind(value.listener_id.as_uuid())
        .bind(&value.virtual_host).bind(&value.route_name).bind(value.cleanup_listener).bind(value.cleanup_route_config)
        .fetch_one(&mut **tx).await.map_err(|e| match &e {
            sqlx::Error::Database(db) if db.code().as_deref()==Some("23505") =>
                DomainError::conflict(format!("exposure \"{}\" already owns that name, cluster or route",value.name)),
            _=>db_error("create exposure",e),
        })?;
    Ok(from_row(&row))
}

pub async fn delete(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    exposure: &Exposure,
) -> DomainResult<()> {
    let result = sqlx::query("DELETE FROM exposures WHERE team_id=$1 AND id=$2 AND version=$3")
        .bind(team_id.as_uuid())
        .bind(exposure.id.as_uuid())
        .bind(exposure.version)
        .execute(&mut **tx)
        .await
        .map_err(|e| db_error("delete exposure", e))?;
    if result.rows_affected() != 1 {
        return Err(DomainError::conflict(
            "exposure identity changed; inspect and retry",
        ));
    }
    Ok(())
}

/// Exact named exposure guards precede ordinary dependency hints and the DELETE itself.
pub async fn guard_resource_delete(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    resource: Resource,
    name: &str,
) -> DomainResult<()> {
    let (table, column) = match resource {
        Resource::Clusters => ("clusters", "cluster_id"),
        Resource::RouteConfigs => ("route_configs", "route_config_id"),
        Resource::Listeners => ("listeners", "listener_id"),
        _ => {
            return Err(DomainError::internal(
                "unsupported exposure dependency resource",
            ))
        }
    };
    let names:Vec<String>=sqlx::query_scalar(&format!("SELECT e.name FROM exposures e JOIN {table} r ON r.id=e.{column} AND r.team_id=e.team_id WHERE e.team_id=$1 AND r.name=$2 ORDER BY e.name"))
        .bind(team_id.as_uuid()).bind(name).fetch_all(&mut **tx).await.map_err(|e| db_error("exposure dependents",e))?;
    if !names.is_empty() {
        return Err(DomainError::conflict(format!(
            "{name} is referenced by exposures: {}",
            names.join(", ")
        ))
        .with_hint(format!("use unexpose <name> for: {}", names.join(", "))));
    }
    Ok(())
}

/// Preserve unrelated DB errors; only the named new exposure constraints become 409.
pub fn delete_error(context: &str, error: sqlx::Error) -> DomainError {
    if let sqlx::Error::Database(db) = &error {
        if db.code().as_deref() == Some("23503")
            && matches!(
                db.constraint(),
                Some(
                    "exposures_cluster_fk" | "exposures_route_config_fk" | "exposures_listener_fk"
                )
            )
        {
            return DomainError::conflict("resource acquired an exposure reference concurrently")
                .with_hint(
                    "inspect current resources and use unexpose <name>; retry after rereading",
                );
        }
    }
    db_error(context, error)
}

/// Shortcut-only translation after reusable helpers preserved SQLSTATE identity.
/// Ordinary callers retain their historical Internal result and redacted REST envelope.
pub fn shortcut_error(error: DomainError) -> DomainError {
    let state = error
        .details
        .as_ref()
        .and_then(|details| details.get(crate::TRANSACTION_SQLSTATE_DETAIL))
        .and_then(serde_json::Value::as_str);
    if error.code == fp_domain::ErrorCode::Internal && matches!(state, Some("40P01" | "40001")) {
        return DomainError::conflict("gateway transaction conflicted with a concurrent mutation")
            .with_hint(
                "inspect current resources and retry explicitly; no automatic replay occurred",
            );
    }
    error
}

pub fn db_error(context: &str, error: sqlx::Error) -> DomainError {
    if let sqlx::Error::Database(db) = &error {
        if matches!(db.code().as_deref(), Some("40P01" | "40001")) {
            return DomainError::conflict(
                "gateway transaction conflicted with a concurrent mutation",
            )
            .with_hint(
                "inspect current resources and retry explicitly; no automatic replay occurred",
            );
        }
    }
    DomainError::internal(format!("{context}: {error}"))
}

pub async fn count_resources(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    resource: Resource,
) -> DomainResult<i64> {
    // Cluster quotas historically count ALL owners, unlike listener/config quotas.
    if resource == Resource::Clusters {
        return super::clusters::count_for_team_in_tx(tx, team_id).await;
    }
    let table = match resource {
        Resource::Clusters => "clusters",
        Resource::RouteConfigs => "route_configs",
        Resource::Listeners => "listeners",
        _ => return Err(DomainError::internal("unsupported shortcut quota resource")),
    };
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {table} WHERE team_id=$1 AND owner_kind='user'"
    ))
    .bind(team_id.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| db_error("shortcut quota count", e))
}

pub async fn listener_ports(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
) -> DomainResult<Vec<i32>> {
    // Match current user-listener allocation visibility; unique index remains authoritative for all owners.
    sqlx::query_scalar(
        "SELECT (spec->>'port')::int FROM listeners WHERE team_id=$1 AND owner_kind='user'",
    )
    .bind(team_id.as_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| db_error("shortcut listener ports", e))
}

/// Includes normalized references and authored route/filter specs, all scoped to this team.
pub async fn surviving_cluster_dependents(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    cluster: &str,
) -> DomainResult<Vec<String>> {
    let mut result: Vec<String> =
        super::gateway::route_configs_referencing_cluster(tx, team_id, cluster)
            .await?
            .into_iter()
            .map(|name| format!("route-config/{name}"))
            .collect();
    let specs: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT name,spec FROM route_configs WHERE team_id=$1")
            .bind(team_id.as_uuid())
            .fetch_all(&mut **tx)
            .await
            .map_err(|e| db_error("route cluster inventory", e))?;
    for (name, value) in specs {
        let spec: fp_domain::gateway::route_config::RouteConfigSpec = serde_json::from_value(value)
            .map_err(|e| DomainError::internal(format!("route spec inventory: {e}")))?;
        if spec.referenced_clusters().contains(cluster) {
            result.push(format!("route-config/{name}"));
        }
    }
    let specs: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT name,spec FROM listeners WHERE team_id=$1")
            .bind(team_id.as_uuid())
            .fetch_all(&mut **tx)
            .await
            .map_err(|e| db_error("listener cluster inventory", e))?;
    for (name, value) in specs {
        let spec: fp_domain::gateway::listener::ListenerSpec = serde_json::from_value(value)
            .map_err(|e| DomainError::internal(format!("listener spec inventory: {e}")))?;
        if spec.http_filters.iter().any(|entry| {
            use fp_domain::gateway::filters::{HttpFilterSpec, JwksSource};
            // Exhaustive: adding a filter kind must reconsider cluster-reference inventory.
            match &entry.filter {
                HttpFilterSpec::ExtAuthz(c) => c.cluster == cluster,
                HttpFilterSpec::JwtAuth(c) => c.providers.values().any(|provider| {
                    matches!(&provider.jwks, JwksSource::Remote { cluster: referenced, .. } if referenced == cluster)
                }),
                HttpFilterSpec::GlobalRateLimit(c) => c.service_cluster == cluster,
                HttpFilterSpec::Cors(_)
                | HttpFilterSpec::LocalRateLimit(_)
                | HttpFilterSpec::HeaderMutation(_)
                | HttpFilterSpec::HealthCheck(_)
                | HttpFilterSpec::Compressor(_)
                | HttpFilterSpec::Rbac(_) => false,
            }
        }) {
            result.push(format!("listener/{name}"));
        }
    }
    let specs: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT name,spec FROM clusters WHERE team_id=$1")
            .bind(team_id.as_uuid())
            .fetch_all(&mut **tx)
            .await
            .map_err(|e| db_error("aggregate cluster inventory", e))?;
    for (name, value) in specs {
        let spec: fp_domain::gateway::cluster::ClusterSpec = serde_json::from_value(value)
            .map_err(|e| DomainError::internal(format!("aggregate spec inventory: {e}")))?;
        if spec
            .aggregate_clusters
            .iter()
            .any(|referenced| referenced == cluster)
        {
            result.push(format!("cluster/{name}"));
        }
    }
    result.sort();
    result.dedup();
    Ok(result)
}

pub async fn route_dependents(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    config: RouteConfigId,
    vhost: &str,
    route: Option<&str>,
) -> DomainResult<Vec<String>> {
    sqlx::query_scalar("SELECT label FROM (
        SELECT 'api-binding/'||name AS label FROM api_route_bindings WHERE team_id=$1 AND route_config_id=$2 AND (virtual_host IS NULL OR virtual_host=$3) AND ($4::text IS NULL OR route IS NULL OR route=$4)
        UNION ALL SELECT 'active-capture/'||name FROM capture_sessions WHERE team_id=$1 AND route_config_id=$2 AND status='capturing' AND (virtual_host IS NULL OR virtual_host=$3) AND ($4::text IS NULL OR route IS NULL OR route=$4)
    ) d ORDER BY label")
        .bind(team_id.as_uuid()).bind(config.as_uuid()).bind(vhost).bind(route)
        .fetch_all(&mut **tx).await.map_err(|e| db_error("protected route dependents",e))
}

pub async fn scaffold_dependents(
    tx: &mut Transaction<'_, Postgres>,
    team_id: TeamId,
    exposure: &Exposure,
) -> DomainResult<Vec<String>> {
    sqlx::query_scalar("SELECT label FROM (
        SELECT 'listener/'||l.name AS label FROM listeners l JOIN listener_route_config_refs r ON r.listener_id=l.id AND r.team_id=l.team_id WHERE l.team_id=$1 AND r.route_config_id=$2 AND l.id<>$3
        UNION ALL SELECT 'exposure/'||name FROM exposures WHERE team_id=$1 AND id<>$4 AND (route_config_id=$2 OR listener_id=$3)
        UNION ALL SELECT 'api-binding/'||name FROM api_route_bindings WHERE team_id=$1 AND (route_config_id=$2 OR listener_id=$3)
        UNION ALL SELECT 'capture-history/'||name FROM capture_sessions WHERE team_id=$1 AND (route_config_id=$2 OR listener_id=$3)
        UNION ALL SELECT 'ai-budget/'||name FROM ai_budgets WHERE team_id=$1 AND route_config_id=$2
    ) d ORDER BY label")
        .bind(team_id.as_uuid()).bind(exposure.route_config_id.as_uuid()).bind(exposure.listener_id.as_uuid()).bind(exposure.id.as_uuid())
        .fetch_all(&mut **tx).await.map_err(|e| db_error("scaffold dependents",e))
}
