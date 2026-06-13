//! Route-config and listener services (the clusters pattern: authz → validate → quota →
//! one transaction of row + event + audit).

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::gateway::listener::{Listener, ListenerSpec};
use fp_domain::gateway::route_config::{RouteConfig, RouteConfigSpec};
use fp_domain::{validate_name, DomainResult, RequestId};
use fp_storage::repos::{audit, gateway};
use sqlx::PgPool;

async fn authorize(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    resource: Resource,
    action: Action,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, resource, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(pool, ctx, request_id, resource, action, Some(team), reason).await;
            Err(deny_to_error(resource, action, reason))
        }
    }
}

fn mutation_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    team: TeamRef,
    action: &str,
    resource: String,
) -> audit::AuditEntry {
    let (actor_type, actor_id) = actor_of(ctx);
    audit::AuditEntry {
        request_id: Some(request_id),
        actor_type,
        actor_id,
        actor_label: String::new(),
        surface: audit::Surface::Rest,
        action: action.into(),
        resource,
        org_id: Some(team.org_id),
        team_id: Some(team.id),
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}

// ---------------- route configs ----------------

pub async fn create_route_config(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: RouteConfigSpec,
    request_id: RequestId,
) -> DomainResult<RouteConfig> {
    authorize(
        pool,
        ctx,
        Resource::RouteConfigs,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    validate_name(name)?;
    spec.validate()?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::RouteConfigs)
        .await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create rc: begin"))?;
    let rc = gateway::create_route_config(&mut tx, team, name, &spec).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::RouteConfigUpserted {
            route_config_id: rc.id.as_uuid(),
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
        &mutation_audit(
            ctx,
            request_id,
            team,
            "route_config.create",
            format!("route-configs/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create rc: commit"))?;
    Ok(rc)
}

pub async fn get_route_config(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<RouteConfig> {
    authorize(
        pool,
        ctx,
        Resource::RouteConfigs,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    gateway::get_route_config(pool, team.id, name)
        .await?
        .ok_or_else(|| fp_domain::DomainError::not_found("route config", name))
}

pub async fn list_route_configs(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<RouteConfig>, i64)> {
    authorize(
        pool,
        ctx,
        Resource::RouteConfigs,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    gateway::list_route_configs(pool, team.id, limit, offset).await
}

pub async fn update_route_config(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: RouteConfigSpec,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<RouteConfig> {
    authorize(
        pool,
        ctx,
        Resource::RouteConfigs,
        Action::Update,
        team,
        request_id,
    )
    .await?;
    spec.validate()?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update rc: begin"))?;
    let rc = gateway::update_route_config(&mut tx, team, name, &spec, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::RouteConfigUpserted {
            route_config_id: rc.id.as_uuid(),
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
        &mutation_audit(
            ctx,
            request_id,
            team,
            "route_config.update",
            format!("route-configs/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update rc: commit"))?;
    Ok(rc)
}

pub async fn delete_route_config(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize(
        pool,
        ctx,
        Resource::RouteConfigs,
        Action::Delete,
        team,
        request_id,
    )
    .await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete rc: begin"))?;
    let rc_id = gateway::delete_route_config(&mut tx, team.id, name, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::RouteConfigDeleted {
            route_config_id: rc_id.as_uuid(),
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
        &mutation_audit(
            ctx,
            request_id,
            team,
            "route_config.delete",
            format!("route-configs/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete rc: commit"))?;
    Ok(())
}

// ---------------- listeners ----------------

pub async fn create_listener(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ListenerSpec,
    request_id: RequestId,
) -> DomainResult<Listener> {
    authorize(
        pool,
        ctx,
        Resource::Listeners,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    validate_name(name)?;
    spec.validate()?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::Listeners).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create listener: begin"))?;
    let listener = gateway::create_listener(&mut tx, team, name, &spec).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ListenerUpserted {
            listener_id: listener.id.as_uuid(),
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
        &mutation_audit(
            ctx,
            request_id,
            team,
            "listener.create",
            format!("listeners/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create listener: commit"))?;
    Ok(listener)
}

pub async fn get_listener(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<Listener> {
    authorize(
        pool,
        ctx,
        Resource::Listeners,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    gateway::get_listener(pool, team.id, name)
        .await?
        .ok_or_else(|| fp_domain::DomainError::not_found("listener", name))
}

pub async fn list_listeners(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<Listener>, i64)> {
    authorize(
        pool,
        ctx,
        Resource::Listeners,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    gateway::list_listeners(pool, team.id, limit, offset).await
}

pub async fn update_listener(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: ListenerSpec,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<Listener> {
    authorize(
        pool,
        ctx,
        Resource::Listeners,
        Action::Update,
        team,
        request_id,
    )
    .await?;
    spec.validate()?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update listener: begin"))?;
    let listener = gateway::update_listener(&mut tx, team, name, &spec, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ListenerUpserted {
            listener_id: listener.id.as_uuid(),
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
        &mutation_audit(
            ctx,
            request_id,
            team,
            "listener.update",
            format!("listeners/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update listener: commit"))?;
    Ok(listener)
}

pub async fn delete_listener(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize(
        pool,
        ctx,
        Resource::Listeners,
        Action::Delete,
        team,
        request_id,
    )
    .await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete listener: begin"))?;
    let listener_id = gateway::delete_listener(&mut tx, team.id, name, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::ListenerDeleted {
            listener_id: listener_id.as_uuid(),
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
        &mutation_audit(
            ctx,
            request_id,
            team,
            "listener.delete",
            format!("listeners/{name}"),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete listener: commit"))?;
    Ok(())
}
