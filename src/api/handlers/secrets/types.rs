//! Request and response types for secrets API

use crate::domain::{SecretId, SecretType};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// Request to create a new secret
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateSecretRequest {
    /// Name of the secret (must be unique within the team)
    #[validate(length(min = 1, max = 255))]
    pub name: String,

    /// Type of the secret
    pub secret_type: SecretType,

    /// Optional description
    pub description: Option<String>,

    /// Secret configuration (varies by type)
    /// For GenericSecret: { "secret": "base64-encoded-value" }
    /// For TlsCertificate: { "certificate_chain": "...", "private_key": "..." }
    /// For CertificateValidationContext: { "trusted_ca": "..." }
    /// For SessionTicketKeys: { "keys": [...] }
    pub configuration: serde_json::Value,

    /// Optional expiration time (ISO 8601 format)
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Request to create a reference-based secret (external backend)
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateSecretReferenceRequest {
    /// Name of the secret (must be unique within the team)
    #[validate(length(min = 1, max = 255))]
    pub name: String,

    /// Type of the secret
    pub secret_type: SecretType,

    /// Optional description
    pub description: Option<String>,

    /// Backend type: "vault", "aws_secrets_manager", "gcp_secret_manager"
    #[validate(length(min = 1))]
    pub backend: String,

    /// Backend-specific reference (Vault path, AWS ARN, GCP resource name)
    #[validate(length(min = 1))]
    pub reference: String,

    /// Optional version specifier for the external secret
    pub reference_version: Option<String>,

    /// Optional expiration time (ISO 8601 format)
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Request to update an existing secret
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct UpdateSecretRequest {
    /// Optional description update
    pub description: Option<String>,

    /// New secret configuration (replaces existing)
    pub configuration: Option<serde_json::Value>,

    /// Optional expiration time update
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Secret metadata response (never includes decrypted secret values)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SecretResponse {
    /// Unique identifier
    pub id: String,

    /// Name of the secret
    pub name: String,

    /// Type of the secret
    pub secret_type: SecretType,

    /// Optional description
    pub description: Option<String>,

    /// Version number (incremented on updates)
    pub version: i64,

    /// Source of the secret
    pub source: String,

    /// Team that owns this secret
    pub team: String,

    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Last update timestamp
    pub updated_at: chrono::DateTime<chrono::Utc>,

    /// Expiration timestamp (if set)
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,

    /// Backend type for reference-based secrets (None for database-stored secrets)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,

    /// Backend-specific reference (Vault path, AWS ARN, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,

    /// Optional version specifier for the external secret
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_version: Option<String>,
}

impl SecretResponse {
    /// Create a response from secret data (without exposing encrypted configuration)
    pub fn from_data(data: &crate::storage::SecretData) -> Self {
        Self {
            id: data.id.to_string(),
            name: data.name.clone(),
            secret_type: data.secret_type,
            description: data.description.clone(),
            version: data.version,
            source: data.source.clone(),
            team: data.team.clone(),
            created_at: data.created_at,
            updated_at: data.updated_at,
            expires_at: data.expires_at,
            backend: data.backend.clone(),
            reference: data.reference.clone(),
            reference_version: data.reference_version.clone(),
        }
    }
}

/// Query parameters for listing secrets
#[derive(Debug, Clone, Deserialize, ToSchema, Default)]
pub struct ListSecretsQuery {
    /// Maximum number of secrets to return
    #[serde(default)]
    pub limit: Option<i64>,

    /// Offset for pagination
    #[serde(default)]
    pub offset: Option<i64>,

    /// Filter by secret type
    #[serde(default)]
    pub secret_type: Option<SecretType>,
}

/// Path parameters for team-scoped secret operations
#[derive(Debug, Clone, Deserialize)]
pub struct TeamSecretPath {
    pub team: String,
    pub secret_id: SecretId,
}

/// Path parameters for team-scoped operations
#[derive(Debug, Clone, Deserialize)]
pub struct TeamPath {
    pub team: String,
}
