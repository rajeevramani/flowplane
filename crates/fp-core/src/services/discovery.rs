//! Traffic-first discovery lifecycle.

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::{actor_of, deny_to_error, record_authz_denial, trace_context_json};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::discovery::{DiscoverySession, DiscoverySessionSpec, DiscoverySessionStatus};
use fp_domain::event::{DomainEvent, EventScope};
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy, UpstreamTlsConfig};
use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec};
use fp_domain::gateway::route_config::{
    PathMatch, RouteAction, RouteConfigSpec, RouteRule, VirtualHost,
};
use fp_domain::{validate_name, DiscoverySessionId, DomainError, DomainResult, RequestId};
use fp_storage::repos::{audit, clusters, discovery, gateway};
use sqlx::PgPool;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Debug, Clone)]
pub struct StartDiscoveryInput {
    pub name: String,
    pub spec: DiscoverySessionSpec,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoveryForwardingPolicy {
    denied_destinations: Vec<SocketAddr>,
}

impl DiscoveryForwardingPolicy {
    pub fn new(denied_destinations: Vec<SocketAddr>) -> Self {
        Self {
            denied_destinations: denied_destinations
                .into_iter()
                .map(|addr| SocketAddr::new(canonical_ip(addr.ip()), addr.port()))
                .collect(),
        }
    }

    pub async fn from_server_config(config: &crate::config::ServerConfig) -> Self {
        let mut denied = vec![config.api_addr, config.xds_addr];
        if let Some((host, port)) = postgres_host_port(&config.database_url) {
            if let Ok(addrs) = tokio::net::lookup_host((host.as_str(), port)).await {
                denied.extend(addrs);
            }
        }
        Self::new(denied)
    }
}

fn postgres_host_port(database_url: &str) -> Option<(String, u16)> {
    let authority = database_url.split_once("://")?.1.split('/').next()?;
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, hp)| hp)
        .unwrap_or(authority);
    let (host, port) = host_port.rsplit_once(':')?;
    Some((
        host.trim_matches(&['[', ']'][..]).to_string(),
        port.parse().ok()?,
    ))
}

async fn authorize(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    action: Action,
    team: TeamRef,
    request_id: RequestId,
) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::LearningSessions, action, Some(team)) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => {
            record_authz_denial(
                pool,
                ctx,
                request_id,
                Resource::LearningSessions,
                action,
                Some(team),
                reason,
            )
            .await;
            Err(deny_to_error(Resource::LearningSessions, action, reason))
        }
    }
}

pub async fn start_session(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    input: StartDiscoveryInput,
    request_id: RequestId,
) -> DomainResult<DiscoverySession> {
    start_session_with_policy(
        pool,
        ctx,
        team,
        input,
        request_id,
        &DiscoveryForwardingPolicy::default(),
    )
    .await
}

pub async fn start_session_with_policy(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    input: StartDiscoveryInput,
    request_id: RequestId,
    policy: &DiscoveryForwardingPolicy,
) -> DomainResult<DiscoverySession> {
    authorize(pool, ctx, Action::Create, team, request_id).await?;
    validate_name(&input.name)?;
    input.spec.validate()?;
    let validated_ip = validate_upstream(
        &input.spec.upstream_host,
        input.spec.upstream_port as u16,
        policy,
    )
    .await?;

    let session_id = DiscoverySessionId::generate();
    let short = session_id
        .as_uuid()
        .simple()
        .to_string()
        .chars()
        .take(12)
        .collect::<String>();
    let cluster_name = format!("discovery-{short}-upstream");
    let route_config_name = format!("discovery-{short}-routes");
    let listener_name = format!("discovery-{short}");

    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("start discovery: begin"))?;

    let cluster = clusters::create_discovery_owned(
        &mut tx,
        team,
        session_id.as_uuid(),
        &cluster_name,
        &cluster_spec(&validated_ip, input.spec.upstream_port as u16, &input.spec),
    )
    .await?;
    let route_config = gateway::create_discovery_route_config(
        &mut tx,
        team,
        session_id.as_uuid(),
        &route_config_name,
        &route_config_spec(&cluster_name),
    )
    .await?;
    let listener = gateway::create_discovery_listener(
        &mut tx,
        team,
        session_id.as_uuid(),
        &listener_name,
        &listener_spec(input.spec.listener_port as u16, &route_config_name),
    )
    .await?;
    let session = discovery::create(
        &mut tx,
        team,
        discovery::DiscoverySessionInsert {
            id: session_id,
            name: &input.name,
            spec: &input.spec,
            validated_upstream_ip: &validated_ip.to_string(),
            cluster_name: &cluster_name,
            route_config_name: &route_config_name,
            listener_name: &listener_name,
        },
    )
    .await?;
    append_gateway_upserts(
        &mut tx,
        team,
        GatewayResourceEvents {
            cluster_id: cluster.id.as_uuid(),
            cluster_name: &cluster.name,
            route_config_id: route_config.id.as_uuid(),
            route_config_name: &route_config.name,
            listener_id: listener.id.as_uuid(),
            listener_name: &listener.name,
        },
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "learn.discover.start",
            format!("discovery-sessions/{}", session.name),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("start discovery: commit"))?;
    Ok(session)
}

pub async fn list_sessions(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    status: Option<DiscoverySessionStatus>,
    limit: i64,
    offset: i64,
    request_id: RequestId,
) -> DomainResult<(Vec<DiscoverySession>, i64)> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    discovery::list(pool, team.id, status, limit, offset).await
}

pub async fn get_session(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    session: &str,
    request_id: RequestId,
) -> DomainResult<DiscoverySession> {
    authorize(pool, ctx, Action::Read, team, request_id).await?;
    discovery::get(pool, team.id, session)
        .await?
        .ok_or_else(|| DomainError::not_found("discovery session", session))
}

pub async fn stop_session(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    session: &str,
    request_id: RequestId,
) -> DomainResult<DiscoverySession> {
    authorize(pool, ctx, Action::Execute, team, request_id).await?;
    let current = discovery::get(pool, team.id, session)
        .await?
        .ok_or_else(|| DomainError::not_found("discovery session", session))?;
    let mut tx = pool
        .begin()
        .await
        .map_err(crate::services::db_err("stop discovery: begin"))?;
    let stopped = discovery::complete(&mut tx, team.id, session).await?;
    let listener_id = gateway::delete_discovery_listener(
        &mut tx,
        team.id,
        current.id.as_uuid(),
        &current.listener_name,
    )
    .await?;
    let route_config_id = gateway::delete_discovery_route_config(
        &mut tx,
        team.id,
        current.id.as_uuid(),
        &current.route_config_name,
    )
    .await?;
    let cluster_id = clusters::delete_discovery_owned(
        &mut tx,
        team.id,
        current.id.as_uuid(),
        &current.cluster_name,
    )
    .await?;
    append_gateway_deletes(
        &mut tx,
        team,
        OptionalGatewayResourceEvents {
            cluster_id: cluster_id.map(|id| id.as_uuid()),
            cluster_name: &current.cluster_name,
            route_config_id: route_config_id.map(|id| id.as_uuid()),
            route_config_name: &current.route_config_name,
            listener_id: listener_id.map(|id| id.as_uuid()),
            listener_name: &current.listener_name,
        },
    )
    .await?;
    audit::record_in_tx(
        &mut tx,
        &mutation_audit(
            ctx,
            request_id,
            team,
            "learn.discover.stop",
            format!("discovery-sessions/{}", stopped.name),
        ),
    )
    .await?;
    tx.commit()
        .await
        .map_err(crate::services::db_err("stop discovery: commit"))?;
    Ok(stopped)
}

fn cluster_spec(ip: &IpAddr, port: u16, spec: &DiscoverySessionSpec) -> ClusterSpec {
    ClusterSpec {
        endpoints: vec![Endpoint {
            host: ip.to_string(),
            port,
            weight: None,
        }],
        lb_policy: LbPolicy::RoundRobin,
        least_request: None,
        ring_hash: None,
        maglev: None,
        dns_lookup_family: None,
        connect_timeout_secs: 5,
        use_tls: spec.upstream_tls,
        upstream_tls: spec.upstream_tls.then(|| UpstreamTlsConfig {
            sni: Some(spec.upstream_host.clone()),
            validation_context_sds_secret_name: None,
            auto_sni_san_validation: false,
        }),
        protocol: None,
        health_checks: None,
        circuit_breakers: None,
        outlier_detection: None,
    }
}

fn route_config_spec(cluster_name: &str) -> RouteConfigSpec {
    RouteConfigSpec {
        virtual_hosts: vec![VirtualHost {
            name: "discovery".into(),
            domains: vec!["*".into()],
            routes: vec![RouteRule {
                name: "catch-all".into(),
                matcher: PathMatch::Prefix { prefix: "/".into() },
                headers: Vec::new(),
                query_parameters: Vec::new(),
                action: RouteAction {
                    cluster: Some(cluster_name.into()),
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
    }
}

fn listener_spec(port: u16, route_config_name: &str) -> ListenerSpec {
    ListenerSpec {
        address: "0.0.0.0".into(),
        port,
        protocol: ListenerProtocol::Http,
        route_config: Some(route_config_name.into()),
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: None,
    }
}

async fn validate_upstream(
    host: &str,
    port: u16,
    policy: &DiscoveryForwardingPolicy,
) -> DomainResult<IpAddr> {
    if host.contains('/') || host.contains('@') || host == "*" || port == 0 {
        return Err(DomainError::validation(
            "discovery upstream must be host:port without scheme, path, credentials, or wildcard",
        ));
    }
    let mut resolved = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| {
            DomainError::validation(format!("cannot resolve discovery upstream \"{host}\": {e}"))
        })?
        .map(|addr| canonical_ip(addr.ip()))
        .collect::<Vec<_>>();
    resolved.sort();
    resolved.dedup();
    if resolved.is_empty() {
        return Err(DomainError::validation(
            "discovery upstream did not resolve to an address",
        ));
    }
    if resolved
        .iter()
        .any(|ip| deny_reason(ip, port, policy).is_some())
    {
        return Err(DomainError::validation(
            "discovery upstream resolves to a denied internal destination",
        )
        .with_details(serde_json::json!({"reason": "denied_internal_destination"})));
    }
    Ok(resolved[0])
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        other => other,
    }
}

fn deny_reason(ip: &IpAddr, port: u16, policy: &DiscoveryForwardingPolicy) -> Option<&'static str> {
    if policy
        .denied_destinations
        .iter()
        .any(|addr| addr.ip() == *ip && addr.port() == port)
    {
        return Some("denied_flowplane_destination");
    }
    match ip {
        IpAddr::V4(ip) => {
            if ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_unspecified()
                || *ip == Ipv4Addr::new(169, 254, 169, 254)
            {
                Some("denied_internal_destination")
            } else {
                None
            }
        }
        IpAddr::V6(ip) => {
            if ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || is_ipv6_link_local(ip)
                || is_ipv6_unique_local(ip)
                || is_6to4(ip)
                || is_nat64_well_known(ip)
                || *ip == "fd00:ec2::254".parse::<Ipv6Addr>().ok()?
            {
                Some("denied_internal_destination")
            } else {
                None
            }
        }
    }
}

fn is_ipv6_link_local(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn is_ipv6_unique_local(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_6to4(ip: &Ipv6Addr) -> bool {
    ip.segments()[0] == 0x2002
}

fn is_nat64_well_known(ip: &Ipv6Addr) -> bool {
    ip.segments()[0] == 0x0064 && ip.segments()[1] == 0xff9b
}

struct GatewayResourceEvents<'a> {
    cluster_id: uuid::Uuid,
    cluster_name: &'a str,
    route_config_id: uuid::Uuid,
    route_config_name: &'a str,
    listener_id: uuid::Uuid,
    listener_name: &'a str,
}

async fn append_gateway_upserts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    events: GatewayResourceEvents<'_>,
) -> DomainResult<()> {
    for event in [
        DomainEvent::ClusterUpserted {
            cluster_id: events.cluster_id,
            name: events.cluster_name.into(),
        },
        DomainEvent::RouteConfigUpserted {
            route_config_id: events.route_config_id,
            name: events.route_config_name.into(),
        },
        DomainEvent::ListenerUpserted {
            listener_id: events.listener_id,
            name: events.listener_name.into(),
        },
    ] {
        fp_storage::outbox::append(
            tx,
            &event,
            EventScope {
                org_id: Some(team.org_id),
                team_id: Some(team.id),
            },
            trace_context_json(),
        )
        .await?;
    }
    Ok(())
}

struct OptionalGatewayResourceEvents<'a> {
    cluster_id: Option<uuid::Uuid>,
    cluster_name: &'a str,
    route_config_id: Option<uuid::Uuid>,
    route_config_name: &'a str,
    listener_id: Option<uuid::Uuid>,
    listener_name: &'a str,
}

async fn append_gateway_deletes(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    events: OptionalGatewayResourceEvents<'_>,
) -> DomainResult<()> {
    let mut outbox_events = Vec::new();
    if let Some(listener_id) = events.listener_id {
        outbox_events.push(DomainEvent::ListenerDeleted {
            listener_id,
            name: events.listener_name.into(),
        });
    }
    if let Some(route_config_id) = events.route_config_id {
        outbox_events.push(DomainEvent::RouteConfigDeleted {
            route_config_id,
            name: events.route_config_name.into(),
        });
    }
    if let Some(cluster_id) = events.cluster_id {
        outbox_events.push(DomainEvent::ClusterDeleted {
            cluster_id,
            name: events.cluster_name.into(),
        });
    }
    for event in outbox_events {
        fp_storage::outbox::append(
            tx,
            &event,
            EventScope {
                org_id: Some(team.org_id),
                team_id: Some(team.id),
            },
            trace_context_json(),
        )
        .await?;
    }
    Ok(())
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

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_ipv4_mapped_ipv6_before_deny_checks() {
        let mapped = "::ffff:169.254.169.254".parse::<IpAddr>().unwrap();
        let canonical = canonical_ip(mapped);
        assert_eq!(canonical, IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)));
        assert!(deny_reason(&canonical, 80, &DiscoveryForwardingPolicy::default()).is_some());
    }

    #[test]
    fn denies_embedded_address_prefixes_until_extraction_exists() {
        let policy = DiscoveryForwardingPolicy::default();
        assert!(
            deny_reason(&"2002:0808:0808::1".parse::<IpAddr>().unwrap(), 80, &policy).is_some()
        );
        assert!(deny_reason(
            &"64:ff9b::0808:0808".parse::<IpAddr>().unwrap(),
            80,
            &policy
        )
        .is_some());
    }

    #[test]
    fn configured_flowplane_destinations_are_denied_by_ip_and_port() {
        let ip = "203.0.113.10".parse::<IpAddr>().unwrap();
        let policy = DiscoveryForwardingPolicy::new(vec![SocketAddr::new(ip, 5432)]);
        assert_eq!(
            deny_reason(&ip, 5432, &policy),
            Some("denied_flowplane_destination")
        );
        assert_eq!(deny_reason(&ip, 5433, &policy), None);
    }

    #[test]
    fn postgres_host_port_parses_basic_database_urls() {
        assert_eq!(
            postgres_host_port("postgres://user:pass@db.example.test:5432/flowplane"),
            Some(("db.example.test".into(), 5432))
        );
        assert_eq!(
            postgres_host_port("postgres://user:pass@[2001:db8::10]:5432/flowplane"),
            Some(("2001:db8::10".into(), 5432))
        );
    }
}
