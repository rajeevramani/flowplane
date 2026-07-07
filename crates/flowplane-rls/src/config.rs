//! Process configuration, read from the environment. mTLS for the Envoy-facing gRPC and the
//! CP-facing admin endpoint is built in S6/S7 (DataplaneTlsConfig); S4 serves plaintext, which
//! is the dev path and is explicit here.

use std::net::SocketAddr;
use std::path::Path;

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

#[derive(Debug, Clone)]
pub struct RlsConfig {
    /// Envoy-facing gRPC `RateLimitService` listen address.
    pub grpc_listen: SocketAddr,
    /// CP-facing HTTP admin (policy push + health) listen address.
    pub admin_listen: SocketAddr,
    /// Credential required for mutating CP→RLS admin requests. `None` is accepted only with the
    /// explicit local-only unauthenticated escape hatch on a loopback admin bind.
    pub admin_credential: Option<AdminCredential>,
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
        Self::resolve(
            grpc_listen,
            admin_listen,
            admin_credential,
            allow_unauth_admin,
        )
    }

    pub fn resolve(
        grpc_listen: SocketAddr,
        admin_listen: SocketAddr,
        admin_credential: Option<AdminCredential>,
        allow_unauth_admin: bool,
    ) -> Result<Self, String> {
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
        })
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

    #[test]
    fn rejects_non_loopback_admin_bind_without_credential_or_escape_hatch() {
        let err = RlsConfig::resolve(addr("127.0.0.1:50051"), addr("0.0.0.0:8081"), None, false)
            .expect_err("unsafe admin bind must fail closed");
        assert!(err.contains("FLOWPLANE_RLS_ADMIN_TOKEN_FILE"));
    }

    #[test]
    fn rejects_non_loopback_admin_bind_with_unauth_escape_hatch() {
        let err = RlsConfig::resolve(addr("127.0.0.1:50051"), addr("0.0.0.0:8081"), None, true)
            .expect_err("local-only escape hatch must not apply to non-loopback binds");
        assert!(err.contains("loopback"));
    }

    #[test]
    fn accepts_token_file_equivalent_credential_on_non_loopback_bind() {
        let cfg = RlsConfig::resolve(
            addr("0.0.0.0:50051"),
            addr("0.0.0.0:8081"),
            Some(AdminCredential::new("secret\n".to_string()).unwrap()),
            false,
        )
        .expect("credentialed non-loopback admin bind is allowed");
        assert_eq!(
            cfg.admin_credential.as_ref().map(AdminCredential::token),
            Some("secret")
        );
    }

    #[test]
    fn accepts_explicit_local_only_unauth_escape_hatch_on_loopback_bind() {
        let cfg = RlsConfig::resolve(addr("127.0.0.1:50051"), addr("127.0.0.1:8081"), None, true)
            .expect("loopback escape hatch is allowed");
        assert!(cfg.admin_credential.is_none());
    }
}
