//! Secret repository for managing SDS secrets
//!
//! This module provides CRUD operations for secret resources with
//! encryption at rest for sensitive values.

use crate::domain::{SecretId, SecretSpec, SecretType};
use crate::errors::{FlowplaneError, Result};
use crate::services::{SecretEncryption, SecretEncryptionConfig};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use tracing::instrument;

/// Database row structure for secrets
#[derive(Debug, Clone, FromRow)]
struct SecretRow {
    pub id: String,
    pub name: String,
    pub secret_type: String,
    pub description: Option<String>,
    pub configuration_encrypted: Vec<u8>,
    #[allow(dead_code)] // Retrieved from DB but not currently used in code
    pub encryption_key_id: String,
    pub nonce: Vec<u8>,
    pub version: i64,
    pub source: String,
    pub team: String,
    /// Team display name (resolved via JOIN, used for API responses)
    pub team_name: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Backend type (None = legacy database encrypted secret)
    pub backend: Option<String>,
    /// Backend-specific reference (Vault path, AWS ARN, etc.)
    pub reference: Option<String>,
    /// Optional version specifier for external secret
    pub reference_version: Option<String>,
}

/// Secret data (metadata + decrypted configuration)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretData {
    pub id: SecretId,
    pub name: String,
    pub secret_type: SecretType,
    pub description: Option<String>,
    /// Decrypted configuration JSON (for database-stored secrets)
    /// May be None for reference-based secrets where config is fetched on-demand
    pub configuration: String,
    pub version: i64,
    pub source: String,
    /// Team UUID (used for access control)
    pub team: String,
    /// Team display name (resolved via JOIN, used for API responses)
    pub team_name: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Backend type (None = legacy database encrypted secret)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Backend-specific reference (Vault path, AWS ARN, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Optional version specifier for external secret
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_version: Option<String>,
}

impl crate::api::handlers::TeamOwned for SecretData {
    fn team(&self) -> Option<&str> {
        Some(&self.team)
    }

    fn resource_name(&self) -> &str {
        &self.name
    }

    fn resource_type() -> &'static str {
        "Secret"
    }

    fn resource_type_metric() -> &'static str {
        "secrets"
    }
}

/// Create secret request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSecretRequest {
    pub name: String,
    pub secret_type: SecretType,
    pub description: Option<String>,
    /// Secret configuration (will be validated and encrypted)
    pub configuration: SecretSpec,
    pub team: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Update secret request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSecretRequest {
    pub description: Option<String>,
    /// New secret configuration (optional, will be validated and encrypted)
    pub configuration: Option<SecretSpec>,
    pub expires_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
}

/// Create secret reference request (for external backends)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSecretReferenceRequest {
    pub name: String,
    pub secret_type: SecretType,
    pub description: Option<String>,
    /// Backend type (vault, aws_secrets_manager, gcp_secret_manager)
    pub backend: String,
    /// Backend-specific reference (Vault path, AWS ARN, etc.)
    pub reference: String,
    /// Optional version specifier
    pub reference_version: Option<String>,
    pub team: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Repository for secret data access
#[derive(Clone)]
pub struct SecretRepository {
    pool: DbPool,
    encryption: Arc<SecretEncryption>,
}

impl SecretRepository {
    /// Create a new secret repository
    pub fn new(pool: DbPool, encryption: Arc<SecretEncryption>) -> Self {
        Self { pool, encryption }
    }

    /// Create a new secret repository with encryption from environment
    pub fn with_env_encryption(pool: DbPool) -> Result<Self> {
        let config = SecretEncryptionConfig::from_env()?;
        let encryption = SecretEncryption::new(&config)?;
        Ok(Self { pool, encryption: Arc::new(encryption) })
    }

    /// Get the database pool reference
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Create a new secret
    #[instrument(skip(self, request), fields(secret_name = %request.name), name = "db_create_secret")]
    pub async fn create(&self, request: CreateSecretRequest) -> Result<SecretData> {
        // Validate the configuration
        request.configuration.validate().map_err(|e| {
            FlowplaneError::validation(format!("Invalid secret configuration: {}", e))
        })?;

        let id = SecretId::new();
        let now = chrono::Utc::now();

        // Serialize and encrypt the configuration
        let config_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::internal(format!("Failed to serialize secret configuration: {}", e))
        })?;

        let (encrypted, nonce) = self.encryption.encrypt(config_json.as_bytes())?;

        let result = sqlx::query(
            "INSERT INTO secrets (id, name, secret_type, description, configuration_encrypted, encryption_key_id, nonce, version, source, team, expires_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 1, 'native_api', $8, $9, $10, $11)"
        )
        .bind(id.as_str())
        .bind(&request.name)
        .bind(request.secret_type.as_str())
        .bind(&request.description)
        .bind(&encrypted)
        .bind(self.encryption.key_version())
        .bind(&nonce)
        .bind(&request.team)
        .bind(request.expires_at)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, secret_name = %request.name, "Failed to create secret");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create secret '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create secret"));
        }

        tracing::info!(
            secret_id = %id,
            secret_name = %request.name,
            secret_type = %request.secret_type,
            team = %request.team,
            "Created new secret"
        );

        self.get_by_id(&id).await
    }

    /// Get secret by ID
    #[instrument(skip(self), fields(secret_id = %id), name = "db_get_secret_by_id")]
    pub async fn get_by_id(&self, id: &SecretId) -> Result<SecretData> {
        let row = sqlx::query_as::<sqlx::Postgres, SecretRow>(
            "SELECT s.id, s.name, s.secret_type, s.description, s.configuration_encrypted, s.encryption_key_id, s.nonce, s.version, s.source, s.team, t.name as team_name, s.created_at, s.updated_at, s.expires_at, s.backend, s.reference, s.reference_version \
             FROM secrets s LEFT JOIN teams t ON s.team = t.id WHERE s.id = $1"
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, secret_id = %id, "Failed to get secret by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get secret with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => self.decrypt_row(row),
            None => {
                Err(FlowplaneError::not_found_msg(format!("Secret with ID '{}' not found", id)))
            }
        }
    }

    /// Get secret by name within a team
    #[instrument(skip(self), fields(secret_name = %name, team = %team), name = "db_get_secret_by_name")]
    pub async fn get_by_name(&self, team: &str, name: &str) -> Result<SecretData> {
        let row = sqlx::query_as::<sqlx::Postgres, SecretRow>(
            "SELECT s.id, s.name, s.secret_type, s.description, s.configuration_encrypted, s.encryption_key_id, s.nonce, s.version, s.source, s.team, t.name as team_name, s.created_at, s.updated_at, s.expires_at, s.backend, s.reference, s.reference_version \
             FROM secrets s LEFT JOIN teams t ON s.team = t.id WHERE s.team = $1 AND s.name = $2"
        )
        .bind(team)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, secret_name = %name, team = %team, "Failed to get secret by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get secret '{}' for team '{}'", name, team),
            }
        })?;

        match row {
            Some(row) => self.decrypt_row(row),
            None => Err(FlowplaneError::not_found_msg(format!(
                "Secret '{}' not found for team '{}'",
                name, team
            ))),
        }
    }

    /// List all secrets
    #[instrument(skip(self), name = "db_list_secrets")]
    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<SecretData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<sqlx::Postgres, SecretRow>(
            "SELECT s.id, s.name, s.secret_type, s.description, s.configuration_encrypted, s.encryption_key_id, s.nonce, s.version, s.source, s.team, t.name as team_name, s.created_at, s.updated_at, s.expires_at, s.backend, s.reference, s.reference_version \
             FROM secrets s LEFT JOIN teams t ON s.team = t.id ORDER BY s.created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list secrets");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list secrets".to_string(),
            }
        })?;

        rows.into_iter().map(|row| self.decrypt_row(row)).collect()
    }

    /// List secrets filtered by team names
    ///
    /// # Security Note
    ///
    /// Empty teams array returns ALL resources. This is intentional for admin:all
    /// scope but could be a security issue if authorization logic has bugs.
    /// A warning is logged when this occurs for auditing purposes.
    #[instrument(skip(self), fields(teams = ?teams, limit = ?limit, offset = ?offset), name = "db_list_secrets_by_teams")]
    pub async fn list_by_teams(
        &self,
        teams: &[String],
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<SecretData>> {
        // SECURITY: Empty teams array returns ALL resources (admin scope).
        // Log warning for audit trail - this should only happen for admin:all scope.
        if teams.is_empty() {
            tracing::warn!(
                resource = "secrets",
                "list_by_teams called with empty teams array - returning all resources (admin scope)"
            );
            return self.list(limit, offset).await;
        }

        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        // Build the query with IN clause for team filtering
        let placeholders = teams
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let query_str = format!(
            "SELECT s.id, s.name, s.secret_type, s.description, s.configuration_encrypted, s.encryption_key_id, s.nonce, s.version, s.source, s.team, t.name as team_name, s.created_at, s.updated_at, s.expires_at, s.backend, s.reference, s.reference_version \
             FROM secrets s LEFT JOIN teams t ON s.team = t.id \
             WHERE s.team IN ({}) \
             ORDER BY s.created_at DESC \
             LIMIT ${} OFFSET ${}",
            placeholders,
            teams.len() + 1,
            teams.len() + 2
        );

        let mut query = sqlx::query_as::<sqlx::Postgres, SecretRow>(&query_str);

        // Bind team names
        for team in teams {
            query = query.bind(team);
        }

        // Bind limit and offset
        query = query.bind(limit).bind(offset);

        let rows = query.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to list secrets by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list secrets for teams: {:?}", teams),
            }
        })?;

        rows.into_iter().map(|row| self.decrypt_row(row)).collect()
    }

    /// List only default/shared secrets (team IS NULL)
    ///
    /// Used for Allowlist scope where clients should only see shared infrastructure,
    /// not team-specific resources.
    ///
    /// Note: Secrets are always team-scoped in practice, so this typically returns empty.
    #[instrument(skip(self), fields(limit = ?limit, offset = ?offset), name = "db_list_default_secrets")]
    pub async fn list_default_only(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<SecretData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<sqlx::Postgres, SecretRow>(
            "SELECT s.id, s.name, s.secret_type, s.description, s.configuration_encrypted, s.encryption_key_id, s.nonce, s.version, s.source, s.team, t.name as team_name, s.created_at, s.updated_at, s.expires_at, s.backend, s.reference, s.reference_version \
             FROM secrets s LEFT JOIN teams t ON s.team = t.id \
             WHERE s.team IS NULL \
             ORDER BY s.created_at DESC \
             LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list default secrets");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list default secrets".to_string(),
            }
        })?;

        rows.into_iter().map(|row| self.decrypt_row(row)).collect()
    }

    /// Update a secret
    #[instrument(skip(self, request), fields(secret_id = %id), name = "db_update_secret")]
    pub async fn update(&self, id: &SecretId, request: UpdateSecretRequest) -> Result<SecretData> {
        // Get current secret to check if it exists
        let current = self.get_by_id(id).await?;

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        // Update description if provided
        let new_description = request.description.or(current.description);

        // Update expires_at if provided
        let new_expires_at = request.expires_at.unwrap_or(current.expires_at);

        // Update configuration if provided
        if let Some(new_config) = request.configuration {
            // Validate new configuration
            new_config.validate().map_err(|e| {
                FlowplaneError::validation(format!("Invalid secret configuration: {}", e))
            })?;

            // Serialize and encrypt
            let config_json = serde_json::to_string(&new_config).map_err(|e| {
                FlowplaneError::internal(format!("Failed to serialize secret configuration: {}", e))
            })?;

            let (encrypted, nonce) = self.encryption.encrypt(config_json.as_bytes())?;

            sqlx::query(
                "UPDATE secrets SET description = $1, configuration_encrypted = $2, encryption_key_id = $3, nonce = $4, version = $5, expires_at = $6, updated_at = $7 WHERE id = $8"
            )
            .bind(&new_description)
            .bind(&encrypted)
            .bind(self.encryption.key_version())
            .bind(&nonce)
            .bind(new_version)
            .bind(new_expires_at)
            .bind(now)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to update secret '{}'", id),
                }
            })?;
        } else {
            // Only update metadata, keep existing encrypted config
            sqlx::query(
                "UPDATE secrets SET description = $1, version = $2, expires_at = $3, updated_at = $4 WHERE id = $5"
            )
            .bind(&new_description)
            .bind(new_version)
            .bind(new_expires_at)
            .bind(now)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to update secret '{}'", id),
                }
            })?;
        }

        tracing::info!(
            secret_id = %id,
            new_version = new_version,
            "Updated secret"
        );

        self.get_by_id(id).await
    }

    /// Delete a secret
    #[instrument(skip(self), fields(secret_id = %id), name = "db_delete_secret")]
    pub async fn delete(&self, id: &SecretId) -> Result<()> {
        let result = sqlx::query("DELETE FROM secrets WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, secret_id = %id, "Failed to delete secret");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete secret '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Secret with ID '{}' not found",
                id
            )));
        }

        tracing::info!(secret_id = %id, "Deleted secret");
        Ok(())
    }

    /// Decrypt a database row into SecretData
    fn decrypt_row(&self, row: SecretRow) -> Result<SecretData> {
        // For reference-based secrets, configuration is empty (fetched on-demand)
        let configuration = if row.backend.is_some() && row.reference.is_some() {
            // Reference-based secret - no encrypted config to decrypt
            // The configuration will be fetched on-demand from the external backend
            String::new()
        } else {
            // Legacy database-stored secret - decrypt the configuration
            let decrypted = self.encryption.decrypt(&row.configuration_encrypted, &row.nonce)?;
            String::from_utf8(decrypted).map_err(|e| {
                FlowplaneError::internal(format!("Invalid UTF-8 in decrypted secret: {}", e))
            })?
        };

        let secret_type = row.secret_type.parse::<SecretType>().map_err(|_| {
            FlowplaneError::internal(format!("Unknown secret type: {}", row.secret_type))
        })?;

        Ok(SecretData {
            id: SecretId::from_string(row.id),
            name: row.name,
            secret_type,
            description: row.description,
            configuration,
            version: row.version,
            source: row.source,
            team: row.team,
            team_name: row.team_name,
            created_at: row.created_at,
            updated_at: row.updated_at,
            expires_at: row.expires_at,
            backend: row.backend,
            reference: row.reference,
            reference_version: row.reference_version,
        })
    }

    /// Create a reference-based secret (stores reference, not the actual secret)
    #[instrument(skip(self, request), fields(secret_name = %request.name, backend = %request.backend), name = "db_create_secret_reference")]
    pub async fn create_reference(
        &self,
        request: CreateSecretReferenceRequest,
    ) -> Result<SecretData> {
        let id = SecretId::new();
        let now = chrono::Utc::now();

        // For reference-based secrets, we store empty encrypted data
        // The actual secret is fetched on-demand from the external backend
        let empty_config: Vec<u8> = vec![];
        let empty_nonce: Vec<u8> = vec![0u8; 12]; // Standard GCM nonce size

        let result = sqlx::query(
            "INSERT INTO secrets (id, name, secret_type, description, configuration_encrypted, encryption_key_id, nonce, version, source, team, expires_at, backend, reference, reference_version, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 1, 'native_api', $8, $9, $10, $11, $12, $13, $14)"
        )
        .bind(id.as_str())
        .bind(&request.name)
        .bind(request.secret_type.as_str())
        .bind(&request.description)
        .bind(&empty_config)
        .bind("reference") // Special key_id to indicate reference-based
        .bind(&empty_nonce)
        .bind(&request.team)
        .bind(request.expires_at)
        .bind(&request.backend)
        .bind(&request.reference)
        .bind(&request.reference_version)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, secret_name = %request.name, "Failed to create secret reference");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create secret reference '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create secret reference"));
        }

        tracing::info!(
            secret_id = %id,
            secret_name = %request.name,
            secret_type = %request.secret_type,
            backend = %request.backend,
            reference = %request.reference,
            team = %request.team,
            "Created new secret reference"
        );

        self.get_by_id(&id).await
    }
}

impl std::fmt::Debug for SecretRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretRepository")
            .field("pool", &"[DbPool]")
            .field("encryption", &self.encryption)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::GenericSecretSpec;

    // Note: Integration tests require a real database and encryption key
    // These tests verify the data structures and transformations

    #[test]
    fn test_create_request_serialization() {
        use base64::Engine;

        let secret = base64::engine::general_purpose::STANDARD.encode(b"my-oauth-secret");
        let request = CreateSecretRequest {
            name: "oauth-token-secret".to_string(),
            secret_type: SecretType::GenericSecret,
            description: Some("OAuth2 client secret".to_string()),
            configuration: SecretSpec::GenericSecret(GenericSecretSpec { secret }),
            team: "my-team".to_string(),
            expires_at: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("oauth-token-secret"));
        assert!(json.contains("generic_secret"));
    }

    #[test]
    fn test_secret_type_roundtrip() {
        for st in [
            SecretType::GenericSecret,
            SecretType::TlsCertificate,
            SecretType::CertificateValidationContext,
            SecretType::SessionTicketKeys,
        ] {
            let s = st.as_str();
            let parsed: SecretType = s.parse().unwrap();
            assert_eq!(st, parsed);
        }
    }
}
