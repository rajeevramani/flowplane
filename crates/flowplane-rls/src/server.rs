//! Server construction for the Envoy-facing gRPC listener (fpv2-9sf S1).
//!
//! When the `FLOWPLANE_RLS_GRPC_TLS_*` triad is configured the listener terminates mTLS: it
//! presents the server identity and **requires** a client certificate chaining to the
//! configured client CA. tonic's `client_ca_root` makes client auth mandatory — this module
//! must never call `client_auth_optional(true)` (that would silently drop the Envoy-fleet
//! authentication this feature exists to add).

use std::path::Path;

use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};

use crate::config::{RlsConfig, RlsGrpcTls};

/// Read one PEM file, surfacing unreadable, empty, and non-PEM material as a boot error
/// naming the offending variable and path. (A structurally-PEM-but-unparsable file is
/// rejected by `Server::builder().tls_config(..)` below — also at boot, with the triad's
/// variables/paths attached — never as a silent plaintext fallback.)
fn read_pem(path: &Path, what: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("cannot read RLS {what} at {}: {e}", path.display()))?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(format!("RLS {what} at {} is empty", path.display()));
    }
    if !bytes.windows(10).any(|w| w == b"-----BEGIN") {
        return Err(format!(
            "RLS {what} at {} is not PEM (no '-----BEGIN' block)",
            path.display()
        ));
    }
    Ok(bytes)
}

/// Build the mandatory-mTLS server TLS config from the triad.
pub fn server_tls_config(tls: &RlsGrpcTls) -> Result<ServerTlsConfig, String> {
    let identity = Identity::from_pem(
        read_pem(
            &tls.cert_path,
            "gRPC server certificate (FLOWPLANE_RLS_GRPC_TLS_CERT)",
        )?,
        read_pem(
            &tls.key_path,
            "gRPC server key (FLOWPLANE_RLS_GRPC_TLS_KEY)",
        )?,
    );
    let client_ca = Certificate::from_pem(read_pem(
        &tls.client_ca_path,
        "gRPC client CA (FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA)",
    )?);
    Ok(ServerTlsConfig::new()
        .identity(identity)
        .client_ca_root(client_ca))
}

/// The gRPC server builder for the resolved config: mTLS when the triad is present, plain
/// (loopback dev, gated by the config layer) otherwise. TLS material problems — unreadable,
/// empty, or malformed PEM — surface here as a boot error.
pub fn grpc_server(config: &RlsConfig) -> Result<Server, String> {
    match &config.grpc_tls {
        Some(tls) => Server::builder()
            .tls_config(server_tls_config(tls)?)
            .map_err(|e| {
                format!(
                    "invalid RLS gRPC TLS material (check FLOWPLANE_RLS_GRPC_TLS_CERT={}, \
                     FLOWPLANE_RLS_GRPC_TLS_KEY={}, FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA={}): {e}",
                    tls.cert_path.display(),
                    tls.key_path.display(),
                    tls.client_ca_path.display()
                )
            }),
        None => Ok(Server::builder()),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn triad(dir: &Path) -> RlsGrpcTls {
        RlsGrpcTls {
            cert_path: dir.join("server.pem"),
            key_path: dir.join("server.key"),
            client_ca_path: dir.join("client-ca.pem"),
        }
    }

    // AC9: unreadable PEM path fails closed with the offending material named.
    #[test]
    fn unreadable_pem_is_named_error() {
        let tls = triad(&PathBuf::from("/nonexistent-fpv2-9sf"));
        let err = server_tls_config(&tls).unwrap_err();
        assert!(err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT"), "{err}");
        assert!(err.contains("/nonexistent-fpv2-9sf"), "{err}");
    }

    // AC9: empty PEM fails closed.
    #[test]
    fn empty_pem_is_named_error() {
        let dir = std::env::temp_dir().join(format!("rls-s1-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["server.pem", "server.key", "client-ca.pem"] {
            std::fs::write(dir.join(f), b"\n").unwrap();
        }
        let err = server_tls_config(&triad(&dir)).unwrap_err();
        assert!(err.contains("is empty"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // AC9: malformed (non-PEM) material is a boot error naming the offending var/path.
    #[test]
    fn malformed_pem_fails_at_server_build() {
        let dir = std::env::temp_dir().join(format!("rls-s1-garbage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["server.pem", "server.key", "client-ca.pem"] {
            std::fs::write(dir.join(f), b"not a pem at all").unwrap();
        }
        let config = RlsConfig {
            grpc_listen: "127.0.0.1:0".parse().unwrap(),
            admin_listen: "127.0.0.1:0".parse().unwrap(),
            grpc_tls: Some(triad(&dir)),
        };
        let err = grpc_server(&config).unwrap_err();
        assert!(
            err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT"),
            "names the var: {err}"
        );
        assert!(err.contains("server.pem"), "names the path: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // AC9: PEM-shaped but unparsable material also fails at boot, with the triad's
    // vars/paths attached (tonic can't say which file, so all three are named).
    #[test]
    fn corrupt_pem_body_fails_at_server_build_naming_vars() {
        let dir = std::env::temp_dir().join(format!("rls-s1-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake =
            b"-----BEGIN CERTIFICATE-----\nnot base64 at all!!!\n-----END CERTIFICATE-----\n";
        for f in ["server.pem", "server.key", "client-ca.pem"] {
            std::fs::write(dir.join(f), fake).unwrap();
        }
        let config = RlsConfig {
            grpc_listen: "127.0.0.1:0".parse().unwrap(),
            admin_listen: "127.0.0.1:0".parse().unwrap(),
            grpc_tls: Some(triad(&dir)),
        };
        let err = grpc_server(&config).unwrap_err();
        assert!(
            err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT")
                && err.contains("FLOWPLANE_RLS_GRPC_TLS_KEY")
                && err.contains("FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA"),
            "names the triad vars: {err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
