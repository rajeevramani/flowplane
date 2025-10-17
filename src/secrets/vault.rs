//! HashiCorp Vault secrets backend implementation.
//!
//! This module provides integration with HashiCorp Vault's KV v2 secrets engine.
//! It implements the [`SecretsClient`] trait to provide secure, centralized secrets
//! management with versioning, audit logging, and access control.
//!
//! # Configuration
//!
//! Vault integration requires:
//! - Vault server address (HTTPS recommended)
//! - Authentication token or AppRole credentials
//! - Optional namespace for multi-tenancy
//! - KV v2 mount path (default: "secret")
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::secrets::{VaultSecretsClient, VaultConfig};
//!
//! let config = VaultConfig {
//!     address: "https://vault.example.com".to_string(),
//!     token: Some("vault-token".to_string()),
//!     namespace: Some("flowplane".to_string()),
//!     mount_path: "secret".to_string(),
//! };
//!
//! let client = VaultSecretsClient::new(config).await?;
//! let secret = client.get_secret("bootstrap_token").await?;
//! ```
//!
//! # Security
//!
//! - All communication uses TLS
//! - Tokens are never logged
//! - Audit logging enabled in Vault tracks all access
//! - Secrets are encrypted at rest in Vault
//! - KV v2 provides automatic versioning

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use vaultrs::client::{VaultClient, VaultClientSettingsBuilder};
use vaultrs::kv2;

use super::client::{SecretMetadata, SecretsClient};
use super::error::{Result, SecretsError};

/// Configuration for HashiCorp Vault backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    /// Vault server address (e.g., "https://vault.example.com:8200")
    pub address: String,

    /// Vault authentication token (if using token auth)
    pub token: Option<String>,

    /// Vault namespace (for Enterprise multi-tenancy)
    pub namespace: Option<String>,

    /// KV v2 mount path (default: "secret")
    #[serde(default = "default_mount_path")]
    pub mount_path: String,
}

fn default_mount_path() -> String {
    "secret".to_string()
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            address: "http://127.0.0.1:8200".to_string(),
            token: None,
            namespace: None,
            mount_path: default_mount_path(),
        }
    }
}

/// HashiCorp Vault secrets backend client.
///
/// Implements the [`SecretsClient`] trait using Vault's KV v2 secrets engine.
/// Provides automatic versioning, audit logging, and secure storage.
///
/// # Thread Safety
///
/// This client is `Send + Sync` and can be safely shared across async tasks.
///
/// # Example
///
/// ```rust,ignore
/// let config = VaultConfig::default();
/// let client = VaultSecretsClient::new(config).await?;
///
/// // Store a secret
/// client.set_secret("jwt_secret", "secret-value").await?;
///
/// // Retrieve it
/// let value = client.get_secret("jwt_secret").await?;
///
/// // Rotate with new value
/// let new_value = client.rotate_secret("jwt_secret").await?;
/// ```
pub struct VaultSecretsClient {
    client: VaultClient,
    mount_path: String,
}

impl VaultSecretsClient {
    /// Creates a new Vault secrets client with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Vault configuration including address and credentials
    ///
    /// # Errors
    ///
    /// - [`SecretsError::ConnectionFailed`] if Vault is unreachable
    /// - [`SecretsError::AuthenticationFailed`] if credentials are invalid
    /// - [`SecretsError::ConfigError`] if configuration is invalid
    pub async fn new(config: VaultConfig) -> Result<Self> {
        // Validate configuration
        if config.address.is_empty() {
            return Err(SecretsError::config_error("Vault address cannot be empty"));
        }

        // Build Vault client settings
        let mut settings_builder = VaultClientSettingsBuilder::default();
        settings_builder.address(&config.address);

        if let Some(ref token) = config.token {
            settings_builder.token(token);
        }

        if let Some(namespace) = config.namespace {
            settings_builder.namespace(Some(namespace));
        }

        let settings = settings_builder.build().map_err(|e| {
            SecretsError::config_error(format!("Invalid Vault configuration: {}", e))
        })?;

        // Create Vault client
        let client = VaultClient::new(settings).map_err(|e| {
            SecretsError::connection_failed(format!("Failed to create Vault client: {}", e))
        })?;

        // Test connection by checking Vault health
        match vaultrs::sys::health(&client).await {
            Ok(_) => {
                tracing::info!(address = %config.address, "Successfully connected to Vault");
            }
            Err(e) => {
                tracing::error!(error = %e, address = %config.address, "Failed to connect to Vault");
                return Err(SecretsError::connection_failed(format!(
                    "Vault health check failed: {}",
                    e
                )));
            }
        }

        Ok(Self { client, mount_path: config.mount_path.clone() })
    }

    /// Creates a Vault client from environment variables.
    ///
    /// Reads configuration from:
    /// - `VAULT_ADDR`: Vault server address
    /// - `VAULT_TOKEN`: Authentication token
    /// - `VAULT_NAMESPACE`: Optional namespace
    /// - `VAULT_MOUNT_PATH`: Optional mount path (default: "secret")
    ///
    /// # Errors
    ///
    /// - [`SecretsError::ConfigError`] if required env vars are missing
    pub async fn from_env() -> Result<Self> {
        let address = std::env::var("VAULT_ADDR")
            .map_err(|_| SecretsError::config_error("VAULT_ADDR environment variable not set"))?;

        let token = std::env::var("VAULT_TOKEN").ok();
        let namespace = std::env::var("VAULT_NAMESPACE").ok();
        let mount_path = std::env::var("VAULT_MOUNT_PATH").unwrap_or_else(|_| "secret".to_string());

        let config = VaultConfig { address, token, namespace, mount_path };

        Self::new(config).await
    }
}

#[async_trait]
impl SecretsClient for VaultSecretsClient {
    async fn get_secret(&self, key: &str) -> Result<String> {
        // Read the latest version of the secret from KV v2
        let secret: HashMap<String, String> =
            kv2::read(&self.client, &self.mount_path, key).await.map_err(|e| {
                tracing::error!(error = %e, key = %key, "Failed to read secret from Vault");
                SecretsError::not_found(format!("Secret '{}' not found: {}", key, e))
            })?;

        // Extract the "value" field (our convention for storing secrets)
        secret.get("value").cloned().ok_or_else(|| {
            SecretsError::backend_error(format!("Secret '{}' has no 'value' field", key))
        })
    }

    async fn set_secret(&self, key: &str, value: &str) -> Result<()> {
        // Store secret as a map with "value" field
        let mut data = HashMap::new();
        data.insert("value".to_string(), value.to_string());

        kv2::set(&self.client, &self.mount_path, key, &data).await.map_err(|e| {
            tracing::error!(error = %e, key = %key, "Failed to write secret to Vault");
            SecretsError::backend_error(format!("Failed to store secret '{}': {}", key, e))
        })?;

        tracing::info!(key = %key, mount_path = %self.mount_path, "Successfully stored secret in Vault");
        Ok(())
    }

    async fn rotate_secret(&self, key: &str) -> Result<String> {
        // Generate a cryptographically secure random value (32 bytes = 256 bits)
        let random_bytes: Vec<u8> = {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            (0..32).map(|_| rng.gen()).collect()
        };
        let new_value =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &random_bytes);

        // Store the new value atomically
        self.set_secret(key, &new_value).await?;

        tracing::info!(key = %key, "Successfully rotated secret in Vault");
        Ok(new_value)
    }

    async fn list_secrets(&self) -> Result<Vec<SecretMetadata>> {
        // List all keys in the KV v2 mount
        let keys: Vec<String> = kv2::list(&self.client, &self.mount_path, "")
            .await
            .map_err(|e| {
                tracing::error!(error = %e, mount_path = %self.mount_path, "Failed to list secrets from Vault");
                SecretsError::backend_error(format!("Failed to list secrets: {}", e))
            })?;

        // Fetch metadata for each key
        let mut secrets = Vec::new();
        for key in keys {
            // Read metadata for this key
            match kv2::read_metadata(&self.client, &self.mount_path, &key).await {
                Ok(metadata) => {
                    let created_time = chrono::DateTime::parse_from_rfc3339(&metadata.created_time)
                        .ok()
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now);

                    let updated_time = chrono::DateTime::parse_from_rfc3339(&metadata.updated_time)
                        .ok()
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or(created_time);

                    secrets.push(SecretMetadata {
                        key: key.clone(),
                        version: Some(metadata.current_version),
                        created_at: created_time,
                        updated_at: updated_time,
                        description: None,
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, key = %key, "Failed to read metadata for secret");
                    // Continue to next secret instead of failing entirely
                }
            }
        }

        Ok(secrets)
    }

    async fn delete_secret(&self, key: &str) -> Result<()> {
        // Delete all versions of the secret (metadata delete)
        kv2::delete_metadata(&self.client, &self.mount_path, key).await.map_err(|e| {
            tracing::error!(error = %e, key = %key, "Failed to delete secret from Vault");
            SecretsError::backend_error(format!("Failed to delete secret '{}': {}", key, e))
        })?;

        tracing::info!(key = %key, mount_path = %self.mount_path, "Successfully deleted secret from Vault");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vault_config_default() {
        let config = VaultConfig::default();
        assert_eq!(config.address, "http://127.0.0.1:8200");
        assert_eq!(config.mount_path, "secret");
        assert!(config.token.is_none());
        assert!(config.namespace.is_none());
    }

    #[test]
    fn test_vault_config_serialization() {
        let config = VaultConfig {
            address: "https://vault.example.com".to_string(),
            token: Some("token".to_string()),
            namespace: Some("ns".to_string()),
            mount_path: "kv".to_string(),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: VaultConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.address, deserialized.address);
        assert_eq!(config.token, deserialized.token);
        assert_eq!(config.namespace, deserialized.namespace);
        assert_eq!(config.mount_path, deserialized.mount_path);
    }
}
