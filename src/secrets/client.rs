//! Core secrets client trait and types.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::Result;

/// Metadata about a secret stored in the backend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecretMetadata {
    /// Secret key/name
    pub key: String,

    /// Secret version (if backend supports versioning)
    pub version: Option<u64>,

    /// When the secret was created
    pub created_at: DateTime<Utc>,

    /// When the secret was last rotated/updated
    pub updated_at: DateTime<Utc>,

    /// Optional description or tags
    pub description: Option<String>,
}

impl SecretMetadata {
    /// Create new secret metadata.
    pub fn new(key: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            key: key.into(),
            version: Some(1),
            created_at: now,
            updated_at: now,
            description: None,
        }
    }

    /// Create metadata with a description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the version.
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = Some(version);
        self
    }
}

/// Trait for secrets management backends.
///
/// Provides a unified interface for storing, retrieving, and rotating secrets
/// across different backends (Vault, AWS Secrets Manager, environment variables, etc.).
///
/// # Security Considerations
///
/// - Implementations MUST NOT log secret values
/// - All operations SHOULD be audited
/// - Secrets SHOULD be stored encrypted at rest
/// - Network communication MUST use TLS
///
/// # Example Implementation
///
/// ```rust,ignore
/// use flowplane::secrets::{SecretsClient, SecretMetadata, Result};
/// use async_trait::async_trait;
///
/// struct MySecretsBackend {
///     // Backend-specific fields
/// }
///
/// #[async_trait]
/// impl SecretsClient for MySecretsBackend {
///     async fn get_secret(&self, key: &str) -> Result<String> {
///         // Fetch from backend
///         Ok("secret-value".to_string())
///     }
///
///     async fn set_secret(&self, key: &str, value: &str) -> Result<()> {
///         // Store in backend
///         Ok(())
///     }
///
///     async fn rotate_secret(&self, key: &str) -> Result<String> {
///         // Generate new value and store
///         let new_value = generate_secure_random();
///         self.set_secret(key, &new_value).await?;
///         Ok(new_value)
///     }
///
///     async fn list_secrets(&self) -> Result<Vec<SecretMetadata>> {
///         // List available secrets
///         Ok(vec![])
///     }
///
///     async fn delete_secret(&self, key: &str) -> Result<()> {
///         // Delete from backend
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait SecretsClient: Send + Sync {
    /// Retrieve a secret value by key.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key/name
    ///
    /// # Returns
    ///
    /// The secret value as a string.
    ///
    /// # Errors
    ///
    /// - [`SecretsError::NotFound`] if the secret doesn't exist
    /// - [`SecretsError::ConnectionFailed`] if backend is unreachable
    /// - [`SecretsError::AuthenticationFailed`] if auth fails
    async fn get_secret(&self, key: &str) -> Result<String>;

    /// Store or update a secret value.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key/name
    /// * `value` - The secret value to store
    ///
    /// # Security
    ///
    /// The value MUST NOT be logged or exposed in error messages.
    ///
    /// # Errors
    ///
    /// - [`SecretsError::InvalidKey`] if key format is invalid
    /// - [`SecretsError::InvalidValue`] if value is invalid
    /// - [`SecretsError::BackendError`] if storage fails
    async fn set_secret(&self, key: &str, value: &str) -> Result<()>;

    /// Rotate a secret by generating a new value.
    ///
    /// This generates a cryptographically secure random value and stores it,
    /// replacing the previous value. The old value becomes inaccessible.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key to rotate
    ///
    /// # Returns
    ///
    /// The newly generated secret value.
    ///
    /// # Errors
    ///
    /// - [`SecretsError::NotFound`] if the secret doesn't exist
    /// - [`SecretsError::RotationFailed`] if rotation fails
    async fn rotate_secret(&self, key: &str) -> Result<String>;

    /// List all available secrets with metadata.
    ///
    /// Returns metadata only - secret values are NOT included.
    ///
    /// # Returns
    ///
    /// A vector of [`SecretMetadata`] for all secrets.
    async fn list_secrets(&self) -> Result<Vec<SecretMetadata>>;

    /// Delete a secret from the backend.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key to delete
    ///
    /// # Errors
    ///
    /// - [`SecretsError::NotFound`] if the secret doesn't exist
    /// - [`SecretsError::BackendError`] if deletion fails
    async fn delete_secret(&self, key: &str) -> Result<()>;

    /// Get metadata for a specific secret without retrieving its value.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key
    ///
    /// # Returns
    ///
    /// Metadata about the secret.
    ///
    /// # Errors
    ///
    /// - [`SecretsError::NotFound`] if the secret doesn't exist
    async fn get_secret_metadata(&self, key: &str) -> Result<SecretMetadata> {
        // Default implementation that backends can override
        let secrets = self.list_secrets().await?;
        secrets
            .into_iter()
            .find(|s| s.key == key)
            .ok_or_else(|| super::error::SecretsError::not_found(key))
    }

    /// Check if a secret exists.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key to check
    ///
    /// # Returns
    ///
    /// `true` if the secret exists, `false` otherwise.
    async fn secret_exists(&self, key: &str) -> Result<bool> {
        match self.get_secret_metadata(key).await {
            Ok(_) => Ok(true),
            Err(super::error::SecretsError::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_metadata_creation() {
        let metadata = SecretMetadata::new("test_key");
        assert_eq!(metadata.key, "test_key");
        assert_eq!(metadata.version, Some(1));
        assert!(metadata.description.is_none());
        assert_eq!(metadata.created_at, metadata.updated_at);
    }

    #[test]
    fn test_secret_metadata_builder() {
        let metadata =
            SecretMetadata::new("test_key").with_description("Test secret").with_version(5);

        assert_eq!(metadata.description, Some("Test secret".to_string()));
        assert_eq!(metadata.version, Some(5));
    }

    #[test]
    fn test_secret_metadata_serialization() {
        let metadata = SecretMetadata::new("test_key").with_description("Test");
        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: SecretMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(metadata, deserialized);
    }
}
