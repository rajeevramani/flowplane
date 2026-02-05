//! Secrets management abstraction for secure configuration.
//!
//! This module provides a unified interface for managing sensitive configuration
//! such as bootstrap tokens, JWT secrets, database passwords, and TLS keys.
//! It supports multiple backends including HashiCorp Vault, AWS Secrets Manager,
//! Azure Key Vault, and environment variables for development.
//!
//! # Architecture
//!
//! The secrets system is built around the [`SecretsClient`] trait, which provides
//! a backend-agnostic interface for secret operations:
//! - **get_secret**: Retrieve a secret value
//! - **set_secret**: Store or update a secret
//! - **rotate_secret**: Generate and store a new secret value
//! - **list_secrets**: List available secrets with metadata
//!
//! ## Pluggable Backend Architecture
//!
//! For SDS (Secret Discovery Service), we also support a pluggable backend system
//! via the [`backends`] module. This allows Flowplane to store only REFERENCES
//! to secrets (not encrypted values) and fetch them on-demand from external systems.
//!
//! # Supported Backends
//!
//! - **HashiCorp Vault**: Production-ready secrets backend with KV v2 engine
//! - **Environment Variables**: Development fallback using `FLOWPLANE_SECRET_*` prefix
//! - **AWS Secrets Manager**: (Optional feature)
//! - **GCP Secret Manager**: (Optional feature)
//! - **Database**: Legacy encrypted storage mode
//!
//! # Basic Example
//!
//! ```rust,ignore
//! use flowplane::secrets::{SecretsClient, VaultSecretsClient, VaultConfig};
//!
//! // Create a Vault client
//! let config = VaultConfig {
//!     address: "https://vault.example.com".to_string(),
//!     token: Some("vault-token".to_string()),
//!     namespace: None,
//!     mount_path: "secret".to_string(),
//! };
//! let client = VaultSecretsClient::new(config).await?;
//!
//! // Store a secret
//! client.set_secret("jwt_secret", "my-secret-value").await?;
//!
//! // Retrieve a secret
//! let secret = client.get_secret("jwt_secret").await?;
//!
//! // Rotate a secret
//! let new_value = client.rotate_secret("bootstrap_token").await?;
//! ```
//!
//! # Composable Architecture Example
//!
//! ```rust,ignore
//! use flowplane::secrets::{
//!     VaultSecretsClient, EnvVarSecretsClient, FallbackSecretsClient,
//!     CachedSecretsClient, AuditedSecretsClient
//! };
//! use std::time::Duration;
//!
//! // Build a production-ready secrets client with:
//! // 1. Vault as primary backend
//! // 2. Environment variables as fallback
//! // 3. In-memory caching with 5-minute TTL
//! // 4. Automatic audit logging
//!
//! let vault = VaultSecretsClient::new(vault_config).await?;
//! let env_fallback = EnvVarSecretsClient::new();
//!
//! // Add fallback: Vault -> Env Vars
//! let with_fallback = FallbackSecretsClient::new(vault, env_fallback);
//!
//! // Add caching: 5-minute TTL
//! let with_cache = CachedSecretsClient::new(with_fallback, Duration::from_secs(300));
//!
//! // Add auditing: Log all operations
//! let client = AuditedSecretsClient::new(with_cache, audit_repo);
//!
//! // Now use the fully-featured client
//! let secret = client.get_secret("jwt_secret").await?;  // Cached, audited, with fallback
//!
//! // Manually refresh secrets after rotation
//! with_cache.refresh_secret("jwt_secret").await?;
//!
//! // Periodic background refresh
//! tokio::spawn(async move {
//!     let mut interval = tokio::time::interval(Duration::from_secs(3600));
//!     loop {
//!         interval.tick().await;
//!         with_cache.refresh_all().await.ok();
//!     }
//! });
//! ```
//!
//! # Security Considerations
//!
//! - Secrets are never logged or exposed in error messages
//! - All secret access is audited via the audit log
//! - Secrets are stored encrypted in the backend
//! - Rotation is atomic and traceable
//! - Environment variable fallback is for development only

pub mod audited;
pub mod backends;
pub mod cached;
pub mod client;
pub mod env;
pub mod error;
pub mod fallback;
pub mod types;
pub mod vault;

// Re-export main types
pub use audited::AuditedSecretsClient;
pub use cached::CachedSecretsClient;
pub use client::{SecretMetadata, SecretsClient};
pub use env::EnvVarSecretsClient;
pub use error::{Result, SecretsError};
pub use fallback::FallbackSecretsClient;
pub use types::SecretString;
pub use vault::{
    parse_proxy_id_from_spiffe_uri, parse_team_from_spiffe_uri, GeneratedCertificate, PkiConfig,
    VaultConfig, VaultSecretsClient,
};

// Re-export backend types
pub use backends::{
    DatabaseSecretBackend, SecretBackend, SecretBackendRegistry, SecretBackendType, SecretCache,
    VaultSecretBackend,
};
