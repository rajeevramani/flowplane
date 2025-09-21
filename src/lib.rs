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
//! - **REST API Gateway**: Axum-based HTTP server for configuration management
//! - **Configuration Manager**: Translates REST API calls to Envoy xDS configurations
//! - **xDS Server**: Tonic-based gRPC server implementing Envoy discovery protocols
//! - **Persistence Layer**: SQLx with PostgreSQL for configuration storage
//!
//! ## Example Usage
//!
//! ```rust,no_run
//! use magaya::{Config, Result, Server};
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let config = Config::from_env()?;
//!     let server = Server::new(config).await?;
//!     server.run().await
//! }
//! ```

pub mod api;
pub mod auth;
pub mod config;
pub mod errors;
pub mod observability;
pub mod storage;
pub mod utils;
pub mod xds;

// Re-export commonly used types and traits
pub use config::{Config, Environment};
pub use errors::{Error, Result};
pub use observability::{init_tracing, HealthCheck};

/// Application version from Cargo.toml
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Application name from Cargo.toml
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");

/// Main server structure that orchestrates all components
pub struct Server {
    config: Config,
}

impl Server {
    /// Create a new server instance with the given configuration
    pub async fn new(config: Config) -> Result<Self> {
        Ok(Self { config })
    }

    /// Run the server with all components initialized
    pub async fn run(self) -> Result<()> {
        tracing::info!(
            app_name = APP_NAME,
            version = VERSION,
            "Starting Magaya control plane server"
        );

        // TODO: Initialize and start all server components
        // - HTTP API server
        // - xDS gRPC server
        // - Database connections
        // - Health checks
        // - Metrics collection

        tracing::info!("Server started successfully");

        // For now, just keep the server running
        tokio::signal::ctrl_c().await.map_err(Error::from)?;

        tracing::info!("Shutting down server");
        Ok(())
    }

    /// Get a reference to the server configuration
    pub fn config(&self) -> &Config {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_available() {
        assert!(!VERSION.is_empty());
        assert_eq!(APP_NAME, "magaya");
    }
}