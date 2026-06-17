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
    /// Public base URL clients use to reach this listener, e.g. `https://api.example.com`.
    /// This is product metadata for invocation descriptors, not an Envoy bind address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_base_url: Option<String>,
    /// Downstream HTTP protocol mode. `https` is HTTP over downstream TLS; TCP listener support
    /// remains intentionally deferred until V2 has a first-class TCP route action.
    #[serde(default, skip_serializing_if = "is_default_listener_protocol")]
    pub protocol: ListenerProtocol,
    /// Route configuration served by this listener, by name (same team). Optional until
    /// bound; resolved and reference-tracked by the service layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_config: Option<String>,
    /// HTTP filter chain, in order (S5.8). The router filter is appended automatically.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub http_filters: Vec<crate::gateway::filters::HttpFilterEntry>,
    /// File access logs attached to the HTTP connection manager.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access_logs: Vec<AccessLogConfig>,
    /// Downstream TLS for the single filter chain v2 currently emits. Certificate material
    /// may be inline file paths or SDS secret names delivered over ADS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_context: Option<ListenerTlsConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessLogConfig {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_format: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListenerProtocol {
    #[default]
    Http,
    Http2,
    Https,
}

fn is_default_listener_protocol(protocol: &ListenerProtocol) -> bool {
    *protocol == ListenerProtocol::Http
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ListenerTlsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_chain_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert_file: Option<String>,
    #[serde(default)]
    pub require_client_certificate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_certificate_sds_secret_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_context_sds_secret_name: Option<String>,
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
        if let Some(base_url) = &self.public_base_url {
            validate_public_base_url(base_url)?;
        }
        if let Some(rc) = &self.route_config {
            crate::identity::validate_name(rc)?;
        }
        if self.protocol == ListenerProtocol::Https && self.tls_context.is_none() {
            return Err(DomainError::validation(
                "https listener protocol requires tls_context",
            ));
        }
        if let Some(tls) = &self.tls_context {
            tls.validate()?;
        }
        if self.access_logs.len() > 8 {
            return Err(DomainError::validation(
                "listener access_logs must contain at most 8 entries",
            ));
        }
        for log in &self.access_logs {
            log.validate()?;
        }
        crate::gateway::filters::validate_filter_chain(&self.http_filters)?;
        Ok(())
    }
}

fn validate_public_base_url(value: &str) -> DomainResult<()> {
    if value.len() > 2048 {
        return Err(DomainError::validation(
            "listener public_base_url must be at most 2048 characters",
        ));
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(DomainError::validation(
            "listener public_base_url must not contain whitespace or control characters",
        ));
    }
    let (scheme, rest) = value.split_once("://").ok_or_else(|| {
        DomainError::validation("listener public_base_url must start with http:// or https://")
    })?;
    if scheme != "http" && scheme != "https" {
        return Err(DomainError::validation(
            "listener public_base_url must start with http:// or https://",
        ));
    }
    if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#') {
        return Err(DomainError::validation(
            "listener public_base_url must include a host",
        ));
    }
    if rest.contains('?') || rest.contains('#') {
        return Err(DomainError::validation(
            "listener public_base_url must not include query or fragment",
        ));
    }
    if rest.trim_end_matches('/').contains('/') {
        return Err(DomainError::validation(
            "listener public_base_url must not include a path",
        ));
    }
    Ok(())
}

impl AccessLogConfig {
    pub fn validate(&self) -> DomainResult<()> {
        validate_path("access_log.path", &self.path)?;
        if let Some(format) = &self.text_format {
            if format.is_empty() || format.len() > 4096 || format.contains('\0') {
                return Err(DomainError::validation(
                    "access_log.text_format must be 1-4096 chars and contain no NUL",
                ));
            }
        }
        Ok(())
    }
}

impl ListenerTlsConfig {
    pub fn validate(&self) -> DomainResult<()> {
        let inline_cert = self.cert_chain_file.is_some() || self.private_key_file.is_some();
        if inline_cert && (self.cert_chain_file.is_none() || self.private_key_file.is_none()) {
            return Err(DomainError::validation(
                "cert_chain_file and private_key_file must be set together",
            ));
        }
        if inline_cert && self.tls_certificate_sds_secret_name.is_some() {
            return Err(DomainError::validation(
                "TLS certificate source must be either file paths or an SDS secret name, not both",
            ));
        }
        if !inline_cert && self.tls_certificate_sds_secret_name.is_none() {
            return Err(DomainError::validation(
                "TLS context requires cert_chain_file/private_key_file or tls_certificate_sds_secret_name",
            ));
        }
        if self.ca_cert_file.is_some() && self.validation_context_sds_secret_name.is_some() {
            return Err(DomainError::validation(
                "validation context source must be either ca_cert_file or validation_context_sds_secret_name, not both",
            ));
        }
        if self.require_client_certificate
            && self.ca_cert_file.is_none()
            && self.validation_context_sds_secret_name.is_none()
        {
            return Err(DomainError::validation(
                "require_client_certificate needs ca_cert_file or validation_context_sds_secret_name",
            ));
        }
        for (name, path) in [
            ("cert_chain_file", &self.cert_chain_file),
            ("private_key_file", &self.private_key_file),
            ("ca_cert_file", &self.ca_cert_file),
        ] {
            if let Some(path) = path {
                validate_path(name, path)?;
            }
        }
        for secret in [
            &self.tls_certificate_sds_secret_name,
            &self.validation_context_sds_secret_name,
        ]
        .into_iter()
        .flatten()
        {
            crate::identity::validate_name(secret)?;
        }
        Ok(())
    }
}

fn validate_path(name: &str, value: &str) -> DomainResult<()> {
    if value.trim().is_empty() {
        return Err(DomainError::validation(format!("{name} must not be empty")));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(DomainError::validation(format!(
            "{name} must not contain control characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn spec(address: &str, port: u16) -> ListenerSpec {
        ListenerSpec {
            address: address.into(),
            port,
            public_base_url: None,
            protocol: ListenerProtocol::Http,
            route_config: None,
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: None,
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

    #[test]
    fn public_base_url_is_explicit_http_base_url() {
        let mut valid = spec("0.0.0.0", 8080);
        valid.public_base_url = Some("https://gateway.example".into());
        assert!(valid.validate().is_ok());

        for value in [
            "gateway.example",
            "ftp://gateway.example",
            "https://gateway.example/path",
            "https://gateway.example?q=1",
            "https://gateway.example#frag",
            "https://gateway.example bad",
        ] {
            let mut invalid = spec("0.0.0.0", 8080);
            invalid.public_base_url = Some(value.into());
            assert!(invalid.validate().is_err(), "{value} should be invalid");
        }
    }

    #[test]
    fn tls_context_validation_rejects_ambiguous_sources() {
        let mut spec = spec("0.0.0.0", 8443);
        spec.tls_context = Some(ListenerTlsConfig {
            cert_chain_file: Some("/certs/tls.crt".into()),
            private_key_file: None,
            ca_cert_file: None,
            require_client_certificate: false,
            tls_certificate_sds_secret_name: None,
            validation_context_sds_secret_name: None,
        });
        assert!(spec.validate().is_err(), "partial file pair rejected");

        spec.tls_context = Some(ListenerTlsConfig {
            cert_chain_file: None,
            private_key_file: None,
            ca_cert_file: None,
            require_client_certificate: false,
            tls_certificate_sds_secret_name: Some("edge-cert".into()),
            validation_context_sds_secret_name: None,
        });
        assert!(spec.validate().is_ok(), "SDS cert source accepted");

        spec.tls_context = Some(ListenerTlsConfig {
            cert_chain_file: None,
            private_key_file: None,
            ca_cert_file: Some("/certs/ca.crt".into()),
            require_client_certificate: false,
            tls_certificate_sds_secret_name: Some("edge-cert".into()),
            validation_context_sds_secret_name: Some("edge-ca".into()),
        });
        assert!(
            spec.validate().is_err(),
            "ambiguous validation source rejected"
        );
    }

    #[test]
    fn https_protocol_requires_tls_context() {
        let mut spec = spec("0.0.0.0", 8443);
        spec.protocol = ListenerProtocol::Https;
        assert!(spec.validate().is_err(), "https must name TLS material");

        spec.tls_context = Some(ListenerTlsConfig {
            cert_chain_file: None,
            private_key_file: None,
            ca_cert_file: None,
            require_client_certificate: false,
            tls_certificate_sds_secret_name: Some("edge-cert".into()),
            validation_context_sds_secret_name: None,
        });
        assert!(spec.validate().is_ok(), "https with SDS TLS is valid");
    }

    #[test]
    fn access_log_validation_is_bounded() {
        let mut spec = spec("0.0.0.0", 8080);
        spec.access_logs = vec![AccessLogConfig {
            path: "/var/log/envoy/access.log".into(),
            text_format: Some("%REQ(:METHOD)% %RESPONSE_CODE%\n".into()),
        }];
        assert!(spec.validate().is_ok());

        spec.access_logs[0].path = "".into();
        assert!(spec.validate().is_err(), "empty access log path rejected");
    }
}
