//! # Flowplane
//!
//! Flowplane is an infrastructure-agnostic Envoy proxy control plane inspired by the
//! Sanskrit word *Pravāha* (प्रवाह), meaning "stream" or "steady flow." It provides
//! RESTful interfaces for Envoy configuration management, with planned extensions for
//! A2A (Agent-to-Agent) protocols and MCP (Model Context Protocol) integration.
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
//! - **REST API Gateway**: Axum-based HTTP server for configuration management
//! - **Persistence Layer**: SQLx repositories (SQLite by default, PostgreSQL planned)

pub mod api;
pub mod config;
pub mod errors;
pub mod storage;
pub mod utils;
pub mod xds;

// Re-export commonly used types and traits
pub use config::Config;
pub use errors::{Error, FlowplaneError, Result};

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
        assert_eq!(APP_NAME, "flowplane");
    }
}
