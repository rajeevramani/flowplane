//! Route-config and listener services (the clusters pattern: authz → validate → quota →
//! one transaction of row + event + audit).

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::rls_sync::{compose_domain, namespace_uuid};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::gateway::cluster::RESERVED_RATE_LIMIT_CLUSTER;
use fp_domain::gateway::filters::HttpFilterSpec;
use fp_domain::gateway::listener::{Listener, ListenerSpec};
use fp_domain::gateway::route_config::{RouteConfig, RouteConfigSpec};
use fp_domain::{validate_name, DomainError, DomainResult, RequestId};
use fp_storage::repos::{audit, clusters, gateway};
use fp_storage::scope::TeamScope;
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

/// Fail-closed config-time handling of any `global_rate_limit` filter the listener carries
/// (design Security L242 "fail closed at config time" + L223 "the domain value the filter is
/// configured with"). Runs *after* `spec.validate()` and *before* the write tx; mutates `spec`
/// in place so the persisted row is the source of truth (xDS `translate.rs` then copies it
/// verbatim — composition does not live there).
///
/// Two filter shapes:
/// - **Built-in path** (`service_cluster == rate_limit_cluster`): valid only when the CP injects
///   the cluster (`FLOWPLANE_RLS_GRPC_URL` set) — else reject 400, since translation would emit a
///   filter pointing at a non-existent cluster. The CP then *owns* the Envoy filter `domain`,
///   composing it to the `{org}|{team}|{domain}` namespace via the S5 helper. This is the SAME
///   byte string S5 pushes and S4 keys counters by — a divergence silently disables enforcement.
/// - **User-supplied cluster**: must resolve to an existing same-team user cluster (read-only
///   lookup) — else 404, matching the cross-tenant disclosure rule. The `domain` is left as the
///   user value (they own the external-limiter semantics).
async fn resolve_global_rate_limit_filters(
    pool: &PgPool,
    team: TeamRef,
    spec: &mut ListenerSpec,
    rls_grpc_configured: bool,
) -> DomainResult<()> {
    for entry in spec.http_filters.iter_mut() {
        let HttpFilterSpec::GlobalRateLimit(cfg) = &mut entry.filter else {
            continue;
        };
        if cfg.service_cluster == RESERVED_RATE_LIMIT_CLUSTER {
            if !rls_grpc_configured {
                return Err(DomainError::validation(format!(
                    "global_rate_limit: service_cluster '{RESERVED_RATE_LIMIT_CLUSTER}' requires \
                     FLOWPLANE_RLS_GRPC_URL to be set (the built-in rate-limit cluster is only \
                     injected then); set it, or reference an existing same-team cluster",
                )));
            }
            // CP-composed Envoy domain: namespaces the filter so the RLS resolves (org, team)
            // and counters bind. Idempotent: a GET returns the composed value, so a GET→PATCH
            // round-trip resubmits it — strip this team's own prefix first so we re-compose the
            // base domain instead of double-namespacing (which would silently break enforcement).
            let prefix = format!(
                "{}|{}|",
                namespace_uuid(team.org_id.as_uuid()),
                namespace_uuid(team.id.as_uuid()),
            );
            let base = cfg.domain.strip_prefix(&prefix).unwrap_or(&cfg.domain);
            cfg.domain = compose_domain(team.org_id, team.id, base);
            // Re-validate — the composed value must still fit the (S6-raised) domain ceiling even
            // if the user supplied an over-long base domain.
            cfg.validate()?;
        } else if clusters::get(pool, TeamScope::Team(team.id), &cfg.service_cluster)
            .await?
            .is_none()
        {
            return Err(DomainError::not_found("cluster", &cfg.service_cluster));
        }
    }
    Ok(())
}

pub async fn create_listener(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    mut spec: ListenerSpec,
    request_id: RequestId,
    rls_grpc_configured: bool,
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
    validate_user_listener_name(name)?;
    spec.validate()?;
    resolve_global_rate_limit_filters(pool, team, &mut spec, rls_grpc_configured).await?;
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

// The listener update path carries one argument past clippy's threshold (the `rls_grpc_configured`
// gate added in S7); the alternative — bundling the long-standing service args into a struct — is a
// larger churn than the lint is worth, and matches how `rate_limit.rs` handles the same shape.
#[allow(clippy::too_many_arguments)]
pub async fn update_listener(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    mut spec: ListenerSpec,
    expected_version: i64,
    request_id: RequestId,
    rls_grpc_configured: bool,
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
    resolve_global_rate_limit_filters(pool, team, &mut spec, rls_grpc_configured).await?;
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

fn validate_user_listener_name(name: &str) -> DomainResult<()> {
    validate_name(name)?;
    if name.starts_with("ai-") {
        return Err(DomainError::validation(
            "listener names starting with \"ai-\" are reserved for AI routes",
        ));
    }
    Ok(())
}
