//! Pluggable secret and certificate backend architecture
//!
//! This module provides a unified interface for fetching secrets from various backends.
//! Flowplane can store only REFERENCES to secrets (not values) and fetch on-demand.
//!
//! ## Supported Secret Backends
//!
//! - **Vault**: HashiCorp Vault KV v2 engine
//! - **Database**: Legacy mode - encrypted secrets stored in SQLite/PostgreSQL
//! - **AWS Secrets Manager**: (Optional feature)
//! - **GCP Secret Manager**: (Optional feature - enable with `--features gcp`)
//!
//! ## Supported Certificate Backends
//!
//! - **VaultPki**: HashiCorp Vault PKI secrets engine
//! - **Mock**: Deterministic backend for testing

pub mod backend;
pub mod cache;
pub mod certificates;
pub mod database;
pub mod registry;
pub mod vault;

// GCP Secret Manager backend
// Module is always compiled for config/tests, but client implementation requires feature
pub mod gcp;

pub use backend::{SecretBackend, SecretBackendType};
pub use cache::SecretCache;
pub use database::DatabaseSecretBackend;
pub use registry::SecretBackendRegistry;
pub use vault::VaultSecretBackend;

// Certificate backends
pub use certificates::{
    CertificateBackend, CertificateBackendType, MockCertificateBackend, RetryConfig,
    VaultPkiBackend,
};

// GCP backend config is always available
pub use gcp::GcpBackendConfig;

// GCP backend implementation requires feature
#[cfg(feature = "gcp")]
pub use gcp::GcpSecretBackend;
