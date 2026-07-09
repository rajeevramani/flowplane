//! gRPC server construction for the Envoy-facing RateLimitService listener.

use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};

use crate::config::{RlsConfig, RlsGrpcTls};

pub fn grpc_server(config: &RlsConfig) -> Result<Server, String> {
    let server = Server::builder();
    match &config.grpc_tls {
        Some(tls) => server
            .tls_config(server_tls_config(tls)?)
            .map_err(|e| format!("invalid RLS gRPC TLS config: {e}")),
        None => Ok(server),
    }
}

pub fn server_tls_config(tls: &RlsGrpcTls) -> Result<ServerTlsConfig, String> {
    let cert = read_pem("FLOWPLANE_RLS_GRPC_TLS_CERT", &tls.cert_path)?;
    let key = read_pem("FLOWPLANE_RLS_GRPC_TLS_KEY", &tls.key_path)?;
    let client_ca = read_pem("FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA", &tls.client_ca_path)?;
    Ok(ServerTlsConfig::new()
        .identity(Identity::from_pem(cert, key))
        .client_ca_root(Certificate::from_pem(client_ca)))
}

fn read_pem(var: &str, path: &std::path::Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {var} {}: {e}", path.display()))
}
