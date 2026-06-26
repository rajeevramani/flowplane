//! Rate-limit service: the ONLY mutation path for rate-limit domains, policies, and overrides
//! (spec/10 §2, feature fpv2-4ht slice S2). Every mutation is one transaction containing the
//! row change, its domain event (outbox, where defined), and its audit entry — all or nothing,
//! exactly like `clusters.rs`.
//!
//! Policy mutations append a `RateLimitPolicyUpserted`/`Deleted` event used as an optional
//! immediate reconcile kick for the CP `rls_sync` worker (S5); the 60 s reconcile is the
//! correctness backstop, so domain-only mutations rely on it rather than a bespoke event.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::rate_limit::{
    validate_rate_limit_domain_name, validate_rate_limit_policy_name, RateLimitDomain,
    RateLimitPolicy, RateLimitPolicySpec, RateLimitTeamOverride, RateLimitTeamOverrideSpec,
};
use fp_domain::{DomainError, DomainResult, RateLimitDomainId, RequestId};
use fp_storage::repos::{audit, rate_limit};
use fp_storage::scope::TeamScope;
use sqlx::PgPool;

async fn authorize(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    action: Action,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::RateLimits, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::RateLimits,
                action,
                Some(team),
                reason,
            )
            .await;
            Err(deny_to_error(Resource::RateLimits, action, reason))
        }
    }
}

async fn resolve_domain(
    pool: &PgPool,
    team: TeamRef,
    domain: &str,
) -> DomainResult<RateLimitDomain> {
    rate_limit::get_domain(pool, TeamScope::Team(team.id), domain)
        .await?
        .ok_or_else(|| DomainError::not_found("rate-limit domain", domain))
}

async fn resolve_policy(
    pool: &PgPool,
    team: TeamRef,
    domain_id: RateLimitDomainId,
    policy: &str,
) -> DomainResult<RateLimitPolicy> {
    rate_limit::get_policy(pool, TeamScope::Team(team.id), domain_id, policy)
        .await?
        .ok_or_else(|| DomainError::not_found("rate-limit policy", policy))
}

// ---- Domains ----------------------------------------------------------------------------

pub async fn create_domain(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<RateLimitDomain> {
    authorize(pool, ctx, Action::Create, team, request_id).await?;
    validate_rate_limit_domain_name(name)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create rate-limit domain: begin"))?;
    let domain = rate_limit::create_domain(&mut tx, team, name).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "rate_limit_domain.create",
            "domains",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create rate-limit domain: commit"))?;
    Ok(domain)
}

pub async fn get_domain(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<RateLimitDomain> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    resolve_domain(pool, team, name).await
}

pub async fn list_domains(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<RateLimitDomain>, i64)> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    rate_limit::list_domains(pool, TeamScope::Team(team.id), limit, offset).await
}

pub async fn update_domain(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    new_name: &str,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<RateLimitDomain> {
    authorize(pool, ctx, Action::Update, team, request_id).await?;
    validate_rate_limit_domain_name(new_name)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update rate-limit domain: begin"))?;
    let domain =
        rate_limit::update_domain(&mut tx, team.id, name, new_name, expected_version).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "rate_limit_domain.update",
            "domains",
            new_name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update rate-limit domain: commit"))?;
    Ok(domain)
}

pub async fn delete_domain(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize(pool, ctx, Action::Delete, team, request_id).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete rate-limit domain: begin"))?;
    // The repo cascades the soft-delete to the domain's policies and overrides; the 60 s
    // reconcile drops them from the RLS (no per-policy kick event for a domain delete).
    rate_limit::delete_domain(&mut tx, team.id, name, expected_version).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "rate_limit_domain.delete",
            "domains",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete rate-limit domain: commit"))?;
    Ok(())
}

// ---- Policies ---------------------------------------------------------------------------

pub async fn create_policy(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    domain: &str,
    name: &str,
    spec: RateLimitPolicySpec,
    request_id: RequestId,
) -> DomainResult<RateLimitPolicy> {
    authorize(pool, ctx, Action::Create, team, request_id).await?;
    validate_rate_limit_policy_name(name)?;
    spec.validate()?;
    let domain_row = resolve_domain(pool, team, domain).await?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::RateLimits).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create rate-limit policy: begin"))?;
    let policy = rate_limit::create_policy(&mut tx, team, domain_row.id, name, &spec).await?;
    append_policy_event(&mut tx, team, &policy).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "rate_limit_policy.create",
            "policies",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create rate-limit policy: commit"))?;
    Ok(policy)
}

pub async fn get_policy(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    domain: &str,
    name: &str,
    request_id: RequestId,
) -> DomainResult<RateLimitPolicy> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    let domain_row = resolve_domain(pool, team, domain).await?;
    resolve_policy(pool, team, domain_row.id, name).await
}

pub async fn list_policies(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    domain: &str,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<RateLimitPolicy>, i64)> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    let domain_row = resolve_domain(pool, team, domain).await?;
    rate_limit::list_policies(pool, TeamScope::Team(team.id), domain_row.id, limit, offset).await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_policy(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    domain: &str,
    name: &str,
    spec: RateLimitPolicySpec,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<RateLimitPolicy> {
    authorize(pool, ctx, Action::Update, team, request_id).await?;
    spec.validate()?;
    let domain_row = resolve_domain(pool, team, domain).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update rate-limit policy: begin"))?;
    let policy = rate_limit::update_policy(
        &mut tx,
        team.id,
        domain_row.id,
        name,
        &spec,
        expected_version,
    )
    .await?;
    append_policy_event(&mut tx, team, &policy).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "rate_limit_policy.update",
            "policies",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update rate-limit policy: commit"))?;
    Ok(policy)
}

pub async fn delete_policy(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    domain: &str,
    name: &str,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize(pool, ctx, Action::Delete, team, request_id).await?;
    let domain_row = resolve_domain(pool, team, domain).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete rate-limit policy: begin"))?;
    let policy_id =
        rate_limit::delete_policy(&mut tx, team.id, domain_row.id, name, expected_version).await?;
    fp_storage::outbox::append(
        &mut tx,
        &DomainEvent::RateLimitPolicyDeleted {
            policy_id: policy_id.as_uuid(),
            domain_id: domain_row.id.as_uuid(),
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
            "rate_limit_policy.delete",
            "policies",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete rate-limit policy: commit"))?;
    Ok(())
}

// ---- Team overrides ---------------------------------------------------------------------

pub async fn create_override(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    domain: &str,
    policy: &str,
    spec: RateLimitTeamOverrideSpec,
    request_id: RequestId,
) -> DomainResult<RateLimitTeamOverride> {
    authorize(pool, ctx, Action::Create, team, request_id).await?;
    spec.validate()?;
    let domain_row = resolve_domain(pool, team, domain).await?;
    let policy_row = resolve_policy(pool, team, domain_row.id, policy).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create rate-limit override: begin"))?;
    let overridden = rate_limit::create_override(&mut tx, team, policy_row.id, &spec).await?;
    // An override changes the policy's effective limit -> re-push the policy.
    append_policy_event(&mut tx, team, &policy_row).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "rate_limit_override.create",
            "overrides",
            policy,
        ),
    )
    .await?;
    tx.commit().await.map_err(crate::services::db_err(
        "create rate-limit override: commit",
    ))?;
    Ok(overridden)
}

pub async fn get_override(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    domain: &str,
    policy: &str,
    request_id: RequestId,
) -> DomainResult<RateLimitTeamOverride> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    let domain_row = resolve_domain(pool, team, domain).await?;
    let policy_row = resolve_policy(pool, team, domain_row.id, policy).await?;
    rate_limit::get_override(pool, TeamScope::Team(team.id), policy_row.id)
        .await?
        .ok_or_else(|| DomainError::not_found("rate-limit override", policy))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_override(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    domain: &str,
    policy: &str,
    spec: RateLimitTeamOverrideSpec,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<RateLimitTeamOverride> {
    authorize(pool, ctx, Action::Update, team, request_id).await?;
    spec.validate()?;
    let domain_row = resolve_domain(pool, team, domain).await?;
    let policy_row = resolve_policy(pool, team, domain_row.id, policy).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update rate-limit override: begin"))?;
    let overridden =
        rate_limit::update_override(&mut tx, team.id, policy_row.id, &spec, expected_version)
            .await?;
    append_policy_event(&mut tx, team, &policy_row).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "rate_limit_override.update",
            "overrides",
            policy,
        ),
    )
    .await?;
    tx.commit().await.map_err(crate::services::db_err(
        "update rate-limit override: commit",
    ))?;
    Ok(overridden)
}

pub async fn delete_override(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    domain: &str,
    policy: &str,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<()> {
    authorize(pool, ctx, Action::Delete, team, request_id).await?;
    let domain_row = resolve_domain(pool, team, domain).await?;
    let policy_row = resolve_policy(pool, team, domain_row.id, policy).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete rate-limit override: begin"))?;
    rate_limit::delete_override(&mut tx, team.id, policy_row.id, expected_version).await?;
    // The policy reverts to its base limit -> re-push it (still an upsert of effective config).
    append_policy_event(&mut tx, team, &policy_row).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "rate_limit_override.delete",
            "overrides",
            policy,
        ),
    )
    .await?;
    tx.commit().await.map_err(crate::services::db_err(
        "delete rate-limit override: commit",
    ))?;
    Ok(())
}

// ---- helpers ----------------------------------------------------------------------------

async fn append_policy_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    policy: &RateLimitPolicy,
) -> DomainResult<()> {
    fp_storage::outbox::append(
        tx,
        &DomainEvent::RateLimitPolicyUpserted {
            policy_id: policy.id.as_uuid(),
            domain_id: policy.domain_id.as_uuid(),
        },
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await
}

fn mutation_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    team: TeamRef,
    action: &str,
    kind: &str,
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
        resource: format!("rate-limits/{kind}/{name}"),
        org_id: Some(team.org_id),
        team_id: Some(team.id),
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}
