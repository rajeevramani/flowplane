//! Secret cache for reducing backend calls
//!
//! Provides an in-memory TTL cache for fetched secrets to avoid
//! repeatedly calling external backends like Vault.

use crate::domain::SecretSpec;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::debug;

/// Cache key combining backend type, reference, and expected type
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CacheKey {
    pub backend_type: String,
    pub reference: String,
    pub secret_type: String,
}

impl CacheKey {
    pub fn new(backend_type: &str, reference: &str, secret_type: &str) -> Self {
        Self {
            backend_type: backend_type.to_string(),
            reference: reference.to_string(),
            secret_type: secret_type.to_string(),
        }
    }

    pub fn key_string(&self) -> String {
        format!("{}:{}:{}", self.backend_type, self.reference, self.secret_type)
    }
}

/// Cached secret entry with expiration
#[derive(Debug, Clone)]
struct CacheEntry {
    spec: SecretSpec,
    inserted_at: std::time::Instant,
}

/// Simple TTL cache for secrets
///
/// Uses a HashMap with manual TTL checking. For production use with
/// high concurrency, consider using moka::future::Cache.
#[derive(Debug)]
pub struct SecretCache {
    inner: Arc<RwLock<std::collections::HashMap<String, CacheEntry>>>,
    ttl: Duration,
}

impl SecretCache {
    /// Create a new cache with the specified TTL
    pub fn new(ttl: Duration) -> Self {
        Self { inner: Arc::new(RwLock::new(std::collections::HashMap::new())), ttl }
    }

    /// Create a new cache with default TTL (5 minutes)
    pub fn with_default_ttl() -> Self {
        Self::new(Duration::from_secs(300))
    }

    /// Get a cached secret if present and not expired
    pub async fn get(&self, key: &CacheKey) -> Option<SecretSpec> {
        let key_str = key.key_string();
        let cache = self.inner.read().await;

        if let Some(entry) = cache.get(&key_str) {
            if entry.inserted_at.elapsed() < self.ttl {
                debug!(key = %key_str, "Cache hit for secret");
                return Some(entry.spec.clone());
            } else {
                debug!(key = %key_str, "Cache entry expired");
            }
        }
        None
    }

    /// Insert a secret into the cache
    pub async fn insert(&self, key: &CacheKey, spec: SecretSpec) {
        let key_str = key.key_string();
        let mut cache = self.inner.write().await;

        debug!(key = %key_str, ttl_secs = %self.ttl.as_secs(), "Caching secret");

        cache.insert(key_str, CacheEntry { spec, inserted_at: std::time::Instant::now() });
    }

    /// Invalidate a specific cache entry
    pub async fn invalidate(&self, key: &CacheKey) {
        let key_str = key.key_string();
        let mut cache = self.inner.write().await;

        debug!(key = %key_str, "Invalidating cached secret");
        cache.remove(&key_str);
    }

    /// Invalidate all entries for a specific reference (across all backends/types)
    pub async fn invalidate_reference(&self, reference: &str) {
        let mut cache = self.inner.write().await;
        let keys_to_remove: Vec<String> =
            cache.keys().filter(|k| k.contains(&format!(":{}:", reference))).cloned().collect();

        for key in keys_to_remove {
            debug!(key = %key, "Invalidating cached secret by reference");
            cache.remove(&key);
        }
    }

    /// Clear all cache entries
    pub async fn clear(&self) {
        let mut cache = self.inner.write().await;
        debug!("Clearing entire secret cache");
        cache.clear();
    }

    /// Remove expired entries (cleanup)
    pub async fn cleanup_expired(&self) {
        let mut cache = self.inner.write().await;
        let ttl = self.ttl;

        cache.retain(|key, entry| {
            let expired = entry.inserted_at.elapsed() >= ttl;
            if expired {
                debug!(key = %key, "Removing expired cache entry");
            }
            !expired
        });
    }

    /// Get the number of entries in the cache
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Check if the cache is empty
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    /// Get the TTL for this cache
    pub fn ttl(&self) -> Duration {
        self.ttl
    }
}

impl Clone for SecretCache {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner), ttl: self.ttl }
    }
}

impl Default for SecretCache {
    fn default() -> Self {
        Self::with_default_ttl()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::GenericSecretSpec;

    fn make_test_spec() -> SecretSpec {
        SecretSpec::GenericSecret(GenericSecretSpec { secret: "dGVzdA==".to_string() })
    }

    #[tokio::test]
    async fn test_cache_insert_and_get() {
        let cache = SecretCache::new(Duration::from_secs(60));
        let key = CacheKey::new("vault", "path/to/secret", "generic_secret");
        let spec = make_test_spec();

        // Insert
        cache.insert(&key, spec.clone()).await;

        // Get
        let cached = cache.get(&key).await;
        assert!(cached.is_some());
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        let cache = SecretCache::new(Duration::from_millis(50));
        let key = CacheKey::new("vault", "path/to/secret", "generic_secret");
        let spec = make_test_spec();

        cache.insert(&key, spec).await;

        // Should exist immediately
        assert!(cache.get(&key).await.is_some());

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should be expired
        assert!(cache.get(&key).await.is_none());
    }

    #[tokio::test]
    async fn test_cache_invalidate() {
        let cache = SecretCache::new(Duration::from_secs(60));
        let key = CacheKey::new("vault", "path/to/secret", "generic_secret");
        let spec = make_test_spec();

        cache.insert(&key, spec).await;
        assert!(cache.get(&key).await.is_some());

        cache.invalidate(&key).await;
        assert!(cache.get(&key).await.is_none());
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = SecretCache::new(Duration::from_secs(60));

        cache.insert(&CacheKey::new("vault", "secret1", "generic_secret"), make_test_spec()).await;
        cache.insert(&CacheKey::new("vault", "secret2", "generic_secret"), make_test_spec()).await;

        assert_eq!(cache.len().await, 2);

        cache.clear().await;
        assert!(cache.is_empty().await);
    }

    #[tokio::test]
    async fn test_cache_key_uniqueness() {
        let cache = SecretCache::new(Duration::from_secs(60));

        let key1 = CacheKey::new("vault", "secret", "generic_secret");
        let key2 = CacheKey::new("database", "secret", "generic_secret");
        let key3 = CacheKey::new("vault", "secret", "tls_certificate");

        cache.insert(&key1, make_test_spec()).await;
        cache.insert(&key2, make_test_spec()).await;
        cache.insert(&key3, make_test_spec()).await;

        // All three should be separate entries
        assert_eq!(cache.len().await, 3);
    }
}
