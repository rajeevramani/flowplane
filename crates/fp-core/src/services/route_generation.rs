//! S9 route generation plans: dry-run persists concrete gateway specs; apply replays them.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{clusters, gateway, record_authz_denial};
use fp_domain::api_lifecycle::{SpecReviewDecision, SpecSourceKind};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::gateway::cluster::{Cluster, ClusterSpec, Endpoint, UpstreamTlsConfig};
use fp_domain::gateway::listener::{Listener, ListenerProtocol, ListenerSpec};
use fp_domain::gateway::route_config::{
    HeaderMatch, HeaderValueMatch, PathMatch, RouteAction, RouteConfig, RouteConfigSpec, RouteRule,
    VirtualHost,
};
use fp_domain::{
    ApiDefinitionId, DomainError, DomainResult, RequestId, RouteGenerationPlan,
    RouteGenerationPlanId, RouteGenerationPlanSpec, RouteGenerationPlanStatus, SpecVersionId,
};
use fp_storage::repos::{api_lifecycle, route_generation};
use sqlx::PgPool;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct CreateRoutePlanInput {
    pub spec_version_id: SpecVersionId,
    pub listener_port: u16,
}

#[derive(Debug, Clone)]
pub struct AppliedRoutePlan {
    pub plan: RouteGenerationPlan,
    pub cluster: Cluster,
    pub route_config: RouteConfig,
    pub listener: Listener,
}

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
            Err(crate::services::deny_to_error(resource, action, reason))
        }
    }
}

pub async fn create_plan(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    input: CreateRoutePlanInput,
    request_id: RequestId,
) -> DomainResult<RouteGenerationPlan> {
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
        Resource::Clusters,
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
    let mut tx = pool.begin().await.map_err(crate::services::db_err(
        "create route generation plan: begin",
    ))?;
    let spec =
        api_lifecycle::get_spec_version_by_id(&mut tx, team.id, input.spec_version_id).await?;
    if spec.source_kind != SpecSourceKind::Learned {
        return Err(DomainError::conflict(
            "route generation requires a learned spec version",
        ));
    }
    let api =
        ensure_spec_still_approved(pool, &mut tx, team, spec.api_definition_id, spec.id).await?;
    let mut plan = build_plan(
        &api.name,
        spec.api_definition_id,
        &spec.spec,
        input.listener_port,
    )?;
    plan.conflicts = detect_conflicts(pool, ctx, team, &plan, request_id).await?;
    let persisted = route_generation::create(&mut tx, team, spec.id, &plan).await?;
    tx.commit().await.map_err(crate::services::db_err(
        "create route generation plan: commit",
    ))?;
    Ok(persisted)
}

pub async fn apply_plan(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    plan_id: RouteGenerationPlanId,
    request_id: RequestId,
    advisory: crate::services::egress_advisory::EgressAdvisoryPolicy,
) -> DomainResult<AppliedRoutePlan> {
    // Authorize the full apply up front (it creates a cluster, route config, and listener) —
    // an unauthorized caller must get the authz denial, not advisory side effects.
    for resource in [
        Resource::Clusters,
        Resource::RouteConfigs,
        Resource::Listeners,
    ] {
        authorize(pool, ctx, resource, Action::Create, team, request_id).await?;
    }
    let plan = route_generation::get(pool, team.id, plan_id)
        .await?
        .ok_or_else(|| DomainError::not_found("route generation plan", &plan_id.to_string()))?;
    if plan.status != RouteGenerationPlanStatus::DryRun {
        return Err(DomainError::conflict(
            "route generation plan has already been applied",
        ));
    }
    if !plan.plan.conflicts.is_empty() {
        return Err(DomainError::conflict(
            "route generation plan has blocking conflicts",
        ));
    }
    // Path-specific egress advisory (fpv2-1hp.4): a learned/generated hostname is checked with
    // the route-generation mutation label before anything is applied; the inner cluster create
    // re-checks as defense-in-depth. The policy comes from ServerConfig via AppState — never
    // from call-site env reads (finding 13/A5 satisfied by construction).
    advisory
        .enforce_hosts(
            pool,
            ctx,
            request_id,
            team,
            "route_generation.apply",
            &format!("route-plans/{plan_id}"),
            plan.plan
                .cluster_spec
                .endpoints
                .iter()
                .map(|e| e.host.clone())
                .collect(),
        )
        .await?;
    let mut tx = pool.begin().await.map_err(crate::services::db_err(
        "validate route generation plan approval: begin",
    ))?;
    ensure_spec_still_approved(
        pool,
        &mut tx,
        team,
        plan.plan.api_definition_id,
        plan.spec_version_id,
    )
    .await?;
    tx.commit().await.map_err(crate::services::db_err(
        "validate route generation plan approval: commit",
    ))?;

    let cluster = clusters::create_cluster(
        pool,
        ctx,
        team,
        &plan.plan.cluster_name,
        plan.plan.cluster_spec.clone(),
        request_id,
        advisory.clone(),
    )
    .await?;
    let route_config = match gateway::create_route_config(
        pool,
        ctx,
        team,
        &plan.plan.route_config_name,
        plan.plan.route_config_spec.clone(),
        request_id,
    )
    .await
    {
        Ok(route_config) => route_config,
        Err(err) => {
            let _ = clusters::delete_cluster(
                pool,
                ctx,
                team,
                &cluster.name,
                cluster.version,
                request_id,
            )
            .await;
            return Err(err);
        }
    };
    let listener = match gateway::create_listener(
        pool,
        ctx,
        team,
        &plan.plan.listener_name,
        plan.plan.listener_spec.clone(),
        request_id,
        // Generated listeners carry no http_filters (see listener_spec below), so the
        // global_rate_limit reference check is a no-op regardless of RLS configuration.
        false,
    )
    .await
    {
        Ok(listener) => listener,
        Err(err) => {
            let _ = gateway::delete_route_config(
                pool,
                ctx,
                team,
                &route_config.name,
                route_config.version,
                request_id,
            )
            .await;
            let _ = clusters::delete_cluster(
                pool,
                ctx,
                team,
                &cluster.name,
                cluster.version,
                request_id,
            )
            .await;
            return Err(err);
        }
    };

    let mut tx = pool.begin().await.map_err(crate::services::db_err(
        "mark route generation plan applied: begin",
    ))?;
    let applied = route_generation::mark_applied(&mut tx, team.id, plan.id).await?;
    tx.commit().await.map_err(crate::services::db_err(
        "mark route generation plan applied: commit",
    ))?;
    Ok(AppliedRoutePlan {
        plan: applied,
        cluster,
        route_config,
        listener,
    })
}

async fn ensure_spec_still_approved(
    pool: &PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    api_definition_id: ApiDefinitionId,
    spec_version_id: SpecVersionId,
) -> DomainResult<fp_domain::api_lifecycle::ApiDefinition> {
    let api = api_lifecycle::get_api_definition_by_id(pool, team.id, api_definition_id)
        .await?
        .ok_or_else(|| DomainError::not_found("api", &api_definition_id.to_string()))?;
    let latest = api_lifecycle::latest_spec_review_decision(tx, team.id, spec_version_id).await?;
    if api.published_spec_version_id == Some(spec_version_id)
        || latest == Some(SpecReviewDecision::Reviewed)
    {
        return Ok(api);
    }
    Err(DomainError::conflict(
        "route generation requires a currently reviewed or published learned spec version",
    ))
}

fn build_plan(
    api_name: &str,
    api_definition_id: ApiDefinitionId,
    spec: &serde_json::Value,
    listener_port: u16,
) -> DomainResult<RouteGenerationPlanSpec> {
    let source = spec
        .get("x-flowplane-learning-source")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| DomainError::validation("learned spec is missing discovery provenance"))?;
    let upstream_host = string_field(source, "forwarded_upstream_host")?;
    let upstream_port = int_field(source, "forwarded_upstream_port")?;
    let upstream_tls = bool_field(source, "forwarded_upstream_tls")?;
    let observed_host = source
        .get("observed_host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("*");
    let cluster_name = format!("{api_name}-upstream");
    let route_config_name = format!("{api_name}-routes");
    let listener_name = api_name.to_string();
    let cluster_spec = ClusterSpec {
        aggregate_clusters: Vec::new(),
        endpoints: vec![Endpoint {
            host: upstream_host.to_string(),
            port: upstream_port,
            weight: None,
        }],
        lb_policy: Default::default(),
        least_request: None,
        ring_hash: None,
        maglev: None,
        dns_lookup_family: None,
        connect_timeout_secs: 5,
        use_tls: upstream_tls,
        upstream_tls: upstream_tls.then_some(UpstreamTlsConfig {
            sni: Some(upstream_host.to_string()),
            validation_context_sds_secret_name: None,
            ca_cert_file: None,
            auto_sni_san_validation: true,
            insecure_skip_verify: false,
        }),
        protocol: None,
        health_checks: None,
        circuit_breakers: None,
        outlier_detection: None,
    };
    let route_config_spec = RouteConfigSpec {
        virtual_hosts: vec![VirtualHost {
            name: normalize_name(if observed_host == "*" {
                "wildcard"
            } else {
                observed_host
            }),
            domains: vec![observed_host.to_string()],
            routes: openapi_routes(spec, &cluster_name)?,
            rate_limits: Vec::new(),
            filter_overrides: Vec::new(),
        }],
    };
    let listener_spec = ListenerSpec {
        address: "0.0.0.0".into(),
        port: listener_port,
        public_base_url: None,
        protocol: ListenerProtocol::Http,
        route_config: Some(route_config_name.clone()),
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: None,
    };
    cluster_spec.validate()?;
    route_config_spec.validate()?;
    listener_spec.validate()?;
    Ok(RouteGenerationPlanSpec {
        api_definition_id,
        api_name: api_name.into(),
        cluster_name,
        route_config_name,
        listener_name,
        listener_port,
        cluster_spec,
        route_config_spec,
        listener_spec,
        conflicts: Vec::new(),
        metadata: serde_json::json!({
            "observed_host": observed_host,
            "forwarded_upstream_host": upstream_host,
            "forwarded_upstream_port": upstream_port,
            "forwarded_upstream_tls": upstream_tls,
        }),
    })
}

async fn detect_conflicts(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    plan: &RouteGenerationPlanSpec,
    request_id: RequestId,
) -> DomainResult<Vec<String>> {
    let mut conflicts = Vec::new();
    if clusters::get_cluster(pool, ctx, team, &plan.cluster_name, request_id)
        .await
        .is_ok()
    {
        conflicts.push(format!("cluster \"{}\" already exists", plan.cluster_name));
    }
    if gateway::get_route_config(pool, ctx, team, &plan.route_config_name, request_id)
        .await
        .is_ok()
    {
        conflicts.push(format!(
            "route config \"{}\" already exists",
            plan.route_config_name
        ));
    }
    if gateway::get_listener(pool, ctx, team, &plan.listener_name, request_id)
        .await
        .is_ok()
    {
        conflicts.push(format!(
            "listener \"{}\" already exists",
            plan.listener_name
        ));
    }
    let (listeners, _) = gateway::list_listeners(pool, ctx, team, 500, 0, request_id).await?;
    if listeners
        .iter()
        .any(|listener| listener.spec.port == plan.listener_port)
    {
        conflicts.push(format!(
            "listener port {} is already in use",
            plan.listener_port
        ));
    }
    Ok(conflicts)
}

fn openapi_routes(spec: &serde_json::Value, cluster_name: &str) -> DomainResult<Vec<RouteRule>> {
    let paths = spec
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| DomainError::validation("OpenAPI spec must contain paths object"))?;
    let mut operations = BTreeMap::new();
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for method in [
            "delete", "get", "head", "options", "patch", "post", "put", "trace",
        ] {
            if let Some(operation) = item.get(method) {
                let operation_id = operation
                    .get("operationId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                operations.insert((path.clone(), method.to_string()), operation_id.to_string());
            }
        }
    }
    if operations.is_empty() {
        return Err(DomainError::validation(
            "OpenAPI spec must contain at least one operation",
        ));
    }
    let mut names = BTreeSet::new();
    let mut routes = Vec::new();
    for ((path, method), operation_id) in operations {
        let base = if operation_id.is_empty() {
            normalize_name(&format!("{method}-{path}"))
        } else {
            normalize_name(&operation_id)
        };
        routes.push(RouteRule {
            name: unique_name(base, &mut names),
            matcher: PathMatch::Template { template: path },
            headers: vec![HeaderMatch {
                name: ":method".into(),
                invert_match: false,
                matcher: HeaderValueMatch::Exact {
                    value: method.to_ascii_uppercase(),
                },
            }],
            query_parameters: Vec::new(),
            action: RouteAction {
                cluster: Some(cluster_name.into()),
                weighted_clusters: None,
                redirect: None,
                direct_response: None,
                prefix_rewrite: None,
                template_rewrite: None,
                timeout_secs: 15,
                retry_policy: None,
                rate_limits: Vec::new(),
            },
            filter_overrides: Vec::new(),
        });
    }
    Ok(routes)
}

fn unique_name(base: String, names: &mut BTreeSet<String>) -> String {
    if names.insert(base.clone()) {
        return base;
    }
    for suffix in 2.. {
        let mut candidate = base.clone();
        candidate.truncate(95);
        candidate.push('-');
        candidate.push_str(&suffix.to_string());
        if names.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn normalize_name(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    let mut name = if trimmed.is_empty() {
        "generated".to_string()
    } else {
        trimmed.to_string()
    };
    if !name.as_bytes()[0].is_ascii_lowercase() {
        name.insert_str(0, "r-");
    }
    name.truncate(100);
    name.trim_end_matches('-').to_string()
}

fn string_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> DomainResult<&'a str> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| DomainError::validation(format!("discovery provenance missing {field}")))
}

fn int_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> DomainResult<u16> {
    let value = object
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| DomainError::validation(format!("discovery provenance missing {field}")))?;
    u16::try_from(value)
        .map_err(|_| DomainError::validation(format!("discovery provenance {field} is invalid")))
}

fn bool_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> DomainResult<bool> {
    object
        .get(field)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| DomainError::validation(format!("discovery provenance missing {field}")))
}
