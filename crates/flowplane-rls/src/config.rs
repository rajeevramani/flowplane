//! Process configuration, read from the environment. The Envoy-facing gRPC listener is plaintext
//! only when an explicit loopback dev escape hatch is set; split-node binds require server-side
//! TLS plus a client CA for Envoy/dataplane client-certificate validation.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredential {
    token: String,
}

impl AdminCredential {
    pub fn new(token: String) -> Result<Self, String> {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err("RLS admin credential token must not be empty".to_string());
        }
        Ok(Self { token })
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RlsGrpcTls {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub client_ca_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RlsConfig {
    /// Envoy-facing gRPC `RateLimitService` listen address.
    pub grpc_listen: SocketAddr,
    /// CP-facing HTTP admin (policy push + health) listen address.
    pub admin_listen: SocketAddr,
    /// Credential required for mutating CP→RLS admin requests. `None` is accepted only with the
    /// explicit local-only unauthenticated escape hatch on a loopback admin bind.
    pub admin_credential: Option<AdminCredential>,
    /// Envoy-facing gRPC TLS material. When present, Envoy clients must present certificates
    /// chaining to `client_ca_path`.
    pub grpc_tls: Option<RlsGrpcTls>,
    /// True only for explicitly acknowledged loopback plaintext gRPC in local development.
    pub allow_insecure_grpc: bool,
}

impl RlsConfig {
    pub fn from_env() -> Result<Self, String> {
        let grpc_listen = parse_addr("FLOWPLANE_RLS_GRPC_LISTEN", "0.0.0.0:50051")?;
        let admin_listen = parse_addr("FLOWPLANE_RLS_ADMIN_LISTEN", "0.0.0.0:8081")?;
        let admin_credential = match std::env::var("FLOWPLANE_RLS_ADMIN_TOKEN_FILE") {
            Ok(path) => Some(read_admin_credential_file(Path::new(&path))?),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err("FLOWPLANE_RLS_ADMIN_TOKEN_FILE is not valid Unicode".to_string())
            }
        };
        let allow_unauth_admin = std::env::var("FLOWPLANE_RLS_ALLOW_UNAUTH_ADMIN")
            .ok()
            .as_deref()
            == Some("yes-this-is-local-only");
        let allow_insecure_grpc = std::env::var("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC")
            .ok()
            .as_deref()
            == Some("yes-this-is-local-only");
        let grpc_tls = resolve_grpc_tls_from_env()?;
        Self::resolve(
            grpc_listen,
            admin_listen,
            admin_credential,
            grpc_tls,
            allow_unauth_admin,
            allow_insecure_grpc,
        )
    }

    pub fn resolve(
        grpc_listen: SocketAddr,
        admin_listen: SocketAddr,
        admin_credential: Option<AdminCredential>,
        grpc_tls: Option<RlsGrpcTls>,
        allow_unauth_admin: bool,
        allow_insecure_grpc: bool,
    ) -> Result<Self, String> {
        if grpc_tls.is_none() {
            if !allow_insecure_grpc {
                return Err(
                    "set FLOWPLANE_RLS_GRPC_TLS_CERT, FLOWPLANE_RLS_GRPC_TLS_KEY, and \
                     FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA for authenticated RLS gRPC; plaintext \
                     requires FLOWPLANE_RLS_ALLOW_INSECURE_GRPC=yes-this-is-local-only on a \
                     loopback bind"
                        .to_string(),
                );
            }
            if !grpc_listen.ip().is_loopback() {
                return Err(
                    "FLOWPLANE_RLS_ALLOW_INSECURE_GRPC=yes-this-is-local-only is only valid for a \
                     loopback RLS gRPC bind; non-loopback binds require the RLS gRPC TLS triad"
                        .to_string(),
                );
            }
        }
        if admin_credential.is_none() {
            if !allow_unauth_admin {
                return Err(
                    "FLOWPLANE_RLS_ADMIN_TOKEN_FILE is required for the RLS admin listener"
                        .to_string(),
                );
            }
            if !admin_listen.ip().is_loopback() {
                return Err(
                    "FLOWPLANE_RLS_ALLOW_UNAUTH_ADMIN=yes-this-is-local-only is only valid for a loopback RLS admin bind"
                        .to_string(),
                );
            }
        }
        Ok(Self {
            grpc_listen,
            admin_listen,
            admin_credential,
            grpc_tls,
            allow_insecure_grpc,
        })
    }
}

fn resolve_grpc_tls_from_env() -> Result<Option<RlsGrpcTls>, String> {
    let cert_path = optional_path("FLOWPLANE_RLS_GRPC_TLS_CERT")?;
    let key_path = optional_path("FLOWPLANE_RLS_GRPC_TLS_KEY")?;
    let client_ca_path = optional_path("FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA")?;
    resolve_grpc_tls(cert_path, key_path, client_ca_path)
}

fn resolve_grpc_tls(
    cert_path: Option<PathBuf>,
    key_path: Option<PathBuf>,
    client_ca_path: Option<PathBuf>,
) -> Result<Option<RlsGrpcTls>, String> {
    match (cert_path, key_path, client_ca_path) {
        (None, None, None) => Ok(None),
        (Some(cert_path), Some(key_path), Some(client_ca_path)) => Ok(Some(RlsGrpcTls {
            cert_path,
            key_path,
            client_ca_path,
        })),
        _ => Err(
            "FLOWPLANE_RLS_GRPC_TLS_CERT, FLOWPLANE_RLS_GRPC_TLS_KEY, and \
             FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA must be set together"
                .to_string(),
        ),
    }
}

fn optional_path(var: &str) -> Result<Option<PathBuf>, String> {
    match std::env::var(var) {
        Ok(path) if path.trim().is_empty() => {
            Err(format!("{var} must not be empty when configured"))
        }
        Ok(path) => Ok(Some(PathBuf::from(path))),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{var} is not valid Unicode")),
    }
}

fn read_admin_credential_file(path: &Path) -> Result<AdminCredential, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "cannot read FLOWPLANE_RLS_ADMIN_TOKEN_FILE {}: {e}",
            path.display()
        )
    })?;
    AdminCredential::new(raw)
}

fn parse_addr(var: &str, default: &str) -> Result<SocketAddr, String> {
    let raw = std::env::var(var).unwrap_or_else(|_| default.to_string());
    raw.parse()
        .map_err(|e| format!("{var}=\"{raw}\" is not a valid socket address: {e}"))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn addr(raw: &str) -> SocketAddr {
        raw.parse().unwrap()
    }

    fn tls() -> RlsGrpcTls {
        RlsGrpcTls {
            cert_path: "/cert.pem".into(),
            key_path: "/key.pem".into(),
            client_ca_path: "/client-ca.pem".into(),
        }
    }

    #[test]
    fn rejects_non_loopback_admin_bind_without_credential_or_escape_hatch() {
        let err = RlsConfig::resolve(
            addr("127.0.0.1:50051"),
            addr("0.0.0.0:8081"),
            None,
            None,
            false,
            true,
        )
        .expect_err("unsafe admin bind must fail closed");
        assert!(err.contains("FLOWPLANE_RLS_ADMIN_TOKEN_FILE"));
    }

    #[test]
    fn rejects_non_loopback_admin_bind_with_unauth_escape_hatch() {
        let err = RlsConfig::resolve(
            addr("127.0.0.1:50051"),
            addr("0.0.0.0:8081"),
            None,
            None,
            true,
            true,
        )
        .expect_err("local-only escape hatch must not apply to non-loopback binds");
        assert!(err.contains("loopback"));
    }

    #[test]
    fn rejects_insecure_grpc_dev_gate_on_non_loopback_even_with_admin_credential() {
        let err = RlsConfig::resolve(
            addr("0.0.0.0:50051"),
            addr("0.0.0.0:8081"),
            Some(AdminCredential::new("secret\n".to_string()).unwrap()),
            None,
            false,
            true,
        )
        .expect_err("insecure gRPC escape hatch must not apply to non-loopback binds");
        assert!(err.contains("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC"));
    }

    #[test]
    fn accepts_loopback_grpc_and_token_file_equivalent_admin_credential() {
        let cfg = RlsConfig::resolve(
            addr("127.0.0.1:50051"),
            addr("0.0.0.0:8081"),
            Some(AdminCredential::new("secret\n".to_string()).unwrap()),
            None,
            false,
            true,
        )
        .expect("credentialed admin plus explicit loopback insecure gRPC is allowed");
        assert_eq!(
            cfg.admin_credential.as_ref().map(AdminCredential::token),
            Some("secret")
        );
        assert!(cfg.grpc_tls.is_none());
        assert!(cfg.allow_insecure_grpc);
    }

    #[test]
    fn accepts_explicit_local_only_unauth_escape_hatch_on_loopback_bind() {
        let cfg = RlsConfig::resolve(
            addr("127.0.0.1:50051"),
            addr("127.0.0.1:8081"),
            None,
            None,
            true,
            true,
        )
        .expect("loopback escape hatch is allowed");
        assert!(cfg.admin_credential.is_none());
    }

    #[test]
    fn rejects_plaintext_grpc_without_explicit_dev_gate() {
        let err = RlsConfig::resolve(
            addr("127.0.0.1:50051"),
            addr("127.0.0.1:8081"),
            None,
            None,
            true,
            false,
        )
        .expect_err("production insecure RLS gRPC must fail closed");
        assert!(err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT"));
    }

    #[test]
    fn rejects_insecure_grpc_dev_gate_on_non_loopback_bind() {
        let err = RlsConfig::resolve(
            addr("0.0.0.0:50051"),
            addr("127.0.0.1:8081"),
            None,
            None,
            true,
            true,
        )
        .expect_err("insecure gRPC dev gate must be loopback-only");
        assert!(err.contains("loopback RLS gRPC bind"));
    }

    #[test]
    fn rejects_partial_grpc_tls_material() {
        let err = resolve_grpc_tls(Some("/cert.pem".into()), None, Some("/ca.pem".into()))
            .expect_err("partial TLS material must fail closed");
        assert!(err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT"));
        assert!(err.contains("FLOWPLANE_RLS_GRPC_TLS_KEY"));
        assert!(err.contains("FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA"));
    }

    #[test]
    fn accepts_complete_grpc_tls_material() {
        let resolved = resolve_grpc_tls(
            Some("/cert.pem".into()),
            Some("/key.pem".into()),
            Some("/ca.pem".into()),
        )
        .expect("complete TLS material is accepted");
        assert_eq!(
            resolved,
            Some(RlsGrpcTls {
                cert_path: "/cert.pem".into(),
                key_path: "/key.pem".into(),
                client_ca_path: "/ca.pem".into(),
            })
        );
    }

    #[test]
    fn accepts_non_loopback_grpc_when_tls_triad_is_configured() {
        let cfg = RlsConfig::resolve(
            addr("0.0.0.0:50051"),
            addr("127.0.0.1:8081"),
            None,
            Some(tls()),
            true,
            false,
        )
        .expect("authenticated split-node gRPC bind is allowed");
        assert_eq!(cfg.grpc_tls, Some(tls()));
        assert!(!cfg.allow_insecure_grpc);
    }

    #[test]
    fn keeps_admin_credential_separate_from_grpc_tls() {
        let cfg = RlsConfig::resolve(
            addr("0.0.0.0:50051"),
            addr("0.0.0.0:8081"),
            Some(AdminCredential::new("admin-secret".to_string()).unwrap()),
            Some(tls()),
            false,
            false,
        )
        .expect("gRPC mTLS does not replace or block CP-facing admin auth");
        assert_eq!(
            cfg.admin_credential.as_ref().map(AdminCredential::token),
            Some("admin-secret")
        );
        assert!(cfg.grpc_tls.is_some());
    }
}
