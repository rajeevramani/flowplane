//! Listener: an address:port Envoy binds (spec/03 §7.2 rules). The ≥1024 port floor is the
//! unprivileged-dataplane invariant carried from v1.

use crate::error::{DomainError, DomainResult};
use crate::id::{ListenerId, TeamId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const LISTENER_PORT_MIN: u16 = 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listener {
    pub id: ListenerId,
    pub team_id: TeamId,
    pub name: String,
    pub spec: ListenerSpec,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListenerSpec {
    /// Bind address: IPv4, bare IPv6, or hostname (v1 rules; `*.` wildcard prefix allowed).
    pub address: String,
    /// 1024–65535 (privileged ports forbidden — dataplanes run unprivileged).
    pub port: u16,
    /// Route configuration served by this listener, by name (same team). Optional until
    /// bound; resolved and reference-tracked by the service layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_config: Option<String>,
}

fn valid_hostname(address: &str) -> bool {
    let body = address.strip_prefix("*.").unwrap_or(address);
    if body.is_empty() || body.len() > 253 || body.contains("..") {
        return false;
    }
    body.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

fn valid_ipv4(address: &str) -> bool {
    address.parse::<std::net::Ipv4Addr>().is_ok()
}

fn valid_ipv6(address: &str) -> bool {
    // Bare form only (no brackets), matching v1.
    address.contains(':') && address.parse::<std::net::Ipv6Addr>().is_ok()
}

impl ListenerSpec {
    pub fn validate(&self) -> DomainResult<()> {
        let looks_numeric_v4 = self
            .address
            .split('.')
            .next()
            .is_some_and(|first| first.chars().all(|c| c.is_ascii_digit()));
        let address_ok = if valid_ipv6(&self.address) {
            true
        } else if looks_numeric_v4 {
            // "256.1.1.1" must be rejected as a malformed IPv4, not accepted as a hostname.
            valid_ipv4(&self.address)
        } else {
            valid_hostname(&self.address)
        };
        if !address_ok {
            return Err(DomainError::validation(format!(
                "\"{}\" is not a valid listener address",
                self.address
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(64)
                    .collect::<String>()
            ))
            .with_hint("use an IPv4 address, bare IPv6, or hostname (e.g. 0.0.0.0)"));
        }
        if self.port < LISTENER_PORT_MIN {
            return Err(DomainError::validation(format!(
                "listener port must be >= {LISTENER_PORT_MIN} (privileged ports are forbidden), got {}",
                self.port
            )));
        }
        if let Some(rc) = &self.route_config {
            crate::identity::validate_name(rc)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn spec(address: &str, port: u16) -> ListenerSpec {
        ListenerSpec {
            address: address.into(),
            port,
            route_config: None,
        }
    }

    #[test]
    fn valid_addresses_pass() {
        for address in [
            "0.0.0.0",
            "10.1.2.3",
            "::1",
            "fe80::1",
            "gateway.acme.test",
            "*.acme.test",
            "localhost",
        ] {
            assert!(
                spec(address, 8080).validate().is_ok(),
                "{address} should be valid"
            );
        }
    }

    #[test]
    fn adversarial_addresses_and_ports_rejected() {
        for address in [
            "",
            "256.1.1.1",
            "host..name",
            "-bad.example",
            "bad-.example",
            "a b",
            "[::1]",
        ] {
            assert!(
                spec(address, 8080).validate().is_err(),
                "{address:?} must be rejected"
            );
        }
        for port in [0u16, 1, 80, 443, 1023] {
            assert!(
                spec("0.0.0.0", port).validate().is_err(),
                "port {port} must be rejected"
            );
        }
        assert!(spec("0.0.0.0", 1024).validate().is_ok());
    }
}
