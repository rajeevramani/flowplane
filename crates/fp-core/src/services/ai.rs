//! AI gateway services.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, UpstreamTlsConfig};
use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec};
use fp_domain::gateway::route_config::{
    DirectResponseAction, HeaderMatch, HeaderValueMatch, PathMatch, RetryPolicy, RouteAction,
    RouteConfigSpec, RouteRule, VirtualHost, WeightedClusterTarget,
};
use fp_domain::{
    ai_error_envelope, validate_ai_budget_name, validate_ai_provider_name, validate_ai_route_name,
    validate_trace_ttl_days, AiBudget, AiBudgetSpec, AiProvider, AiProviderSpec, AiRetentionPolicy,
    AiRoute, AiRouteMaterializedResources, AiRouteSpec, AiTraceEvent, AiUsageSummary, DomainError,
    DomainResult, RequestId, AI_MODEL_HEADER, DEFAULT_AI_ROUTE_TIMEOUT_SECS,
};
use fp_storage::repos::{ai, ai_trace, audit, clusters as cluster_repo, gateway as gateway_repo};
use sqlx::PgPool;
use std::collections::BTreeMap;

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

pub async fn create_provider(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: AiProviderSpec,
    request_id: RequestId,
    advisory: crate::services::egress_advisory::EgressAdvisoryPolicy,
) -> DomainResult<AiProvider> {
    authorize(
        pool,
        ctx,
        Resource::AiProviders,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    validate_ai_provider_name(name)?;
    validate_provider_spec(pool, ctx, team, &spec, request_id).await?;
    advisory
        .enforce_hosts(
            pool,
            ctx,
            request_id,
            team,
            "ai_provider.create",
            &format!("ai-providers/{name}"),
            vec![
                crate::services::egress_advisory::authority_host(&spec.base_url).ok_or_else(
                    || DomainError::validation("AI provider base_url must include a host"),
                )?,
            ],
        )
        .await?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::AiProviders).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create AI provider: begin"))?;
    let provider = ai::create(&mut tx, team, name, &spec).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "ai_provider.create",
            "ai-providers",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create AI provider: commit"))?;
    Ok(provider)
}

pub async fn list_providers(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<AiProvider>, i64)> {
    authorize(
        pool,
        ctx,
        Resource::AiProviders,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    ai::list(pool, team.id, limit, offset).await
}

pub async fn get_provider(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<AiProvider> {
    authorize(
        pool,
        ctx,
        Resource::AiProviders,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    ai::get(pool, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("AI provider", name))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_provider(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: AiProviderSpec,
    expected_version: i64,
    request_id: RequestId,
    advisory: crate::services::egress_advisory::EgressAdvisoryPolicy,
) -> DomainResult<AiProvider> {
    authorize(
        pool,
        ctx,
        Resource::AiProviders,
        Action::Update,
        team,
        request_id,
    )
    .await?;
    validate_provider_spec(pool, ctx, team, &spec, request_id).await?;
    advisory
        .enforce_hosts(
            pool,
            ctx,
            request_id,
            team,
            "ai_provider.update",
            &format!("ai-providers/{name}"),
            vec![
                crate::services::egress_advisory::authority_host(&spec.base_url).ok_or_else(
                    || DomainError::validation("AI provider base_url must include a host"),
                )?,
            ],
        )
        .await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update AI provider: begin"))?;
    // Per-team AI-materialization lock; must be the first statement in the tx, before any
    // materialization-sensitive read or write. Route create/update/delete take the same lock
    // (fpv2-8am), so provider updates and route materialization serialize per team.
    ai::acquire_materialization_lock(&mut tx, team.id).await?;
    let provider = ai::update(&mut tx, team.id, name, &spec, expected_version).await?;
    // Re-materialize every dependent backend cluster from the committed-to-be provider spec
    // and emit the ClusterUpserted events that drive the xDS re-push. base_url is the only
    // provider field snapshotted into materialized state (endpoint host/port, use_tls, SNI);
    // path_prefix/auth_header/credential are read live per-request by the ExtProc.
    let dependents = ai::routes_referencing_provider(&mut tx, team.id, provider.id).await?;
    if !dependents.is_empty() {
        let cluster_spec = provider_cluster_spec(&provider)?;
        for route in &dependents {
            for (position, backend) in route.spec.backends.iter().enumerate() {
                if backend.provider_id != provider.id {
                    continue;
                }
                let cluster_name =
                    route.materialized.cluster_names.get(position).ok_or_else(|| {
                        DomainError::internal(format!(
                            "AI route \"{}\" has no materialized cluster at backend position {position}",
                            route.name
                        ))
                    })?;
                let cluster = cluster_repo::update_ai_owned_spec(
                    &mut tx,
                    team.id,
                    cluster_name,
                    &cluster_spec,
                )
                .await?;
                append_gateway_event(
                    &mut tx,
                    team,
                    DomainEvent::ClusterUpserted {
                        cluster_id: cluster.id.as_uuid(),
                        name: cluster.name,
                    },
                )
                .await?;
            }
        }
        // Conflict signal for racing route writers holding a pre-update route snapshot:
        // their OCC check fails with RevisionMismatch instead of silently materializing
        // from the superseded provider spec. Deliberately no status write.
        ai::bump_routes_version_for_provider(&mut tx, team.id, provider.id).await?;
    }
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "ai_provider.update",
            "ai-providers",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update AI provider: commit"))?;
    Ok(provider)
}

pub async fn delete_provider(
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
        Resource::AiProviders,
        Action::Delete,
        team,
        request_id,
    )
    .await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete AI provider: begin"))?;
    let provider = ai::get(pool, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("AI provider", name))?;
    let dependents = ai::route_names_referencing_provider(&mut tx, team.id, provider.id).await?;
    if !dependents.is_empty() {
        return Err(DomainError::conflict(format!(
            "AI provider \"{name}\" is referenced by AI routes: {}",
            dependents.join(", ")
        ))
        .with_hint("update or delete those AI routes first"));
    }
    let dependents = ai::budget_names_referencing_provider(&mut tx, team.id, provider.id).await?;
    if !dependents.is_empty() {
        return Err(DomainError::conflict(format!(
            "AI provider \"{name}\" is referenced by AI budgets: {}",
            dependents.join(", ")
        ))
        .with_hint("update or delete those AI budgets first"));
    }
    ai::delete(&mut tx, team.id, name, expected_version).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "ai_provider.delete",
            "ai-providers",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete AI provider: commit"))?;
    Ok(())
}

pub async fn create_route(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: AiRouteSpec,
    request_id: RequestId,
) -> DomainResult<AiRoute> {
    authorize(
        pool,
        ctx,
        Resource::AiRoutes,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    authorize(
        pool,
        ctx,
        Resource::Clusters,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    authorize(
        pool,
        ctx,
        Resource::RouteConfigs,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    authorize(
        pool,
        ctx,
        Resource::Listeners,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    authorize_materialized_cleanup(pool, ctx, team, request_id).await?;
    validate_ai_route_name(name)?;
    spec.validate()?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::AiRoutes).await?;
    let materialized = materialized_names(name, &spec)?;
    // Single tx (fpv2-8am): lock first, then every materialization-sensitive read and write.
    // Materialization, its outbox upserts, and the route row commit or roll back together —
    // no transient half-materialized state, no best-effort cleanup on failure.
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create AI route: begin"))?;
    ai::acquire_materialization_lock(&mut tx, team.id).await?;
    let providers = load_route_providers(&mut tx, team, &spec).await?;
    create_materialized(&mut tx, team, name, &spec, &providers, &materialized).await?;
    let route = ai::create_route(&mut tx, team, name, &spec, &materialized).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "ai_route.create", "ai-routes", name),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create AI route: commit"))?;
    Ok(route)
}

pub async fn list_routes(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<AiRoute>, i64)> {
    authorize(
        pool,
        ctx,
        Resource::AiRoutes,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    ai::list_routes(pool, team.id, limit, offset).await
}

pub async fn get_route(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<AiRoute> {
    authorize(
        pool,
        ctx,
        Resource::AiRoutes,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    ai::get_route(pool, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("AI route", name))
}

pub async fn create_budget(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: AiBudgetSpec,
    request_id: RequestId,
) -> DomainResult<AiBudget> {
    authorize(
        pool,
        ctx,
        Resource::AiBudgets,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    validate_ai_budget_name(name)?;
    validate_budget_spec(pool, team, &spec).await?;
    crate::services::quota::check_team_resource_quota(pool, team.id, Resource::AiBudgets).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create AI budget: begin"))?;
    let budget = ai::create_budget(&mut tx, team, name, &spec).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "ai_budget.create",
            "ai-budgets",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("create AI budget: commit"))?;
    Ok(budget)
}

pub async fn list_budgets(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<AiBudget>, i64)> {
    authorize(
        pool,
        ctx,
        Resource::AiBudgets,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    ai::list_budgets(pool, team.id, limit, offset).await
}

pub async fn get_budget(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<AiBudget> {
    authorize(
        pool,
        ctx,
        Resource::AiBudgets,
        Action::Read,
        team,
        request_id,
    )
    .await?;
    ai::get_budget(pool, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("AI budget", name))
}

pub async fn update_budget(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: AiBudgetSpec,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<AiBudget> {
    authorize(
        pool,
        ctx,
        Resource::AiBudgets,
        Action::Update,
        team,
        request_id,
    )
    .await?;
    validate_budget_spec(pool, team, &spec).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update AI budget: begin"))?;
    let budget = ai::update_budget(&mut tx, team.id, name, &spec, expected_version).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "ai_budget.update",
            "ai-budgets",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update AI budget: commit"))?;
    Ok(budget)
}

pub async fn delete_budget(
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
        Resource::AiBudgets,
        Action::Delete,
        team,
        request_id,
    )
    .await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete AI budget: begin"))?;
    ai::delete_budget(&mut tx, team.id, name, expected_version).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "ai_budget.delete",
            "ai-budgets",
            name,
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete AI budget: commit"))?;
    Ok(())
}

/// The largest span `until - since` a windowed usage read may request. An omitted `since`
/// is an explicit all-time read and is exempt (it preserves the endpoint's original
/// all-time semantics); the cap only applies when the caller pins the lower bound.
pub const MAX_USAGE_WINDOW_DAYS: i64 = 92;

/// Windowed, team-scoped usage summary. Window semantics (design ui-f5): half-open
/// `[since, until)`; an omitted `until` resolves server-side to `now` BEFORE validation;
/// `since >= until` (after resolution) and spans over [`MAX_USAGE_WINDOW_DAYS`] fail
/// validation. Returns the summary rows plus the total grouped-row count for the same
/// filters (the `Page.total`).
pub async fn usage_summary(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    mut query: ai::AiUsageQuery,
    request_id: RequestId,
) -> DomainResult<(Vec<AiUsageSummary>, i64)> {
    authorize(pool, ctx, Resource::AiUsage, Action::Read, team, request_id).await?;
    query.until = Some(resolve_usage_window(
        query.since,
        query.until,
        chrono::Utc::now(),
    )?);
    ai::usage_summary(pool, team.id, query).await
}

/// Pure window resolution: returns the effective `until` (caller value or `now`), after
/// validating `since < until` and the [`MAX_USAGE_WINDOW_DAYS`] span cap (which only
/// applies when `since` is present — an omitted `since` is an explicit all-time read).
pub fn resolve_usage_window(
    since: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> DomainResult<chrono::DateTime<chrono::Utc>> {
    let until = until.unwrap_or(now);
    if let Some(since) = since {
        if since >= until {
            return Err(DomainError::validation(
                "usage window is empty: `since` must be strictly before `until` \
                 (`until` defaults to now when omitted)",
            ));
        }
        if until - since > chrono::Duration::days(MAX_USAGE_WINDOW_DAYS) {
            return Err(DomainError::validation(format!(
                "usage window span exceeds the {MAX_USAGE_WINDOW_DAYS}-day maximum; \
                 omit `since` for an all-time read"
            )));
        }
    }
    Ok(until)
}

/// Team-scoped read of AI trace rows: authorization happens before any repo read, and the
/// repo query is scoped by the resolved team id, so a foreign request_id can never match.
pub async fn trace_events(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    query: ai_trace::AiTraceQuery<'_>,
    request_id: RequestId,
) -> DomainResult<Vec<AiTraceEvent>> {
    authorize(pool, ctx, Resource::AiUsage, Action::Read, team, request_id).await?;
    ai_trace::list_trace_events(pool, team.id, query).await
}

/// The team's AI trace retention policy row; `None` means the built-in 30-day default is in
/// force. Guarded by the same resource the trace read uses.
pub async fn get_retention_policy(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<Option<AiRetentionPolicy>> {
    authorize(pool, ctx, Resource::AiUsage, Action::Read, team, request_id).await?;
    ai_trace::get_retention_policy(pool, team.id).await
}

/// Create-or-replace the team's AI trace retention policy (PUT semantics: one row per team).
/// Affects only rows inserted after the change — existing rows keep the expiry they were
/// stamped with.
pub async fn set_retention_policy(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    trace_ttl_days: i32,
    expected_version: Option<i64>,
    request_id: RequestId,
) -> DomainResult<AiRetentionPolicy> {
    authorize(
        pool,
        ctx,
        Resource::AiUsage,
        Action::Update,
        team,
        request_id,
    )
    .await?;
    validate_trace_ttl_days(trace_ttl_days)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("set AI retention policy: begin"))?;
    let policy =
        ai_trace::set_retention_policy(&mut tx, team, trace_ttl_days, expected_version).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "ai_retention.set",
            "ai-retention",
            "trace",
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("set AI retention policy: commit"))?;
    Ok(policy)
}

pub async fn update_route(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: AiRouteSpec,
    expected_version: i64,
    request_id: RequestId,
) -> DomainResult<AiRoute> {
    authorize(
        pool,
        ctx,
        Resource::AiRoutes,
        Action::Update,
        team,
        request_id,
    )
    .await?;
    authorize(
        pool,
        ctx,
        Resource::Clusters,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    authorize(
        pool,
        ctx,
        Resource::RouteConfigs,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    authorize(
        pool,
        ctx,
        Resource::Listeners,
        Action::Create,
        team,
        request_id,
    )
    .await?;
    authorize_materialized_cleanup(pool, ctx, team, request_id).await?;
    spec.validate()?;
    let materialized = materialized_names(name, &spec)?;
    // Single tx (fpv2-8am): the lock precedes the route-row read + revision check and the
    // provider loads — a pre-lock check could pass, lose to a provider update's version
    // bump, and strand a torn rebuild. Teardown, rebuild, upserts, and the route row commit
    // or roll back together.
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update AI route: begin"))?;
    ai::acquire_materialization_lock(&mut tx, team.id).await?;
    let current = ai::get_route_in_tx(&mut tx, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("AI route", name))?;
    if current.version != expected_version {
        return Err(revision_mismatch(
            "AI route",
            name,
            current.version,
            expected_version,
        ));
    }
    reject_if_budgets_pin_route_config(&mut tx, team, name, &current.materialized).await?;
    let providers = load_route_providers(&mut tx, team, &spec).await?;
    cleanup_materialized(&mut tx, team, &current.materialized).await?;
    create_materialized(&mut tx, team, name, &spec, &providers, &materialized).await?;
    let route = ai::update_route(
        &mut tx,
        team.id,
        name,
        &spec,
        &materialized,
        expected_version,
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "ai_route.update", "ai-routes", name),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("update AI route: commit"))?;
    Ok(route)
}

pub async fn delete_route(
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
        Resource::AiRoutes,
        Action::Delete,
        team,
        request_id,
    )
    .await?;
    authorize_materialized_cleanup(pool, ctx, team, request_id).await?;
    // Single tx (fpv2-8am): a failed teardown rolls the whole delete back — the route row
    // can no longer outlive its materialized resources (or vice versa).
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete AI route: begin"))?;
    ai::acquire_materialization_lock(&mut tx, team.id).await?;
    let route = ai::get_route_in_tx(&mut tx, team.id, name)
        .await?
        .ok_or_else(|| DomainError::not_found("AI route", name))?;
    if route.version != expected_version {
        return Err(revision_mismatch(
            "AI route",
            name,
            route.version,
            expected_version,
        ));
    }
    reject_if_budgets_pin_route_config(&mut tx, team, name, &route.materialized).await?;
    cleanup_materialized(&mut tx, team, &route.materialized).await?;
    ai::delete_route(&mut tx, team.id, name, expected_version).await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(ctx, request_id, team, "ai_route.delete", "ai-routes", name),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("delete AI route: commit"))?;
    Ok(())
}

fn revision_mismatch(resource: &str, name: &str, current: i64, supplied: i64) -> DomainError {
    DomainError::new(
        fp_domain::ErrorCode::RevisionMismatch,
        format!("{resource} \"{name}\" is at revision {current}, you supplied {supplied}"),
    )
    .with_hint("re-read the resource and retry with the current revision")
}

async fn authorize_materialized_cleanup(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    for resource in [
        Resource::Listeners,
        Resource::RouteConfigs,
        Resource::Clusters,
    ] {
        authorize(pool, ctx, resource, Action::Read, team, request_id).await?;
        authorize(pool, ctx, resource, Action::Delete, team, request_id).await?;
    }
    Ok(())
}

/// Refuse route update/delete while an AI budget pins the route's materialized route config
/// (FK `ai_budgets.route_config_id` ON DELETE RESTRICT). Mirrors delete_provider's dependent
/// checks: a clean conflict with a hint beats the pre-fix behavior, which swallowed the FK
/// failure and silently leaked the route config + clusters behind the budget (fpv2-8am).
async fn reject_if_budgets_pin_route_config(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    route_name: &str,
    materialized: &AiRouteMaterializedResources,
) -> DomainResult<()> {
    let budgets = ai::budget_names_referencing_route_config_name(
        tx,
        team.id,
        &materialized.route_config_name,
    )
    .await?;
    if budgets.is_empty() {
        return Ok(());
    }
    Err(DomainError::conflict(format!(
        "AI route \"{route_name}\" is referenced by AI budgets scoped to its route config: {}",
        budgets.join(", ")
    ))
    .with_hint("update or delete those AI budgets first"))
}

/// Load every backend's provider inside the mutation tx, after the AI-materialization lock
/// (fpv2-8am): a pre-lock read could materialize clusters from a provider spec a concurrent
/// provider update supersedes mid-mutation.
async fn load_route_providers(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    spec: &AiRouteSpec,
) -> DomainResult<Vec<AiProvider>> {
    let mut providers = Vec::with_capacity(spec.backends.len());
    for backend in &spec.backends {
        let provider = ai::get_provider_by_id_in_tx(tx, team.id, backend.provider_id)
            .await?
            .ok_or_else(|| {
                DomainError::not_found("AI provider", &backend.provider_id.to_string())
            })?;
        providers.push(provider);
    }
    Ok(providers)
}

fn materialized_names(
    name: &str,
    spec: &AiRouteSpec,
) -> DomainResult<AiRouteMaterializedResources> {
    let targets = ai_route_targets(name, spec)?;
    let mut cluster_names = (0..spec.backends.len())
        .map(|idx| generated_name(name, &format!("-b{}", idx + 1)))
        .collect::<Vec<_>>();
    for target in targets {
        for chain in target.aggregate_chains {
            cluster_names.push(chain.name);
        }
    }
    Ok(AiRouteMaterializedResources {
        cluster_names,
        route_config_name: generated_name(name, "-routes"),
        listener_name: generated_name(name, "-listener"),
    })
}

fn backend_cluster_names(names: &AiRouteMaterializedResources, backend_count: usize) -> &[String] {
    &names.cluster_names[..backend_count]
}

#[derive(Debug, Clone)]
struct AiRouteTarget {
    name: String,
    headers: Vec<HeaderMatch>,
    indexes: Vec<usize>,
    aggregate_chains: Vec<AiAggregateChain>,
}

#[derive(Debug, Clone)]
struct AiAggregateChain {
    name: String,
    members: Vec<usize>,
    weight: u32,
}

fn ai_route_targets(route_name: &str, spec: &AiRouteSpec) -> DomainResult<Vec<AiRouteTarget>> {
    let mut by_model: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut catch_all = Vec::new();
    for (idx, backend) in spec.backends.iter().enumerate() {
        if backend.models.is_empty() {
            catch_all.push(idx);
        } else {
            for model in &backend.models {
                by_model.entry(model.clone()).or_default().push(idx);
            }
        }
    }
    let mut targets = Vec::new();
    for (model, indexes) in by_model {
        let name = format!("model-{}", route_token(&model));
        targets.push(AiRouteTarget {
            aggregate_chains: aggregate_chains(route_name, spec, &name, &indexes)?,
            name,
            headers: vec![HeaderMatch {
                name: AI_MODEL_HEADER.into(),
                invert_match: false,
                matcher: HeaderValueMatch::Exact { value: model },
            }],
            indexes,
        });
    }
    if !catch_all.is_empty() {
        targets.push(AiRouteTarget {
            aggregate_chains: aggregate_chains(route_name, spec, "default", &catch_all)?,
            name: "default".into(),
            headers: Vec::new(),
            indexes: catch_all,
        });
    }
    Ok(targets)
}

fn aggregate_chains(
    route_name: &str,
    spec: &AiRouteSpec,
    target_name: &str,
    indexes: &[usize],
) -> DomainResult<Vec<AiAggregateChain>> {
    let mut tiers: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for idx in indexes {
        tiers
            .entry(spec.backends[*idx].priority)
            .or_default()
            .push(*idx);
    }
    if tiers.len() <= 1 {
        return Ok(Vec::new());
    }

    let mut chains = vec![(Vec::<usize>::new(), 1_u64)];
    for tier_indexes in tiers.values() {
        let mut next = Vec::new();
        for (members, weight) in &chains {
            for idx in tier_indexes {
                let mut tier_members = members.clone();
                tier_members.push(*idx);
                next.push((
                    tier_members,
                    weight.saturating_mul(u64::from(spec.backends[*idx].weight)),
                ));
            }
        }
        chains = next;
        if chains.len() > 32 {
            return Err(DomainError::validation(
                "AI priority failover materializes at most 32 weighted failover chains",
            ));
        }
    }

    let weights = normalize_chain_weights(chains.iter().map(|(_, weight)| *weight).collect());
    Ok(chains
        .into_iter()
        .zip(weights)
        .enumerate()
        .map(|(idx, ((members, _), weight))| AiAggregateChain {
            name: generated_name(
                route_name,
                &format!("-agg-{}-{}", route_token(target_name), idx + 1),
            ),
            members,
            weight,
        })
        .collect())
}

fn normalize_chain_weights(raw: Vec<u64>) -> Vec<u32> {
    let total = raw.iter().copied().sum::<u64>().max(1);
    let mut weights = raw
        .iter()
        .map(|weight| ((*weight).saturating_mul(10_000) / total).clamp(1, 10_000) as u32)
        .collect::<Vec<_>>();
    let mut sum = weights.iter().sum::<u32>();
    while sum > 10_000 {
        if let Some(weight) = weights.iter_mut().max() {
            if *weight == 1 {
                break;
            }
            *weight -= 1;
            sum -= 1;
        } else {
            break;
        }
    }
    weights
}

fn generated_name(route_name: &str, suffix: &str) -> String {
    const MAX_NAME_LEN: usize = 100;
    let prefix = "ai-";
    let max_route_len = MAX_NAME_LEN
        .saturating_sub(prefix.len())
        .saturating_sub(suffix.len());
    let mut base = route_name
        .chars()
        .take(max_route_len)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string();
    if base.is_empty() {
        base = "route".into();
    }
    format!("{prefix}{base}{suffix}")
}

/// Materialize a route's clusters/route-config/listener inside the caller's transaction
/// (fpv2-8am: no own tx — creation, its outbox upserts, and the route-row write commit or
/// roll back together, under the caller's AI-materialization lock).
async fn create_materialized(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    route_name: &str,
    spec: &AiRouteSpec,
    providers: &[AiProvider],
    names: &AiRouteMaterializedResources,
) -> DomainResult<()> {
    let targets = ai_route_targets(route_name, spec)?;
    let backend_names = backend_cluster_names(names, spec.backends.len());
    let mut cluster_specs = Vec::with_capacity(names.cluster_names.len());
    for (provider, cluster_name) in providers.iter().zip(backend_names.iter()) {
        let cluster_spec = match provider_cluster_spec(provider) {
            Ok(spec) => spec,
            Err(err) => return Err(err),
        };
        cluster_specs.push((cluster_name.clone(), cluster_spec));
    }
    for target in &targets {
        for chain in &target.aggregate_chains {
            cluster_specs.push((
                chain.name.clone(),
                aggregate_cluster_spec(
                    chain
                        .members
                        .iter()
                        .map(|idx| backend_names[*idx].clone())
                        .collect(),
                ),
            ));
        }
    }
    let route_config_spec = ai_route_config_spec(spec, &targets, names)?;
    let listener_spec = ListenerSpec {
        address: "0.0.0.0".into(),
        port: spec.listener_port,
        public_base_url: None,
        protocol: ListenerProtocol::Http,
        route_config: Some(names.route_config_name.clone()),
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: None,
    };
    let owner_id = uuid::Uuid::now_v7();
    let mut cluster_events = Vec::with_capacity(cluster_specs.len());
    let existing_clusters = fp_storage::repos::clusters::count_for_team_in_tx(tx, team.id).await?;
    let cluster_limit = crate::services::quota::default_limit(Resource::Clusters);
    for (cluster_name, cluster_spec) in cluster_specs {
        let used = existing_clusters + cluster_events.len() as i64;
        if used >= cluster_limit {
            return Err(crate::services::quota::quota_exceeded(
                Resource::Clusters,
                used,
                cluster_limit,
            ));
        }
        let cluster =
            cluster_repo::create_ai_owned(tx, team, owner_id, &cluster_name, &cluster_spec).await?;
        cluster_events.push((cluster.id.as_uuid(), cluster.name));
    }
    let route_config = gateway_repo::create_ai_route_config(
        tx,
        team,
        owner_id,
        &names.route_config_name,
        &route_config_spec,
    )
    .await?;
    let listener =
        gateway_repo::create_ai_listener(tx, team, owner_id, &names.listener_name, &listener_spec)
            .await?;
    append_materialized_upserts(
        tx,
        team,
        &cluster_events,
        route_config.id.as_uuid(),
        &route_config.name,
        listener.id.as_uuid(),
        &listener.name,
    )
    .await?;
    Ok(())
}

async fn append_materialized_upserts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    clusters: &[(uuid::Uuid, String)],
    route_config_id: uuid::Uuid,
    route_config_name: &str,
    listener_id: uuid::Uuid,
    listener_name: &str,
) -> DomainResult<()> {
    for (cluster_id, cluster_name) in clusters {
        append_gateway_event(
            tx,
            team,
            DomainEvent::ClusterUpserted {
                cluster_id: *cluster_id,
                name: cluster_name.clone(),
            },
        )
        .await?;
    }
    append_gateway_event(
        tx,
        team,
        DomainEvent::RouteConfigUpserted {
            route_config_id,
            name: route_config_name.into(),
        },
    )
    .await?;
    append_gateway_event(
        tx,
        team,
        DomainEvent::ListenerUpserted {
            listener_id,
            name: listener_name.into(),
        },
    )
    .await
}

async fn append_gateway_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    event: DomainEvent,
) -> DomainResult<()> {
    fp_storage::outbox::append(
        tx,
        &event,
        EventScope {
            org_id: Some(team.org_id),
            team_id: Some(team.id),
        },
        trace_context_json(),
    )
    .await
}

/// Tear down a route's materialized resources inside the caller's transaction. Fallible by
/// design (fpv2-8am): every repository delete and every outbox append propagates, so a failed
/// cleanup rolls the whole mutation back instead of committing half-deleted materialized
/// state or silently dropping deletion events. An absent row is not an error (idempotent
/// teardown); only real failures propagate.
async fn cleanup_materialized(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    names: &AiRouteMaterializedResources,
) -> DomainResult<()> {
    if let Some(listener_id) =
        gateway_repo::delete_ai_listener(tx, team.id, &names.listener_name).await?
    {
        append_gateway_event(
            tx,
            team,
            DomainEvent::ListenerDeleted {
                listener_id: listener_id.as_uuid(),
                name: names.listener_name.clone(),
            },
        )
        .await?;
    }
    if let Some(route_config_id) =
        gateway_repo::delete_ai_route_config(tx, team.id, &names.route_config_name).await?
    {
        append_gateway_event(
            tx,
            team,
            DomainEvent::RouteConfigDeleted {
                route_config_id: route_config_id.as_uuid(),
                name: names.route_config_name.clone(),
            },
        )
        .await?;
    }
    for cluster_name in &names.cluster_names {
        if let Some(cluster_id) = cluster_repo::delete_ai_owned(tx, team.id, cluster_name).await? {
            append_gateway_event(
                tx,
                team,
                DomainEvent::ClusterDeleted {
                    cluster_id: cluster_id.as_uuid(),
                    name: cluster_name.clone(),
                },
            )
            .await?;
        }
    }
    Ok(())
}

fn provider_cluster_spec(provider: &AiProvider) -> DomainResult<ClusterSpec> {
    // Single canonical derivation shared with validate() and the ExtProc :authority
    // rewrite (fpv2-ti2): endpoint host/port, use_tls, and SNI all come from origin(),
    // so the TLS SNI and the rewritten Host can never disagree.
    let origin = provider.spec.origin()?;
    let host = origin.host;
    let use_tls = origin.use_tls;
    let port = origin.port;
    Ok(ClusterSpec {
        endpoints: vec![Endpoint {
            host: host.clone(),
            port,
            weight: None,
        }],
        aggregate_clusters: Vec::new(),
        lb_policy: Default::default(),
        least_request: None,
        ring_hash: None,
        maglev: None,
        dns_lookup_family: None,
        connect_timeout_secs: 10,
        use_tls,
        upstream_tls: use_tls.then_some(UpstreamTlsConfig {
            sni: Some(host),
            validation_context_sds_secret_name: None,
            ca_cert_file: None,
            auto_sni_san_validation: true,
            insecure_skip_verify: false,
        }),
        protocol: None,
        health_checks: None,
        circuit_breakers: None,
        outlier_detection: None,
    })
}

fn aggregate_cluster_spec(cluster_names: Vec<String>) -> ClusterSpec {
    ClusterSpec {
        endpoints: Vec::new(),
        aggregate_clusters: cluster_names,
        lb_policy: Default::default(),
        least_request: None,
        ring_hash: None,
        maglev: None,
        dns_lookup_family: None,
        connect_timeout_secs: 10,
        use_tls: false,
        upstream_tls: None,
        protocol: None,
        health_checks: None,
        circuit_breakers: None,
        outlier_detection: None,
    }
}

fn ai_route_config_spec(
    spec: &AiRouteSpec,
    targets: &[AiRouteTarget],
    names: &AiRouteMaterializedResources,
) -> DomainResult<RouteConfigSpec> {
    let mut routes = Vec::new();
    for target in targets {
        routes.push(route_rule(
            &target.name,
            &spec.path,
            target.headers.clone(),
            target,
            spec,
            names,
        )?);
    }
    if !targets.iter().any(|target| target.headers.is_empty()) {
        routes.push(no_eligible_backend_route(&spec.path));
    }
    if routes.is_empty() {
        return Err(DomainError::validation(
            "AI route has no eligible backend routes",
        ));
    }
    Ok(RouteConfigSpec {
        virtual_hosts: vec![VirtualHost {
            name: "ai".into(),
            domains: vec!["*".into()],
            routes,
            rate_limits: Vec::new(),
            filter_overrides: Vec::new(),
        }],
    })
}

fn no_eligible_backend_route(path: &str) -> RouteRule {
    RouteRule {
        name: "no-eligible-backend".into(),
        matcher: PathMatch::Exact { path: path.into() },
        headers: Vec::new(),
        query_parameters: Vec::new(),
        action: RouteAction {
            cluster: None,
            weighted_clusters: None,
            redirect: None,
            direct_response: Some(DirectResponseAction {
                status: 400,
                body: Some(ai_error_envelope(
                    "no_eligible_ai_backend",
                    "no eligible AI backend for requested model",
                )),
            }),
            prefix_rewrite: None,
            template_rewrite: None,
            timeout_secs: DEFAULT_AI_ROUTE_TIMEOUT_SECS,
            retry_policy: None,
            rate_limits: Vec::new(),
        },
        filter_overrides: Vec::new(),
    }
}

fn route_rule(
    name: &str,
    path: &str,
    headers: Vec<HeaderMatch>,
    target: &AiRouteTarget,
    spec: &AiRouteSpec,
    names: &AiRouteMaterializedResources,
) -> DomainResult<RouteRule> {
    let backend_names = backend_cluster_names(names, spec.backends.len());
    let targets = if target.aggregate_chains.is_empty() {
        target
            .indexes
            .iter()
            .map(|idx| WeightedClusterTarget {
                cluster: backend_names[*idx].clone(),
                weight: spec.backends[*idx].weight,
            })
            .collect::<Vec<_>>()
    } else {
        target
            .aggregate_chains
            .iter()
            .map(|chain| WeightedClusterTarget {
                cluster: chain.name.clone(),
                weight: chain.weight,
            })
            .collect::<Vec<_>>()
    };
    let (cluster, weighted_clusters) = match targets.as_slice() {
        [single] => (Some(single.cluster.clone()), None),
        _ => (None, Some(targets)),
    };
    let retry_policy = (!target.aggregate_chains.is_empty()).then_some(RetryPolicy {
        retry_on: "connect-failure,refused-stream,reset".into(),
        num_retries: Some(
            target
                .aggregate_chains
                .iter()
                .map(|chain| chain.members.len().saturating_sub(1) as u32)
                .max()
                .unwrap_or(1)
                .clamp(1, 10),
        ),
        per_try_timeout_secs: Some(10),
        retriable_status_codes: Vec::new(),
        previous_priorities_retry: true,
    });
    Ok(RouteRule {
        name: name.into(),
        matcher: PathMatch::Exact { path: path.into() },
        headers,
        query_parameters: Vec::new(),
        action: RouteAction {
            cluster,
            weighted_clusters,
            redirect: None,
            direct_response: None,
            prefix_rewrite: None,
            template_rewrite: None,
            timeout_secs: DEFAULT_AI_ROUTE_TIMEOUT_SECS,
            retry_policy,
            rate_limits: Vec::new(),
        },
        filter_overrides: Vec::new(),
    })
}

fn route_token(value: &str) -> String {
    let token = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = token.trim_matches('-').chars().take(60).collect::<String>();
    if trimmed.is_empty() {
        "model".into()
    } else {
        trimmed
    }
}

async fn validate_provider_spec(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    spec: &AiProviderSpec,
    request_id: RequestId,
) -> DomainResult<()> {
    spec.validate()?;
    authorize(pool, ctx, Resource::Secrets, Action::Read, team, request_id).await?;
    let secret =
        fp_storage::repos::secrets::get_secret_by_id(pool, team.id, spec.credential_secret_id)
            .await?
            .ok_or_else(|| {
                let id = spec.credential_secret_id.to_string();
                DomainError::not_found("secret", &id)
            })?;
    if !matches!(secret.secret_type, fp_domain::SecretType::GenericSecret) {
        return Err(DomainError::validation(
            "AI provider credential_secret_id must reference a generic_secret",
        ));
    }
    Ok(())
}

async fn validate_budget_spec(
    pool: &PgPool,
    team: TeamRef,
    spec: &AiBudgetSpec,
) -> DomainResult<()> {
    spec.validate()?;
    if let Some(provider_id) = spec.provider_id {
        ai::get_provider_by_id(pool, team.id, provider_id)
            .await?
            .ok_or_else(|| DomainError::not_found("AI provider", &provider_id.to_string()))?;
    }
    if let Some(route_config_id) = spec.route_config_id {
        let exists: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM route_configs WHERE team_id = $1 AND id = $2")
                .bind(team.id.as_uuid())
                .bind(route_config_id.as_uuid())
                .fetch_optional(pool)
                .await
                .map_err(crate::services::db_err(
                    "validate AI budget route_config_id",
                ))?;
        if exists.is_none() {
            return Err(DomainError::not_found(
                "route config",
                &route_config_id.to_string(),
            ));
        }
    }
    Ok(())
}

fn mutation_audit(
    ctx: &PrincipalCtx,
    request_id: RequestId,
    team: TeamRef,
    action: &str,
    resource_prefix: &str,
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
        resource: format!("{resource_prefix}/{name}"),
        org_id: Some(team.org_id),
        team_id: Some(team.id),
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use fp_domain::AiRouteBackend;

    fn backend(priority: u32, weight: u32) -> AiRouteBackend {
        AiRouteBackend {
            provider_id: fp_domain::id::AiProviderId::generate(),
            models: Vec::new(),
            model_override: None,
            weight,
            priority,
        }
    }

    fn route_spec(backends: Vec<AiRouteBackend>) -> AiRouteSpec {
        AiRouteSpec {
            listener_port: 18_080,
            path: "/v1/chat/completions".into(),
            backends,
        }
    }

    fn provider_with_base_url(base_url: &str) -> AiProvider {
        AiProvider {
            id: fp_domain::id::AiProviderId::generate(),
            team_id: fp_domain::id::TeamId::generate(),
            name: "prov".into(),
            spec: AiProviderSpec {
                kind: fp_domain::AiProviderKind::OpenaiCompatible,
                base_url: base_url.into(),
                path_prefix: None,
                credential_secret_id: fp_domain::id::SecretId::generate(),
                models: Vec::new(),
                auth_header: "authorization".into(),
                auth_scheme: None,
            },
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn provider_cluster_spec_endpoint_and_sni_match_origin_host() {
        // fpv2-ti2 AC 2 consistency property: for every accepted base_url, the
        // materialized endpoint host and SNI equal origin().host — the same value the
        // ExtProc :authority rewrite uses.
        let cases = [
            "https://openrouter.ai",
            "https://openrouter.ai/",
            "https://host:8443",
            "http://10.0.0.4:8080",
            "https://[fd00::7]:8443",
            "https://%65xample.com",
        ];
        for base_url in cases {
            let provider = provider_with_base_url(base_url);
            let origin = provider.spec.origin().expect(base_url);
            let cluster = provider_cluster_spec(&provider).expect(base_url);
            assert_eq!(
                cluster.endpoints[0].host, origin.host,
                "endpoint for {base_url}"
            );
            assert_eq!(
                cluster.endpoints[0].port, origin.port,
                "port for {base_url}"
            );
            assert_eq!(cluster.use_tls, origin.use_tls, "use_tls for {base_url}");
            if origin.use_tls {
                let tls = cluster.upstream_tls.as_ref().expect("tls config");
                assert_eq!(
                    tls.sni.as_deref(),
                    Some(origin.host.as_str()),
                    "sni for {base_url}"
                );
            }
        }
    }

    #[test]
    fn provider_cluster_spec_is_byte_identical_across_auth_scheme_change() {
        // Design AC 6 (2026-07-ai-credential-auth-scheme): the materialized cluster
        // derives from base_url origin only, so an auth_scheme-only change must
        // produce an identical ClusterSpec (no new xDS sensitivity).
        let mut provider = provider_with_base_url("https://api.openai.com");
        let before = provider_cluster_spec(&provider).expect("cluster before");
        provider.spec.auth_scheme = Some("Bearer".into());
        let after = provider_cluster_spec(&provider).expect("cluster after");
        assert_eq!(
            serde_json::to_vec(&before).expect("serialize before"),
            serde_json::to_vec(&after).expect("serialize after"),
            "auth_scheme change must not alter materialized cluster content"
        );
    }

    #[test]
    fn provider_cluster_spec_rejects_what_origin_rejects() {
        for base_url in [
            "https://user:pw@host",
            "https://host:abc",
            "https://host/api",
        ] {
            let provider = provider_with_base_url(base_url);
            assert!(provider_cluster_spec(&provider).is_err(), "{base_url}");
        }
    }

    #[test]
    fn priority_failover_materializes_weighted_aggregate_chains() {
        let spec = route_spec(vec![backend(0, 80), backend(0, 20), backend(1, 1)]);
        let names = materialized_names("llm", &spec).expect("names");
        let targets = ai_route_targets("llm", &spec).expect("targets");
        let route_config = ai_route_config_spec(&spec, &targets, &names).expect("route config");

        assert_eq!(names.cluster_names.len(), 5);
        let default_route = route_config.virtual_hosts[0]
            .routes
            .iter()
            .find(|route| route.name == "default")
            .expect("default route");
        let weighted = default_route
            .action
            .weighted_clusters
            .as_ref()
            .expect("weighted aggregate chains");
        assert_eq!(weighted.len(), 2);
        assert_eq!(weighted[0].cluster, names.cluster_names[3]);
        assert_eq!(weighted[0].weight, 8_000);
        assert_eq!(weighted[1].cluster, names.cluster_names[4]);
        assert_eq!(weighted[1].weight, 2_000);

        let retry = default_route.action.retry_policy.as_ref().expect("retry");
        assert_eq!(retry.retry_on, "connect-failure,refused-stream,reset");
        assert_eq!(retry.retriable_status_codes, Vec::<u16>::new());
        assert!(retry.previous_priorities_retry);

        let chains = &targets[0].aggregate_chains;
        assert_eq!(chains[0].members, vec![0, 2]);
        assert_eq!(chains[1].members, vec![1, 2]);
    }

    #[test]
    fn same_priority_backends_remain_weighted_without_retry() {
        let spec = route_spec(vec![backend(0, 3), backend(0, 7)]);
        let names = materialized_names("llm", &spec).expect("names");
        let targets = ai_route_targets("llm", &spec).expect("targets");
        let route_config = ai_route_config_spec(&spec, &targets, &names).expect("route config");

        assert_eq!(names.cluster_names.len(), 2);
        let default_route = route_config.virtual_hosts[0]
            .routes
            .iter()
            .find(|route| route.name == "default")
            .expect("default route");
        let weighted = default_route
            .action
            .weighted_clusters
            .as_ref()
            .expect("weighted backend clusters");
        assert_eq!(weighted[0].cluster, names.cluster_names[0]);
        assert_eq!(weighted[0].weight, 3);
        assert_eq!(weighted[1].cluster, names.cluster_names[1]);
        assert_eq!(weighted[1].weight, 7);
        assert!(default_route.action.retry_policy.is_none());
    }

    #[test]
    fn no_eligible_backend_route_keeps_direct_response_body() {
        let spec = route_spec(vec![AiRouteBackend {
            provider_id: fp_domain::id::AiProviderId::generate(),
            models: vec!["gpt-5".into()],
            model_override: None,
            weight: 1,
            priority: 0,
        }]);
        let names = materialized_names("llm", &spec).expect("names");
        let targets = ai_route_targets("llm", &spec).expect("targets");
        let route_config = ai_route_config_spec(&spec, &targets, &names).expect("route config");

        let route = route_config.virtual_hosts[0]
            .routes
            .iter()
            .find(|route| route.name == "no-eligible-backend")
            .expect("no eligible backend route");
        let direct_response = route
            .action
            .direct_response
            .as_ref()
            .expect("direct response");

        assert_eq!(direct_response.status, 400);
        assert_eq!(
            direct_response.body.as_deref(),
            Some(
                r#"{"code":"no_eligible_ai_backend","message":"no eligible AI backend for requested model"}"#
            )
        );
    }

    mod usage_window {
        use super::super::{resolve_usage_window, MAX_USAGE_WINDOW_DAYS};
        use chrono::{DateTime, Duration, Utc};

        fn ts(raw: &str) -> DateTime<Utc> {
            raw.parse().expect("test timestamp")
        }

        #[test]
        fn omitted_until_resolves_to_now() {
            let now = ts("2026-07-19T12:00:00Z");
            let until = resolve_usage_window(None, None, now).unwrap();
            assert_eq!(until, now);
        }

        #[test]
        fn explicit_until_is_kept() {
            let now = ts("2026-07-19T12:00:00Z");
            let explicit = ts("2026-07-01T00:00:00Z");
            let until = resolve_usage_window(None, Some(explicit), now).unwrap();
            assert_eq!(until, explicit);
        }

        #[test]
        fn since_equal_to_resolved_until_is_empty_window() {
            let now = ts("2026-07-19T12:00:00Z");
            let err = resolve_usage_window(Some(now), None, now).unwrap_err();
            assert_eq!(err.code, fp_domain::ErrorCode::ValidationFailed);
        }

        #[test]
        fn since_after_until_rejected() {
            let now = ts("2026-07-19T12:00:00Z");
            let err =
                resolve_usage_window(Some(now), Some(now - Duration::hours(1)), now).unwrap_err();
            assert_eq!(err.code, fp_domain::ErrorCode::ValidationFailed);
        }

        #[test]
        fn span_at_cap_allowed_beyond_cap_rejected() {
            let now = ts("2026-07-19T12:00:00Z");
            let at_cap = now - Duration::days(MAX_USAGE_WINDOW_DAYS);
            assert!(resolve_usage_window(Some(at_cap), None, now).is_ok());
            let beyond = at_cap - Duration::seconds(1);
            let err = resolve_usage_window(Some(beyond), None, now).unwrap_err();
            assert_eq!(err.code, fp_domain::ErrorCode::ValidationFailed);
        }

        #[test]
        fn omitted_since_is_exempt_from_span_cap() {
            let now = ts("2026-07-19T12:00:00Z");
            assert!(resolve_usage_window(None, Some(now), now).is_ok());
        }
    }
}
