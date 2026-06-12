//! Cluster service: the ONLY mutation path (spec/10 §2). Every mutation is one
//! transaction containing the row change, its domain event (outbox), and its audit entry —
//! they commit together or not at all.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::gateway::cluster::{validate_cluster_name, Cluster, ClusterSpec};
use fp_domain::{DomainResult, RequestId};
use fp_storage::repos::{audit, clusters};
use fp_storage::scope::TeamScope;
use sqlx::PgPool;

fn authorize(ctx: &PrincipalCtx, action: Action, team: TeamRef) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::Clusters, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => Err(deny_to_error(Resource::Clusters, action, reason)),
    }
}

pub async fn create_cluster(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ClusterSpec,
    request_id: RequestId,
) -> DomainResult<Cluster> {
    authorize(ctx, Action::Create, team)?;
    validate_cluster_name(name)?;
    spec.validate()?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::Clusters).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create cluster: begin"))?;
    let cluster = clusters::create(&mut tx, team, name, &spec).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ClusterUpserted {
            cluster_id: cluster.id.as_uuid(),
            name: name.into(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "cluster.create", name),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create cluster: commit"))?;
    Ok(cluster)
}

pub async fn get_cluster(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
) -> DomainResult<Cluster> {
    authorize(ctx, Action::Read, team)?;
    clusters::get(pool, TeamScope::Team(team.id), name)
        .await?
        .ok_or_else(|| fp_domain::DomainError::not_found("cluster", name))
}

pub async fn list_clusters(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
) -> DomainResult<(Vec<Cluster>, i64)> {
    authorize(ctx, Action::Read, team)?;
    clusters::list(pool, TeamScope::Team(team.id), limit, offset).await
}

pub async fn update_cluster(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ClusterSpec,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<Cluster> {
    authorize(ctx, Action::Update, team)?;
    spec.validate()?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update cluster: begin"))?;
    let cluster = clusters::update(&mut tx, team.id, name, &spec, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ClusterUpserted {
            cluster_id: cluster.id.as_uuid(),
            name: name.into(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "cluster.update", name),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update cluster: commit"))?;
    Ok(cluster)
}

pub async fn delete_cluster(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize(ctx, Action::Delete, team)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete cluster: begin"))?;
    // Referenced clusters cannot be deleted (no silent cascade — spec/10 §3.4.1); the
    // error lists dependents so the operator knows exactly what to unwind.
    let dependents =
        fp_storage::repos::gateway::route_configs_referencing_cluster(&mut tx, team.id, name)
            .await?;
    if !dependents.is_empty() {
        return Err(fp_domain::DomainError::conflict(format!(
            "cluster \"{name}\" is referenced by route configs: {}",
            dependents.join(", ")
        ))
        .with_hint("update or delete those route configs first"));
    }
    let cluster_id = clusters::delete(&mut tx, team.id, name, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ClusterDeleted {
            cluster_id: cluster_id.as_uuid(),
            name: name.into(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "cluster.delete", name),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete cluster: commit"))?;
    Ok(())
}

fn mutation_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    team: TeamRef,
    action: &str,
    name: &str,
) -> audit::AuditEntry {
    let (actor_type, actor_id) = actor_of(ctx);
    audit::AuditEntry {
        request_id: Some(request_id),
        actor_type,
        actor_id,
        actor_label: String::new(),
        surface: audit::Surface::Rest,
        action: action.into(),
        resource: format!("clusters/{name}"),
        org_id: Some(team.org_id),
        team_id: Some(team.id),
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}
