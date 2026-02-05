//! HashiCorp Vault secrets backend implementation.
//!
//! This module provides integration with HashiCorp Vault's KV v2 secrets engine
//! and PKI secrets engine for mTLS certificate generation.
//!
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
//! For PKI/mTLS certificate generation, additional configuration via environment:
//! - `FLOWPLANE_VAULT_PKI_MOUNT_PATH`: PKI engine mount path (enables mTLS if set)
//! - `FLOWPLANE_SPIFFE_TRUST_DOMAIN`: SPIFFE trust domain (default: "flowplane.local")
//! - `FLOWPLANE_VAULT_PKI_ROLE`: PKI role name (default: "envoy-proxy")
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
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use vaultrs::client::{VaultClient, VaultClientSettingsBuilder};
use vaultrs::kv2;

use super::client::{SecretMetadata, SecretsClient};
use super::error::{Result, SecretsError};

// ============================================================================
// PKI Configuration Types
// ============================================================================

/// Configuration for Vault PKI secrets engine.
///
/// This configuration controls mTLS certificate generation for Envoy proxies.
/// If `FLOWPLANE_VAULT_PKI_MOUNT_PATH` is not set, mTLS is disabled and the
/// control plane operates in insecure mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkiConfig {
    /// PKI secrets engine mount path (e.g., "pki_int_proxies")
    pub mount_path: String,

    /// Vault PKI role name for certificate issuance
    pub role_name: String,

    /// SPIFFE trust domain for certificate identity URIs
    pub trust_domain: String,
}

impl PkiConfig {
    /// Load PKI configuration from environment variables.
    ///
    /// Returns `None` if `FLOWPLANE_VAULT_PKI_MOUNT_PATH` is not set,
    /// indicating mTLS is disabled.
    ///
    /// # Environment Variables
    ///
    /// - `FLOWPLANE_VAULT_PKI_MOUNT_PATH`: Required to enable mTLS
    /// - `FLOWPLANE_VAULT_PKI_ROLE`: Role name (default: "envoy-proxy")
    /// - `FLOWPLANE_SPIFFE_TRUST_DOMAIN`: Trust domain (default: "flowplane.local")
    pub fn from_env() -> Option<Self> {
        let mount_path = std::env::var("FLOWPLANE_VAULT_PKI_MOUNT_PATH").ok()?;

        let role_name =
            std::env::var("FLOWPLANE_VAULT_PKI_ROLE").unwrap_or_else(|_| "envoy-proxy".to_string());

        let trust_domain = std::env::var("FLOWPLANE_SPIFFE_TRUST_DOMAIN")
            .unwrap_or_else(|_| "flowplane.local".to_string());

        Some(Self { mount_path, role_name, trust_domain })
    }

    /// Check if mTLS is enabled based on environment configuration.
    pub fn is_mtls_enabled() -> bool {
        std::env::var("FLOWPLANE_VAULT_PKI_MOUNT_PATH").is_ok()
    }

    /// Build a SPIFFE URI for the given team and proxy with validation.
    ///
    /// Format: `spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}`
    ///
    /// # Arguments
    ///
    /// * `team` - Team name (validated for injection attacks)
    /// * `proxy_id` - Unique identifier for the proxy instance
    ///
    /// # Returns
    ///
    /// The constructed SPIFFE URI if validation passes.
    ///
    /// # Errors
    ///
    /// Returns `SecretsError::InvalidValue` if any component contains:
    /// - Path separators (`/`), at signs (`@`), or colons (`:`)
    /// - Path traversal sequences (`..`)
    /// - Empty strings or strings exceeding 128 characters
    pub fn build_spiffe_uri(&self, team: &str, proxy_id: &str) -> Result<String> {
        // Validate components before constructing URI
        validate_spiffe_component(team, "team")?;
        validate_spiffe_component(proxy_id, "proxy_id")?;

        Ok(format!("spiffe://{}/team/{}/proxy/{}", self.trust_domain, team, proxy_id))
    }
}

/// Validates a SPIFFE URI component for injection attacks.
///
/// Rejects components containing:
/// - Path separators (`/`) - prevents path traversal
/// - At signs (`@`) - prevents authority injection
/// - Colons (`:`) - prevents scheme/port injection
/// - Path traversal sequences (`..`)
/// - Empty strings
/// - Strings longer than 128 characters (prevents DoS)
///
/// # Arguments
///
/// * `component` - The component to validate (team name, proxy ID, etc.)
/// * `component_name` - Human-readable name for error messages
///
/// # Returns
///
/// `Ok(())` if valid, `Err(SecretsError::InvalidValue)` if invalid.
///
/// # Example
///
/// ```rust,ignore
/// validate_spiffe_component("engineering", "team")?; // OK
/// validate_spiffe_component("../admin", "team")?;    // Error: contains '..'
/// validate_spiffe_component("team/admin", "team")?;  // Error: contains '/'
/// ```
pub fn validate_spiffe_component(component: &str, component_name: &str) -> Result<()> {
    // Reject empty strings
    if component.is_empty() {
        return Err(SecretsError::invalid_value(format!(
            "SPIFFE {} cannot be empty",
            component_name
        )));
    }

    // Reject strings longer than 128 characters (prevent DoS)
    if component.len() > 128 {
        return Err(SecretsError::invalid_value(format!(
            "SPIFFE {} exceeds maximum length of 128 characters (got {})",
            component_name,
            component.len()
        )));
    }

    // Reject path separators (prevents path traversal)
    if component.contains('/') {
        return Err(SecretsError::invalid_value(format!(
            "SPIFFE {} cannot contain '/' (path separator)",
            component_name
        )));
    }

    // Reject at signs (prevents authority injection)
    if component.contains('@') {
        return Err(SecretsError::invalid_value(format!(
            "SPIFFE {} cannot contain '@' (authority separator)",
            component_name
        )));
    }

    // Reject colons (prevents scheme/port injection)
    if component.contains(':') {
        return Err(SecretsError::invalid_value(format!(
            "SPIFFE {} cannot contain ':' (scheme/port separator)",
            component_name
        )));
    }

    // Reject path traversal sequences
    if component.contains("..") {
        return Err(SecretsError::invalid_value(format!(
            "SPIFFE {} cannot contain '..' (path traversal)",
            component_name
        )));
    }

    Ok(())
}

/// Certificate bundle generated by Vault PKI.
///
/// Contains all materials needed for an Envoy proxy to establish mTLS
/// connections to the control plane.
///
/// # Security
///
/// The `private_key` field uses [`SecretString`] to prevent accidental
/// logging. Debug output will show `[REDACTED]` for the private key.
#[derive(Clone, Serialize, Deserialize)]
pub struct GeneratedCertificate {
    /// PEM-encoded X.509 certificate
    pub certificate: String,

    /// PEM-encoded private key (redacted in logs)
    pub private_key: super::types::SecretString,

    /// PEM-encoded CA certificate chain
    pub ca_chain: String,

    /// Certificate serial number (from Vault)
    pub serial_number: String,

    /// Certificate expiration timestamp
    pub expires_at: DateTime<Utc>,

    /// SPIFFE URI embedded in the certificate SAN
    pub spiffe_uri: String,
}

impl std::fmt::Debug for GeneratedCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedCertificate")
            .field("certificate", &format!("[{} bytes PEM]", self.certificate.len()))
            .field("private_key", &self.private_key) // Uses SecretString's Debug
            .field("ca_chain", &format!("[{} bytes PEM]", self.ca_chain.len()))
            .field("serial_number", &self.serial_number)
            .field("expires_at", &self.expires_at)
            .field("spiffe_uri", &self.spiffe_uri)
            .finish()
    }
}

/// Parse team name from a SPIFFE URI.
///
/// URI format: `spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}`
///
/// # Arguments
///
/// * `uri` - The SPIFFE URI to parse
///
/// # Returns
///
/// The team name if the URI is valid, otherwise `None`.
///
/// # Example
///
/// ```rust,ignore
/// let team = parse_team_from_spiffe_uri("spiffe://flowplane.local/team/engineering/proxy/proxy-1");
/// assert_eq!(team, Some("engineering".to_string()));
/// ```
pub fn parse_team_from_spiffe_uri(uri: &str) -> Option<String> {
    let parts: Vec<&str> = uri.split('/').collect();
    // Expected: ["spiffe:", "", "{domain}", "team", "{team}", "proxy", "{proxy_id}"]
    if parts.len() >= 5 && parts[3] == "team" {
        Some(parts[4].to_string())
    } else {
        None
    }
}

/// Parse proxy ID from a SPIFFE URI.
///
/// URI format: `spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}`
///
/// # Arguments
///
/// * `uri` - The SPIFFE URI to parse
///
/// # Returns
///
/// The proxy ID if the URI is valid, otherwise `None`.
pub fn parse_proxy_id_from_spiffe_uri(uri: &str) -> Option<String> {
    let parts: Vec<&str> = uri.split('/').collect();
    // Expected: ["spiffe:", "", "{domain}", "team", "{team}", "proxy", "{proxy_id}"]
    if parts.len() >= 7 && parts[3] == "team" && parts[5] == "proxy" {
        Some(parts[6].to_string())
    } else {
        None
    }
}

/// Get configured TTL for certificate generation.
///
/// Reads from `FLOWPLANE_VAULT_PKI_TTL_HOURS` environment variable.
/// Default: 12 hours
/// Range: 4-24 hours (clamped with warning if out of range)
///
/// Per ADR-008, short-lived certificates (4-24 hours) are required for
/// shared dataplane certificates to limit blast radius on compromise.
pub fn get_certificate_ttl_hours() -> u32 {
    const DEFAULT_TTL: u32 = 12;
    const MIN_TTL: u32 = 4;
    const MAX_TTL: u32 = 24;

    let ttl = std::env::var("FLOWPLANE_VAULT_PKI_TTL_HOURS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_TTL);

    if ttl < MIN_TTL {
        tracing::warn!(
            requested_ttl = ttl,
            min_ttl = MIN_TTL,
            "Certificate TTL below minimum, clamping to {} hours",
            MIN_TTL
        );
        MIN_TTL
    } else if ttl > MAX_TTL {
        tracing::warn!(
            requested_ttl = ttl,
            max_ttl = MAX_TTL,
            "Certificate TTL exceeds maximum (security risk per ADR-008), clamping to {} hours",
            MAX_TTL
        );
        MAX_TTL
    } else {
        ttl
    }
}

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

    /// Generate a certificate for an Envoy proxy with SPIFFE identity.
    ///
    /// This method issues a certificate via Vault's PKI secrets engine with
    /// a SPIFFE URI embedded in the Subject Alternative Name (SAN).
    ///
    /// # Arguments
    ///
    /// * `pki_config` - PKI engine configuration (mount path, role, trust domain)
    /// * `team` - Team name (used in SPIFFE URI)
    /// * `proxy_id` - Unique identifier for the proxy instance
    ///
    /// # Returns
    ///
    /// Certificate bundle containing cert, private key, CA chain, and metadata.
    ///
    /// # Errors
    ///
    /// - [`SecretsError::BackendError`] if Vault PKI fails to issue certificate
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let pki_config = PkiConfig::from_env().expect("PKI not configured");
    /// let cert = client.generate_proxy_certificate(&pki_config, "engineering", "proxy-1").await?;
    /// ```
    pub async fn generate_proxy_certificate(
        &self,
        pki_config: &PkiConfig,
        team: &str,
        proxy_id: &str,
    ) -> Result<GeneratedCertificate> {
        use vaultrs::pki::cert;

        let spiffe_uri = pki_config.build_spiffe_uri(team, proxy_id)?;

        tracing::info!(
            team = %team,
            proxy_id = %proxy_id,
            spiffe_uri = %spiffe_uri,
            pki_mount = %pki_config.mount_path,
            role = %pki_config.role_name,
            "Generating proxy certificate via Vault PKI"
        );

        // Build certificate generation options
        let mut opts = vaultrs::api::pki::requests::GenerateCertificateRequestBuilder::default();
        // uri_sans expects a comma-separated string for multiple SANs
        opts.uri_sans(spiffe_uri.clone());

        // Configure TTL with secure defaults (4-24 hour range per ADR-008)
        let ttl_hours = get_certificate_ttl_hours();
        opts.ttl(format!("{}h", ttl_hours));

        // Generate certificate via Vault PKI
        let response = cert::generate(
            &self.client,
            &pki_config.mount_path,
            &pki_config.role_name,
            Some(&mut opts),
        )
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                team = %team,
                proxy_id = %proxy_id,
                "Failed to generate certificate via Vault PKI"
            );
            SecretsError::backend_error(format!("Vault PKI certificate generation failed: {}", e))
        })?;

        // Parse expiration timestamp - return error if invalid or missing
        let expires_at = match response.expiration {
            Some(ts) => DateTime::from_timestamp(ts as i64, 0).ok_or_else(|| {
                tracing::error!(timestamp = ts, "Invalid expiration timestamp from Vault PKI");
                SecretsError::backend_error(format!(
                    "Invalid expiration timestamp from Vault PKI: {}",
                    ts
                ))
            })?,
            None => {
                tracing::error!("Vault PKI response missing expiration timestamp");
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

        tracing::info!(
            team = %team,
            proxy_id = %proxy_id,
            serial_number = %response.serial_number,
            expires_at = %expires_at,
            "Successfully generated proxy certificate"
        );

        Ok(GeneratedCertificate {
            certificate: response.certificate,
            private_key: super::types::SecretString::new(response.private_key),
            ca_chain,
            serial_number: response.serial_number,
            expires_at,
            spiffe_uri,
        })
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

    // =========================================================================
    // PKI Configuration Tests
    // =========================================================================

    #[test]
    fn test_pki_config_build_spiffe_uri() {
        let config = PkiConfig {
            mount_path: "pki_int_proxies".to_string(),
            role_name: "envoy-proxy".to_string(),
            trust_domain: "flowplane.local".to_string(),
        };

        let uri = config.build_spiffe_uri("engineering", "proxy-1").unwrap();
        assert_eq!(uri, "spiffe://flowplane.local/team/engineering/proxy/proxy-1");
    }

    #[test]
    fn test_pki_config_build_spiffe_uri_special_chars() {
        let config = PkiConfig {
            mount_path: "pki".to_string(),
            role_name: "proxy".to_string(),
            trust_domain: "example.com".to_string(),
        };

        // Team and proxy names with hyphens and numbers
        let uri = config.build_spiffe_uri("team-123", "proxy-abc-456").unwrap();
        assert_eq!(uri, "spiffe://example.com/team/team-123/proxy/proxy-abc-456");
    }

    #[test]
    fn test_pki_config_serialization() {
        let config = PkiConfig {
            mount_path: "pki_int".to_string(),
            role_name: "my-role".to_string(),
            trust_domain: "prod.example.com".to_string(),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: PkiConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.mount_path, deserialized.mount_path);
        assert_eq!(config.role_name, deserialized.role_name);
        assert_eq!(config.trust_domain, deserialized.trust_domain);
    }

    // =========================================================================
    // SPIFFE URI Parsing Tests
    // =========================================================================

    #[test]
    fn test_parse_team_from_spiffe_uri_valid() {
        let uri = "spiffe://flowplane.local/team/engineering/proxy/proxy-1";
        assert_eq!(parse_team_from_spiffe_uri(uri), Some("engineering".to_string()));
    }

    #[test]
    fn test_parse_team_from_spiffe_uri_different_domain() {
        let uri = "spiffe://prod.example.com/team/payments/proxy/edge-gateway";
        assert_eq!(parse_team_from_spiffe_uri(uri), Some("payments".to_string()));
    }

    #[test]
    fn test_parse_team_from_spiffe_uri_team_with_hyphen() {
        let uri = "spiffe://flowplane.local/team/platform-team/proxy/p1";
        assert_eq!(parse_team_from_spiffe_uri(uri), Some("platform-team".to_string()));
    }

    #[test]
    fn test_parse_team_from_spiffe_uri_invalid_format() {
        // Missing team segment
        assert_eq!(parse_team_from_spiffe_uri("spiffe://domain/proxy/p1"), None);
        // Wrong segment name
        assert_eq!(parse_team_from_spiffe_uri("spiffe://domain/group/eng/proxy/p1"), None);
        // Empty string
        assert_eq!(parse_team_from_spiffe_uri(""), None);
        // Not a SPIFFE URI
        assert_eq!(parse_team_from_spiffe_uri("https://example.com"), None);
    }

    #[test]
    fn test_parse_proxy_id_from_spiffe_uri_valid() {
        let uri = "spiffe://flowplane.local/team/engineering/proxy/proxy-1";
        assert_eq!(parse_proxy_id_from_spiffe_uri(uri), Some("proxy-1".to_string()));
    }

    #[test]
    fn test_parse_proxy_id_from_spiffe_uri_complex_id() {
        let uri = "spiffe://flowplane.local/team/eng/proxy/k8s-pod-abc123-xyz";
        assert_eq!(parse_proxy_id_from_spiffe_uri(uri), Some("k8s-pod-abc123-xyz".to_string()));
    }

    #[test]
    fn test_parse_proxy_id_from_spiffe_uri_invalid_format() {
        // Missing proxy segment
        assert_eq!(parse_proxy_id_from_spiffe_uri("spiffe://domain/team/eng"), None);
        // Wrong segment name
        assert_eq!(parse_proxy_id_from_spiffe_uri("spiffe://domain/team/eng/node/n1"), None);
    }

    #[test]
    fn test_spiffe_uri_roundtrip() {
        let config = PkiConfig {
            mount_path: "pki".to_string(),
            role_name: "proxy".to_string(),
            trust_domain: "flowplane.local".to_string(),
        };

        let team = "payments";
        let proxy_id = "edge-proxy-1";

        let uri = config.build_spiffe_uri(team, proxy_id).unwrap();

        // Parse should return the original values
        assert_eq!(parse_team_from_spiffe_uri(&uri), Some(team.to_string()));
        assert_eq!(parse_proxy_id_from_spiffe_uri(&uri), Some(proxy_id.to_string()));
    }

    // =========================================================================
    // SPIFFE URI Injection Security Tests
    // =========================================================================

    #[test]
    fn test_validate_spiffe_component_valid() {
        // Valid components - should all pass
        assert!(validate_spiffe_component("engineering", "team").is_ok());
        assert!(validate_spiffe_component("proxy-1", "proxy_id").is_ok());
        assert!(validate_spiffe_component("team-alpha-123", "team").is_ok());
        assert!(validate_spiffe_component("a", "team").is_ok()); // Single char
        assert!(validate_spiffe_component("with.dots", "team").is_ok()); // Dots allowed
        assert!(validate_spiffe_component("with_underscores", "team").is_ok());
    }

    #[test]
    fn test_validate_spiffe_component_empty() {
        let err = validate_spiffe_component("", "team").unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_spiffe_component_too_long() {
        let long_string = "a".repeat(129);
        let err = validate_spiffe_component(&long_string, "team").unwrap_err();
        assert!(err.to_string().contains("exceeds maximum length"));

        // Exactly 128 should be fine
        let max_length = "a".repeat(128);
        assert!(validate_spiffe_component(&max_length, "team").is_ok());
    }

    #[test]
    fn test_validate_spiffe_component_path_separator() {
        let err = validate_spiffe_component("team/admin", "team").unwrap_err();
        assert!(err.to_string().contains("cannot contain '/'"));

        let err = validate_spiffe_component("a/b/c", "proxy_id").unwrap_err();
        assert!(err.to_string().contains("cannot contain '/'"));
    }

    #[test]
    fn test_validate_spiffe_component_at_sign() {
        let err = validate_spiffe_component("user@domain", "team").unwrap_err();
        assert!(err.to_string().contains("cannot contain '@'"));
    }

    #[test]
    fn test_validate_spiffe_component_colon() {
        let err = validate_spiffe_component("https:test", "team").unwrap_err();
        assert!(err.to_string().contains("cannot contain ':'"));

        let err = validate_spiffe_component("host:8080", "proxy_id").unwrap_err();
        assert!(err.to_string().contains("cannot contain ':'"));
    }

    #[test]
    fn test_validate_spiffe_component_path_traversal() {
        // Embedded path traversal (no slash, so hits ".." check)
        let err = validate_spiffe_component("team..admin", "team").unwrap_err();
        assert!(err.to_string().contains("cannot contain '..'"));

        // Trailing path traversal
        let err = validate_spiffe_component("admin..", "team").unwrap_err();
        assert!(err.to_string().contains("cannot contain '..'"));

        // Leading path traversal - this also contains '/', so test separately
        let err = validate_spiffe_component("..admin", "team").unwrap_err();
        assert!(err.to_string().contains("cannot contain '..'"));

        // With slash - will be caught by '/' check first (that's fine, both are blocked)
        assert!(validate_spiffe_component("../admin", "team").is_err());
    }

    #[test]
    fn test_build_spiffe_uri_injection_prevention() {
        let config = PkiConfig {
            mount_path: "pki".to_string(),
            role_name: "proxy".to_string(),
            trust_domain: "flowplane.local".to_string(),
        };

        // Path traversal attack
        assert!(config.build_spiffe_uri("../admin", "proxy-1").is_err());
        assert!(config.build_spiffe_uri("../../etc", "passwd").is_err());

        // Path separator injection in proxy_id
        assert!(config.build_spiffe_uri("team", "../../team/other/proxy/x").is_err());

        // Authority injection
        assert!(config.build_spiffe_uri("admin@evil.com", "proxy").is_err());

        // Scheme injection
        assert!(config.build_spiffe_uri("evil.com:443", "proxy").is_err());

        // Empty components
        assert!(config.build_spiffe_uri("", "proxy-1").is_err());
        assert!(config.build_spiffe_uri("team", "").is_err());

        // Valid inputs should still work
        assert!(config.build_spiffe_uri("engineering", "proxy-1").is_ok());
        assert!(config.build_spiffe_uri("team-123", "envoy-gateway").is_ok());
    }

    #[test]
    fn test_build_spiffe_uri_preserves_format() {
        let config = PkiConfig {
            mount_path: "pki".to_string(),
            role_name: "proxy".to_string(),
            trust_domain: "prod.example.com".to_string(),
        };

        let uri = config.build_spiffe_uri("payments", "gateway-1").unwrap();
        assert_eq!(uri, "spiffe://prod.example.com/team/payments/proxy/gateway-1");

        // Verify the URI can be parsed back
        assert_eq!(parse_team_from_spiffe_uri(&uri), Some("payments".to_string()));
        assert_eq!(parse_proxy_id_from_spiffe_uri(&uri), Some("gateway-1".to_string()));
    }

    // =========================================================================
    // GeneratedCertificate Security Tests
    // =========================================================================

    #[test]
    fn test_generated_certificate_debug_redacts_private_key() {
        use crate::secrets::SecretString;
        let cert = GeneratedCertificate {
            certificate: "-----BEGIN CERTIFICATE-----\nMOCK_CERT\n-----END CERTIFICATE-----"
                .to_string(),
            private_key: SecretString::new(
                "-----BEGIN PRIVATE KEY-----\nSUPER_SECRET_KEY\n-----END PRIVATE KEY-----",
            ),
            ca_chain: "-----BEGIN CERTIFICATE-----\nCA_CHAIN\n-----END CERTIFICATE-----"
                .to_string(),
            serial_number: "1234567890".to_string(),
            expires_at: chrono::Utc::now(),
            spiffe_uri: "spiffe://test.local/team/eng/proxy/p1".to_string(),
        };

        let debug_output = format!("{:?}", cert);

        // Verify private key material is NOT in debug output
        assert!(!debug_output.contains("SUPER_SECRET_KEY"));
        assert!(!debug_output.contains("BEGIN PRIVATE KEY"));

        // Verify the debug output shows it's redacted
        assert!(debug_output.contains("[REDACTED]"));

        // Verify other non-sensitive fields are visible
        assert!(debug_output.contains("1234567890")); // serial_number
        assert!(debug_output.contains("spiffe://test.local")); // spiffe_uri
    }

    #[test]
    fn test_generated_certificate_private_key_accessible() {
        use crate::secrets::SecretString;
        let secret_key = "-----BEGIN PRIVATE KEY-----\nMY_SECRET\n-----END PRIVATE KEY-----";
        let cert = GeneratedCertificate {
            certificate: "cert".to_string(),
            private_key: SecretString::new(secret_key),
            ca_chain: "ca".to_string(),
            serial_number: "123".to_string(),
            expires_at: chrono::Utc::now(),
            spiffe_uri: "spiffe://test".to_string(),
        };

        // The secret should still be accessible via expose_secret()
        assert_eq!(cert.private_key.expose_secret(), secret_key);
    }

    // =========================================================================
    // Certificate TTL Configuration Tests
    // =========================================================================

    #[test]
    fn test_certificate_ttl_default() {
        // Clear any existing env var
        std::env::remove_var("FLOWPLANE_VAULT_PKI_TTL_HOURS");
        assert_eq!(get_certificate_ttl_hours(), 12);
    }

    #[test]
    fn test_certificate_ttl_custom_valid() {
        std::env::set_var("FLOWPLANE_VAULT_PKI_TTL_HOURS", "8");
        assert_eq!(get_certificate_ttl_hours(), 8);
        std::env::remove_var("FLOWPLANE_VAULT_PKI_TTL_HOURS");
    }

    #[test]
    fn test_certificate_ttl_boundary_min() {
        std::env::set_var("FLOWPLANE_VAULT_PKI_TTL_HOURS", "4");
        assert_eq!(get_certificate_ttl_hours(), 4); // Exactly at minimum
        std::env::remove_var("FLOWPLANE_VAULT_PKI_TTL_HOURS");
    }

    #[test]
    fn test_certificate_ttl_boundary_max() {
        std::env::set_var("FLOWPLANE_VAULT_PKI_TTL_HOURS", "24");
        assert_eq!(get_certificate_ttl_hours(), 24); // Exactly at maximum
        std::env::remove_var("FLOWPLANE_VAULT_PKI_TTL_HOURS");
    }

    #[test]
    fn test_certificate_ttl_clamped_below_min() {
        std::env::set_var("FLOWPLANE_VAULT_PKI_TTL_HOURS", "2");
        assert_eq!(get_certificate_ttl_hours(), 4); // Clamped to minimum
        std::env::remove_var("FLOWPLANE_VAULT_PKI_TTL_HOURS");
    }

    #[test]
    fn test_certificate_ttl_clamped_above_max() {
        std::env::set_var("FLOWPLANE_VAULT_PKI_TTL_HOURS", "72");
        assert_eq!(get_certificate_ttl_hours(), 24); // Clamped to maximum
        std::env::remove_var("FLOWPLANE_VAULT_PKI_TTL_HOURS");
    }

    #[test]
    fn test_certificate_ttl_invalid_parse() {
        std::env::set_var("FLOWPLANE_VAULT_PKI_TTL_HOURS", "invalid");
        assert_eq!(get_certificate_ttl_hours(), 12); // Falls back to default
        std::env::remove_var("FLOWPLANE_VAULT_PKI_TTL_HOURS");
    }

    #[test]
    fn test_certificate_ttl_zero_clamped() {
        std::env::set_var("FLOWPLANE_VAULT_PKI_TTL_HOURS", "0");
        assert_eq!(get_certificate_ttl_hours(), 4); // Clamped to minimum
        std::env::remove_var("FLOWPLANE_VAULT_PKI_TTL_HOURS");
    }
}
