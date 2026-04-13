//! CLI + environment configuration for `flowplane-agent`.
//!
//! Env vars take precedence over clap defaults (clap's `env = "..."`
//! attribute handles this automatically).

use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "flowplane-agent",
    version,
    about = "Flowplane dataplane-side diagnostics agent (warming failure reporter)"
)]
pub struct AgentConfig {
    /// Envoy admin endpoint (MUST be loopback in prod; a WARN is logged if not).
    #[arg(long, env = "FLOWPLANE_AGENT_ENVOY_ADMIN_URL", default_value = "http://127.0.0.1:9901")]
    pub envoy_admin_url: String,

    /// Flowplane control plane gRPC endpoint (e.g. `https://cp.flowplane.local:50051`).
    #[arg(long, env = "FLOWPLANE_AGENT_CP_ENDPOINT")]
    pub cp_endpoint: String,

    /// Interval between `/config_dump` polls, in seconds.
    #[arg(long, env = "FLOWPLANE_AGENT_POLL_INTERVAL_SECS", default_value_t = 10)]
    pub poll_interval_secs: u64,

    /// Dataplane identity this agent reports for. Must match the SPIFFE
    /// identity presented by the client cert when mTLS is enabled.
    #[arg(long, env = "FLOWPLANE_AGENT_DATAPLANE_ID")]
    pub dataplane_id: String,

    /// PEM-encoded client certificate for mTLS to the CP.
    #[arg(long, env = "FLOWPLANE_AGENT_TLS_CERT_PATH")]
    pub tls_cert_path: Option<String>,

    /// PEM-encoded client private key for mTLS to the CP.
    #[arg(long, env = "FLOWPLANE_AGENT_TLS_KEY_PATH")]
    pub tls_key_path: Option<String>,

    /// Optional PEM-encoded CA bundle to verify the CP's server cert.
    #[arg(long, env = "FLOWPLANE_AGENT_TLS_CA_PATH")]
    pub tls_ca_path: Option<String>,

    /// Maximum number of buffered reports before dropping oldest on overflow.
    #[arg(long, env = "FLOWPLANE_AGENT_QUEUE_CAP", default_value_t = 256)]
    pub queue_cap: usize,
}

/// Heuristic check: does `url` point at a loopback host?
///
/// Accepts `http://127.0.0.1[:port]`, `http://localhost[:port]`, `http://[::1][:port]`,
/// `http://::1[:port]`. Anything else is treated as non-loopback and the caller
/// logs a WARN on startup (but does NOT refuse — the bead explicitly says to
/// continue with a warning, to support unusual topologies).
pub fn admin_is_loopback(url: &str) -> bool {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host_port = rest.split('/').next().unwrap_or("");
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        // IPv6 literal: strip brackets and drop anything after the closing bracket
        stripped.split(']').next().unwrap_or("")
    } else {
        // IPv4 or hostname: drop :port if present
        host_port.rsplit_once(':').map(|(h, _)| h).unwrap_or(host_port)
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_ipv4_default() {
        assert!(admin_is_loopback("http://127.0.0.1:9901"));
    }

    #[test]
    fn loopback_localhost_no_port() {
        assert!(admin_is_loopback("http://localhost"));
    }

    #[test]
    fn loopback_localhost_with_path() {
        assert!(admin_is_loopback("http://localhost:9901/config_dump"));
    }

    #[test]
    fn loopback_ipv6_bracketed() {
        assert!(admin_is_loopback("http://[::1]:9901"));
    }

    #[test]
    fn non_loopback_public_ip() {
        assert!(!admin_is_loopback("http://203.0.113.7:9901"));
    }

    #[test]
    fn non_loopback_hostname() {
        assert!(!admin_is_loopback("http://envoy.prod.svc.cluster.local:9901"));
    }

    #[test]
    fn non_loopback_public_ipv6() {
        assert!(!admin_is_loopback("http://[2001:db8::1]:9901"));
    }

    #[test]
    fn malformed_no_scheme_still_handled() {
        // Best-effort: we don't crash, we just classify as non-loopback if it's
        // unparseable. This exists so a typo in env var doesn't panic the agent.
        assert!(!admin_is_loopback("nonsense"));
    }
}
