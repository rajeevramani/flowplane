//! Secret backend registry
//!
//! Manages multiple secret backends and provides unified access with caching.

use super::backend::{SecretBackend, SecretBackendType};
use super::cache::{CacheKey, SecretCache};
use super::database::DatabaseSecretBackend;
use super::vault::VaultSecretBackend;
use crate::domain::{SecretSpec, SecretType};
use crate::errors::{FlowplaneError, Result};
use crate::services::SecretEncryption;
use crate::storage::DbPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Registry of secret backends with caching
///
/// Provides a unified interface for fetching secrets from multiple backends.
/// Includes an in-memory cache to reduce calls to external systems.
#[derive(Debug)]
pub struct SecretBackendRegistry {
    backends: HashMap<SecretBackendType, Arc<dyn SecretBackend>>,
    cache: SecretCache,
}

impl SecretBackendRegistry {
    /// Create a new registry with no backends
    pub fn new(cache_ttl: Duration) -> Self {
        Self { backends: HashMap::new(), cache: SecretCache::new(cache_ttl) }
    }

    /// Create a new registry with default cache TTL (5 minutes)
    pub fn with_default_cache() -> Self {
        Self::new(Duration::from_secs(300))
    }

    /// Register a backend
    pub fn register(&mut self, backend: Arc<dyn SecretBackend>) {
        let backend_type = backend.backend_type();
        info!(backend_type = %backend_type, "Registering secret backend");
        self.backends.insert(backend_type, backend);
    }

    /// Check if a backend is registered
    pub fn has_backend(&self, backend_type: SecretBackendType) -> bool {
        self.backends.contains_key(&backend_type)
    }

    /// Get list of registered backend types
    pub fn registered_backends(&self) -> Vec<SecretBackendType> {
        self.backends.keys().copied().collect()
    }

    /// Initialize from environment configuration
    ///
    /// Automatically detects and registers available backends based on
    /// environment variables.
    pub async fn from_env(
        pool: DbPool,
        encryption: Option<Arc<SecretEncryption>>,
        cache_ttl_secs: Option<u64>,
    ) -> Result<Self> {
        let ttl = Duration::from_secs(cache_ttl_secs.unwrap_or(300));
        let mut registry = Self::new(ttl);

        // Always try to register database backend if encryption is available
        if let Some(encryption) = encryption {
            let db_backend = DatabaseSecretBackend::new(pool.clone(), encryption);
            registry.register(Arc::new(db_backend));
            info!("Registered database secret backend");
        }

        // Try to register Vault backend if configured
        match VaultSecretBackend::from_env().await {
            Ok(Some(vault_backend)) => {
                registry.register(Arc::new(vault_backend));
                info!("Registered Vault secret backend");
            }
            Ok(None) => {
                debug!("Vault backend not configured (FLOWPLANE_VAULT_ADDR not set)");
            }
            Err(e) => {
                warn!(error = %e, "Failed to initialize Vault backend");
            }
        }

        Ok(registry)
    }

    /// Fetch a secret from the specified backend (cache-aware)
    ///
    /// First checks the cache, then fetches from the backend if not cached.
    /// Successfully fetched secrets are cached for the configured TTL.
    pub async fn fetch_secret(
        &self,
        backend_type: SecretBackendType,
        reference: &str,
        expected_type: SecretType,
    ) -> Result<SecretSpec> {
        // Build cache key
        let cache_key = CacheKey::new(backend_type.as_str(), reference, expected_type.as_str());

        // Check cache first
        if let Some(cached) = self.cache.get(&cache_key).await {
            return Ok(cached);
        }

        // Get the backend
        let backend = self.backends.get(&backend_type).ok_or_else(|| {
            FlowplaneError::config(format!("Backend type '{}' not registered", backend_type))
        })?;

        // Fetch from backend
        let spec = backend.fetch_secret(reference, expected_type).await?;

        // Cache the result
        self.cache.insert(&cache_key, spec.clone()).await;

        Ok(spec)
    }

    /// Validate a reference exists in the specified backend
    pub async fn validate_reference(
        &self,
        backend_type: SecretBackendType,
        reference: &str,
    ) -> Result<bool> {
        let backend = self.backends.get(&backend_type).ok_or_else(|| {
            FlowplaneError::config(format!("Backend type '{}' not registered", backend_type))
        })?;

        backend.validate_reference(reference).await
    }

    /// Perform health check on all backends
    pub async fn health_check_all(&self) -> HashMap<SecretBackendType, Result<()>> {
        let mut results = HashMap::new();

        for (backend_type, backend) in &self.backends {
            let result = backend.health_check().await;
            results.insert(*backend_type, result);
        }

        results
    }

    /// Perform health check on a specific backend
    pub async fn health_check(&self, backend_type: SecretBackendType) -> Result<()> {
        let backend = self.backends.get(&backend_type).ok_or_else(|| {
            FlowplaneError::config(format!("Backend type '{}' not registered", backend_type))
        })?;

        backend.health_check().await
    }

    /// Invalidate a cached secret
    pub async fn invalidate_cache(&self, cache_key: &CacheKey) {
        self.cache.invalidate(cache_key).await;
    }

    /// Invalidate all cached entries for a reference
    pub async fn invalidate_reference(&self, reference: &str) {
        self.cache.invalidate_reference(reference).await;
    }

    /// Clear the entire cache
    pub async fn clear_cache(&self) {
        self.cache.clear().await;
    }

    /// Get cache statistics
    pub async fn cache_size(&self) -> usize {
        self.cache.len().await
    }

    /// Get the cache TTL
    pub fn cache_ttl(&self) -> Duration {
        self.cache.ttl()
    }
}

impl Clone for SecretBackendRegistry {
    fn clone(&self) -> Self {
        Self { backends: self.backends.clone(), cache: self.cache.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = SecretBackendRegistry::with_default_cache();
        assert!(registry.backends.is_empty());
        assert_eq!(registry.cache_ttl(), Duration::from_secs(300));
    }

    #[test]
    fn test_registered_backends_empty() {
        let registry = SecretBackendRegistry::with_default_cache();
        assert!(registry.registered_backends().is_empty());
    }

    #[test]
    fn test_has_backend_false() {
        let registry = SecretBackendRegistry::with_default_cache();
        assert!(!registry.has_backend(SecretBackendType::Vault));
        assert!(!registry.has_backend(SecretBackendType::Database));
    }
}
