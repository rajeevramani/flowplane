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
pub mod auth;
pub mod cli;
pub mod config;
pub mod domain;
pub mod errors;
pub mod mcp;
pub mod observability;
pub mod openapi;
pub mod schema;
pub mod secrets;
pub mod services;
pub mod startup;
pub mod storage;
pub mod utils;
pub mod validation;
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
    fn version_looks_like_semver() {
        let components: Vec<_> = VERSION.split('.').collect();
        assert!(components.len() >= 3, "version should follow semver: {VERSION}");
        assert!(components.iter().all(|part| !part.is_empty()));
        assert_eq!(APP_NAME, "flowplane");
    }
}
