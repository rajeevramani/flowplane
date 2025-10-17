//! Environment variable secrets backend implementation.
//!
//! This module provides a simple secrets backend that reads from environment variables.
//! It's intended for **development and testing only** - NOT for production use.
//!
//! # Security Warning
//!
//! Environment variables are NOT secure for production secrets:
//! - Visible in process listings (`ps aux`)
//! - Stored in shell history
//! - No encryption at rest
//! - No audit logging
//! - No versioning or rotation support
//!
//! Use HashiCorp Vault or a cloud provider secrets manager for production.
//!
//! # Usage
//!
//! Secrets are read from environment variables with the `FLOWPLANE_SECRET_` prefix:
//!
//! ```bash
//! export FLOWPLANE_SECRET_BOOTSTRAP_TOKEN="my-token"
//! export FLOWPLANE_SECRET_JWT_SECRET="jwt-secret"
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::secrets::{EnvVarSecretsClient, SecretsClient};
//!
//! let client = EnvVarSecretsClient::new();
//!
//! // Reads from FLOWPLANE_SECRET_BOOTSTRAP_TOKEN
//! let token = client.get_secret("bootstrap_token").await?;
//! ```
//!
//! # Limitations
//!
//! - `set_secret()` and `delete_secret()` are not supported (read-only)
//! - `rotate_secret()` generates a new value but cannot persist it
//! - No versioning or metadata tracking
//! - No audit logging

use async_trait::async_trait;
use chrono::Utc;
use std::env;

use super::client::{SecretMetadata, SecretsClient};
use super::error::{Result, SecretsError};

/// Environment variable prefix for secrets.
const SECRET_PREFIX: &str = "FLOWPLANE_SECRET_";

/// Environment variable secrets backend (development only).
///
/// Reads secrets from environment variables with the `FLOWPLANE_SECRET_` prefix.
/// This backend is read-only and does not support persistence operations.
///
/// # Security
///
/// **DO NOT use in production.** This backend:
/// - Exposes secrets in process environment
/// - Provides no encryption or access control
/// - Cannot rotate or persist secrets
/// - Does not audit access
///
/// # Example
///
/// ```rust,ignore
/// let client = EnvVarSecretsClient::new();
///
/// // Reads FLOWPLANE_SECRET_JWT_SECRET
/// let secret = client.get_secret("jwt_secret").await?;
/// ```
#[derive(Debug, Clone, Default)]
pub struct EnvVarSecretsClient {
    // No internal state needed - reads directly from env
}

impl EnvVarSecretsClient {
    /// Creates a new environment variable secrets client.
    pub fn new() -> Self {
        Self::default()
    }

    /// Converts a secret key to the environment variable name.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// assert_eq!(
    ///     EnvVarSecretsClient::key_to_env_var("bootstrap_token"),
    ///     "FLOWPLANE_SECRET_BOOTSTRAP_TOKEN"
    /// );
    /// ```
    fn key_to_env_var(key: &str) -> String {
        format!("{}{}", SECRET_PREFIX, key.to_uppercase())
    }
}

#[async_trait]
impl SecretsClient for EnvVarSecretsClient {
    async fn get_secret(&self, key: &str) -> Result<String> {
        let env_var = Self::key_to_env_var(key);

        env::var(&env_var).map_err(|_| {
            SecretsError::not_found(format!(
                "Secret '{}' not found in environment (looking for {})",
                key, env_var
            ))
        })
    }

    async fn set_secret(&self, key: &str, _value: &str) -> Result<()> {
        Err(SecretsError::backend_error(format!(
            "Cannot set secret '{}' - EnvVarSecretsClient is read-only. Use a proper secrets backend for persistence.",
            key
        )))
    }

    async fn rotate_secret(&self, key: &str) -> Result<String> {
        Err(SecretsError::rotation_failed(
            key,
            "EnvVarSecretsClient cannot persist rotated secrets. Use HashiCorp Vault or a cloud secrets manager.",
        ))
    }

    async fn list_secrets(&self) -> Result<Vec<SecretMetadata>> {
        let now = Utc::now();
        let secrets: Vec<SecretMetadata> = env::vars()
            .filter_map(|(key, _)| {
                if key.starts_with(SECRET_PREFIX) {
                    let secret_key = key.strip_prefix(SECRET_PREFIX).unwrap().to_lowercase();
                    Some(SecretMetadata {
                        key: secret_key,
                        version: None,
                        created_at: now,
                        updated_at: now,
                        description: Some("From environment variable".to_string()),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(secrets)
    }

    async fn delete_secret(&self, key: &str) -> Result<()> {
        Err(SecretsError::backend_error(format!(
            "Cannot delete secret '{}' - EnvVarSecretsClient is read-only",
            key
        )))
    }

    async fn get_secret_metadata(&self, key: &str) -> Result<SecretMetadata> {
        // Check if secret exists first
        self.get_secret(key).await?;

        let now = Utc::now();
        Ok(SecretMetadata {
            key: key.to_string(),
            version: None,
            created_at: now,
            updated_at: now,
            description: Some(format!("From {}", Self::key_to_env_var(key))),
        })
    }

    async fn secret_exists(&self, key: &str) -> Result<bool> {
        let env_var = Self::key_to_env_var(key);
        Ok(env::var(&env_var).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_to_env_var() {
        assert_eq!(
            EnvVarSecretsClient::key_to_env_var("bootstrap_token"),
            "FLOWPLANE_SECRET_BOOTSTRAP_TOKEN"
        );
        assert_eq!(
            EnvVarSecretsClient::key_to_env_var("jwt_secret"),
            "FLOWPLANE_SECRET_JWT_SECRET"
        );
    }

    #[tokio::test]
    async fn test_get_secret_not_found() {
        let client = EnvVarSecretsClient::new();
        let result = client.get_secret("nonexistent_secret").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_secret_from_env() {
        env::set_var("FLOWPLANE_SECRET_TEST_KEY", "test-value");

        let client = EnvVarSecretsClient::new();
        let result = client.get_secret("test_key").await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-value");

        env::remove_var("FLOWPLANE_SECRET_TEST_KEY");
    }

    #[tokio::test]
    async fn test_set_secret_not_supported() {
        let client = EnvVarSecretsClient::new();
        let result = client.set_secret("test_key", "test-value").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecretsError::BackendError { .. }));
    }

    #[tokio::test]
    async fn test_rotate_secret_not_supported() {
        let client = EnvVarSecretsClient::new();
        let result = client.rotate_secret("test_key").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecretsError::RotationFailed { .. }));
    }

    #[tokio::test]
    async fn test_delete_secret_not_supported() {
        let client = EnvVarSecretsClient::new();
        let result = client.delete_secret("test_key").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecretsError::BackendError { .. }));
    }

    #[tokio::test]
    async fn test_list_secrets() {
        env::set_var("FLOWPLANE_SECRET_KEY1", "value1");
        env::set_var("FLOWPLANE_SECRET_KEY2", "value2");
        env::set_var("OTHER_VAR", "other");

        let client = EnvVarSecretsClient::new();
        let secrets = client.list_secrets().await.unwrap();

        let keys: Vec<String> = secrets.iter().map(|s| s.key.clone()).collect();
        assert!(keys.contains(&"key1".to_string()));
        assert!(keys.contains(&"key2".to_string()));
        assert!(!keys.contains(&"other_var".to_string()));

        env::remove_var("FLOWPLANE_SECRET_KEY1");
        env::remove_var("FLOWPLANE_SECRET_KEY2");
        env::remove_var("OTHER_VAR");
    }

    #[tokio::test]
    async fn test_secret_exists() {
        env::set_var("FLOWPLANE_SECRET_EXISTS_TEST", "value");

        let client = EnvVarSecretsClient::new();
        assert!(client.secret_exists("exists_test").await.unwrap());
        assert!(!client.secret_exists("does_not_exist").await.unwrap());

        env::remove_var("FLOWPLANE_SECRET_EXISTS_TEST");
    }

    #[tokio::test]
    async fn test_get_secret_metadata() {
        env::set_var("FLOWPLANE_SECRET_METADATA_TEST", "value");

        let client = EnvVarSecretsClient::new();
        let metadata = client.get_secret_metadata("metadata_test").await.unwrap();

        assert_eq!(metadata.key, "metadata_test");
        assert!(metadata.description.is_some());
        assert!(metadata.description.unwrap().contains("FLOWPLANE_SECRET_METADATA_TEST"));

        env::remove_var("FLOWPLANE_SECRET_METADATA_TEST");
    }
}
