//! Vault PKI certificate backend implementation.
//!
//! This module provides a `CertificateBackend` implementation that uses
//! HashiCorp Vault's PKI secrets engine to generate mTLS certificates.

use super::backend::{CertificateBackend, CertificateBackendType, RetryConfig};
use crate::secrets::error::{Result, SecretsError};
use crate::secrets::types::SecretString;
use crate::secrets::vault::{
    get_certificate_ttl_hours, validate_spiffe_component, GeneratedCertificate, PkiConfig,
};
use async_trait::async_trait;
use chrono::DateTime;
use tracing::{debug, error, info, warn};
use vaultrs::client::VaultClient;

/// Vault PKI certificate backend.
///
/// This backend uses HashiCorp Vault's PKI secrets engine to generate
/// X.509 certificates with SPIFFE URIs for mTLS authentication.
///
/// # Configuration
///
/// The backend is configured via environment variables:
/// - `VAULT_ADDR`: Vault server address
/// - `VAULT_TOKEN`: Authentication token
/// - `FLOWPLANE_VAULT_PKI_MOUNT_PATH`: PKI engine mount path
/// - `FLOWPLANE_VAULT_PKI_ROLE`: PKI role name for certificate issuance
/// - `FLOWPLANE_SPIFFE_TRUST_DOMAIN`: SPIFFE trust domain
///
/// # Example
///
/// ```rust,ignore
/// use flowplane::secrets::backends::certificates::VaultPkiBackend;
///
/// let backend = VaultPkiBackend::from_env().await?;
/// if let Some(backend) = backend {
///     let cert = backend.generate_certificate("engineering", "proxy-1", Some(720)).await?;
/// }
/// ```
pub struct VaultPkiBackend {
    client: VaultClient,
    pki_config: PkiConfig,
    retry_config: RetryConfig,
}

impl std::fmt::Debug for VaultPkiBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultPkiBackend")
            .field("pki_mount", &self.pki_config.mount_path)
            .field("pki_role", &self.pki_config.role_name)
            .field("trust_domain", &self.pki_config.trust_domain)
            .field("retry_config", &self.retry_config)
            .field("client", &"[VaultClient]")
            .finish()
    }
}

impl VaultPkiBackend {
    /// Create a new Vault PKI backend with the given client and configuration.
    ///
    /// # Arguments
    ///
    /// * `client` - A configured Vault client
    /// * `pki_config` - PKI secrets engine configuration
    pub fn new(client: VaultClient, pki_config: PkiConfig) -> Self {
        Self { client, pki_config, retry_config: RetryConfig::default() }
    }

    /// Configure custom retry behavior.
    ///
    /// # Arguments
    ///
    /// * `retry_config` - Retry configuration for transient failures
    pub fn with_retry_config(mut self, retry_config: RetryConfig) -> Self {
        self.retry_config = retry_config;
        self
    }

    /// Create a Vault PKI backend from environment variables.
    ///
    /// Returns `Ok(None)` if PKI is not configured (FLOWPLANE_VAULT_PKI_MOUNT_PATH not set).
    /// Returns `Ok(Some(backend))` if PKI is configured and connection succeeds.
    /// Returns `Err(...)` if PKI is configured but connection fails.
    ///
    /// # Environment Variables
    ///
    /// - `VAULT_ADDR`: Vault server address (required)
    /// - `VAULT_TOKEN`: Authentication token (optional, uses other auth methods if not set)
    /// - `VAULT_NAMESPACE`: Vault namespace (optional, for Enterprise)
    /// - `FLOWPLANE_VAULT_PKI_MOUNT_PATH`: PKI mount path (required to enable)
    /// - `FLOWPLANE_VAULT_PKI_ROLE`: PKI role name (default: "envoy-proxy")
    /// - `FLOWPLANE_SPIFFE_TRUST_DOMAIN`: Trust domain (default: "flowplane.local")
    pub async fn from_env() -> Result<Option<Self>> {
        // Check if PKI is enabled
        let pki_config = match PkiConfig::from_env() {
            Some(config) => config,
            None => {
                debug!("Vault PKI not configured (FLOWPLANE_VAULT_PKI_MOUNT_PATH not set)");
                return Ok(None);
            }
        };

        // Get Vault connection info
        let vault_addr = std::env::var("VAULT_ADDR").map_err(|_| {
            SecretsError::config_error(
                "VAULT_ADDR environment variable required when PKI is enabled",
            )
        })?;

        let vault_token = std::env::var("VAULT_TOKEN").ok();
        let vault_namespace = std::env::var("VAULT_NAMESPACE").ok();

        // Build Vault client
        let mut settings_builder = vaultrs::client::VaultClientSettingsBuilder::default();
        settings_builder.address(&vault_addr);

        if let Some(ref token) = vault_token {
            settings_builder.token(token);
        }

        if let Some(namespace) = vault_namespace {
            settings_builder.namespace(Some(namespace));
        }

        let settings = settings_builder.build().map_err(|e| {
            SecretsError::config_error(format!("Invalid Vault configuration: {}", e))
        })?;

        let client = VaultClient::new(settings).map_err(|e| {
            SecretsError::connection_failed(format!("Failed to create Vault client: {}", e))
        })?;

        // Test connection
        match vaultrs::sys::health(&client).await {
            Ok(_) => {
                info!(
                    vault_addr = %vault_addr,
                    pki_mount = %pki_config.mount_path,
                    pki_role = %pki_config.role_name,
                    trust_domain = %pki_config.trust_domain,
                    "Vault PKI certificate backend initialized"
                );
            }
            Err(e) => {
                error!(
                    error = %e,
                    vault_addr = %vault_addr,
                    "Failed to connect to Vault for PKI backend"
                );
                return Err(SecretsError::connection_failed(format!(
                    "Vault health check failed: {}",
                    e
                )));
            }
        }

        Ok(Some(Self::new(client, pki_config)))
    }

    /// Generate a certificate with retry logic for transient failures.
    async fn generate_with_retry(
        &self,
        team: &str,
        proxy_id: &str,
        ttl_hours: Option<u32>,
    ) -> Result<GeneratedCertificate> {
        use vaultrs::pki::cert;

        let spiffe_uri = self.pki_config.build_spiffe_uri(team, proxy_id)?;
        // Use provided TTL or get from environment with secure defaults (4-24 hours per ADR-008)
        let effective_ttl = ttl_hours.unwrap_or_else(get_certificate_ttl_hours);
        let ttl = format!("{}h", effective_ttl);

        let mut last_error = None;

        for attempt in 0..self.retry_config.max_attempts {
            if attempt > 0 {
                let backoff = self.retry_config.backoff_for_attempt(attempt);
                warn!(
                    attempt = attempt + 1,
                    max_attempts = self.retry_config.max_attempts,
                    backoff_ms = backoff.as_millis(),
                    "Retrying certificate generation after backoff"
                );
                tokio::time::sleep(backoff).await;
            }

            // Build certificate generation options
            let mut opts =
                vaultrs::api::pki::requests::GenerateCertificateRequestBuilder::default();
            opts.uri_sans(spiffe_uri.clone());
            opts.ttl(&ttl);

            match cert::generate(
                &self.client,
                &self.pki_config.mount_path,
                &self.pki_config.role_name,
                Some(&mut opts),
            )
            .await
            {
                Ok(response) => {
                    // Parse expiration timestamp
                    let expires_at = match response.expiration {
                        Some(ts) => DateTime::from_timestamp(ts as i64, 0).ok_or_else(|| {
                            error!(timestamp = ts, "Invalid expiration timestamp from Vault PKI");
                            SecretsError::backend_error(format!(
                                "Invalid expiration timestamp from Vault PKI: {}",
                                ts
                            ))
                        })?,
                        None => {
                            error!("Vault PKI response missing expiration timestamp");
                            return Err(SecretsError::backend_error(
                                "Vault PKI response missing expiration timestamp",
                            ));
                        }
                    };

                    // Build CA chain from response
                    let ca_chain = response
                        .ca_chain
                        .map(|chain| chain.join("\n"))
                        .unwrap_or_else(|| response.issuing_ca.clone());

                    info!(
                        team = %team,
                        proxy_id = %proxy_id,
                        serial_number = %response.serial_number,
                        expires_at = %expires_at,
                        spiffe_uri = %spiffe_uri,
                        "Successfully generated proxy certificate via Vault PKI"
                    );

                    return Ok(GeneratedCertificate {
                        certificate: response.certificate,
                        private_key: SecretString::new(response.private_key),
                        ca_chain,
                        serial_number: response.serial_number,
                        expires_at,
                        spiffe_uri,
                    });
                }
                Err(e) => {
                    let error_str = e.to_string();
                    let is_retryable = is_retryable_vault_error(&error_str);

                    if is_retryable && attempt + 1 < self.retry_config.max_attempts {
                        warn!(
                            error = %e,
                            team = %team,
                            proxy_id = %proxy_id,
                            attempt = attempt + 1,
                            "Transient Vault PKI error, will retry"
                        );
                        last_error = Some(e);
                        continue;
                    }

                    error!(
                        error = %e,
                        team = %team,
                        proxy_id = %proxy_id,
                        "Failed to generate certificate via Vault PKI"
                    );
                    return Err(SecretsError::backend_error(format!(
                        "Vault PKI certificate generation failed: {}",
                        e
                    )));
                }
            }
        }

        // All retries exhausted
        Err(SecretsError::backend_error(format!(
            "Vault PKI certificate generation failed after {} attempts: {}",
            self.retry_config.max_attempts,
            last_error.map(|e| e.to_string()).unwrap_or_default()
        )))
    }
}

/// Check if a Vault error is retryable (transient).
fn is_retryable_vault_error(error: &str) -> bool {
    let error_lower = error.to_lowercase();

    // Network errors
    if error_lower.contains("connection refused")
        || error_lower.contains("connection reset")
        || error_lower.contains("connection closed")
        || error_lower.contains("timed out")
        || error_lower.contains("timeout")
    {
        return true;
    }

    // Rate limiting
    if error_lower.contains("429") || error_lower.contains("too many requests") {
        return true;
    }

    // Server errors (5xx)
    if error_lower.contains("500")
        || error_lower.contains("502")
        || error_lower.contains("503")
        || error_lower.contains("504")
    {
        return true;
    }

    false
}

#[async_trait]
impl CertificateBackend for VaultPkiBackend {
    async fn generate_certificate(
        &self,
        team: &str,
        proxy_id: &str,
        ttl_hours: Option<u32>,
    ) -> Result<GeneratedCertificate> {
        // Validate inputs before making any network calls
        validate_spiffe_component(team, "team")?;
        validate_spiffe_component(proxy_id, "proxy_id")?;

        info!(
            team = %team,
            proxy_id = %proxy_id,
            ttl_hours = ?ttl_hours,
            pki_mount = %self.pki_config.mount_path,
            role = %self.pki_config.role_name,
            "Generating proxy certificate via Vault PKI"
        );

        self.generate_with_retry(team, proxy_id, ttl_hours).await
    }

    fn backend_type(&self) -> CertificateBackendType {
        CertificateBackendType::VaultPki
    }

    async fn health_check(&self) -> Result<()> {
        match vaultrs::sys::health(&self.client).await {
            Ok(_) => {
                debug!("Vault PKI backend health check passed");
                Ok(())
            }
            Err(e) => {
                error!(error = %e, "Vault PKI backend health check failed");
                Err(SecretsError::connection_failed(format!("Vault health check failed: {}", e)))
            }
        }
    }

    fn trust_domain(&self) -> &str {
        &self.pki_config.trust_domain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_is_retryable_vault_error_connection_errors() {
        assert!(is_retryable_vault_error("connection refused"));
        assert!(is_retryable_vault_error("Connection reset by peer"));
        assert!(is_retryable_vault_error("request timed out"));
        assert!(is_retryable_vault_error("timeout"));
    }

    #[test]
    fn test_is_retryable_vault_error_rate_limiting() {
        assert!(is_retryable_vault_error("429 Too Many Requests"));
        assert!(is_retryable_vault_error("Too Many Requests"));
    }

    #[test]
    fn test_is_retryable_vault_error_server_errors() {
        assert!(is_retryable_vault_error("500 Internal Server Error"));
        assert!(is_retryable_vault_error("502 Bad Gateway"));
        assert!(is_retryable_vault_error("503 Service Unavailable"));
        assert!(is_retryable_vault_error("504 Gateway Timeout"));
    }

    #[test]
    fn test_is_retryable_vault_error_non_retryable() {
        // Client errors should not be retried
        assert!(!is_retryable_vault_error("400 Bad Request"));
        assert!(!is_retryable_vault_error("403 Forbidden"));
        assert!(!is_retryable_vault_error("404 Not Found"));

        // Permission errors should not be retried
        assert!(!is_retryable_vault_error("permission denied"));

        // Configuration errors should not be retried
        assert!(!is_retryable_vault_error("invalid role"));
        assert!(!is_retryable_vault_error("unknown mount path"));
    }

    #[test]
    fn test_vault_pki_backend_debug_redacts_client() {
        // We can't easily test the actual debug output without a VaultClient,
        // but we can verify the Debug trait is implemented.
        // The test here just ensures the code compiles with Debug.
    }

    #[test]
    fn test_retry_config_used() {
        let config = RetryConfig {
            max_attempts: 5,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_secs(2),
            backoff_multiplier: 1.5,
        };

        // Verify config can be applied
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.backoff_for_attempt(1), Duration::from_millis(50));
    }
}
