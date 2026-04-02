//! Vault secret backend implementation
//!
//! Fetches secrets from HashiCorp Vault KV v2 engine by reference path.

use super::backend::{SecretBackend, SecretBackendType};
use crate::domain::{
    CertificateValidationContextSpec, GenericSecretSpec, SecretSpec, SecretType, TlsCertificateSpec,
};
use crate::errors::{FlowplaneError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, error, info};
use vaultrs::client::{VaultClient, VaultClientSettingsBuilder};
use vaultrs::kv2;

/// Configuration for Vault backend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultBackendConfig {
    /// Vault server address
    pub address: String,
    /// Vault authentication token
    pub token: Option<String>,
    /// Vault namespace (for Enterprise)
    pub namespace: Option<String>,
    /// KV v2 mount path (default: "secret")
    #[serde(default = "default_kv_mount")]
    pub kv_mount_path: String,
}

fn default_kv_mount() -> String {
    "secret".to_string()
}

impl VaultBackendConfig {
    /// Load configuration from environment variables
    ///
    /// Uses:
    /// - `FLOWPLANE_VAULT_ADDR` or `VAULT_ADDR`
    /// - `FLOWPLANE_VAULT_TOKEN` or `VAULT_TOKEN`
    /// - `FLOWPLANE_VAULT_NAMESPACE` or `VAULT_NAMESPACE`
    /// - `FLOWPLANE_VAULT_KV_MOUNT` (default: "secret")
    pub fn from_env() -> Result<Option<Self>> {
        // Check for address - required for Vault backend
        // Gate on env var being present AND non-empty
        let address = std::env::var("FLOWPLANE_VAULT_ADDR")
            .or_else(|_| std::env::var("VAULT_ADDR"))
            .ok()
            .filter(|s| !s.trim().is_empty());

        let Some(address) = address else {
            return Ok(None);
        };

        let token =
            std::env::var("FLOWPLANE_VAULT_TOKEN").or_else(|_| std::env::var("VAULT_TOKEN")).ok();

        let namespace = std::env::var("FLOWPLANE_VAULT_NAMESPACE")
            .or_else(|_| std::env::var("VAULT_NAMESPACE"))
            .ok();

        let kv_mount_path =
            std::env::var("FLOWPLANE_VAULT_KV_MOUNT").unwrap_or_else(|_| default_kv_mount());

        Ok(Some(Self { address, token, namespace, kv_mount_path }))
    }
}

/// HashiCorp Vault secret backend
///
/// Fetches secrets from Vault KV v2 engine by path reference.
/// The reference format is: `path/to/secret` within the KV mount.
///
/// ## Secret Format in Vault
///
/// Secrets must be stored in Vault with a structure that can be mapped
/// to `SecretSpec`. The backend expects one of these formats:
///
/// ### Generic Secret
/// ```json
/// {
///   "type": "generic_secret",
///   "secret": "<base64-encoded-value>"
/// }
/// ```
///
/// ### TLS Certificate
/// ```json
/// {
///   "type": "tls_certificate",
///   "certificate_chain": "<PEM>",
///   "private_key": "<PEM>",
///   "password": "<optional>",
///   "ocsp_staple": "<optional-base64>"
/// }
/// ```
///
/// ### Certificate Validation Context
/// ```json
/// {
///   "type": "certificate_validation_context",
///   "trusted_ca": "<PEM>"
/// }
/// ```
pub struct VaultSecretBackend {
    client: VaultClient,
    kv_mount_path: String,
}

impl std::fmt::Debug for VaultSecretBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultSecretBackend")
            .field("kv_mount_path", &self.kv_mount_path)
            .field("client", &"[VaultClient]")
            .finish()
    }
}

impl VaultSecretBackend {
    /// Create a new Vault backend with the given configuration
    pub async fn new(config: VaultBackendConfig) -> Result<Self> {
        let mut settings_builder = VaultClientSettingsBuilder::default();
        settings_builder.address(&config.address);

        if let Some(ref token) = config.token {
            settings_builder.token(token);
        }

        if let Some(ref namespace) = config.namespace {
            settings_builder.namespace(Some(namespace.clone()));
        }

        let settings = settings_builder.build().map_err(|e| {
            FlowplaneError::config(format!("Invalid Vault backend configuration: {}", e))
        })?;

        let client = VaultClient::new(settings)
            .map_err(|e| FlowplaneError::config(format!("Failed to create Vault client: {}", e)))?;

        info!(address = %config.address, kv_mount = %config.kv_mount_path, "Initialized Vault secret backend");

        Ok(Self { client, kv_mount_path: config.kv_mount_path })
    }

    /// Create backend from environment configuration
    pub async fn from_env() -> Result<Option<Self>> {
        match VaultBackendConfig::from_env()? {
            Some(config) => Ok(Some(Self::new(config).await?)),
            None => Ok(None),
        }
    }

    /// Get the underlying Vault client
    pub fn client(&self) -> &VaultClient {
        &self.client
    }

    /// Convert Vault data to SecretSpec
    fn parse_secret_data(
        &self,
        data: HashMap<String, serde_json::Value>,
        expected_type: SecretType,
    ) -> Result<SecretSpec> {
        // Try to deserialize directly if it has a "type" field
        if data.contains_key("type") {
            let spec: SecretSpec =
                serde_json::from_value(serde_json::Value::Object(data.into_iter().collect()))
                    .map_err(|e| {
                        FlowplaneError::config(format!("Invalid secret format in Vault: {}", e))
                    })?;

            // Verify type matches
            if spec.secret_type() != expected_type {
                return Err(FlowplaneError::config(format!(
                    "Secret type mismatch: expected {:?}, found {:?}",
                    expected_type,
                    spec.secret_type()
                )));
            }

            return Ok(spec);
        }

        // Otherwise, infer from expected_type and available fields
        match expected_type {
            SecretType::GenericSecret => {
                let secret = data
                    .get("secret")
                    .or_else(|| data.get("value"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        FlowplaneError::config("Generic secret must have 'secret' or 'value' field")
                    })?
                    .to_string();

                Ok(SecretSpec::GenericSecret(GenericSecretSpec { secret }))
            }
            SecretType::TlsCertificate => {
                let certificate_chain = data
                    .get("certificate_chain")
                    .or_else(|| data.get("cert"))
                    .or_else(|| data.get("certificate"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        FlowplaneError::config(
                            "TLS certificate must have 'certificate_chain' field",
                        )
                    })?
                    .to_string();

                let private_key = data
                    .get("private_key")
                    .or_else(|| data.get("key"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        FlowplaneError::config("TLS certificate must have 'private_key' field")
                    })?
                    .to_string();

                let password = data.get("password").and_then(|v| v.as_str()).map(String::from);
                let ocsp_staple =
                    data.get("ocsp_staple").and_then(|v| v.as_str()).map(String::from);

                Ok(SecretSpec::TlsCertificate(TlsCertificateSpec {
                    certificate_chain,
                    private_key,
                    password,
                    ocsp_staple,
                }))
            }
            SecretType::CertificateValidationContext => {
                let trusted_ca = data
                    .get("trusted_ca")
                    .or_else(|| data.get("ca"))
                    .or_else(|| data.get("ca_cert"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        FlowplaneError::config("Validation context must have 'trusted_ca' field")
                    })?
                    .to_string();

                let crl = data.get("crl").and_then(|v| v.as_str()).map(String::from);
                let only_verify_leaf_cert_crl = data
                    .get("only_verify_leaf_cert_crl")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                Ok(SecretSpec::CertificateValidationContext(CertificateValidationContextSpec {
                    trusted_ca,
                    match_subject_alt_names: vec![],
                    crl,
                    only_verify_leaf_cert_crl,
                }))
            }
            SecretType::SessionTicketKeys => {
                // Session ticket keys are complex - require explicit type field
                Err(FlowplaneError::config(
                    "Session ticket keys must have explicit 'type' field in Vault",
                ))
            }
        }
    }
}

#[async_trait]
impl SecretBackend for VaultSecretBackend {
    async fn fetch_secret(&self, reference: &str, expected_type: SecretType) -> Result<SecretSpec> {
        debug!(
            reference = %reference,
            expected_type = ?expected_type,
            kv_mount = %self.kv_mount_path,
            "Fetching secret from Vault"
        );

        let data: HashMap<String, serde_json::Value> =
            kv2::read(&self.client, &self.kv_mount_path, reference).await.map_err(|e| {
                error!(
                    reference = %reference,
                    error = %e,
                    "Failed to fetch secret from Vault"
                );
                FlowplaneError::not_found_msg(format!(
                    "Secret '{}' not found in Vault: {}",
                    reference, e
                ))
            })?;

        self.parse_secret_data(data, expected_type)
    }

    async fn validate_reference(&self, reference: &str) -> Result<bool> {
        debug!(
            reference = %reference,
            kv_mount = %self.kv_mount_path,
            "Validating Vault secret reference"
        );

        // Try to read metadata (doesn't fetch the secret value)
        match kv2::read_metadata(&self.client, &self.kv_mount_path, reference).await {
            Ok(_) => Ok(true),
            Err(e) => {
                debug!(
                    reference = %reference,
                    error = %e,
                    "Secret reference not found in Vault"
                );
                Ok(false)
            }
        }
    }

    fn backend_type(&self) -> SecretBackendType {
        SecretBackendType::Vault
    }

    async fn health_check(&self) -> Result<()> {
        vaultrs::sys::health(&self.client)
            .await
            .map_err(|e| FlowplaneError::config(format!("Vault health check failed: {}", e)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env vars are process-global; serialize all tests that modify them
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_vault_backend_config_default() {
        let config = VaultBackendConfig {
            address: "http://localhost:8200".to_string(),
            token: None,
            namespace: None,
            kv_mount_path: default_kv_mount(),
        };
        assert_eq!(config.kv_mount_path, "secret");
    }

    #[test]
    fn test_backend_type() {
        assert_eq!(SecretBackendType::Vault.as_str(), "vault");
    }

    #[test]
    fn test_config_from_env_no_addr() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved_fp = std::env::var("FLOWPLANE_VAULT_ADDR").ok();
        let saved_va = std::env::var("VAULT_ADDR").ok();
        std::env::remove_var("FLOWPLANE_VAULT_ADDR");
        std::env::remove_var("VAULT_ADDR");

        let result = VaultBackendConfig::from_env();
        assert!(result.is_ok());
        assert!(result.unwrap().is_none(), "Should return None when VAULT_ADDR is not set");

        if let Some(v) = saved_fp {
            std::env::set_var("FLOWPLANE_VAULT_ADDR", v);
        }
        if let Some(v) = saved_va {
            std::env::set_var("VAULT_ADDR", v);
        }
    }

    #[test]
    fn test_config_from_env_empty_addr() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved_fp = std::env::var("FLOWPLANE_VAULT_ADDR").ok();
        let saved_va = std::env::var("VAULT_ADDR").ok();
        std::env::remove_var("FLOWPLANE_VAULT_ADDR");
        std::env::set_var("VAULT_ADDR", "");

        let result = VaultBackendConfig::from_env();
        assert!(result.is_ok());
        assert!(result.unwrap().is_none(), "Should return None when VAULT_ADDR is empty");

        if let Some(v) = saved_fp {
            std::env::set_var("FLOWPLANE_VAULT_ADDR", v);
        }
        if let Some(v) = saved_va {
            std::env::set_var("VAULT_ADDR", v);
        } else {
            std::env::remove_var("VAULT_ADDR");
        }
    }

    #[test]
    fn test_config_from_env_whitespace_addr() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved_fp = std::env::var("FLOWPLANE_VAULT_ADDR").ok();
        let saved_va = std::env::var("VAULT_ADDR").ok();
        std::env::remove_var("FLOWPLANE_VAULT_ADDR");
        std::env::set_var("VAULT_ADDR", "   ");

        let result = VaultBackendConfig::from_env();
        assert!(result.is_ok());
        assert!(result.unwrap().is_none(), "Should return None when VAULT_ADDR is whitespace-only");

        if let Some(v) = saved_fp {
            std::env::set_var("FLOWPLANE_VAULT_ADDR", v);
        }
        if let Some(v) = saved_va {
            std::env::set_var("VAULT_ADDR", v);
        } else {
            std::env::remove_var("VAULT_ADDR");
        }
    }

    #[test]
    fn test_config_from_env_valid_addr() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved_fp = std::env::var("FLOWPLANE_VAULT_ADDR").ok();
        let saved_va = std::env::var("VAULT_ADDR").ok();
        std::env::remove_var("FLOWPLANE_VAULT_ADDR");
        std::env::set_var("VAULT_ADDR", "http://localhost:8200");

        let result = VaultBackendConfig::from_env();
        assert!(result.is_ok());
        let config = result.unwrap();
        assert!(config.is_some(), "Should return Some when VAULT_ADDR is set");
        let config = config.unwrap();
        assert_eq!(config.address, "http://localhost:8200");
        assert_eq!(config.kv_mount_path, "secret");

        if let Some(v) = saved_fp {
            std::env::set_var("FLOWPLANE_VAULT_ADDR", v);
        }
        if let Some(v) = saved_va {
            std::env::set_var("VAULT_ADDR", v);
        } else {
            std::env::remove_var("VAULT_ADDR");
        }
    }

    #[test]
    fn test_config_from_env_flowplane_prefix_takes_precedence() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved_fp = std::env::var("FLOWPLANE_VAULT_ADDR").ok();
        let saved_va = std::env::var("VAULT_ADDR").ok();
        std::env::set_var("FLOWPLANE_VAULT_ADDR", "http://preferred:8200");
        std::env::set_var("VAULT_ADDR", "http://fallback:8200");

        let result = VaultBackendConfig::from_env();
        assert!(result.is_ok());
        let config = result.unwrap().unwrap();
        assert_eq!(config.address, "http://preferred:8200");

        if let Some(v) = saved_fp {
            std::env::set_var("FLOWPLANE_VAULT_ADDR", v);
        } else {
            std::env::remove_var("FLOWPLANE_VAULT_ADDR");
        }
        if let Some(v) = saved_va {
            std::env::set_var("VAULT_ADDR", v);
        } else {
            std::env::remove_var("VAULT_ADDR");
        }
    }
}
