//! Fallback secrets client with primary and secondary backends.
//!
//! This module provides a fallback secrets client that tries a primary backend
//! first, and falls back to a secondary backend (typically environment variables)
//! if the primary is unavailable or fails.
//!
//! # Use Cases
//!
//! - **Development**: Use environment variables as fallback when Vault is unavailable
//! - **Graceful Degradation**: Continue operating with cached/env secrets if Vault goes down
//! - **Hybrid Deployments**: Use Vault in production, env vars in development
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::secrets::{FallbackSecretsClient, VaultSecretsClient, EnvVarSecretsClient};
//!
//! // Try Vault first, fall back to environment variables
//! let vault_client = VaultSecretsClient::new(config).await?;
//! let env_client = EnvVarSecretsClient::new();
//!
//! let client = FallbackSecretsClient::new(vault_client, env_client);
//!
//! // Will try Vault first, then env vars if Vault fails
//! let secret = client.get_secret("jwt_secret").await?;
//! ```
//!
//! # Security Considerations
//!
//! - Fallback should only be used in development or for graceful degradation
//! - Production should use primary backend (Vault) exclusively
//! - Environment variables are less secure than dedicated secrets managers

use async_trait::async_trait;

use super::client::{SecretMetadata, SecretsClient};
use super::error::{Result, SecretsError};

/// Fallback secrets client with primary and secondary backends.
///
/// Attempts operations on the primary backend first. If the primary fails,
/// falls back to the secondary backend. This enables graceful degradation
/// and hybrid deployment scenarios.
///
/// # Type Parameters
///
/// * `P` - Primary secrets backend (e.g., VaultSecretsClient)
/// * `S` - Secondary/fallback backend (e.g., EnvVarSecretsClient)
///
/// # Example
///
/// ```rust,ignore
/// // Production: Vault with env fallback for development
/// let primary = VaultSecretsClient::new(vault_config).await?;
/// let fallback = EnvVarSecretsClient::new();
/// let client = FallbackSecretsClient::new(primary, fallback);
/// ```
pub struct FallbackSecretsClient<P: SecretsClient, S: SecretsClient> {
    primary: P,
    secondary: S,
}

impl<P: SecretsClient, S: SecretsClient> FallbackSecretsClient<P, S> {
    /// Creates a new fallback secrets client.
    ///
    /// # Arguments
    ///
    /// * `primary` - Primary secrets backend (tried first)
    /// * `secondary` - Fallback backend (tried if primary fails)
    pub fn new(primary: P, secondary: S) -> Self {
        Self { primary, secondary }
    }

    /// Try an operation on primary, fall back to secondary if it fails.
    async fn try_with_fallback<T, F, G>(&self, primary_op: F, secondary_op: G) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
        G: std::future::Future<Output = Result<T>>,
    {
        match primary_op.await {
            Ok(result) => Ok(result),
            Err(primary_error) => {
                tracing::warn!(
                    error = %primary_error,
                    "Primary secrets backend failed, attempting fallback"
                );
                secondary_op.await.map_err(|secondary_error| {
                    tracing::error!(
                        primary_error = %primary_error,
                        secondary_error = %secondary_error,
                        "Both primary and fallback secrets backends failed"
                    );
                    SecretsError::backend_error(format!(
                        "Primary backend failed: {}. Fallback also failed: {}",
                        primary_error, secondary_error
                    ))
                })
            }
        }
    }
}

#[async_trait]
impl<P: SecretsClient, S: SecretsClient> SecretsClient for FallbackSecretsClient<P, S> {
    async fn get_secret(&self, key: &str) -> Result<String> {
        self.try_with_fallback(self.primary.get_secret(key), self.secondary.get_secret(key)).await
    }

    async fn set_secret(&self, key: &str, value: &str) -> Result<()> {
        self.try_with_fallback(
            self.primary.set_secret(key, value),
            self.secondary.set_secret(key, value),
        )
        .await
    }

    async fn rotate_secret(&self, key: &str) -> Result<String> {
        self.try_with_fallback(self.primary.rotate_secret(key), self.secondary.rotate_secret(key))
            .await
    }

    async fn list_secrets(&self) -> Result<Vec<SecretMetadata>> {
        self.try_with_fallback(self.primary.list_secrets(), self.secondary.list_secrets()).await
    }

    async fn delete_secret(&self, key: &str) -> Result<()> {
        self.try_with_fallback(self.primary.delete_secret(key), self.secondary.delete_secret(key))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::EnvVarSecretsClient;

    /// Mock secrets client that always fails (for testing fallback)
    struct FailingSecretsClient;

    #[async_trait]
    impl SecretsClient for FailingSecretsClient {
        async fn get_secret(&self, _key: &str) -> Result<String> {
            Err(SecretsError::connection_failed("Mock primary failure"))
        }

        async fn set_secret(&self, _key: &str, _value: &str) -> Result<()> {
            Err(SecretsError::connection_failed("Mock primary failure"))
        }

        async fn rotate_secret(&self, _key: &str) -> Result<String> {
            Err(SecretsError::connection_failed("Mock primary failure"))
        }

        async fn list_secrets(&self) -> Result<Vec<SecretMetadata>> {
            Err(SecretsError::connection_failed("Mock primary failure"))
        }

        async fn delete_secret(&self, _key: &str) -> Result<()> {
            Err(SecretsError::connection_failed("Mock primary failure"))
        }
    }

    #[tokio::test]
    async fn test_fallback_on_primary_failure() {
        std::env::set_var("FLOWPLANE_SECRET_TEST_KEY", "fallback-value");

        let primary = FailingSecretsClient;
        let fallback = EnvVarSecretsClient::new();
        let client = FallbackSecretsClient::new(primary, fallback);

        // Primary fails, should get value from fallback (env var)
        let result = client.get_secret("test_key").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "fallback-value");

        std::env::remove_var("FLOWPLANE_SECRET_TEST_KEY");
    }

    #[tokio::test]
    async fn test_both_backends_fail() {
        let primary = FailingSecretsClient;
        let fallback = EnvVarSecretsClient::new();
        let client = FallbackSecretsClient::new(primary, fallback);

        // Both should fail (env var doesn't exist)
        let result = client.get_secret("nonexistent_key").await;
        assert!(result.is_err());

        // Error message should mention both failures
        let error = result.unwrap_err();
        assert!(error.to_string().contains("Primary backend failed"));
        assert!(error.to_string().contains("Fallback also failed"));
    }

    #[tokio::test]
    async fn test_primary_success_skips_fallback() {
        std::env::set_var("FLOWPLANE_SECRET_PRIMARY_KEY", "primary-value");

        let primary = EnvVarSecretsClient::new();
        let fallback = FailingSecretsClient;
        let client = FallbackSecretsClient::new(primary, fallback);

        // Primary succeeds, fallback is never tried
        let result = client.get_secret("primary_key").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "primary-value");

        std::env::remove_var("FLOWPLANE_SECRET_PRIMARY_KEY");
    }
}
