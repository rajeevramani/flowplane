//! Atomic gateway shortcut. Association is membership/provenance, never xDS config.
use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{clusters, gateway, record_authz_denial};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::gateway::cluster::{Cluster, ClusterSpec, Endpoint, UpstreamTlsConfig};
pub use fp_domain::gateway::exposure::{
    ExposeEndpointSource, ExposeRequest, ExposedService, UnexposedService,
};
use fp_domain::gateway::exposure::{Exposure, ExposureMode, ResourceDisposition};
use fp_domain::gateway::listener::{Listener, ListenerProtocol, ListenerSpec};
use fp_domain::gateway::route_config::{
    PathMatch, RouteAction, RouteConfig, RouteConfigSpec, RouteRule, VirtualHost,
};
use fp_domain::{DomainError, DomainResult, ExposureId, RequestId};
use fp_storage::repos::{clusters as cluster_repo, exposures, gateway as gateway_repo};
use reqwest::Url;
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::BTreeSet;

const DEFAULT_PORT_START: u16 = 10_000;
const DEFAULT_PORT_END: u16 = 10_999;
const DEFAULT_PORT_ATTEMPTS: usize = (DEFAULT_PORT_END - DEFAULT_PORT_START + 1) as usize;

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

pub async fn expose(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request: ExposeRequest,
    request_id: RequestId,
    advisory: crate::services::egress_advisory::EgressAdvisoryPolicy,
) -> DomainResult<ExposedService> {
    expose_impl(pool, ctx, team, request, request_id, advisory)
        .await
        .map_err(exposures::shortcut_error)
}

async fn expose_impl(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request: ExposeRequest,
    request_id: RequestId,
    advisory: crate::services::egress_advisory::EgressAdvisoryPolicy,
) -> DomainResult<ExposedService> {
    // Full authorization precedes advisory DNS and every product write.
    for resource in [
        Resource::Clusters,
        Resource::RouteConfigs,
        Resource::Listeners,
    ] {
        authorize(pool, ctx, resource, Action::Create, team, request_id).await?;
    }
    if request.port.is_none() {
        authorize(
            pool,
            ctx,
            Resource::Listeners,
            Action::Read,
            team,
            request_id,
        )
        .await?;
    }
    fp_domain::validate_name(&request.name)?;
    let names = ExposeNames::new(&request.name);
    fp_domain::gateway::cluster::validate_cluster_name(&names.cluster)?;
    fp_domain::validate_name(&names.route_config)?;
    gateway::validate_user_listener_name(&names.listener)?;
    let upstream = parse_upstream(&request.upstream)?;
    let path = normalize_path(&request.path)?;
    let public_base_url = request.public_base_url.clone();
    // Endpoint rendering is validated before transaction, never after a successful commit.
    let curl_url = public_base_url
        .as_deref()
        .map(|base| expose_curl_url(base, &path))
        .transpose()?;
    let endpoint_source = if curl_url.is_some() {
        ExposeEndpointSource::ListenerPublicBaseUrl
    } else {
        ExposeEndpointSource::Unconfigured
    };
    listener_spec(
        &names,
        request.port.unwrap_or(DEFAULT_PORT_START),
        &public_base_url,
    )
    .validate()?;
    advisory
        .enforce_hosts(
            pool,
            ctx,
            request_id,
            team,
            "expose.create",
            &format!("expose/{}", request.name),
            vec![upstream.host.clone()],
        )
        .await?;
    let cluster_spec = ClusterSpec {
        aggregate_clusters: Vec::new(),
        endpoints: vec![Endpoint {
            host: upstream.host,
            port: upstream.port,
            weight: None,
        }],
        lb_policy: Default::default(),
        least_request: None,
        ring_hash: None,
        maglev: None,
        dns_lookup_family: None,
        connect_timeout_secs: 5,
        use_tls: upstream.use_tls,
        upstream_tls: upstream.use_tls.then_some(UpstreamTlsConfig {
            sni: Some(upstream.sni),
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
            name: "default".into(),
            domains: vec!["*".into()],
            routes: vec![RouteRule {
                name: request.name.clone(),
                matcher: PathMatch::Prefix {
                    prefix: path.clone(),
                },
                headers: Vec::new(),
                query_parameters: Vec::new(),
                action: RouteAction {
                    cluster: Some(names.cluster.clone()),
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
            }],
            rate_limits: Vec::new(),
            filter_overrides: Vec::new(),
        }],
    };
    let template = ExposeTemplate {
        names,
        cluster_spec,
        route_config_spec,
        public_base_url,
    };

    template.route_config_spec.validate()?;
    clusters::prepare_cluster_create(
        pool,
        ctx,
        team,
        &template.names.cluster,
        &template.cluster_spec,
        request_id,
        &advisory,
    )
    .await?;
    let mut attempted = BTreeSet::new();
    for _ in 0..DEFAULT_PORT_ATTEMPTS {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| exposures::db_error("expose begin", e))?;
        exposures::lock_team(&mut tx, team.id).await?;
        let port = match request.port {
            Some(port) => port,
            None => {
                let used = exposures::listener_ports(&mut tx, team.id).await?;
                (DEFAULT_PORT_START..=DEFAULT_PORT_END)
                    .find(|p| !attempted.contains(p) && !used.contains(&i32::from(*p)))
                    .ok_or_else(no_ports_available)?
            }
        };
        let result = create_resources_for_port(
            &mut tx,
            ctx,
            team,
            &template,
            &request.name,
            port,
            request_id,
        )
        .await;
        match result {
            Ok((cluster, route_config, listener)) => {
                tx.commit()
                    .await
                    .map_err(|e| exposures::db_error("expose commit", e))?;
                return Ok(ExposedService {
                    name: request.name,
                    upstream: request.upstream,
                    path,
                    port,
                    mode: ExposureMode::Created,
                    cluster,
                    route_config,
                    listener,
                    curl_url,
                    endpoint_source,
                });
            }
            Err(error) => {
                // Only a machine-identified auto-port race is replayed, after explicit rollback.
                tx.rollback()
                    .await
                    .map_err(|e| exposures::db_error("expose rollback", e))?;
                if request.port.is_none() && is_listener_port_conflict(&error) {
                    attempted.insert(port);
                    continue;
                }
                return Err(error);
            }
        }
    }
    Err(no_ports_available())
}

fn listener_spec(names: &ExposeNames, port: u16, public_base_url: &Option<String>) -> ListenerSpec {
    ListenerSpec {
        address: "0.0.0.0".into(),
        port,
        public_base_url: public_base_url.clone(),
        protocol: ListenerProtocol::Http,
        route_config: Some(names.route_config.clone()),
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_resources_for_port(
    tx: &mut Transaction<'_, Postgres>,
    ctx: &PrincipalCtx,
    team: TeamRef,
    template: &ExposeTemplate,
    name: &str,
    port: u16,
    request_id: RequestId,
) -> DomainResult<(Cluster, RouteConfig, Listener)> {
    for resource in [
        Resource::Clusters,
        Resource::RouteConfigs,
        Resource::Listeners,
    ] {
        crate::services::quota::check_team_resource_quota_in_tx(tx, team.id, resource).await?;
    }
    let cluster = clusters::create_cluster_in_tx(
        tx,
        ctx,
        team,
        &template.names.cluster,
        template.cluster_spec.clone(),
        request_id,
    )
    .await?;
    let route_config = gateway::create_route_config_in_tx(
        tx,
        ctx,
        team,
        &template.names.route_config,
        template.route_config_spec.clone(),
        request_id,
    )
    .await?;
    let listener = gateway::create_listener_in_tx(
        tx,
        ctx,
        team,
        &template.names.listener,
        listener_spec(&template.names, port, &template.public_base_url),
        request_id,
    )
    .await?;
    let now = chrono::Utc::now();
    exposures::create(
        tx,
        team,
        &Exposure {
            id: ExposureId::generate(),
            team_id: team.id,
            org_id: team.org_id,
            name: name.into(),
            cluster_id: cluster.id,
            route_config_id: route_config.id,
            listener_id: listener.id,
            virtual_host: "default".into(),
            route_name: name.into(),
            cleanup_listener: true,
            cleanup_route_config: true,
            version: 1,
            created_at: now,
            updated_at: now,
        },
    )
    .await?;
    Ok((cluster, route_config, listener))
}

pub async fn unexpose(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<UnexposedService> {
    unexpose_impl(pool, ctx, team, name, request_id)
        .await
        .map_err(exposures::shortcut_error)
}

async fn unexpose_impl(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<UnexposedService> {
    for resource in [
        Resource::Listeners,
        Resource::RouteConfigs,
        Resource::Clusters,
    ] {
        authorize(pool, ctx, resource, Action::Read, team, request_id).await?;
    }
    fp_domain::validate_name(name)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| exposures::db_error("unexpose begin", e))?;
    let exposure=exposures::get_for_update(&mut tx,team.id,name).await?.ok_or_else(|| DomainError::not_found("exposure",name).with_hint("no shortcut association exists; inspect legacy/manual listener, route-config and cluster with ordinary commands; names alone never permit cleanup, and existing dependencies may block ordinary deletion"))?;
    // Authoritative resource lock order: listener -> route config -> cluster.
    let listener = gateway_repo::get_listener_for_update(&mut tx, team.id, exposure.listener_id)
        .await?
        .ok_or_else(|| stale_exposure(&exposure, None, None, None))?;
    let route_config =
        gateway_repo::get_route_config_for_update(&mut tx, team.id, exposure.route_config_id)
            .await?
            .ok_or_else(|| stale_exposure(&exposure, Some(&listener), None, None))?;
    let cluster = cluster_repo::get_for_update(&mut tx, team.id, exposure.cluster_id)
        .await?
        .ok_or_else(|| stale_exposure(&exposure, Some(&listener), Some(&route_config), None))?;
    if listener.spec.route_config.as_deref() != Some(route_config.name.as_str()) {
        return Err(stale_exposure(
            &exposure,
            Some(&listener),
            Some(&route_config),
            Some(&cluster),
        ));
    }
    let mut remaining = route_config.spec.clone();
    let vhost_index = remaining
        .virtual_hosts
        .iter()
        .position(|v| v.name == exposure.virtual_host)
        .ok_or_else(|| {
            stale_exposure(
                &exposure,
                Some(&listener),
                Some(&route_config),
                Some(&cluster),
            )
        })?;
    let route_index = remaining.virtual_hosts[vhost_index]
        .routes
        .iter()
        .position(|r| r.name == exposure.route_name)
        .ok_or_else(|| {
            stale_exposure(
                &exposure,
                Some(&listener),
                Some(&route_config),
                Some(&cluster),
            )
        })?;
    let route = &remaining.virtual_hosts[vhost_index].routes[route_index];
    if route.action.cluster.as_deref() != Some(cluster.name.as_str())
        || route.action.weighted_clusters.is_some()
        || route.action.redirect.is_some()
        || route.action.direct_response.is_some()
    {
        return Err(stale_exposure(
            &exposure,
            Some(&listener),
            Some(&route_config),
            Some(&cluster),
        ));
    }
    let dependents = exposures::route_dependents(
        &mut tx,
        team.id,
        route_config.id,
        &exposure.virtual_host,
        Some(&exposure.route_name),
    )
    .await?;
    if !dependents.is_empty() {
        return Err(protected_exposure(name,&dependents,"stop active direct captures; delete associated API definitions through supported revision-checked commands when appropriate; stop/cancel does not erase historical scaffold FKs"));
    }
    remaining.virtual_hosts[vhost_index]
        .routes
        .remove(route_index);
    let other_routes = remaining.virtual_hosts.iter().any(|v| !v.routes.is_empty());
    if other_routes && remaining.virtual_hosts[vhost_index].routes.is_empty() {
        let vhost = &remaining.virtual_hosts[vhost_index];
        let dependents = exposures::route_dependents(
            &mut tx,
            team.id,
            route_config.id,
            &exposure.virtual_host,
            None,
        )
        .await?;
        if !vhost.rate_limits.is_empty()
            || !vhost.filter_overrides.is_empty()
            || !dependents.is_empty()
        {
            return Err(protected_exposure(name,&dependents,"empty virtual host still has policy or live dependencies; restore/add a genuine route through ordinary route-config update"));
        }
        remaining.virtual_hosts.remove(vhost_index);
    }
    if !other_routes {
        let dependents = exposures::scaffold_dependents(&mut tx, team.id, &exposure).await?;
        if !exposure.cleanup_listener || !exposure.cleanup_route_config || !dependents.is_empty() {
            return Err(protected_exposure(name,&dependents,"last-route infrastructure cannot be shortcut-deleted; inspect and add a genuine unrelated route before retrying route-only removal; stopping a capture does not erase historical FKs"));
        }
    } else {
        remaining.validate()?;
    }
    // Branch-specific complete grants are decided under locks, before any product write.
    authorize(
        pool,
        ctx,
        Resource::Clusters,
        Action::Delete,
        team,
        request_id,
    )
    .await?;
    if other_routes {
        authorize(
            pool,
            ctx,
            Resource::RouteConfigs,
            Action::Update,
            team,
            request_id,
        )
        .await?;
    } else {
        authorize(
            pool,
            ctx,
            Resource::Listeners,
            Action::Delete,
            team,
            request_id,
        )
        .await?;
        authorize(
            pool,
            ctx,
            Resource::RouteConfigs,
            Action::Delete,
            team,
            request_id,
        )
        .await?;
    }
    exposures::delete(&mut tx, team.id, &exposure).await?;
    let disposition = if other_routes {
        gateway::update_route_config_in_tx(
            &mut tx,
            ctx,
            team,
            &route_config.name,
            remaining,
            route_config.version,
            request_id,
        )
        .await?;
        ResourceDisposition::Retained
    } else {
        gateway::delete_listener_in_tx(
            &mut tx,
            ctx,
            team,
            &listener.name,
            listener.version,
            request_id,
        )
        .await?;
        gateway::delete_route_config_in_tx(
            &mut tx,
            ctx,
            team,
            &route_config.name,
            route_config.version,
            request_id,
        )
        .await?;
        ResourceDisposition::Deleted
    };
    let dependents =
        exposures::surviving_cluster_dependents(&mut tx, team.id, &cluster.name).await?;
    if !dependents.is_empty() {
        return Err(protected_exposure(name,&dependents,"remove surviving upstream references through ordinary gateway authoring, then retry; the whole shortcut transaction rolls back"));
    }
    clusters::delete_cluster_in_tx(
        &mut tx,
        ctx,
        team,
        &cluster.name,
        cluster.version,
        request_id,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|e| exposures::db_error("unexpose commit", e))?;
    Ok(UnexposedService {
        name: name.into(),
        cluster_name: cluster.name,
        route_config_name: route_config.name,
        listener_name: listener.name,
        cluster_disposition: ResourceDisposition::Deleted,
        route_config_disposition: disposition,
        listener_disposition: disposition,
    })
}

fn protected_exposure(name: &str, dependents: &[String], hint: &str) -> DomainError {
    DomainError::conflict(format!(
        "exposure {name} has protected state or dependents: {}",
        dependents.join(", ")
    ))
    .with_hint(hint)
}
fn stale_exposure(
    e: &Exposure,
    listener: Option<&Listener>,
    config: Option<&RouteConfig>,
    cluster: Option<&Cluster>,
) -> DomainError {
    DomainError::conflict(format!("exposure {} has stale ownership; expected listener {} -> config {}, virtual host {}, route {} -> upstream {}",e.name,listener.map(|l|l.name.clone()).unwrap_or_else(||e.listener_id.to_string()),config.map(|r|r.name.clone()).unwrap_or_else(||e.route_config_id.to_string()),e.virtual_host,e.route_name,cluster.map(|c|c.name.clone()).unwrap_or_else(||e.cluster_id.to_string())))
        .with_hint(format!("inspect with listener get, route-config get and cluster get; restore route '{}' with its exact direct upstream and the listener->config binding using ordinary revision-checked updates, then retry unexpose {}",e.route_name,e.name))
}
fn no_ports_available() -> DomainError {
    DomainError::conflict(format!(
        "no listener ports available in {DEFAULT_PORT_START}-{DEFAULT_PORT_END}"
    ))
    .with_hint("pass --port with an available listener port")
}
fn is_listener_port_conflict(error: &DomainError) -> bool {
    error
        .details
        .as_ref()
        .and_then(|d| d.get("conflict_kind"))
        .and_then(serde_json::Value::as_str)
        == Some("listener_port")
}

#[derive(Debug)]
struct ExposeNames {
    listener: String,
    route_config: String,
    cluster: String,
}

#[derive(Debug)]
struct ExposeTemplate {
    names: ExposeNames,
    cluster_spec: ClusterSpec,
    route_config_spec: RouteConfigSpec,
    public_base_url: Option<String>,
}

impl ExposeNames {
    fn new(name: &str) -> Self {
        Self {
            listener: name.into(),
            route_config: format!("{name}-routes"),
            cluster: format!("{name}-upstream"),
        }
    }
}

#[derive(Debug)]
struct ParsedUpstream {
    host: String,
    port: u16,
    use_tls: bool,
    sni: String,
}

fn parse_upstream(raw: &str) -> DomainResult<ParsedUpstream> {
    let url = Url::parse(raw)
        .map_err(|e| DomainError::validation(format!("upstream must be an absolute URL: {e}")))?;
    let scheme = url.scheme();
    let use_tls = match scheme {
        "http" => false,
        "https" => true,
        _ => {
            return Err(DomainError::validation(
                "upstream scheme must be http or https",
            ))
        }
    };
    if !url.username().is_empty() || url.password().is_some() {
        return Err(DomainError::validation(
            "upstream URL must not contain credentials",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| DomainError::validation("upstream URL must include a host"))?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| DomainError::validation("upstream URL must include a port"))?;
    Ok(ParsedUpstream {
        sni: host.clone(),
        host,
        port,
        use_tls,
    })
}

fn normalize_path(path: &str) -> DomainResult<String> {
    let path = path.trim();
    if path.is_empty() {
        return Ok("/".into());
    }
    if !path.starts_with('/') || path.contains("..") || path.contains('\0') || path.len() > 500 {
        return Err(DomainError::validation(
            "path must start with '/', contain no '..' or NUL, and be <= 500 chars",
        ));
    }
    Ok(path.into())
}

fn expose_curl_url(public_base_url: &str, path: &str) -> DomainResult<String> {
    let mut url = Url::parse(public_base_url.trim_end_matches('/'))
        .map_err(|e| DomainError::validation(format!("invalid listener public_base_url: {e}")))?;
    url.set_path(path);
    Ok(url.to_string())
}
