//! Secret backend registry
//!
//! Manages multiple secret backends and provides unified access with caching.
//! Also manages the certificate backend for mTLS certificate generation.

use super::backend::{SecretBackend, SecretBackendType};
use super::cache::{CacheKey, SecretCache};
use super::certificates::{CertificateBackend, CertificateBackendType, VaultPkiBackend};
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
/// Also manages the optional certificate backend for mTLS certificate generation.
pub struct SecretBackendRegistry {
    backends: HashMap<SecretBackendType, Arc<dyn SecretBackend>>,
    cache: SecretCache,
    /// Certificate backend for mTLS certificate generation (optional)
    certificate_backend: Option<Arc<dyn CertificateBackend>>,
}

impl std::fmt::Debug for SecretBackendRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretBackendRegistry")
            .field("backends", &self.backends.keys().collect::<Vec<_>>())
            .field("cache_ttl", &self.cache.ttl())
            .field(
                "certificate_backend",
                &self.certificate_backend.as_ref().map(|b| b.backend_type()),
            )
            .finish()
    }
}

impl SecretBackendRegistry {
    /// Create a new registry with no backends
    pub fn new(cache_ttl: Duration) -> Self {
        Self {
            backends: HashMap::new(),
            cache: SecretCache::new(cache_ttl),
            certificate_backend: None,
        }
    }

    /// Create a new registry with default cache TTL (5 minutes)
    pub fn with_default_cache() -> Self {
        Self::new(Duration::from_secs(300))
    }

    /// Register a secret backend
    pub fn register(&mut self, backend: Arc<dyn SecretBackend>) {
        let backend_type = backend.backend_type();
        info!(backend_type = %backend_type, "Registering secret backend");
        self.backends.insert(backend_type, backend);
    }

    /// Check if a secret backend is registered
    pub fn has_backend(&self, backend_type: SecretBackendType) -> bool {
        self.backends.contains_key(&backend_type)
    }

    /// Get list of registered secret backend types
    pub fn registered_backends(&self) -> Vec<SecretBackendType> {
        self.backends.keys().copied().collect()
    }

    // =========================================================================
    // Certificate Backend Methods
    // =========================================================================

    /// Register a certificate backend for mTLS certificate generation.
    ///
    /// Only one certificate backend can be registered at a time.
    /// Calling this method again will replace the existing backend.
    pub fn register_certificate_backend(&mut self, backend: Arc<dyn CertificateBackend>) {
        let backend_type = backend.backend_type();
        info!(backend_type = %backend_type, trust_domain = %backend.trust_domain(), "Registering certificate backend");
        self.certificate_backend = Some(backend);
    }

    /// Get the registered certificate backend, if any.
    ///
    /// Returns `None` if no certificate backend is configured.
    pub fn certificate_backend(&self) -> Option<Arc<dyn CertificateBackend>> {
        self.certificate_backend.clone()
    }

    /// Check if a certificate backend is registered.
    pub fn has_certificate_backend(&self) -> bool {
        self.certificate_backend.is_some()
    }

    /// Get the certificate backend type, if registered.
    pub fn certificate_backend_type(&self) -> Option<CertificateBackendType> {
        self.certificate_backend.as_ref().map(|b| b.backend_type())
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

        // Try to register GCP Secret Manager backend if configured (feature-gated)
        #[cfg(feature = "gcp")]
        {
            use super::gcp::GcpSecretBackend;
            match GcpSecretBackend::from_env().await {
                Ok(Some(gcp_backend)) => {
                    registry.register(Arc::new(gcp_backend));
                    info!("Registered GCP Secret Manager backend");
                }
                Ok(None) => {
                    debug!("GCP backend not configured (FLOWPLANE_GCP_PROJECT_ID not set)");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to initialize GCP Secret Manager backend");
                }
            }
        }

        // Try to register certificate backend
        // If FLOWPLANE_USE_MOCK_CERT_BACKEND=1, use MockCertificateBackend for testing
        // Otherwise, try Vault PKI if configured
        if std::env::var("FLOWPLANE_USE_MOCK_CERT_BACKEND").ok().as_deref() == Some("1") {
            use super::certificates::MockCertificateBackend;
            let trust_domain = std::env::var("FLOWPLANE_SPIFFE_TRUST_DOMAIN")
                .unwrap_or_else(|_| "flowplane.local".to_string());
            let mock_backend = MockCertificateBackend::new(trust_domain);
            registry.register_certificate_backend(Arc::new(mock_backend));
            info!("Registered Mock certificate backend for testing (FLOWPLANE_USE_MOCK_CERT_BACKEND=1)");
        } else {
            match VaultPkiBackend::from_env().await {
                Ok(Some(cert_backend)) => {
                    registry.register_certificate_backend(Arc::new(cert_backend));
                    info!("Registered Vault PKI certificate backend");
                }
                Ok(None) => {
                    debug!(
                        "Certificate backend not configured (FLOWPLANE_VAULT_PKI_MOUNT_PATH not set)"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "Failed to initialize Vault PKI certificate backend");
                }
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
        Self {
            backends: self.backends.clone(),
            cache: self.cache.clone(),
            certificate_backend: self.certificate_backend.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::backends::certificates::MockCertificateBackend;

    #[test]
    fn test_registry_creation() {
        let registry = SecretBackendRegistry::with_default_cache();
        assert!(registry.backends.is_empty());
        assert_eq!(registry.cache_ttl(), Duration::from_secs(300));
        assert!(!registry.has_certificate_backend());
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

    #[test]
    fn test_certificate_backend_registration() {
        let mut registry = SecretBackendRegistry::with_default_cache();

        // No certificate backend initially
        assert!(!registry.has_certificate_backend());
        assert!(registry.certificate_backend().is_none());
        assert!(registry.certificate_backend_type().is_none());

        // Register mock certificate backend
        let mock = Arc::new(MockCertificateBackend::new("test.local"));
        registry.register_certificate_backend(mock);

        // Now it should be registered
        assert!(registry.has_certificate_backend());
        assert!(registry.certificate_backend().is_some());
        assert_eq!(registry.certificate_backend_type(), Some(CertificateBackendType::Mock));
    }

    #[tokio::test]
    async fn test_certificate_backend_generate() {
        let mut registry = SecretBackendRegistry::with_default_cache();
        let mock = Arc::new(MockCertificateBackend::new("test.local"));
        registry.register_certificate_backend(mock);

        let backend = registry.certificate_backend().expect("should have backend");
        let cert = backend
            .generate_certificate("engineering", "proxy-1", Some(24))
            .await
            .expect("should generate certificate");

        assert!(cert.spiffe_uri.contains("engineering"));
        assert!(cert.spiffe_uri.contains("proxy-1"));
    }

    #[test]
    fn test_registry_debug_includes_certificate_backend() {
        let mut registry = SecretBackendRegistry::with_default_cache();
        let mock = Arc::new(MockCertificateBackend::new("test.local"));
        registry.register_certificate_backend(mock);

        let debug_output = format!("{:?}", registry);
        assert!(debug_output.contains("Mock"));
    }

    #[test]
    fn test_registry_clone_includes_certificate_backend() {
        let mut registry = SecretBackendRegistry::with_default_cache();
        let mock = Arc::new(MockCertificateBackend::new("test.local"));
        registry.register_certificate_backend(mock);

        let cloned = registry.clone();
        assert!(cloned.has_certificate_backend());
        assert_eq!(cloned.certificate_backend_type(), Some(CertificateBackendType::Mock));
    }
}
