//! Secret backend trait and types
//!
//! Defines the core interface for pluggable secret backends.

use crate::domain::{SecretSpec, SecretType};
use crate::errors::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

/// Type of secret backend
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretBackendType {
    /// HashiCorp Vault KV v2
    Vault,
    /// AWS Secrets Manager
    AwsSecretsManager,
    /// GCP Secret Manager
    GcpSecretManager,
    /// Database (legacy encrypted storage)
    Database,
}

impl SecretBackendType {
    /// Get the database representation of this type
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Vault => "vault",
            Self::AwsSecretsManager => "aws_secrets_manager",
            Self::GcpSecretManager => "gcp_secret_manager",
            Self::Database => "database",
        }
    }
}

impl FromStr for SecretBackendType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "vault" => Ok(Self::Vault),
            "aws_secrets_manager" => Ok(Self::AwsSecretsManager),
            "gcp_secret_manager" => Ok(Self::GcpSecretManager),
            "database" => Ok(Self::Database),
            _ => Err(format!("Unknown secret backend type: {}", s)),
        }
    }
}

impl fmt::Display for SecretBackendType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Trait for secret backends
///
/// Implementations must be Send + Sync for use in async contexts.
#[async_trait]
pub trait SecretBackend: Send + Sync + std::fmt::Debug {
    /// Fetch a secret by backend-specific reference
    ///
    /// # Arguments
    /// - `reference`: Backend-specific reference (Vault path, AWS ARN, etc.)
    /// - `expected_type`: The expected secret type for validation
    ///
    /// # Returns
    /// The decrypted/fetched secret specification
    async fn fetch_secret(&self, reference: &str, expected_type: SecretType) -> Result<SecretSpec>;

    /// Validate that a reference exists in the backend
    ///
    /// This is used during secret creation to verify the reference is valid
    /// before storing it in the database.
    async fn validate_reference(&self, reference: &str) -> Result<bool>;

    /// Get the backend type identifier
    fn backend_type(&self) -> SecretBackendType;

    /// Perform a health check on the backend
    ///
    /// Returns Ok(()) if the backend is healthy, Err otherwise.
    async fn health_check(&self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_type_roundtrip() {
        for bt in [
            SecretBackendType::Vault,
            SecretBackendType::AwsSecretsManager,
            SecretBackendType::GcpSecretManager,
            SecretBackendType::Database,
        ] {
            let s = bt.as_str();
            let parsed: SecretBackendType = s.parse().unwrap();
            assert_eq!(bt, parsed);
        }
    }

    #[test]
    fn test_backend_type_display() {
        assert_eq!(SecretBackendType::Vault.to_string(), "vault");
        assert_eq!(SecretBackendType::AwsSecretsManager.to_string(), "aws_secrets_manager");
        assert_eq!(SecretBackendType::GcpSecretManager.to_string(), "gcp_secret_manager");
        assert_eq!(SecretBackendType::Database.to_string(), "database");
    }

    #[test]
    fn test_backend_type_serialization() {
        let vault = SecretBackendType::Vault;
        let json = serde_json::to_string(&vault).unwrap();
        assert_eq!(json, "\"vault\"");

        let parsed: SecretBackendType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SecretBackendType::Vault);
    }
}
