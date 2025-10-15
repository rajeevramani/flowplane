//! Cached secrets client with TTL and refresh support.
//!
//! This module provides a caching layer for secrets that:
//! - Reduces load on the secrets backend
//! - Improves performance for frequently accessed secrets
//! - Supports configurable TTL (time-to-live)
//! - Enables runtime refresh without service restart
//!
//! # Use Cases
//!
//! - **Performance**: Cache frequently accessed secrets (JWT signing keys, API keys)
//! - **Reliability**: Continue operating if backend is temporarily unavailable
//! - **Cost Optimization**: Reduce API calls to cloud secrets managers
//! - **Dynamic Refresh**: Update secrets without restarting the service
//!
//! # Security Considerations
//!
//! - Cached secrets are stored in memory (not persisted to disk)
//! - TTL ensures secrets are refreshed periodically
//! - Manual refresh supported for rotation scenarios
//! - Cache is cleared on application restart
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::secrets::{CachedSecretsClient, VaultSecretsClient};
//! use std::time::Duration;
//!
//! let vault = VaultSecretsClient::new(config).await?;
//!
//! // Cache secrets for 5 minutes
//! let client = CachedSecretsClient::new(vault, Duration::from_secs(300));
//!
//! // First call fetches from Vault
//! let secret = client.get_secret("jwt_secret").await?;
//!
//! // Subsequent calls within 5 minutes use cache
//! let cached = client.get_secret("jwt_secret").await?;
//!
//! // Manually refresh a specific secret
//! client.refresh_secret("jwt_secret").await?;
//!
//! // Refresh all cached secrets
//! client.refresh_all().await?;
//! ```

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::client::{SecretMetadata, SecretsClient};
use super::error::Result;

/// Cached secret entry with TTL.
#[derive(Debug, Clone)]
struct CachedSecret {
    value: String,
    cached_at: Instant,
}

impl CachedSecret {
    fn new(value: String) -> Self {
        Self { value, cached_at: Instant::now() }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.cached_at.elapsed() > ttl
    }
}

/// Cached secrets client with TTL and refresh support.
///
/// Wraps any [`SecretsClient`] implementation with an in-memory cache.
/// Secrets are cached for a configured TTL and automatically refreshed
/// when expired.
///
/// # Thread Safety
///
/// This client uses `RwLock` for thread-safe caching and can be safely
/// shared across async tasks.
///
/// # Example
///
/// ```rust,ignore
/// use std::time::Duration;
///
/// // Cache for 5 minutes
/// let client = CachedSecretsClient::new(
///     vault_client,
///     Duration::from_secs(300)
/// );
///
/// // Access secrets (cached after first fetch)
/// let secret = client.get_secret("api_key").await?;
///
/// // Force refresh a specific secret
/// client.refresh_secret("api_key").await?;
/// ```
pub struct CachedSecretsClient<T: SecretsClient> {
    inner: T,
    cache: Arc<RwLock<HashMap<String, CachedSecret>>>,
    ttl: Duration,
}

impl<T: SecretsClient> CachedSecretsClient<T> {
    /// Creates a new cached secrets client.
    ///
    /// # Arguments
    ///
    /// * `inner` - The underlying secrets client to wrap
    /// * `ttl` - Time-to-live for cached secrets
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use std::time::Duration;
    ///
    /// // Cache secrets for 10 minutes
    /// let client = CachedSecretsClient::new(
    ///     vault_client,
    ///     Duration::from_secs(600)
    /// );
    /// ```
    pub fn new(inner: T, ttl: Duration) -> Self {
        Self { inner, cache: Arc::new(RwLock::new(HashMap::new())), ttl }
    }

    /// Refresh a specific secret by fetching it from the backend.
    ///
    /// This bypasses the cache and forces a fresh fetch from the underlying
    /// secrets backend. The new value is then cached.
    ///
    /// # Arguments
    ///
    /// * `key` - The secret key to refresh
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Force refresh after rotation
    /// client.refresh_secret("jwt_secret").await?;
    /// ```
    pub async fn refresh_secret(&self, key: &str) -> Result<()> {
        let value = self.inner.get_secret(key).await?;
        let mut cache = self.cache.write().await;
        cache.insert(key.to_string(), CachedSecret::new(value));
        tracing::debug!(key = %key, "Refreshed secret in cache");
        Ok(())
    }

    /// Refresh all currently cached secrets.
    ///
    /// Fetches fresh values for all secrets currently in the cache.
    /// Useful for periodic refresh or after detecting backend changes.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Refresh all cached secrets periodically
    /// tokio::spawn(async move {
    ///     let mut interval = tokio::time::interval(Duration::from_secs(3600));
    ///     loop {
    ///         interval.tick().await;
    ///         if let Err(e) = client.refresh_all().await {
    ///             tracing::error!("Failed to refresh secrets: {}", e);
    ///         }
    ///     }
    /// });
    /// ```
    pub async fn refresh_all(&self) -> Result<()> {
        let keys: Vec<String> = {
            let cache = self.cache.read().await;
            cache.keys().cloned().collect()
        };

        let mut errors = Vec::new();
        for key in &keys {
            if let Err(e) = self.refresh_secret(key).await {
                tracing::error!(key = %key, error = %e, "Failed to refresh secret");
                errors.push((key.clone(), e));
            }
        }

        if errors.is_empty() {
            tracing::info!(count = keys.len(), "Refreshed all cached secrets");
            Ok(())
        } else {
            Err(super::error::SecretsError::backend_error(format!(
                "Failed to refresh {} of {} secrets",
                errors.len(),
                keys.len()
            )))
        }
    }

    /// Clear the entire cache.
    ///
    /// Removes all cached secrets. Next access will fetch from the backend.
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        let count = cache.len();
        cache.clear();
        tracing::info!(count = count, "Cleared secrets cache");
    }

    /// Get cache statistics.
    ///
    /// Returns the number of secrets currently cached.
    pub async fn cache_size(&self) -> usize {
        let cache = self.cache.read().await;
        cache.len()
    }
}

#[async_trait]
impl<T: SecretsClient> SecretsClient for CachedSecretsClient<T> {
    async fn get_secret(&self, key: &str) -> Result<String> {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(key) {
                if !cached.is_expired(self.ttl) {
                    tracing::debug!(key = %key, "Cache hit for secret");
                    return Ok(cached.value.clone());
                }
                tracing::debug!(key = %key, "Cached secret expired");
            }
        }

        // Cache miss or expired - fetch from backend
        tracing::debug!(key = %key, "Cache miss, fetching from backend");
        let value = self.inner.get_secret(key).await?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(key.to_string(), CachedSecret::new(value.clone()));
        }

        Ok(value)
    }

    async fn set_secret(&self, key: &str, value: &str) -> Result<()> {
        // Write through to backend
        self.inner.set_secret(key, value).await?;

        // Update cache
        let mut cache = self.cache.write().await;
        cache.insert(key.to_string(), CachedSecret::new(value.to_string()));
        tracing::debug!(key = %key, "Updated secret in cache after set");

        Ok(())
    }

    async fn rotate_secret(&self, key: &str) -> Result<String> {
        // Rotate in backend
        let new_value = self.inner.rotate_secret(key).await?;

        // Update cache with new value
        let mut cache = self.cache.write().await;
        cache.insert(key.to_string(), CachedSecret::new(new_value.clone()));
        tracing::debug!(key = %key, "Updated secret in cache after rotation");

        Ok(new_value)
    }

    async fn list_secrets(&self) -> Result<Vec<SecretMetadata>> {
        // Always fetch from backend (metadata changes frequently)
        self.inner.list_secrets().await
    }

    async fn delete_secret(&self, key: &str) -> Result<()> {
        // Delete from backend
        self.inner.delete_secret(key).await?;

        // Remove from cache
        let mut cache = self.cache.write().await;
        cache.remove(key);
        tracing::debug!(key = %key, "Removed secret from cache after deletion");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::EnvVarSecretsClient;
    use std::time::Duration;

    #[tokio::test]
    async fn test_cache_hit() {
        std::env::set_var("FLOWPLANE_SECRET_CACHED_KEY", "cached-value");

        let env_client = EnvVarSecretsClient::new();
        let client = CachedSecretsClient::new(env_client, Duration::from_secs(60));

        // First call - cache miss
        let value1 = client.get_secret("cached_key").await.unwrap();
        assert_eq!(value1, "cached-value");
        assert_eq!(client.cache_size().await, 1);

        // Second call - cache hit
        let value2 = client.get_secret("cached_key").await.unwrap();
        assert_eq!(value2, "cached-value");

        std::env::remove_var("FLOWPLANE_SECRET_CACHED_KEY");
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        std::env::set_var("FLOWPLANE_SECRET_EXPIRY_KEY", "initial-value");

        let env_client = EnvVarSecretsClient::new();
        // Very short TTL for testing
        let client = CachedSecretsClient::new(env_client, Duration::from_millis(100));

        // Initial fetch
        let value1 = client.get_secret("expiry_key").await.unwrap();
        assert_eq!(value1, "initial-value");

        // Wait for expiry
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Update env var
        std::env::set_var("FLOWPLANE_SECRET_EXPIRY_KEY", "updated-value");

        // Should fetch new value after expiry
        let value2 = client.get_secret("expiry_key").await.unwrap();
        assert_eq!(value2, "updated-value");

        std::env::remove_var("FLOWPLANE_SECRET_EXPIRY_KEY");
    }

    #[tokio::test]
    async fn test_manual_refresh() {
        std::env::set_var("FLOWPLANE_SECRET_REFRESH_KEY", "before-refresh");

        let env_client = EnvVarSecretsClient::new();
        let client = CachedSecretsClient::new(env_client, Duration::from_secs(3600));

        // Initial fetch
        let value1 = client.get_secret("refresh_key").await.unwrap();
        assert_eq!(value1, "before-refresh");

        // Update env var
        std::env::set_var("FLOWPLANE_SECRET_REFRESH_KEY", "after-refresh");

        // Manually refresh
        client.refresh_secret("refresh_key").await.unwrap();

        // Should have new value
        let value2 = client.get_secret("refresh_key").await.unwrap();
        assert_eq!(value2, "after-refresh");

        std::env::remove_var("FLOWPLANE_SECRET_REFRESH_KEY");
    }

    #[tokio::test]
    async fn test_clear_cache() {
        std::env::set_var("FLOWPLANE_SECRET_CLEAR_KEY", "value");

        let env_client = EnvVarSecretsClient::new();
        let client = CachedSecretsClient::new(env_client, Duration::from_secs(60));

        // Populate cache
        client.get_secret("clear_key").await.unwrap();
        assert_eq!(client.cache_size().await, 1);

        // Clear cache
        client.clear_cache().await;
        assert_eq!(client.cache_size().await, 0);

        std::env::remove_var("FLOWPLANE_SECRET_CLEAR_KEY");
    }

    #[tokio::test]
    async fn test_set_updates_cache() {
        let env_client = EnvVarSecretsClient::new();
        let client = CachedSecretsClient::new(env_client, Duration::from_secs(60));

        // Note: EnvVarSecretsClient doesn't support set, so this will fail
        // but still demonstrates the API
        let result = client.set_secret("test_key", "test_value").await;
        assert!(result.is_err()); // Expected for EnvVarSecretsClient
    }
}
