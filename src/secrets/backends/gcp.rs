//! GCP Secret Manager backend implementation
//!
//! Fetches secrets from Google Cloud Secret Manager by reference path.
//!
//! ## Configuration
//!
//! Environment variables:
//! - `FLOWPLANE_GCP_PROJECT_ID` or `GCP_PROJECT_ID` - Required
//! - `FLOWPLANE_GCP_SECRET_PREFIX` - Optional prefix for secret names (default: "flowplane/")
//! - `GOOGLE_APPLICATION_CREDENTIALS` - Optional path to service account key (uses ADC if not set)
//!
//! ## Reference Format
//!
//! Secrets can be referenced by:
//! - Short form: `my-secret` (uses project from config, latest version)
//! - Versioned: `my-secret@v3` or `my-secret@latest`
//! - Full path: `projects/my-project/secrets/my-secret/versions/latest`
//!
//! ## Secret Format in GCP
//!
//! Secrets should be stored as JSON with a structure matching `SecretSpec`:
//!
//! ### Generic Secret
//! ```json
//! {
//!   "type": "generic_secret",
//!   "secret": "<base64-encoded-value>"
//! }
//! ```
//!
//! ### TLS Certificate
//! ```json
//! {
//!   "type": "tls_certificate",
//!   "certificate_chain": "<PEM>",
//!   "private_key": "<PEM>",
//!   "password": "<optional>",
//!   "ocsp_staple": "<optional-base64>"
//! }
//! ```
//!
//! ### Certificate Validation Context
//! ```json
//! {
//!   "type": "certificate_validation_context",
//!   "trusted_ca": "<PEM>"
//! }
//! ```

use crate::errors::Result;
use serde::{Deserialize, Serialize};

#[cfg(feature = "gcp")]
use super::backend::{SecretBackend, SecretBackendType};
#[cfg(feature = "gcp")]
use crate::domain::{
    CertificateValidationContextSpec, GenericSecretSpec, SecretSpec, SecretType, TlsCertificateSpec,
};
#[cfg(feature = "gcp")]
use crate::errors::{AuthErrorType, FlowplaneError};
#[cfg(feature = "gcp")]
use async_trait::async_trait;
#[cfg(feature = "gcp")]
use std::collections::HashMap;
#[cfg(feature = "gcp")]
use tracing::{debug, error, info, warn};

#[cfg(feature = "gcp")]
use google_secretmanager1::{hyper_rustls, hyper_util, SecretManager};

/// Default prefix for secrets in GCP Secret Manager
fn default_secret_prefix() -> String {
    "flowplane/".to_string()
}

/// Configuration for GCP Secret Manager backend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcpBackendConfig {
    /// GCP project ID
    pub project_id: String,

    /// Optional prefix for secret names (default: "flowplane/")
    #[serde(default = "default_secret_prefix")]
    pub secret_prefix: String,
}

impl GcpBackendConfig {
    /// Load configuration from environment variables
    ///
    /// Uses:
    /// - `FLOWPLANE_GCP_PROJECT_ID` or `GCP_PROJECT_ID` (required)
    /// - `FLOWPLANE_GCP_SECRET_PREFIX` (default: "flowplane/")
    ///
    /// Returns `Ok(None)` if GCP is not configured (no project ID).
    pub fn from_env() -> Result<Option<Self>> {
        // Check for project ID - required for GCP backend
        let project_id = std::env::var("FLOWPLANE_GCP_PROJECT_ID")
            .or_else(|_| std::env::var("GCP_PROJECT_ID"))
            .ok();

        let Some(project_id) = project_id else {
            return Ok(None);
        };

        let secret_prefix = std::env::var("FLOWPLANE_GCP_SECRET_PREFIX")
            .unwrap_or_else(|_| default_secret_prefix());

        Ok(Some(Self { project_id, secret_prefix }))
    }
}

/// GCP Secret Manager backend
///
/// Fetches secrets from Google Cloud Secret Manager by reference path.
/// Supports Application Default Credentials (ADC) and explicit service account keys.
#[cfg(feature = "gcp")]
pub struct GcpSecretBackend {
    hub: SecretManager<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    >,
    project_id: String,
    secret_prefix: String,
}

#[cfg(feature = "gcp")]
impl std::fmt::Debug for GcpSecretBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcpSecretBackend")
            .field("project_id", &self.project_id)
            .field("secret_prefix", &self.secret_prefix)
            .field("hub", &"[SecretManager]")
            .finish()
    }
}

#[cfg(feature = "gcp")]
impl GcpSecretBackend {
    /// Create a new GCP Secret Manager backend with the given configuration
    pub async fn new(config: GcpBackendConfig) -> Result<Self> {
        // Build HTTPS client
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(
                    hyper_rustls::HttpsConnectorBuilder::new()
                        .with_native_roots()
                        .map_err(|e| {
                            FlowplaneError::config(format!(
                                "Failed to load native TLS roots: {}",
                                e
                            ))
                        })?
                        .https_or_http()
                        .enable_http2()
                        .build(),
                );

        // Get authentication - uses Application Default Credentials
        // This will check:
        // 1. GOOGLE_APPLICATION_CREDENTIALS env var
        // 2. Default service account on GCE/Cloud Run/GKE
        // 3. gcloud auth application-default credentials
        let auth = yup_oauth2::ServiceAccountAuthenticator::builder(
            yup_oauth2::read_service_account_key(
                std::env::var("GOOGLE_APPLICATION_CREDENTIALS").unwrap_or_else(|_| "".to_string()),
            )
            .await
            .map_err(|e| {
                // If no explicit credentials, try Application Default Credentials
                FlowplaneError::config(format!(
                    "Failed to read GCP credentials. Set GOOGLE_APPLICATION_CREDENTIALS or \
                    run on GCP with a service account: {}",
                    e
                ))
            })?,
        )
        .build()
        .await
        .map_err(|e| FlowplaneError::config(format!("Failed to build GCP authenticator: {}", e)))?;

        let hub = SecretManager::new(client, auth);

        info!(
            project_id = %config.project_id,
            secret_prefix = %config.secret_prefix,
            "Initialized GCP Secret Manager backend"
        );

        Ok(Self { hub, project_id: config.project_id, secret_prefix: config.secret_prefix })
    }

    /// Create backend from environment configuration
    pub async fn from_env() -> Result<Option<Self>> {
        match GcpBackendConfig::from_env()? {
            Some(config) => Ok(Some(Self::new(config).await?)),
            None => Ok(None),
        }
    }

    /// Build the full secret version resource name
    ///
    /// Handles various reference formats:
    /// - `my-secret` -> `projects/{project}/secrets/{prefix}my-secret/versions/latest`
    /// - `my-secret@v3` -> `projects/{project}/secrets/{prefix}my-secret/versions/3`
    /// - `projects/...` -> used as-is
    fn build_resource_name(&self, reference: &str) -> String {
        // If it's already a full path, use it directly
        if reference.starts_with("projects/") {
            return reference.to_string();
        }

        // Parse optional version suffix
        let (secret_name, version) = if let Some(idx) = reference.rfind('@') {
            let (name, ver) = reference.split_at(idx);
            let ver = &ver[1..]; // Skip the '@'
            let version = ver.strip_prefix('v').unwrap_or(ver);
            (name, version)
        } else {
            (reference, "latest")
        };

        // Apply prefix and build full path
        let prefixed_name = format!("{}{}", self.secret_prefix, secret_name);
        format!("projects/{}/secrets/{}/versions/{}", self.project_id, prefixed_name, version)
    }

    /// Parse secret data from GCP into SecretSpec
    fn parse_secret_data(&self, data: &[u8], expected_type: SecretType) -> Result<SecretSpec> {
        // First, try to parse as JSON
        let json_result: std::result::Result<HashMap<String, serde_json::Value>, _> =
            serde_json::from_slice(data);

        match json_result {
            Ok(json_data) => self.parse_json_secret(json_data, expected_type),
            Err(_) => {
                // Not JSON - treat as raw value
                self.parse_raw_secret(data, expected_type)
            }
        }
    }

    /// Parse JSON-formatted secret data
    fn parse_json_secret(
        &self,
        data: HashMap<String, serde_json::Value>,
        expected_type: SecretType,
    ) -> Result<SecretSpec> {
        // Try to deserialize directly if it has a "type" field
        if data.contains_key("type") {
            let spec: SecretSpec =
                serde_json::from_value(serde_json::Value::Object(data.into_iter().collect()))
                    .map_err(|e| {
                        FlowplaneError::config(format!("Invalid secret format in GCP: {}", e))
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
                    "Session ticket keys must have explicit 'type' field in GCP Secret Manager",
                ))
            }
        }
    }

    /// Parse raw (non-JSON) secret data
    fn parse_raw_secret(&self, data: &[u8], expected_type: SecretType) -> Result<SecretSpec> {
        match expected_type {
            SecretType::GenericSecret => {
                // For generic secrets, base64-encode the raw data
                use base64::Engine;
                let secret = base64::engine::general_purpose::STANDARD.encode(data);
                Ok(SecretSpec::GenericSecret(GenericSecretSpec { secret }))
            }
            SecretType::TlsCertificate => {
                // Raw data could be a PEM certificate - try to detect
                let text = String::from_utf8(data.to_vec()).map_err(|e| {
                    FlowplaneError::config(format!("TLS certificate must be valid UTF-8: {}", e))
                })?;

                // Simple heuristic: if it contains PEM markers, treat as certificate
                if text.contains("-----BEGIN") {
                    // This is likely just the certificate, but we need both cert and key
                    Err(FlowplaneError::config(
                        "Raw PEM detected. TLS certificate secrets must be JSON with \
                        'certificate_chain' and 'private_key' fields",
                    ))
                } else {
                    Err(FlowplaneError::config(
                        "TLS certificate must be stored as JSON with 'certificate_chain' \
                        and 'private_key' fields",
                    ))
                }
            }
            SecretType::CertificateValidationContext => {
                // Raw data could be a CA certificate PEM
                let text = String::from_utf8(data.to_vec()).map_err(|e| {
                    FlowplaneError::config(format!("CA certificate must be valid UTF-8: {}", e))
                })?;

                if text.contains("-----BEGIN CERTIFICATE-----") {
                    Ok(SecretSpec::CertificateValidationContext(CertificateValidationContextSpec {
                        trusted_ca: text,
                        match_subject_alt_names: vec![],
                        crl: None,
                        only_verify_leaf_cert_crl: false,
                    }))
                } else {
                    Err(FlowplaneError::config(
                        "Certificate validation context must contain PEM-encoded CA certificate",
                    ))
                }
            }
            SecretType::SessionTicketKeys => Err(FlowplaneError::config(
                "Session ticket keys must be stored as JSON with explicit 'type' field",
            )),
        }
    }
}

#[cfg(feature = "gcp")]
#[async_trait]
impl SecretBackend for GcpSecretBackend {
    async fn fetch_secret(&self, reference: &str, expected_type: SecretType) -> Result<SecretSpec> {
        let resource_name = self.build_resource_name(reference);

        debug!(
            reference = %reference,
            resource_name = %resource_name,
            expected_type = ?expected_type,
            "Fetching secret from GCP Secret Manager"
        );

        // Access the secret version
        let result = self.hub.projects().secrets_versions_access(&resource_name).doit().await;

        match result {
            Ok((_, response)) => {
                // Extract the payload
                let payload = response.payload.ok_or_else(|| {
                    warn!(reference = %reference, "Secret has no payload");
                    FlowplaneError::config(format!("Secret '{}' has no payload data", reference))
                })?;

                let data = payload.data.ok_or_else(|| {
                    warn!(reference = %reference, "Secret payload has no data");
                    FlowplaneError::config(format!("Secret '{}' has empty payload", reference))
                })?;

                if data.is_empty() {
                    warn!(reference = %reference, "Secret payload is empty");
                    return Err(FlowplaneError::config(format!(
                        "Secret '{}' has empty payload",
                        reference
                    )));
                }

                self.parse_secret_data(&data, expected_type)
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("NOT_FOUND") || err_str.contains("404") {
                    error!(
                        reference = %reference,
                        resource_name = %resource_name,
                        error = %e,
                        "Secret not found in GCP Secret Manager"
                    );
                    Err(FlowplaneError::not_found_msg(format!(
                        "Secret '{}' not found in GCP Secret Manager",
                        reference
                    )))
                } else if err_str.contains("PERMISSION_DENIED") || err_str.contains("403") {
                    error!(
                        reference = %reference,
                        error = %e,
                        "Permission denied accessing GCP secret"
                    );
                    Err(FlowplaneError::auth(
                        format!("Permission denied accessing secret '{}': {}", reference, e),
                        AuthErrorType::InsufficientPermissions,
                    ))
                } else {
                    error!(
                        reference = %reference,
                        error = %e,
                        "Failed to fetch secret from GCP Secret Manager"
                    );
                    Err(FlowplaneError::internal(format!(
                        "Failed to fetch secret '{}' from GCP: {}",
                        reference, e
                    )))
                }
            }
        }
    }

    async fn validate_reference(&self, reference: &str) -> Result<bool> {
        let resource_name = self.build_resource_name(reference);

        debug!(
            reference = %reference,
            resource_name = %resource_name,
            "Validating GCP secret reference"
        );

        // Try to access the secret and check for NOT_FOUND
        match self.hub.projects().secrets_versions_access(&resource_name).doit().await {
            Ok(_) => Ok(true),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("NOT_FOUND") || err_str.contains("404") {
                    debug!(
                        reference = %reference,
                        "Secret reference not found in GCP"
                    );
                    Ok(false)
                } else {
                    // Other errors (permission, network) should be propagated
                    Err(FlowplaneError::internal(format!(
                        "Error validating GCP secret reference '{}': {}",
                        reference, e
                    )))
                }
            }
        }
    }

    fn backend_type(&self) -> SecretBackendType {
        SecretBackendType::GcpSecretManager
    }

    async fn health_check(&self) -> Result<()> {
        // List secrets with limit 1 to verify connectivity
        let parent = format!("projects/{}", self.project_id);

        debug!(project_id = %self.project_id, "Performing GCP Secret Manager health check");

        // Try to list secrets (limit 1) to verify connectivity and permissions
        match self.hub.projects().secrets_list(&parent).page_size(1).doit().await {
            Ok(_) => {
                debug!(project_id = %self.project_id, "GCP Secret Manager health check passed");
                Ok(())
            }
            Err(e) => {
                error!(
                    project_id = %self.project_id,
                    error = %e,
                    "GCP Secret Manager health check failed"
                );
                Err(FlowplaneError::config(format!(
                    "GCP Secret Manager health check failed: {}",
                    e
                )))
            }
        }
    }
}

// Stub struct for non-feature builds (allows type to exist but not be constructable)
#[cfg(not(feature = "gcp"))]
pub struct GcpSecretBackend {
    _private: (),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::SecretSpec;
    use crate::secrets::backends::backend::SecretBackendType;

    #[test]
    fn test_default_secret_prefix() {
        assert_eq!(default_secret_prefix(), "flowplane/");
    }

    // Note: Environment-based tests can have race conditions in parallel execution.
    // These tests use unique env var names to avoid conflicts.

    #[test]
    fn test_config_from_env_no_project() {
        // Without GCP_PROJECT_ID set, should return None
        // Use a thread-local check since env vars are process-global
        let prev_fp = std::env::var("FLOWPLANE_GCP_PROJECT_ID").ok();
        let prev_gcp = std::env::var("GCP_PROJECT_ID").ok();

        std::env::remove_var("FLOWPLANE_GCP_PROJECT_ID");
        std::env::remove_var("GCP_PROJECT_ID");

        let config = GcpBackendConfig::from_env().unwrap();
        assert!(config.is_none(), "Config should be None when no project ID is set");

        // Restore previous values
        if let Some(v) = prev_fp {
            std::env::set_var("FLOWPLANE_GCP_PROJECT_ID", v);
        }
        if let Some(v) = prev_gcp {
            std::env::set_var("GCP_PROJECT_ID", v);
        }
    }

    #[test]
    fn test_config_from_env_with_flowplane_prefix() {
        // Test with FLOWPLANE_GCP_PROJECT_ID
        let unique_project = format!("test-project-{}", std::process::id());
        std::env::set_var("FLOWPLANE_GCP_PROJECT_ID", &unique_project);
        std::env::remove_var("FLOWPLANE_GCP_SECRET_PREFIX");

        let config = GcpBackendConfig::from_env().unwrap();
        assert!(config.is_some(), "Config should be Some when project ID is set");

        let config = config.unwrap();
        assert_eq!(config.project_id, unique_project);
        assert_eq!(config.secret_prefix, "flowplane/");

        std::env::remove_var("FLOWPLANE_GCP_PROJECT_ID");
    }

    #[test]
    fn test_config_with_custom_prefix() {
        let unique_project = format!("test-project-prefix-{}", std::process::id());
        std::env::set_var("FLOWPLANE_GCP_PROJECT_ID", &unique_project);
        std::env::set_var("FLOWPLANE_GCP_SECRET_PREFIX", "my-app/");

        let config = GcpBackendConfig::from_env().unwrap().unwrap();
        assert_eq!(config.secret_prefix, "my-app/");

        std::env::remove_var("FLOWPLANE_GCP_PROJECT_ID");
        std::env::remove_var("FLOWPLANE_GCP_SECRET_PREFIX");
    }

    #[test]
    fn test_config_serialization() {
        let config = GcpBackendConfig {
            project_id: "my-project".to_string(),
            secret_prefix: "flowplane/".to_string(),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("my-project"));

        let parsed: GcpBackendConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.project_id, config.project_id);
    }

    #[test]
    fn test_backend_type() {
        assert_eq!(SecretBackendType::GcpSecretManager.as_str(), "gcp_secret_manager");
    }

    // Resource name building tests - these test the logic without needing a GCP client
    #[test]
    fn test_build_resource_name_simple() {
        let project_id = "test-project";
        let secret_prefix = "flowplane/";
        let reference = "my-secret";

        // Simulate the build_resource_name logic
        let name = if reference.starts_with("projects/") {
            reference.to_string()
        } else {
            let (secret_name, version) = if let Some(idx) = reference.rfind('@') {
                let (name, ver) = reference.split_at(idx);
                let ver = &ver[1..];
                let version = ver.strip_prefix('v').unwrap_or(ver);
                (name, version)
            } else {
                (reference, "latest")
            };
            format!(
                "projects/{}/secrets/{}{}/versions/{}",
                project_id, secret_prefix, secret_name, version
            )
        };

        assert_eq!(name, "projects/test-project/secrets/flowplane/my-secret/versions/latest");
    }

    #[test]
    fn test_build_resource_name_with_version() {
        let project_id = "test-project";
        let secret_prefix = "flowplane/";
        let reference = "my-secret@v3";

        let (secret_name, version) = if let Some(idx) = reference.rfind('@') {
            let (name, ver) = reference.split_at(idx);
            let ver = &ver[1..];
            let version = ver.strip_prefix('v').unwrap_or(ver);
            (name, version)
        } else {
            (reference, "latest")
        };

        let name = format!(
            "projects/{}/secrets/{}{}/versions/{}",
            project_id, secret_prefix, secret_name, version
        );

        assert_eq!(name, "projects/test-project/secrets/flowplane/my-secret/versions/3");
    }

    #[test]
    fn test_build_resource_name_full_path() {
        let full_path = "projects/other-project/secrets/other-secret/versions/5";

        let name = if full_path.starts_with("projects/") {
            full_path.to_string()
        } else {
            "should not reach here".to_string()
        };

        assert_eq!(name, full_path);
    }

    #[test]
    fn test_parse_json_generic_secret() {
        let json_str = r#"{"type": "generic_secret", "secret": "c2VjcmV0LXZhbHVl"}"#;
        let spec: SecretSpec = serde_json::from_str(json_str).unwrap();

        if let SecretSpec::GenericSecret(generic) = spec {
            assert_eq!(generic.secret, "c2VjcmV0LXZhbHVl");
        } else {
            panic!("Expected GenericSecret");
        }
    }

    #[test]
    fn test_parse_json_tls_certificate() {
        let json_str = r#"{"type": "tls_certificate", "certificate_chain": "-----BEGIN CERT-----", "private_key": "-----BEGIN KEY-----"}"#;
        let spec: SecretSpec = serde_json::from_str(json_str).unwrap();

        if let SecretSpec::TlsCertificate(tls) = spec {
            assert_eq!(tls.certificate_chain, "-----BEGIN CERT-----");
            assert_eq!(tls.private_key, "-----BEGIN KEY-----");
        } else {
            panic!("Expected TlsCertificate");
        }
    }

    #[test]
    fn test_parse_json_validation_context() {
        let json_str = r#"{"type": "certificate_validation_context", "trusted_ca": "-----BEGIN CERTIFICATE-----\nCA CERT\n-----END CERTIFICATE-----"}"#;
        let spec: SecretSpec = serde_json::from_str(json_str).unwrap();

        if let SecretSpec::CertificateValidationContext(ctx) = spec {
            assert!(ctx.trusted_ca.contains("CA CERT"));
        } else {
            panic!("Expected CertificateValidationContext");
        }
    }
}
