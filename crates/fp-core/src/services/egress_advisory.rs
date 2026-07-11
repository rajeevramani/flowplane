//! Write-time egress advisory (fpv2-1hp.1, FP-DEC-0008).
//!
//! Boot-time policy that rejects tenant-authored upstream hosts which *currently* resolve into
//! the protected destination set (cloud metadata/credential endpoints, loopback, link-local,
//! resolved CP infra addresses, operator-supplied infra CIDRs). Advisory only: it cannot stop a
//! post-write DNS rebind — the dataplane-node network posture is the enforcement boundary. It
//! deliberately does **not** deny private ranges (RFC1918 / IPv6 ULA): tenant private upstreams
//! are legitimate (constitution inv. 19).

use fp_domain::{DomainError, DomainResult};
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

/// A parsed CIDR block (`10.0.0.0/8`, `fd00:ec2::254/128`, or a bare address).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - u32::from(self.prefix))
                };
                (u32::from(net) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - u32::from(self.prefix))
                };
                (u128::from(net) & mask) == (u128::from(ip) & mask)
            }
            _ => false,
        }
    }
}

impl FromStr for Cidr {
    type Err = DomainError;

    fn from_str(raw: &str) -> DomainResult<Self> {
        let raw = raw.trim();
        let (addr_part, prefix_part) = match raw.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (raw, None),
        };
        let addr: IpAddr = addr_part.parse().map_err(|_| {
            DomainError::invalid_config(format!("\"{raw}\" is not a valid CIDR or IP address"))
                .with_hint("use e.g. 10.0.0.0/8, 192.0.2.7, or fd00:ec2::254/128")
        })?;
        let max = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix = match prefix_part {
            Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max).ok_or_else(|| {
                DomainError::invalid_config(format!(
                    "\"{raw}\" has an invalid prefix length (max {max})"
                ))
            })?,
            None => max,
        };
        Ok(Self { addr, prefix })
    }
}

/// Boot-time advisory policy. Built once from `ServerConfig`; carried in `AppState` and passed
/// into the mutation services that gate tenant-authored upstream hosts (slices fpv2-1hp.2–.4).
///
/// `Default` is the disabled policy (used by tests that do not exercise the advisory).
#[derive(Debug, Clone, Default)]
pub struct EgressAdvisoryPolicy {
    enabled: bool,
    denied_addrs: BTreeSet<IpAddr>,
    denied_cidrs: Vec<Cidr>,
}

impl EgressAdvisoryPolicy {
    /// Test/bespoke constructor. Production code uses [`Self::from_server_config`].
    pub fn new(enabled: bool, denied_addrs: Vec<IpAddr>, denied_cidrs: Vec<Cidr>) -> Self {
        Self {
            enabled,
            denied_addrs: denied_addrs.into_iter().map(canonical_ip).collect(),
            denied_cidrs,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Assemble the deny set from `ServerConfig` at boot. Best-effort on DNS (a transient
    /// resolution failure of an infra hostname logs a warning and narrows the set rather than
    /// failing boot); the operator `egress_advisory_denied_cidrs` list is the authoritative
    /// source for the CP/xDS routable ranges — bind addresses join only when concrete.
    pub async fn from_server_config(config: &crate::config::ServerConfig) -> Self {
        if !config.egress_advisory_enabled {
            tracing::warn!(
                "egress advisory disabled by operator config \
                 (FLOWPLANE_EGRESS_ADVISORY_ENABLED=false): tenant upstream hosts are not \
                 checked against protected destinations at write time"
            );
            return Self {
                enabled: false,
                denied_addrs: BTreeSet::new(),
                denied_cidrs: config.egress_advisory_denied_cidrs.clone(),
            };
        }

        let mut denied_addrs = BTreeSet::new();
        // Listener binds: deny only a concrete bind IP. An unspecified bind (0.0.0.0 / ::)
        // does not identify the CP's routable addresses and contributes nothing — the operator
        // CIDR list is the authoritative CP/xDS source.
        for bind in [config.api_addr.ip(), config.xds_addr.ip()] {
            if !bind.is_unspecified() {
                denied_addrs.insert(canonical_ip(bind));
            }
        }
        // Infra hosts ServerConfig actually knows: database + RLS endpoints. The DB URL's
        // port is optional (postgres defaults 5432) — only the host matters here.
        let mut infra_hosts = Vec::new();
        if let Some(host) = authority_host(&config.database_url) {
            infra_hosts.push(host);
        }
        for url in [
            config.rls_admin_url.as_deref(),
            config.rls_grpc_url.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(host) = authority_host(url) {
                infra_hosts.push(host);
            }
        }
        for host in infra_hosts {
            if let Ok(ip) = host.parse::<IpAddr>() {
                denied_addrs.insert(canonical_ip(ip));
            } else {
                match tokio::net::lookup_host((host.as_str(), 0u16)).await {
                    Ok(addrs) => {
                        denied_addrs.extend(addrs.map(|a| canonical_ip(a.ip())));
                    }
                    Err(error) => tracing::warn!(
                        host,
                        %error,
                        "egress advisory: could not resolve infra host at boot; it will not be \
                         in the advisory deny set"
                    ),
                }
            }
        }

        Self {
            enabled: true,
            denied_addrs,
            denied_cidrs: config.egress_advisory_denied_cidrs.clone(),
        }
    }

    /// Why `ip` is a protected destination, or `None` if it is allowed.
    pub fn denial_reason(&self, ip: IpAddr) -> Option<&'static str> {
        let ip = canonical_ip(ip);
        if let Some(reason) = builtin_denial(&ip) {
            return Some(reason);
        }
        // 6to4 / NAT64 forms deliver to the embedded IPv4 destination: re-apply the full
        // v4 check on the extraction instead of blanket-denying the whole prefix.
        if let IpAddr::V6(v6) = ip {
            if let Some(embedded) = embedded_ipv4(&v6) {
                if let Some(reason) = self.denial_reason(IpAddr::V4(embedded)) {
                    return Some(reason);
                }
            }
        }
        if self.denied_addrs.contains(&ip) {
            return Some("infra_address");
        }
        if self.denied_cidrs.iter().any(|c| c.contains(ip)) {
            return Some("denied_cidr");
        }
        None
    }

    /// Advisory check for one already-resolved address.
    pub fn check_ip(&self, host: &str, ip: IpAddr) -> DomainResult<()> {
        if !self.enabled {
            return Ok(());
        }
        match self.denial_reason(ip) {
            None => Ok(()),
            Some(reason) => Err(denied_error(host, ip, reason, &[ip])),
        }
    }

    /// Advisory check for a tenant-authored host: an IP literal is checked directly; a hostname
    /// is resolved now and **every** answer must be allowed (mixed answers reject). Resolution
    /// failure rejects — the advisory cannot vouch for a host it cannot resolve. A denial error
    /// carries the **full** resolved-address set in `details.resolved_addresses` (the audit
    /// evidence contract), not just the offending answer.
    pub async fn check_host(&self, host: &str) -> DomainResult<()> {
        if !self.enabled {
            return Ok(());
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            return self.check_ip(host, ip);
        }
        let addrs: Vec<IpAddr> = tokio::net::lookup_host((host, 0u16))
            .await
            .map_err(|error| {
                DomainError::validation(format!(
                    "upstream host \"{host}\" did not resolve ({error}); the egress advisory \
                     rejects hosts it cannot resolve"
                ))
                .with_details(serde_json::json!({
                    "class": "egress_advisory_denied",
                    "host": host,
                    "reason": "resolution_failed",
                    "resolved_addresses": [],
                }))
            })?
            .map(|a| a.ip())
            .collect();
        if addrs.is_empty() {
            return Err(DomainError::validation(format!(
                "upstream host \"{host}\" resolved to no addresses; the egress advisory rejects \
                 hosts it cannot resolve"
            ))
            .with_details(serde_json::json!({
                "class": "egress_advisory_denied",
                "host": host,
                "reason": "resolution_failed",
                "resolved_addresses": [],
            })));
        }
        // Any-match: the first denied answer rejects, and the error evidence keeps ALL answers
        // (a mixed-answer rebind attempt is visible in the audit record).
        if let Some((ip, reason)) = addrs
            .iter()
            .find_map(|ip| self.denial_reason(*ip).map(|reason| (*ip, reason)))
        {
            return Err(denied_error(host, ip, reason, &addrs));
        }
        Ok(())
    }

    /// Gate a mutation on its tenant-authored upstream hosts. On the first denied host a
    /// **rejection audit record** is written in its own short transaction — the mutation
    /// transaction never opens, so the record survives the rejected mutation — and the
    /// validation error is returned. `mutation` is the mutation path (`cluster.create`, …);
    /// `resource` matches the mutation-audit resource string (`clusters/<name>`, …).
    #[allow(clippy::too_many_arguments)]
    pub async fn enforce_hosts(
        &self,
        pool: &sqlx::PgPool,
        ctx: &crate::authz::PrincipalCtx,
        request_id: fp_domain::RequestId,
        team: fp_domain::authz::TeamRef,
        mutation: &str,
        resource: &str,
        hosts: Vec<String>,
    ) -> DomainResult<()> {
        if !self.enabled {
            return Ok(());
        }
        for host in hosts {
            if let Err(err) = self.check_host(&host).await {
                let (actor_type, actor_id) = crate::services::actor_of(ctx);
                let mut detail = err
                    .details
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({"class": "egress_advisory_denied"}));
                if let Some(map) = detail.as_object_mut() {
                    map.insert("mutation".into(), serde_json::json!(mutation));
                }
                fp_storage::repos::audit::record_best_effort(
                    pool,
                    &fp_storage::repos::audit::AuditEntry {
                        request_id: Some(request_id),
                        actor_type,
                        actor_id,
                        actor_label: String::new(),
                        surface: fp_storage::repos::audit::Surface::Rest,
                        action: "egress_advisory.denied".into(),
                        resource: resource.into(),
                        org_id: Some(team.org_id),
                        team_id: Some(team.id),
                        outcome: fp_storage::repos::audit::Outcome::Denied,
                        detail,
                    },
                )
                .await;
                return Err(err);
            }
        }
        Ok(())
    }
}

/// The advisory denial error: names the offending answer and carries the full resolved set as
/// structured evidence (`details.resolved_addresses`) for the rejection audit record.
fn denied_error(host: &str, ip: IpAddr, reason: &str, resolved: &[IpAddr]) -> DomainError {
    DomainError::validation(format!(
        "upstream host \"{host}\" resolves to a protected destination ({ip}: {reason}); tenant \
         upstreams may not target cloud metadata, loopback, or Flowplane infrastructure addresses"
    ))
    .with_details(serde_json::json!({
        "class": "egress_advisory_denied",
        "host": host,
        "ip": ip.to_string(),
        "reason": reason,
        "resolved_addresses": resolved.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
    }))
}

/// Destinations denied regardless of configuration: cloud metadata/credential endpoints,
/// loopback, link-local, and non-routable special ranges. Exact endpoints — **not** blanket
/// private/ULA ranges (inv. 19).
fn builtin_denial(ip: &IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => {
            if *v4 == Ipv4Addr::new(169, 254, 169, 254)
                || *v4 == Ipv4Addr::new(169, 254, 170, 2)
                || *v4 == Ipv4Addr::new(168, 63, 129, 16)
            {
                Some("metadata_endpoint")
            } else if v4.is_loopback() {
                Some("loopback")
            } else if v4.is_link_local() {
                Some("link_local")
            } else if v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast() {
                Some("special_range")
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if *v6 == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254) {
                Some("metadata_endpoint")
            } else if v6.is_loopback() {
                Some("loopback")
            } else if is_ipv6_link_local(v6) {
                Some("link_local")
            } else if v6.is_unspecified() || v6.is_multicast() {
                Some("special_range")
            } else {
                None
            }
        }
    }
}

/// The IPv4 destination embedded in a 6to4 (`2002::/16`) or NAT64 well-known-prefix
/// (`64:ff9b::/96`) address, if any.
fn embedded_ipv4(v6: &Ipv6Addr) -> Option<Ipv4Addr> {
    let s = v6.segments();
    if s[0] == 0x2002 {
        return Some(Ipv4Addr::new(
            (s[1] >> 8) as u8,
            s[1] as u8,
            (s[2] >> 8) as u8,
            s[2] as u8,
        ));
    }
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
        return Some(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
    }
    None
}

fn is_ipv6_link_local(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
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

/// Host component of a URL-ish string (`https://h:p/x`, `h:p`, `[::1]:50051`).
pub(crate) fn authority_host(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = after_scheme.split(['/', '?']).next()?;
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, hp)| hp)
        .unwrap_or(authority);
    if host_port.is_empty() {
        return None;
    }
    if let Some(rest) = host_port.strip_prefix('[') {
        return rest
            .split_once(']')
            .map(|(host, _)| host.to_string())
            .filter(|host| !host.is_empty());
    }
    let host = host_port
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty())
        .map(|(host, _)| host)
        .unwrap_or(host_port);
    // Re-validate after userinfo/port stripping: "http://:8080" or "user@:8080" must not
    // yield an empty host.
    if host.is_empty() {
        return None;
    }
    Some(host.to_string())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn enabled_policy() -> EgressAdvisoryPolicy {
        EgressAdvisoryPolicy::new(true, Vec::new(), Vec::new())
    }

    #[test]
    fn cidr_parses_blocks_and_bare_addresses() {
        let block: Cidr = "10.0.0.0/8".parse().unwrap();
        assert!(block.contains(ip("10.255.1.2")));
        assert!(!block.contains(ip("11.0.0.1")));
        let bare: Cidr = "192.0.2.7".parse().unwrap();
        assert!(bare.contains(ip("192.0.2.7")));
        assert!(!bare.contains(ip("192.0.2.8")));
        let v6: Cidr = "fd00:ec2::/64".parse().unwrap();
        assert!(v6.contains(ip("fd00:ec2::254")));
        assert!(!v6.contains(ip("fd00:ec3::1")));
        // Family mismatch never matches.
        assert!(!block.contains(ip("::1")));
    }

    #[test]
    fn cidr_rejects_garbage_and_bad_prefixes() {
        assert!("not-a-cidr".parse::<Cidr>().is_err());
        assert!("10.0.0.0/33".parse::<Cidr>().is_err());
        assert!("fd00::/129".parse::<Cidr>().is_err());
        assert!("10.0.0.0/".parse::<Cidr>().is_err());
    }

    #[test]
    fn builtin_metadata_loopback_and_special_ranges_are_denied() {
        let p = enabled_policy();
        for denied in [
            "169.254.169.254", // EC2/GCP/Azure IMDS
            "169.254.170.2",   // ECS task credentials
            "168.63.129.16",   // Azure wireserver
            "127.0.0.1",
            "127.8.9.10",
            "169.254.1.1", // link-local
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "fe80::1",
            "fd00:ec2::254", // EC2 IMDS IPv6
            "ff02::1",
        ] {
            assert!(
                p.denial_reason(ip(denied)).is_some(),
                "{denied} must be denied"
            );
        }
    }

    #[test]
    fn tenant_private_and_ula_upstreams_are_allowed() {
        // Inv. 19: no blanket private/ULA denial.
        let p = enabled_policy();
        for allowed in [
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.1",
            "fd12:3456:789a::1", // tenant ULA
            "93.184.216.34",     // arbitrary public
            "2600:1406:3a00::1", // public v6
        ] {
            assert!(
                p.denial_reason(ip(allowed)).is_none(),
                "{allowed} must be allowed"
            );
        }
    }

    #[test]
    fn embedded_ipv4_forms_reapply_the_v4_check() {
        let p = enabled_policy();
        // NAT64 well-known prefix wrapping the metadata endpoint → denied via extraction.
        assert!(p.denial_reason(ip("64:ff9b::a9fe:a9fe")).is_some());
        // NAT64 wrapping a public address → allowed.
        assert!(p.denial_reason(ip("64:ff9b::5db8:d822")).is_none());
        // 6to4 wrapping the metadata endpoint → denied; wrapping public → allowed.
        assert!(p.denial_reason(ip("2002:a9fe:a9fe::1")).is_some());
        assert!(p.denial_reason(ip("2002:5db8:d822::1")).is_none());
        // v4-mapped form canonicalizes.
        assert!(p.denial_reason(ip("::ffff:169.254.169.254")).is_some());
    }

    #[test]
    fn operator_cidrs_and_infra_addrs_are_denied() {
        let p = EgressAdvisoryPolicy::new(
            true,
            vec![ip("203.0.113.9")],
            vec!["10.1.0.0/16".parse().unwrap()],
        );
        assert_eq!(p.denial_reason(ip("203.0.113.9")), Some("infra_address"));
        assert_eq!(p.denial_reason(ip("10.1.2.3")), Some("denied_cidr"));
        assert!(p.denial_reason(ip("10.2.0.1")).is_none());
        // NAT64-embedded form of a denied CIDR member is caught by extraction too.
        assert_eq!(
            p.denial_reason(ip("64:ff9b::0a01:0203")),
            Some("denied_cidr")
        );
    }

    #[test]
    fn disabled_policy_allows_everything() {
        let p = EgressAdvisoryPolicy::new(false, Vec::new(), Vec::new());
        assert!(p.check_ip("h", ip("169.254.169.254")).is_ok());
    }

    #[tokio::test]
    async fn check_host_handles_ip_literals_without_dns() {
        let p = enabled_policy();
        assert!(p.check_host("169.254.169.254").await.is_err());
        assert!(p.check_host("93.184.216.34").await.is_ok());
        assert!(p.check_host("fd00:ec2::254").await.is_err());
        assert!(p.check_host("fd12:3456::1").await.is_ok());
    }

    #[tokio::test]
    async fn from_server_config_uses_concrete_binds_and_infra_hosts_only() {
        let mut config = test_config();
        config.egress_advisory_enabled = true;
        // Unspecified binds contribute nothing.
        config.api_addr = "0.0.0.0:8080".parse().unwrap();
        config.xds_addr = "192.0.2.44:18000".parse().unwrap();
        config.database_url = "postgres://user:pass@203.0.113.5:5432/db".into();
        config.rls_grpc_url = Some("198.51.100.7:50051".into());
        config.egress_advisory_denied_cidrs = vec!["100.64.0.0/10".parse().unwrap()];

        let p = EgressAdvisoryPolicy::from_server_config(&config).await;
        assert!(p.enabled());
        assert!(p.denial_reason(ip("0.0.0.0")).is_some()); // builtin special, not bind-derived
        assert_eq!(p.denial_reason(ip("192.0.2.44")), Some("infra_address")); // concrete bind
        assert_eq!(p.denial_reason(ip("203.0.113.5")), Some("infra_address")); // DB host

        // A DB URL without an explicit port must still contribute its host (postgres
        // defaults the port; the deny set is address-based).
        let mut portless = test_config();
        portless.database_url = "postgres://user:pass@203.0.113.6/db".into();
        let p2 = EgressAdvisoryPolicy::from_server_config(&portless).await;
        assert_eq!(p2.denial_reason(ip("203.0.113.6")), Some("infra_address"));
        assert_eq!(p.denial_reason(ip("198.51.100.7")), Some("infra_address")); // RLS gRPC
        assert_eq!(p.denial_reason(ip("100.64.1.1")), Some("denied_cidr")); // operator CIDR
        assert!(p.denial_reason(ip("192.0.2.45")).is_none());
    }

    #[tokio::test]
    async fn from_server_config_disabled_builds_disabled_policy_and_warns_at_startup() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Sink {
            type Writer = Sink;
            fn make_writer(&'a self) -> Sink {
                self.clone()
            }
        }

        let sink = Sink::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(sink.clone())
            .finish();

        let mut config = test_config();
        config.egress_advisory_enabled = false;
        let p = {
            let _guard = tracing::subscriber::set_default(subscriber);
            EgressAdvisoryPolicy::from_server_config(&config).await
        };
        assert!(!p.enabled());
        assert!(p.check_host("169.254.169.254").await.is_ok());

        let logs = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert!(
            logs.contains("egress advisory disabled by operator config"),
            "disabling the advisory must log a startup warning; got: {logs}"
        );
    }

    #[tokio::test]
    async fn enforce_hosts_rejects_denied_and_passes_allowed() {
        // Audit write is best-effort; a lazy unreachable pool exercises the code path without
        // a DB (the DB-backed audit-row assertion lives in the fp-api integration tests).
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(200))
            .connect_lazy("postgres://unused@127.0.0.1:1/unused")
            .unwrap();
        let org_id = fp_domain::OrgId::generate();
        let ctx = crate::authz::PrincipalCtx::User {
            user_id: fp_domain::UserId::generate(),
            platform_admin: false,
            org: Some((org_id, fp_domain::OrgRole::Admin)),
            org_selector_required: false,
            grants: crate::GrantSet::default(),
        };
        let team = fp_domain::authz::TeamRef {
            org_id,
            id: fp_domain::TeamId::generate(),
        };
        let p = enabled_policy();
        let err = p
            .enforce_hosts(
                &pool,
                &ctx,
                fp_domain::RequestId::generate(),
                team,
                "cluster.create",
                "clusters/x",
                vec!["169.254.169.254".into()],
            )
            .await
            .expect_err("metadata endpoint must be rejected");
        assert_eq!(err.code, fp_domain::ErrorCode::ValidationFailed);
        assert_eq!(
            err.details
                .as_ref()
                .and_then(|d| d.get("class"))
                .and_then(|v| v.as_str()),
            Some("egress_advisory_denied")
        );
        p.enforce_hosts(
            &pool,
            &ctx,
            fp_domain::RequestId::generate(),
            team,
            "cluster.create",
            "clusters/x",
            vec!["10.1.2.3".into(), "93.184.216.34".into()],
        )
        .await
        .expect("private + public hosts pass");
    }

    #[test]
    fn a5_no_call_site_env_reads_in_advisory_paths() {
        // Finding 13/A5 (by construction): the advisory policy is built from ServerConfig at
        // boot; neither the policy nor any consuming mutation path may read process env at the
        // call site. Source guard on the advisory module + the four gated mutation services.
        // Needles are concatenated at runtime so this test's own source (included below via
        // include_str!) doesn't match them.
        let needles = [["env", "::var"].concat(), ["std", "::", "env"].concat()];
        for (name, src) in [
            ("egress_advisory.rs", include_str!("egress_advisory.rs")),
            ("clusters.rs", include_str!("clusters.rs")),
            ("ai.rs", include_str!("ai.rs")),
            ("expose.rs", include_str!("expose.rs")),
            ("route_generation.rs", include_str!("route_generation.rs")),
        ] {
            for needle in &needles {
                assert!(
                    !src.contains(needle.as_str()),
                    "{name} must not read process env (A5: egress config comes from ServerConfig)"
                );
            }
        }
    }

    #[test]
    fn authority_host_extracts_hosts() {
        assert_eq!(
            authority_host("https://rls.internal:8081/admin").as_deref(),
            Some("rls.internal")
        );
        assert_eq!(
            authority_host("rls.internal:50051").as_deref(),
            Some("rls.internal")
        );
        assert_eq!(
            authority_host("http://user@10.0.0.4:81/x").as_deref(),
            Some("10.0.0.4")
        );
        assert_eq!(
            authority_host("[fd00::7]:50051").as_deref(),
            Some("fd00::7")
        );
        assert_eq!(authority_host("plainhost").as_deref(), Some("plainhost"));
        // Empty hosts must be rejected, not returned as Some("") (fpv2-1hp.3 pass-1 finding).
        assert_eq!(authority_host("http://:8080"), None);
        assert_eq!(authority_host("http://user@:8080"), None);
        assert_eq!(authority_host("http://user@"), None);
        assert_eq!(authority_host("[]:8080"), None);
        assert_eq!(authority_host(""), None);
    }

    /// Minimal valid ServerConfig for policy tests (fields irrelevant to the advisory are
    /// dev-ish defaults).
    fn test_config() -> crate::config::ServerConfig {
        crate::config::ServerConfig {
            api_addr: "0.0.0.0:8080".parse().unwrap(),
            xds_addr: "0.0.0.0:18000".parse().unwrap(),
            database_url: "postgres://user:pass@203.0.113.5:5432/db".into(),
            db_max_connections: 5,
            api_tls: None,
            xds_tls: None,
            api_insecure: true,
            log_format: crate::config::LogFormat::Pretty,
            log_filter: "info".into(),
            otlp_endpoint: None,
            dev_mode: false,
            oidc: None,
            tenant_write_limit_per_minute: 100,
            allow_logged_bootstrap_token: false,
            dev_token_path: None,
            rls_admin_url: None,
            rls_reconcile_secs: 60,
            rls_grpc_url: None,
            dataplane_tls: None,
            egress_advisory_enabled: true,
            egress_advisory_denied_cidrs: Vec::new(),
        }
    }
}
