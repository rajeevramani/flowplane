//! Certificate backend trait definition.
//!
//! This module defines the `CertificateBackend` trait for pluggable PKI backends
//! that can generate mTLS certificates for Envoy proxies.

use crate::secrets::error::Result;
use crate::secrets::vault::GeneratedCertificate;
use async_trait::async_trait;
use std::time::Duration;

/// Type of certificate backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CertificateBackendType {
    /// HashiCorp Vault PKI secrets engine
    VaultPki,
    /// AWS ACM Private CA (future)
    AwsAcmPca,
    /// GCP Certificate Authority Service (future)
    GcpCas,
    /// Mock backend for testing
    Mock,
}

impl CertificateBackendType {
    /// Returns the string representation of the backend type.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::VaultPki => "vault_pki",
            Self::AwsAcmPca => "aws_acm_pca",
            Self::GcpCas => "gcp_cas",
            Self::Mock => "mock",
        }
    }
}

impl std::fmt::Display for CertificateBackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Configuration for retry behavior on transient errors.
///
/// Used by cloud-based certificate backends to handle rate limiting
/// and transient network failures gracefully.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Initial backoff duration before first retry
    pub initial_backoff: Duration,
    /// Maximum backoff duration (cap for exponential growth)
    pub max_backoff: Duration,
    /// Multiplier for exponential backoff (e.g., 2.0 for doubling)
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(5),
            backoff_multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    /// Calculate the backoff duration for a given attempt number (0-indexed).
    pub fn backoff_for_attempt(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }

        let multiplier = self.backoff_multiplier.powi(attempt as i32 - 1);
        let backoff_ms = self.initial_backoff.as_millis() as f64 * multiplier;
        let capped_ms = backoff_ms.min(self.max_backoff.as_millis() as f64);

        Duration::from_millis(capped_ms as u64)
    }
}

/// Certificate management abstraction for mTLS certificate generation.
///
/// This trait defines the interface for PKI backends that can issue, renew,
/// and revoke certificates for Envoy proxies. Implementations must be thread-safe
/// (`Send + Sync`) and suitable for use in async contexts.
///
/// # SPIFFE Identity
///
/// All certificates include a SPIFFE URI in the Subject Alternative Name (SAN)
/// with the format: `spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}`.
/// The `team` parameter is critical for multi-tenant authorization - it determines
/// which resources the proxy can access.
///
/// # Security
///
/// - The `team` parameter is validated before certificate generation to prevent
///   injection attacks (see `validate_spiffe_component`).
/// - Private keys are returned via `SecretString` which redacts in Debug output.
/// - Generated certificates should be stored securely and transmitted only once.
///
/// # Example
///
/// ```rust,ignore
/// use flowplane::secrets::backends::certificates::CertificateBackend;
///
/// async fn issue_cert(backend: &dyn CertificateBackend, team: &str, proxy_id: &str) {
///     let cert = backend.generate_certificate(team, proxy_id, Some(720)).await?;
///     println!("Issued certificate with SPIFFE URI: {}", cert.spiffe_uri);
///     println!("Expires at: {}", cert.expires_at);
/// }
/// ```
#[async_trait]
pub trait CertificateBackend: Send + Sync + std::fmt::Debug {
    /// Generate a certificate for an Envoy proxy with SPIFFE identity.
    ///
    /// # Arguments
    ///
    /// * `team` - Team name for multi-tenant isolation (included in SPIFFE URI)
    /// * `proxy_id` - Unique identifier for the proxy instance
    /// * `ttl_hours` - Optional certificate validity period in hours (backend may have a default)
    ///
    /// # Returns
    ///
    /// Certificate bundle containing:
    /// - X.509 certificate (PEM)
    /// - Private key (PEM, wrapped in SecretString)
    /// - CA certificate chain (PEM)
    /// - Serial number
    /// - Expiration timestamp
    /// - SPIFFE URI
    ///
    /// # Errors
    ///
    /// - `SecretsError::InvalidValue` if team or proxy_id fail validation
    /// - `SecretsError::BackendError` if the PKI backend fails to issue certificate
    /// - `SecretsError::ConnectionFailed` if the backend is unreachable
    async fn generate_certificate(
        &self,
        team: &str,
        proxy_id: &str,
        ttl_hours: Option<u32>,
    ) -> Result<GeneratedCertificate>;

    /// Get the type of this certificate backend.
    fn backend_type(&self) -> CertificateBackendType;

    /// Perform a health check on the certificate backend.
    ///
    /// This method verifies that the backend is reachable and properly configured.
    /// It should NOT issue a certificate, just verify connectivity.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is unhealthy or unreachable.
    async fn health_check(&self) -> Result<()>;

    /// Get the trust domain for this backend.
    ///
    /// The trust domain is the root of the SPIFFE identity hierarchy
    /// (e.g., "flowplane.local", "prod.example.com").
    fn trust_domain(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.initial_backoff, Duration::from_millis(100));
        assert_eq!(config.max_backoff, Duration::from_secs(5));
        assert!((config.backoff_multiplier - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_retry_config_backoff_calculation() {
        let config = RetryConfig::default();

        // First attempt: no backoff
        assert_eq!(config.backoff_for_attempt(0), Duration::ZERO);

        // Second attempt: initial backoff (100ms)
        assert_eq!(config.backoff_for_attempt(1), Duration::from_millis(100));

        // Third attempt: 100ms * 2 = 200ms
        assert_eq!(config.backoff_for_attempt(2), Duration::from_millis(200));

        // Fourth attempt: 100ms * 4 = 400ms
        assert_eq!(config.backoff_for_attempt(3), Duration::from_millis(400));
    }

    #[test]
    fn test_retry_config_backoff_capped() {
        let config = RetryConfig {
            max_attempts: 10,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(500),
            backoff_multiplier: 2.0,
        };

        // 100ms * 2^6 = 6400ms, but capped at 500ms
        assert_eq!(config.backoff_for_attempt(7), Duration::from_millis(500));
    }

    #[test]
    fn test_certificate_backend_type_as_str() {
        assert_eq!(CertificateBackendType::VaultPki.as_str(), "vault_pki");
        assert_eq!(CertificateBackendType::AwsAcmPca.as_str(), "aws_acm_pca");
        assert_eq!(CertificateBackendType::GcpCas.as_str(), "gcp_cas");
        assert_eq!(CertificateBackendType::Mock.as_str(), "mock");
    }

    #[test]
    fn test_certificate_backend_type_display() {
        assert_eq!(format!("{}", CertificateBackendType::VaultPki), "vault_pki");
    }
}
