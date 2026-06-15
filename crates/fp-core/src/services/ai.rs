//! AI gateway services.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, clusters, deny_to_error, gateway, record_authz_denial};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, UpstreamTlsConfig};
use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec};
use fp_domain::gateway::route_config::{
    DirectResponseAction, HeaderMatch, HeaderValueMatch, PathMatch, RouteAction, RouteConfigSpec,
    RouteRule, VirtualHost, WeightedClusterTarget,
};
use fp_domain::{
    validate_ai_provider_name, validate_ai_route_name, AiProvider, AiProviderSpec, AiRoute,
    AiRouteMaterializedResources, AiRouteSpec, DomainError, DomainResult, RequestId,
    AI_MODEL_HEADER, DEFAULT_AI_ROUTE_TIMEOUT_SECS,
};
use fp_storage::repos::{ai, audit};
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
    let materialized = materialized_names(name, spec.backends.len());
    create_materialized(
        pool,
        ctx,
        team,
        &spec,
        &providers,
        &materialized,
        request_id,
    )
    .await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("create AI route: begin"))?;
    let route = match ai::create_route(&mut tx, team, name, &spec, &materialized).await {
        Ok(route) => route,
        Err(err) => {
            tx.rollback().await.ok();
            cleanup_materialized(pool, ctx, team, &materialized, request_id).await;
            return Err(err);
        }
    };
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
    let materialized = materialized_names(name, spec.backends.len());
    cleanup_materialized(pool, ctx, team, &current.materialized, request_id).await;
    create_materialized(
        pool,
        ctx,
        team,
        &spec,
        &providers,
        &materialized,
        request_id,
    )
    .await?;

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
            cleanup_materialized(pool, ctx, team, &materialized, request_id).await;
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
    cleanup_materialized(pool, ctx, team, &route.materialized, request_id).await;
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

fn materialized_names(name: &str, backend_count: usize) -> AiRouteMaterializedResources {
    AiRouteMaterializedResources {
        cluster_names: (0..backend_count)
            .map(|idx| generated_name(name, &format!("-b{}", idx + 1)))
            .collect(),
        route_config_name: generated_name(name, "-routes"),
        listener_name: generated_name(name, "-listener"),
    }
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
    ctx: &PrincipalCtx,
    team: TeamRef,
    spec: &AiRouteSpec,
    providers: &[AiProvider],
    names: &AiRouteMaterializedResources,
    request_id: RequestId,
) -> DomainResult<()> {
    for (provider, cluster_name) in providers.iter().zip(names.cluster_names.iter()) {
        let cluster_spec = match provider_cluster_spec(provider) {
            Ok(spec) => spec,
            Err(err) => {
                cleanup_materialized(pool, ctx, team, names, request_id).await;
                return Err(err);
            }
        };
        if let Err(err) =
            clusters::create_cluster(pool, ctx, team, cluster_name, cluster_spec, request_id).await
        {
            cleanup_materialized(pool, ctx, team, names, request_id).await;
            return Err(err);
        }
    }
    let route_config_spec = ai_route_config_spec(spec, providers, names)?;
    if let Err(err) = gateway::create_route_config(
        pool,
        ctx,
        team,
        &names.route_config_name,
        route_config_spec,
        request_id,
    )
    .await
    {
        cleanup_materialized(pool, ctx, team, names, request_id).await;
        return Err(err);
    }
    let listener_spec = ListenerSpec {
        address: "0.0.0.0".into(),
        port: spec.listener_port,
        protocol: ListenerProtocol::Http,
        route_config: Some(names.route_config_name.clone()),
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: None,
    };
    if let Err(err) = gateway::create_listener(
        pool,
        ctx,
        team,
        &names.listener_name,
        listener_spec,
        request_id,
    )
    .await
    {
        cleanup_materialized(pool, ctx, team, names, request_id).await;
        return Err(err);
    }
    Ok(())
}

async fn cleanup_materialized(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    names: &AiRouteMaterializedResources,
    request_id: RequestId,
) {
    if let Ok(listener) =
        gateway::get_listener(pool, ctx, team, &names.listener_name, request_id).await
    {
        let _ = gateway::delete_listener(
            pool,
            ctx,
            team,
            &names.listener_name,
            listener.version,
            request_id,
        )
        .await;
    }
    if let Ok(route_config) =
        gateway::get_route_config(pool, ctx, team, &names.route_config_name, request_id).await
    {
        let _ = gateway::delete_route_config(
            pool,
            ctx,
            team,
            &names.route_config_name,
            route_config.version,
            request_id,
        )
        .await;
    }
    for cluster_name in &names.cluster_names {
        if let Ok(cluster) = clusters::get_cluster(pool, ctx, team, cluster_name, request_id).await
        {
            let _ = clusters::delete_cluster(
                pool,
                ctx,
                team,
                cluster_name,
                cluster.version,
                request_id,
            )
            .await;
        }
    }
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

fn ai_route_config_spec(
    spec: &AiRouteSpec,
    providers: &[AiProvider],
    names: &AiRouteMaterializedResources,
) -> DomainResult<RouteConfigSpec> {
    let prefix_rewrite = common_prefix(providers)?;
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
    let mut routes = Vec::new();
    for (model, indexes) in by_model {
        routes.push(route_rule(
            &format!("model-{}", route_token(&model)),
            &spec.path,
            vec![HeaderMatch {
                name: AI_MODEL_HEADER.into(),
                invert_match: false,
                matcher: HeaderValueMatch::Exact { value: model },
            }],
            &indexes,
            spec,
            names,
            prefix_rewrite.clone(),
        )?);
    }
    if !catch_all.is_empty() {
        routes.push(route_rule(
            "default",
            &spec.path,
            Vec::new(),
            &catch_all,
            spec,
            names,
            prefix_rewrite,
        )?);
    } else {
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
    indexes: &[usize],
    spec: &AiRouteSpec,
    names: &AiRouteMaterializedResources,
    prefix_rewrite: Option<String>,
) -> DomainResult<RouteRule> {
    let targets = indexes
        .iter()
        .map(|idx| WeightedClusterTarget {
            cluster: names.cluster_names[*idx].clone(),
            weight: spec.backends[*idx].weight,
        })
        .collect::<Vec<_>>();
    let (cluster, weighted_clusters) = match targets.as_slice() {
        [single] => (Some(single.cluster.clone()), None),
        _ => (None, Some(targets)),
    };
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
            prefix_rewrite,
            template_rewrite: None,
            timeout_secs: DEFAULT_AI_ROUTE_TIMEOUT_SECS,
            retry_policy: None,
            rate_limits: Vec::new(),
        },
        filter_overrides: Vec::new(),
    })
}

fn common_prefix(providers: &[AiProvider]) -> DomainResult<Option<String>> {
    let prefix = providers
        .first()
        .and_then(|provider| provider.spec.path_prefix.clone());
    if providers
        .iter()
        .any(|provider| provider.spec.path_prefix != prefix)
    {
        return Err(DomainError::validation(
            "AI route backends must share the same path_prefix setting",
        ));
    }
    Ok(prefix)
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
