//! Pluggable secret backend architecture
//!
//! This module provides a unified interface for fetching secrets from various backends.
//! Flowplane can store only REFERENCES to secrets (not values) and fetch on-demand.
//!
//! ## Supported Backends
//!
//! - **Vault**: HashiCorp Vault KV v2 engine
//! - **Database**: Legacy mode - encrypted secrets stored in SQLite/PostgreSQL
//! - **AWS Secrets Manager**: (Optional feature)
//! - **GCP Secret Manager**: (Optional feature)

pub mod backend;
pub mod cache;
pub mod database;
pub mod registry;
pub mod vault;

pub use backend::{SecretBackend, SecretBackendType};
pub use cache::SecretCache;
pub use database::DatabaseSecretBackend;
pub use registry::SecretBackendRegistry;
pub use vault::VaultSecretBackend;
