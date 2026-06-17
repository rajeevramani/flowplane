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
    validate_ai_budget_name, validate_ai_provider_name, validate_ai_route_name, AiBudget,
    AiBudgetSpec, AiProvider, AiProviderSpec, AiRoute, AiRouteMaterializedResources, AiRouteSpec,
    AiUsageSummary, DomainError, DomainResult, RequestId, AI_MODEL_HEADER,
    DEFAULT_AI_ROUTE_TIMEOUT_SECS,
};
use fp_storage::repos::{ai, audit, clusters as cluster_repo, gateway as gateway_repo};
use reqwest::Url;
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

pub async fn update_provider(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    spec: AiProviderSpec,
    expected_version: i64,
    request_id: RequestId,
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
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update AI provider: begin"))?;
    let provider = ai::update(&mut tx, team.id, name, &spec, expected_version).await?;
    ai::mark_routes_stale_for_provider(&mut tx, team.id, provider.id).await?;
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
    let providers = load_route_providers(pool, team, &spec).await?;
    let materialized = materialized_names(name, &spec)?;
    let materialized_events =
        create_materialized(pool, team, name, &spec, &providers, &materialized).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create AI route: begin"))?;
    let route = match ai::create_route(&mut tx, team, name, &spec, &materialized).await {
        Ok(route) => route,
        Err(err) => {
            tx.rollback().await.ok();
            cleanup_materialized(pool, team, &materialized).await;
            return Err(err);
        }
    };
    append_materialized_upserts(
        &mut tx,
        team,
        &materialized_events.clusters,
        materialized_events.route_config_id,
        &materialized_events.route_config_name,
        materialized_events.listener_id,
        &materialized_events.listener_name,
    )
    .await?;
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

pub async fn usage_summary(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    query: ai::AiUsageQuery,
    request_id: RequestId,
) -> DomainResult<Vec<AiUsageSummary>> {
    authorize(pool, ctx, Resource::AiUsage, Action::Read, team, request_id).await?;
    ai::usage_summary(pool, team.id, query).await
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
    let current = get_route(pool, ctx, team, name, request_id).await?;
    if current.version != expected_version {
        return Err(revision_mismatch(
            "AI route",
            name,
            current.version,
            expected_version,
        ));
    }
    let providers = load_route_providers(pool, team, &spec).await?;
    let materialized = materialized_names(name, &spec)?;
    cleanup_materialized(pool, team, &current.materialized).await;
    create_materialized(pool, team, name, &spec, &providers, &materialized).await?;

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("update AI route: begin"))?;
    let route = match ai::update_route(
        &mut tx,
        team.id,
        name,
        &spec,
        &materialized,
        expected_version,
    )
    .await
    {
        Ok(route) => route,
        Err(err) => {
            tx.rollback().await.ok();
            cleanup_materialized(pool, team, &materialized).await;
            return Err(err);
        }
    };
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
    let route = get_route(pool, ctx, team, name, request_id).await?;
    if route.version != expected_version {
        return Err(DomainError::new(
            fp_domain::ErrorCode::RevisionMismatch,
            format!(
                "AI route \"{name}\" is at revision {}, you supplied {expected_version}",
                route.version
            ),
        ));
    }
    cleanup_materialized(pool, team, &route.materialized).await;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("delete AI route: begin"))?;
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

async fn load_route_providers(
    pool: &PgPool,
    team: TeamRef,
    spec: &AiRouteSpec,
) -> DomainResult<Vec<AiProvider>> {
    let mut providers = Vec::with_capacity(spec.backends.len());
    for backend in &spec.backends {
        let provider = ai::get_provider_by_id(pool, team.id, backend.provider_id)
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

async fn create_materialized(
    pool: &PgPool,
    team: TeamRef,
    route_name: &str,
    spec: &AiRouteSpec,
    providers: &[AiProvider],
    names: &AiRouteMaterializedResources,
) -> DomainResult<MaterializedResourceEvents> {
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
    let mut tx = pool.begin().await.map_err(crate::services::db_err(
        "create AI materialized resources: begin",
    ))?;
    let mut cluster_events = Vec::with_capacity(cluster_specs.len());
    let existing_clusters = fp_storage::repos::clusters::count_for_team(pool, team.id).await?;
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
            cluster_repo::create_ai_owned(&mut tx, team, owner_id, &cluster_name, &cluster_spec)
                .await?;
        cluster_events.push((cluster.id.as_uuid(), cluster.name));
    }
    let route_config = gateway_repo::create_ai_route_config(
        &mut tx,
        team,
        owner_id,
        &names.route_config_name,
        &route_config_spec,
    )
    .await?;
    let listener = gateway_repo::create_ai_listener(
        &mut tx,
        team,
        owner_id,
        &names.listener_name,
        &listener_spec,
    )
    .await?;
    append_materialized_upserts(
        &mut tx,
        team,
        &cluster_events,
        route_config.id.as_uuid(),
        &route_config.name,
        listener.id.as_uuid(),
        &listener.name,
    )
    .await?;
    tx.commit().await.map_err(crate::services::db_err(
        "create AI materialized resources: commit",
    ))?;
    Ok(MaterializedResourceEvents {
        clusters: cluster_events,
        route_config_id: route_config.id.as_uuid(),
        route_config_name: route_config.name,
        listener_id: listener.id.as_uuid(),
        listener_name: listener.name,
    })
}

struct MaterializedResourceEvents {
    clusters: Vec<(uuid::Uuid, String)>,
    route_config_id: uuid::Uuid,
    route_config_name: String,
    listener_id: uuid::Uuid,
    listener_name: String,
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

async fn cleanup_materialized(pool: &PgPool, team: TeamRef, names: &AiRouteMaterializedResources) {
    let Ok(mut tx) = pool.begin().await else {
        return;
    };
    if let Ok(Some(listener_id)) =
        gateway_repo::delete_ai_listener(&mut tx, team.id, &names.listener_name).await
    {
        let _ = append_gateway_event(
            &mut tx,
            team,
            DomainEvent::ListenerDeleted {
                listener_id: listener_id.as_uuid(),
                name: names.listener_name.clone(),
            },
        )
        .await;
    }
    if let Ok(Some(route_config_id)) =
        gateway_repo::delete_ai_route_config(&mut tx, team.id, &names.route_config_name).await
    {
        let _ = append_gateway_event(
            &mut tx,
            team,
            DomainEvent::RouteConfigDeleted {
                route_config_id: route_config_id.as_uuid(),
                name: names.route_config_name.clone(),
            },
        )
        .await;
    }
    for cluster_name in &names.cluster_names {
        if let Ok(Some(cluster_id)) =
            cluster_repo::delete_ai_owned(&mut tx, team.id, cluster_name).await
        {
            let _ = append_gateway_event(
                &mut tx,
                team,
                DomainEvent::ClusterDeleted {
                    cluster_id: cluster_id.as_uuid(),
                    name: cluster_name.clone(),
                },
            )
            .await;
        };
    }
    let _ = tx.commit().await;
}

fn provider_cluster_spec(provider: &AiProvider) -> DomainResult<ClusterSpec> {
    let url = Url::parse(&provider.spec.base_url)
        .map_err(|_| DomainError::validation("AI provider base_url must be a valid URL"))?;
    let host = url
        .host_str()
        .ok_or_else(|| DomainError::validation("AI provider base_url must include a host"))?
        .to_string();
    let use_tls = url.scheme() == "https";
    let port = url
        .port_or_known_default()
        .ok_or_else(|| DomainError::validation("AI provider base_url must include a port"))?;
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
            auto_sni_san_validation: true,
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
                body: Some(
                    r#"{"code":"no_eligible_ai_backend","message":"no eligible AI backend for requested model"}"#
                        .into(),
                ),
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
}
