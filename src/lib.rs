//! # Magaya
//!
//! Magaya (mägaya rom - "still/quiet" in Gupapuyngu) is an infrastructure-agnostic
//! Envoy proxy control plane that provides RESTful interfaces for Envoy configuration
//! management, with planned extensions for A2A (Agent-to-Agent) protocols and MCP
//! (Model Context Protocol) integration.
//!
//! ## Architecture
//!
//! The system follows a layered architecture pattern:
//!
//! ```text
//! REST API Layer → Configuration Manager → Envoy xDS Server → Envoy Proxies
//!      ↓                    ↓                     ↓
//! Authentication    Persistence Layer    Observability Stack
//! ```
//!
//! ## Core Components
//!
//! - **xDS Server**: Tonic-based gRPC server implementing Envoy discovery protocols
//! - **Configuration Manager**: Translates REST API calls to Envoy xDS configurations
//! - **REST API Gateway**: Axum-based HTTP server for configuration management (planned)
//! - **Persistence Layer**: SQLx with PostgreSQL for configuration storage (planned)

pub mod config;
pub mod errors;
pub mod xds;

// Re-export commonly used types and traits
pub use config::Config;
pub use errors::{Error, Result};

/// Application version from Cargo.toml
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Application name from Cargo.toml
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_available() {
        assert!(!VERSION.is_empty());
        assert_eq!(APP_NAME, "magaya");
    }
}
