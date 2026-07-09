//! Shared service-layer validation for tenant-authored upstream destinations.

use fp_domain::{DomainError, DomainResult};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[derive(Debug, Clone, Default)]
pub struct EgressPolicy {
    denied_destinations: Vec<SocketAddr>,
    allowed_destinations: Vec<SocketAddr>,
    static_hosts: BTreeMap<(String, u16), Vec<IpAddr>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressValidation {
    pub allowlist_match: Option<SocketAddr>,
    pub resolved_ips: Vec<IpAddr>,
}

impl EgressValidation {
    pub fn audit_detail(&self) -> serde_json::Value {
        match self.allowlist_match {
            Some(addr) => serde_json::json!({
                "egress_policy": {
                    "allowlist_match": addr.to_string(),
                }
            }),
            None => serde_json::json!({}),
        }
    }
}

impl EgressPolicy {
    pub fn new(denied_destinations: Vec<SocketAddr>) -> Self {
        Self::with_allowed(denied_destinations, Vec::new())
    }

    pub fn with_allowed(
        denied_destinations: Vec<SocketAddr>,
        allowed_destinations: Vec<SocketAddr>,
    ) -> Self {
        Self {
            denied_destinations: canonical_socket_addrs(denied_destinations),
            allowed_destinations: canonical_socket_addrs(allowed_destinations),
            static_hosts: BTreeMap::new(),
        }
    }

    pub fn with_static_hosts(
        denied_destinations: Vec<SocketAddr>,
        allowed_destinations: Vec<SocketAddr>,
        static_hosts: Vec<(String, u16, Vec<IpAddr>)>,
    ) -> Self {
        let mut policy = Self::with_allowed(denied_destinations, allowed_destinations);
        policy.static_hosts = static_hosts
            .into_iter()
            .map(|(host, port, ips)| {
                (
                    (host.to_ascii_lowercase(), port),
                    ips.into_iter().map(canonical_ip).collect(),
                )
            })
            .collect();
        policy
    }

    pub async fn from_server_config(config: &crate::config::ServerConfig) -> Self {
        let mut denied = vec![config.api_addr, config.xds_addr];
        if let Some((host, port)) = postgres_host_port(&config.database_url) {
            if let Ok(addrs) = tokio::net::lookup_host((host.as_str(), port)).await {
                denied.extend(addrs);
            }
        }
        Self::with_allowed(denied, config.egress_allowed_destinations.clone())
    }

    pub async fn from_process_config() -> Self {
        let mut denied = Vec::new();
        if let Some(addr) = env_socket_addr("FLOWPLANE_API_ADDR", "0.0.0.0:8080") {
            denied.push(addr);
        }
        if let Some(addr) = env_socket_addr("FLOWPLANE_XDS_ADDR", "0.0.0.0:18000") {
            denied.push(addr);
        }
        if let Ok(database_url) =
            std::env::var("FLOWPLANE_DATABASE_URL").or_else(|_| std::env::var("DATABASE_URL"))
        {
            if let Some((host, port)) = postgres_host_port(&database_url) {
                if let Ok(addrs) = tokio::net::lookup_host((host.as_str(), port)).await {
                    denied.extend(addrs);
                }
            }
        }
        let allowed = parse_socket_addr_list(
            std::env::var("FLOWPLANE_EGRESS_ALLOWED_DESTINATIONS")
                .or_else(|_| std::env::var("FLOWPLANE_DISCOVERY_ALLOWED_DESTINATIONS"))
                .ok()
                .as_deref(),
        );
        Self::with_allowed(denied, allowed)
    }

    pub async fn validate_host_port(
        &self,
        host: &str,
        port: u16,
        context: &'static str,
    ) -> DomainResult<EgressValidation> {
        if host.contains('/') || host.contains('@') || host == "*" || port == 0 {
            return Err(DomainError::validation(format!(
                "{context} must be host:port without scheme, path, credentials, wildcard, or port 0"
            )));
        }
        let resolved = self.resolve_host(host, port).await?;
        self.validate_resolved(host, port, context, resolved)
    }

    async fn resolve_host(&self, host: &str, port: u16) -> DomainResult<Vec<IpAddr>> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![canonical_ip(ip)]);
        }
        let key = (host.to_ascii_lowercase(), port);
        let mut resolved = match self.static_hosts.get(&key) {
            Some(ips) => ips.clone(),
            None => tokio::net::lookup_host((host, port))
                .await
                .map_err(|e| {
                    DomainError::validation(format!(
                        "cannot resolve egress destination \"{host}\": {e}"
                    ))
                })?
                .map(|addr| canonical_ip(addr.ip()))
                .collect(),
        };
        resolved.sort();
        resolved.dedup();
        if resolved.is_empty() {
            return Err(DomainError::validation(
                "egress destination did not resolve to an address",
            ));
        }
        Ok(resolved)
    }

    fn validate_resolved(
        &self,
        host: &str,
        port: u16,
        context: &'static str,
        resolved: Vec<IpAddr>,
    ) -> DomainResult<EgressValidation> {
        let mut validation = EgressValidation::default();
        for ip in &resolved {
            match self.destination_decision(*ip, port) {
                DestinationDecision::Allowed => {}
                DestinationDecision::AllowedByConfig(addr) => {
                    validation.allowlist_match.get_or_insert(addr);
                }
                DestinationDecision::Denied(reason) => {
                    return Err(DomainError::validation(format!(
                        "{context} resolves to a denied egress destination"
                    ))
                    .with_details(serde_json::json!({
                        "reason": reason,
                        "host": host,
                        "port": port,
                        "resolved_ip": ip.to_string(),
                    })));
                }
            }
        }
        validation.resolved_ips = resolved;
        Ok(validation)
    }

    fn destination_decision(&self, ip: IpAddr, port: u16) -> DestinationDecision {
        let ip = canonical_ip(ip);
        if self
            .denied_destinations
            .iter()
            .any(|addr| addr.ip() == ip && addr.port() == port)
        {
            return DestinationDecision::Denied("denied_flowplane_destination");
        }
        if always_denied_destination(&ip) {
            return DestinationDecision::Denied("denied_internal_destination");
        }
        if let Some(addr) = self
            .allowed_destinations
            .iter()
            .find(|addr| addr.ip() == ip && addr.port() == port)
        {
            return DestinationDecision::AllowedByConfig(*addr);
        }
        match ip {
            IpAddr::V4(ip) if ip.is_loopback() || ip.is_private() || ip.is_link_local() => {
                DestinationDecision::Denied("denied_internal_destination")
            }
            IpAddr::V6(ip)
                if ip.is_loopback() || is_ipv6_link_local(&ip) || is_ipv6_unique_local(&ip) =>
            {
                DestinationDecision::Denied("denied_internal_destination")
            }
            _ => DestinationDecision::Allowed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DestinationDecision {
    Allowed,
    AllowedByConfig(SocketAddr),
    Denied(&'static str),
}

pub(crate) fn postgres_host_port(database_url: &str) -> Option<(String, u16)> {
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

pub(crate) fn parse_socket_addr_list(raw: Option<&str>) -> Vec<SocketAddr> {
    raw.map(|raw| {
        raw.split(',')
            .filter_map(|entry| entry.trim().parse::<SocketAddr>().ok())
            .collect()
    })
    .unwrap_or_default()
}

pub(crate) fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        other => other,
    }
}

fn canonical_socket_addrs(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    addrs
        .into_iter()
        .map(|addr| SocketAddr::new(canonical_ip(addr.ip()), addr.port()))
        .collect()
}

fn env_socket_addr(key: &str, default: &str) -> Option<SocketAddr> {
    std::env::var(key)
        .unwrap_or_else(|_| default.to_string())
        .parse()
        .ok()
}

fn always_denied_destination(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_multicast() || ip.is_unspecified() || *ip == Ipv4Addr::new(169, 254, 169, 254)
        }
        IpAddr::V6(ip) => {
            ip.is_unspecified()
                || ip.is_multicast()
                || is_6to4(ip)
                || is_nat64_well_known(ip)
                || *ip == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254)
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

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn canonicalizes_ipv4_mapped_ipv6_before_deny_checks() {
        let policy = EgressPolicy::default();
        let err = policy
            .validate_host_port("::ffff:169.254.169.254", 80, "test destination")
            .await
            .expect_err("metadata denied");
        assert_eq!(
            err.details.unwrap()["resolved_ip"],
            serde_json::json!("169.254.169.254")
        );
    }

    #[tokio::test]
    async fn denies_metadata_and_embedded_address_prefixes() {
        let policy = EgressPolicy::default();
        for host in [
            "169.254.169.254",
            "fd00:ec2::254",
            "2002:0808:0808::1",
            "64:ff9b::0808:0808",
        ] {
            policy
                .validate_host_port(host, 80, "test destination")
                .await
                .expect_err("blocked internal destination");
        }
    }

    #[tokio::test]
    async fn default_policy_denies_loopback_and_private_destinations() {
        let policy = EgressPolicy::with_static_hosts(
            Vec::new(),
            Vec::new(),
            vec![(
                "internal.example.test".into(),
                8080,
                vec!["10.0.0.12".parse().unwrap()],
            )],
        );
        for host in ["127.0.0.1", "10.0.0.10", "internal.example.test"] {
            policy
                .validate_host_port(host, 8080, "test destination")
                .await
                .expect_err("default policy denies loopback or private destination");
        }
    }

    #[tokio::test]
    async fn configured_flowplane_destinations_are_denied_by_ip_and_port() {
        let ip = "203.0.113.10".parse::<IpAddr>().unwrap();
        let policy = EgressPolicy::new(vec![SocketAddr::new(ip, 5432)]);
        assert!(policy
            .validate_host_port("203.0.113.10", 5432, "test destination")
            .await
            .is_err());
        policy
            .validate_host_port("203.0.113.10", 5433, "test destination")
            .await
            .expect("different port accepted");
    }

    #[tokio::test]
    async fn explicit_allowlist_admits_private_destination_after_default_denies() {
        let ip = "127.0.0.1".parse::<IpAddr>().unwrap();
        let policy = EgressPolicy::with_allowed(Vec::new(), vec![SocketAddr::new(ip, 3001)]);
        let validation = policy
            .validate_host_port("127.0.0.1", 3001, "test destination")
            .await
            .expect("allowlisted loopback accepted");
        assert_eq!(validation.allowlist_match, Some(SocketAddr::new(ip, 3001)));

        let denied = EgressPolicy::with_allowed(
            vec![SocketAddr::new(ip, 3001)],
            vec![SocketAddr::new(ip, 3001)],
        );
        denied
            .validate_host_port("127.0.0.1", 3001, "test destination")
            .await
            .expect_err("explicit Flowplane destination deny wins");
    }

    #[tokio::test]
    async fn explicit_allowlist_cannot_admit_metadata_destination() {
        let ip = "169.254.169.254".parse::<IpAddr>().unwrap();
        let policy = EgressPolicy::with_allowed(Vec::new(), vec![SocketAddr::new(ip, 80)]);
        policy
            .validate_host_port("169.254.169.254", 80, "test destination")
            .await
            .expect_err("metadata remains denied");
    }

    #[tokio::test]
    async fn static_hostname_resolution_is_hermetic() {
        let policy = EgressPolicy::with_static_hosts(
            Vec::new(),
            Vec::new(),
            vec![(
                "private.example.test".into(),
                443,
                vec!["10.0.0.12".parse().unwrap()],
            )],
        );
        policy
            .validate_host_port("private.example.test", 443, "test destination")
            .await
            .expect_err("static private resolution denied");
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
