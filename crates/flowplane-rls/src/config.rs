//! Process configuration, read from the environment. mTLS for the Envoy-facing gRPC and the
//! CP-facing admin endpoint is built in S6/S7 (DataplaneTlsConfig); S4 serves plaintext, which
//! is the dev path and is explicit here.

use std::net::SocketAddr;

#[derive(Debug, Clone, Copy)]
pub struct RlsConfig {
    /// Envoy-facing gRPC `RateLimitService` listen address.
    pub grpc_listen: SocketAddr,
    /// CP-facing HTTP admin (policy push + health) listen address.
    pub admin_listen: SocketAddr,
}

impl RlsConfig {
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            grpc_listen: parse_addr("FLOWPLANE_RLS_GRPC_LISTEN", "0.0.0.0:50051")?,
            admin_listen: parse_addr("FLOWPLANE_RLS_ADMIN_LISTEN", "0.0.0.0:8081")?,
        })
    }
}

fn parse_addr(var: &str, default: &str) -> Result<SocketAddr, String> {
    let raw = std::env::var(var).unwrap_or_else(|_| default.to_string());
    raw.parse()
        .map_err(|e| format!("{var}=\"{raw}\" is not a valid socket address: {e}"))
}
