//! Process configuration, read from the environment — **fail-closed** (constitution invariant
//! 10, feature fpv2-9sf S1): a non-loopback gRPC bind refuses to start without the full server
//! TLS triad, and plaintext is available only on a loopback bind behind an explicit
//! acknowledged escape hatch. Partial TLS material is always a hard boot error.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

/// The value an insecure escape hatch must literally equal — an explicit operator
/// acknowledgement, mirroring the CP's dev-mode / bootstrap-token opt-ins.
pub const INSECURE_ACK: &str = "yes-this-is-local-only";

/// Server TLS triad for the Envoy-facing gRPC listener: the RLS presents `cert`/`key` and
/// validates the Envoy client certificate against `client_ca` (mTLS, all-or-none).
#[derive(Debug, Clone)]
pub struct RlsGrpcTls {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    /// CA bundle that Envoy client certificates must chain to.
    pub client_ca_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RlsConfig {
    /// Envoy-facing gRPC `RateLimitService` listen address.
    pub grpc_listen: SocketAddr,
    /// CP-facing HTTP admin (policy push + health) listen address.
    pub admin_listen: SocketAddr,
    /// mTLS material for the gRPC listener. `None` => loopback plaintext dev mode (only
    /// reachable behind `FLOWPLANE_RLS_ALLOW_INSECURE_GRPC`).
    pub grpc_tls: Option<RlsGrpcTls>,
}

impl RlsConfig {
    pub fn from_env() -> Result<Self, String> {
        let env: HashMap<String, String> = std::env::vars().collect();
        Self::resolve(&env)
    }

    /// Fail-closed resolution (testable: env is injected). Per listener the rule is:
    /// TLS material present (all of it) => serve TLS; absent => refuse to start unless the
    /// bind is loopback AND the explicit escape hatch is set.
    pub fn resolve(env: &HashMap<String, String>) -> Result<Self, String> {
        let get = |key: &str| env.get(key).map(String::as_str);

        let grpc_listen = parse_addr(env, "FLOWPLANE_RLS_GRPC_LISTEN", "127.0.0.1:50051")?;
        let admin_listen = parse_addr(env, "FLOWPLANE_RLS_ADMIN_LISTEN", "127.0.0.1:8081")?;

        let grpc_tls = resolve_grpc_tls(env)?;
        if grpc_tls.is_none() {
            let hatch = get("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC");
            if !is_loopback(&grpc_listen) {
                return Err(format!(
                    "FLOWPLANE_RLS_GRPC_LISTEN={grpc_listen} is not loopback and no gRPC server \
                     TLS is configured: set FLOWPLANE_RLS_GRPC_TLS_CERT, \
                     FLOWPLANE_RLS_GRPC_TLS_KEY and FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA (mTLS), or \
                     bind loopback for local development"
                ));
            }
            if hatch != Some(INSECURE_ACK) {
                return Err(format!(
                    "plaintext gRPC on {grpc_listen} requires the explicit acknowledgement \
                     FLOWPLANE_RLS_ALLOW_INSECURE_GRPC={INSECURE_ACK} (loopback dev only), or \
                     configure the FLOWPLANE_RLS_GRPC_TLS_* triad"
                ));
            }
        }

        Ok(Self {
            grpc_listen,
            admin_listen,
            grpc_tls,
        })
    }
}

/// All-or-none gRPC TLS triad: any strict subset is a hard configuration error.
fn resolve_grpc_tls(env: &HashMap<String, String>) -> Result<Option<RlsGrpcTls>, String> {
    let get = |key: &str| env.get(key).map(String::as_str);
    let cert = get("FLOWPLANE_RLS_GRPC_TLS_CERT");
    let key = get("FLOWPLANE_RLS_GRPC_TLS_KEY");
    let client_ca = get("FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA");
    match (cert, key, client_ca) {
        (Some(cert), Some(key), Some(client_ca)) => Ok(Some(RlsGrpcTls {
            cert_path: cert.into(),
            key_path: key.into(),
            client_ca_path: client_ca.into(),
        })),
        (None, None, None) => Ok(None),
        _ => {
            let missing: Vec<&str> = [
                ("FLOWPLANE_RLS_GRPC_TLS_CERT", cert),
                ("FLOWPLANE_RLS_GRPC_TLS_KEY", key),
                ("FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA", client_ca),
            ]
            .iter()
            .filter(|(_, v)| v.is_none())
            .map(|(name, _)| *name)
            .collect();
            Err(format!(
                "partial gRPC TLS configuration: {} not set (the triad is all-or-none)",
                missing.join(", ")
            ))
        }
    }
}

/// Literal loopback test on a bind address (constitution invariant 4: loopback is a dev-only
/// convenience). Covers `127.0.0.0/8`, `::1`, and the IPv4-mapped `::ffff:127.0.0.0/8`.
/// Bind addresses are IP literals by construction (`SocketAddr` parse), so there is no
/// hostname — and deliberately no DNS resolution — in this decision.
fn is_loopback(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

fn parse_addr(
    env: &HashMap<String, String>,
    var: &str,
    default: &str,
) -> Result<SocketAddr, String> {
    let raw = env.get(var).map(String::as_str).unwrap_or(default);
    raw.parse()
        .map_err(|e| format!("{var}=\"{raw}\" is not a valid socket address: {e}"))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    const TRIAD: [(&str, &str); 3] = [
        ("FLOWPLANE_RLS_GRPC_TLS_CERT", "/certs/server.pem"),
        ("FLOWPLANE_RLS_GRPC_TLS_KEY", "/certs/server.key"),
        ("FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA", "/certs/client-ca.pem"),
    ];

    // AC1: non-loopback plaintext bind fails closed, naming the missing material.
    #[test]
    fn non_loopback_plaintext_grpc_fails_closed() {
        let e = env(&[("FLOWPLANE_RLS_GRPC_LISTEN", "0.0.0.0:50051")]);
        let err = RlsConfig::resolve(&e).unwrap_err();
        assert!(
            err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT"),
            "names the material: {err}"
        );
        assert!(err.contains("not loopback"), "{err}");
    }

    // AC1 variant: non-loopback even WITH the escape hatch is still an error.
    #[test]
    fn escape_hatch_is_loopback_only() {
        let e = env(&[
            ("FLOWPLANE_RLS_GRPC_LISTEN", "0.0.0.0:50051"),
            ("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC", INSECURE_ACK),
        ]);
        assert!(
            RlsConfig::resolve(&e).is_err(),
            "hatch must not unlock a non-loopback bind"
        );
    }

    // AC2: loopback plaintext requires the explicit escape hatch.
    #[test]
    fn loopback_plaintext_requires_hatch() {
        // Default bind (loopback), no TLS, no hatch: refuse.
        let err = RlsConfig::resolve(&HashMap::new()).unwrap_err();
        assert!(err.contains("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC"), "{err}");

        // Same with the hatch: allowed, plaintext dev.
        let e = env(&[("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC", INSECURE_ACK)]);
        let cfg = RlsConfig::resolve(&e).unwrap();
        assert!(cfg.grpc_tls.is_none());
        assert_eq!(cfg.grpc_listen, "127.0.0.1:50051".parse().unwrap());
        assert_eq!(cfg.admin_listen, "127.0.0.1:8081".parse().unwrap());

        // A wrong hatch value is not an acknowledgement.
        let e = env(&[("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC", "yes")]);
        assert!(RlsConfig::resolve(&e).is_err());
    }

    // AC3: partial triad is a hard error regardless of bind.
    #[test]
    fn partial_triad_is_hard_error() {
        for keep in 0..3 {
            for keep2 in keep..3 {
                let subset: Vec<(&str, &str)> = TRIAD
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i == keep || *i == keep2)
                    .map(|(_, kv)| *kv)
                    .collect();
                if subset.len() == 3 {
                    continue;
                }
                let e = env(&subset);
                let err = RlsConfig::resolve(&e).unwrap_err();
                assert!(err.contains("all-or-none"), "subset {subset:?}: {err}");
            }
        }
    }

    #[test]
    fn full_triad_resolves_tls_on_any_bind() {
        let mut pairs = TRIAD.to_vec();
        pairs.push(("FLOWPLANE_RLS_GRPC_LISTEN", "0.0.0.0:50051"));
        let cfg = RlsConfig::resolve(&env(&pairs)).unwrap();
        let tls = cfg.grpc_tls.expect("triad => TLS");
        assert_eq!(tls.cert_path, PathBuf::from("/certs/server.pem"));
        assert_eq!(tls.key_path, PathBuf::from("/certs/server.key"));
        assert_eq!(tls.client_ca_path, PathBuf::from("/certs/client-ca.pem"));
    }

    #[test]
    fn loopback_literals() {
        for addr in [
            "127.0.0.1:1",
            "127.9.8.7:1",
            "[::1]:1",
            "[::ffff:127.0.0.1]:1",
        ] {
            assert!(is_loopback(&addr.parse().unwrap()), "{addr} is loopback");
        }
        for addr in [
            "0.0.0.0:1",
            "10.0.0.1:1",
            "[::]:1",
            "[2001:db8::1]:1",
            "[::ffff:10.0.0.1]:1",
        ] {
            assert!(
                !is_loopback(&addr.parse().unwrap()),
                "{addr} is not loopback"
            );
        }
    }

    #[test]
    fn invalid_addr_is_named_error() {
        let e = env(&[
            ("FLOWPLANE_RLS_GRPC_LISTEN", "localhost:50051"),
            ("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC", INSECURE_ACK),
        ]);
        // Bind addresses are IP literals: hostname binds (even "localhost") don't parse.
        let err = RlsConfig::resolve(&e).unwrap_err();
        assert!(err.contains("FLOWPLANE_RLS_GRPC_LISTEN"), "{err}");
    }
}
