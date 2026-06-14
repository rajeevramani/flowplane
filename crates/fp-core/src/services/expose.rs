//! Expose shortcut: orchestrates the existing cluster, route-config, and listener services.
//! This is intentionally not a second resource model; the durable state remains the three
//! gateway resources that xDS already understands.

use crate::authz::PrincipalCtx;
use crate::services::{clusters, gateway};
use fp_domain::authz::TeamRef;
use fp_domain::gateway::cluster::{Cluster, ClusterSpec, Endpoint, UpstreamTlsConfig};
use fp_domain::gateway::listener::{Listener, ListenerProtocol, ListenerSpec};
use fp_domain::gateway::route_config::{
    PathMatch, RouteAction, RouteConfig, RouteConfigSpec, RouteRule, VirtualHost,
};
use fp_domain::{DomainError, DomainResult, RequestId};
use reqwest::Url;
use sqlx::PgPool;
use std::collections::BTreeSet;

const DEFAULT_PORT_START: u16 = 10_000;
const DEFAULT_PORT_END: u16 = 10_999;

#[derive(Debug, Clone)]
pub struct ExposeRequest {
    pub name: String,
    pub upstream: String,
    pub path: String,
    pub port: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct ExposedService {
    pub name: String,
    pub upstream: String,
    pub path: String,
    pub port: u16,
    pub cluster: Cluster,
    pub route_config: RouteConfig,
    pub listener: Listener,
    pub curl_url: String,
}

#[derive(Debug, Clone)]
pub struct UnexposedService {
    pub name: String,
    pub cluster_name: String,
    pub route_config_name: String,
    pub listener_name: String,
}

pub async fn expose(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request: ExposeRequest,
    request_id: RequestId,
) -> DomainResult<ExposedService> {
    fp_domain::validate_name(&request.name)?;
    let upstream = parse_upstream(&request.upstream)?;
    let path = normalize_path(&request.path)?;
    let port = match request.port {
        Some(port) => port,
        None => allocate_port(pool, ctx, team, request_id).await?,
    };
    let names = ExposeNames::new(&request.name);

    let cluster_spec = ClusterSpec {
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
            auto_sni_san_validation: true,
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
                name: "all".into(),
                matcher: PathMatch::Prefix {
                    prefix: path.clone(),
                },
                headers: Vec::new(),
                query_parameters: Vec::new(),
                action: RouteAction {
                    cluster: Some(names.cluster.clone()),
                    weighted_clusters: None,
                    redirect: None,
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
    let listener_spec = ListenerSpec {
        address: "0.0.0.0".into(),
        port,
        protocol: ListenerProtocol::Http,
        route_config: Some(names.route_config.clone()),
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: None,
    };

    let cluster =
        clusters::create_cluster(pool, ctx, team, &names.cluster, cluster_spec, request_id).await?;
    let route_config = match gateway::create_route_config(
        pool,
        ctx,
        team,
        &names.route_config,
        route_config_spec,
        request_id,
    )
    .await
    {
        Ok(route_config) => route_config,
        Err(err) => {
            cleanup_cluster(pool, ctx, team, &cluster, request_id).await;
            return Err(err);
        }
    };
    let listener =
        match gateway::create_listener(pool, ctx, team, &names.listener, listener_spec, request_id)
            .await
        {
            Ok(listener) => listener,
            Err(err) => {
                cleanup_route_config(pool, ctx, team, &route_config, request_id).await;
                cleanup_cluster(pool, ctx, team, &cluster, request_id).await;
                return Err(err);
            }
        };

    Ok(ExposedService {
        name: request.name,
        upstream: request.upstream,
        path: path.clone(),
        port,
        cluster,
        route_config,
        listener,
        curl_url: format!("http://127.0.0.1:{port}{path}"),
    })
}

pub async fn unexpose(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    name: &str,
    request_id: RequestId,
) -> DomainResult<UnexposedService> {
    fp_domain::validate_name(name)?;
    let names = ExposeNames::new(name);

    let listener = gateway::get_listener(pool, ctx, team, &names.listener, request_id).await?;
    gateway::delete_listener(
        pool,
        ctx,
        team,
        &names.listener,
        listener.version,
        request_id,
    )
    .await?;

    let route_config =
        gateway::get_route_config(pool, ctx, team, &names.route_config, request_id).await?;
    gateway::delete_route_config(
        pool,
        ctx,
        team,
        &names.route_config,
        route_config.version,
        request_id,
    )
    .await?;

    let cluster = clusters::get_cluster(pool, ctx, team, &names.cluster, request_id).await?;
    clusters::delete_cluster(pool, ctx, team, &names.cluster, cluster.version, request_id).await?;

    Ok(UnexposedService {
        name: name.into(),
        cluster_name: names.cluster,
        route_config_name: names.route_config,
        listener_name: names.listener,
    })
}

async fn allocate_port(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<u16> {
    let (listeners, _) = gateway::list_listeners(pool, ctx, team, 500, 0, request_id).await?;
    let used = listeners
        .into_iter()
        .map(|listener| listener.spec.port)
        .collect::<BTreeSet<_>>();
    (DEFAULT_PORT_START..=DEFAULT_PORT_END)
        .find(|port| !used.contains(port))
        .ok_or_else(|| {
            DomainError::conflict(format!(
                "no listener ports available in {DEFAULT_PORT_START}-{DEFAULT_PORT_END}"
            ))
            .with_hint("pass --port with an available listener port")
        })
}

async fn cleanup_route_config(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    route_config: &RouteConfig,
    request_id: RequestId,
) {
    let _ = gateway::delete_route_config(
        pool,
        ctx,
        team,
        &route_config.name,
        route_config.version,
        request_id,
    )
    .await;
}

async fn cleanup_cluster(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    cluster: &Cluster,
    request_id: RequestId,
) {
    let _ =
        clusters::delete_cluster(pool, ctx, team, &cluster.name, cluster.version, request_id).await;
}

#[derive(Debug)]
struct ExposeNames {
    listener: String,
    route_config: String,
    cluster: String,
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
