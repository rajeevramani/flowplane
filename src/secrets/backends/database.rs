//! Database secret backend implementation
//!
//! Provides legacy mode where secrets are stored encrypted in the database.
//! This backend reads encrypted secrets using SecretEncryption for decryption.

use super::backend::{SecretBackend, SecretBackendType};
use crate::domain::{SecretSpec, SecretType};
use crate::errors::{FlowplaneError, Result};
use crate::services::SecretEncryption;
use crate::storage::DbPool;
use async_trait::async_trait;
use sqlx::FromRow;
use std::sync::Arc;
use tracing::{debug, error};

/// Database row for direct secret lookup
#[derive(Debug, Clone, FromRow)]
struct SecretRow {
    pub configuration_encrypted: Vec<u8>,
    pub nonce: Vec<u8>,
    pub secret_type: String,
}

/// Database secret backend
///
/// This backend fetches secrets from the database where they are stored
/// with AES-256-GCM encryption. The reference is the secret ID or name.
///
/// Used for:
/// - Legacy secrets created before external backend support
/// - Users who prefer to store encrypted secrets in the database
#[derive(Debug)]
pub struct DatabaseSecretBackend {
    pool: DbPool,
    encryption: Arc<SecretEncryption>,
}

impl DatabaseSecretBackend {
    /// Create a new database backend
    pub fn new(pool: DbPool, encryption: Arc<SecretEncryption>) -> Self {
        Self { pool, encryption }
    }

    /// Try to create from environment, returns None if encryption not configured
    pub fn from_env(pool: DbPool) -> Result<Option<Self>> {
        use crate::services::SecretEncryptionConfig;

        match SecretEncryptionConfig::from_env() {
            Ok(config) => {
                let encryption = SecretEncryption::new(&config)?;
                Ok(Some(Self { pool, encryption: Arc::new(encryption) }))
            }
            Err(_) => {
                // Encryption not configured
                Ok(None)
            }
        }
    }
}

#[async_trait]
impl SecretBackend for DatabaseSecretBackend {
    async fn fetch_secret(&self, reference: &str, expected_type: SecretType) -> Result<SecretSpec> {
        debug!(
            reference = %reference,
            expected_type = ?expected_type,
            "Fetching secret from database"
        );

        // Reference can be either ID or name - try ID first
        let row = if reference.starts_with("sec_") {
            // Looks like an ID
            sqlx::query_as::<_, SecretRow>(
                "SELECT configuration_encrypted, nonce, secret_type FROM secrets WHERE id = $1",
            )
            .bind(reference)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                error!(reference = %reference, error = %e, "Database query failed");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to fetch secret".to_string(),
                }
            })?
        } else {
            // Try by name (requires team context - use team from reference if present)
            // Format: team/name or just name (in which case we search any team)
            let (team, name) = if reference.contains('/') {
                let parts: Vec<&str> = reference.splitn(2, '/').collect();
                (Some(parts[0]), parts[1])
            } else {
                (None, reference)
            };

            match team {
                Some(team) => {
                    sqlx::query_as::<_, SecretRow>(
                        "SELECT configuration_encrypted, nonce, secret_type FROM secrets WHERE team = $1 AND name = $2",
                    )
                    .bind(team)
                    .bind(name)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| {
                        error!(reference = %reference, error = %e, "Database query failed");
                        FlowplaneError::Database {
                            source: e,
                            context: "Failed to fetch secret".to_string(),
                        }
                    })?
                }
                None => {
                    // Search by name across all teams (first match)
                    sqlx::query_as::<_, SecretRow>(
                        "SELECT configuration_encrypted, nonce, secret_type FROM secrets WHERE name = $1 LIMIT 1",
                    )
                    .bind(name)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| {
                        error!(reference = %reference, error = %e, "Database query failed");
                        FlowplaneError::Database {
                            source: e,
                            context: "Failed to fetch secret".to_string(),
                        }
                    })?
                }
            }
        };

        let row = row.ok_or_else(|| {
            FlowplaneError::not_found_msg(format!("Secret '{}' not found in database", reference))
        })?;

        // Verify type matches
        let stored_type = row.secret_type.parse::<SecretType>().map_err(|_| {
            FlowplaneError::config(format!("Unknown secret type: {}", row.secret_type))
        })?;

        if stored_type != expected_type {
            return Err(FlowplaneError::config(format!(
                "Secret type mismatch: expected {:?}, found {:?}",
                expected_type, stored_type
            )));
        }

        // Decrypt the configuration
        let decrypted = self.encryption.decrypt(&row.configuration_encrypted, &row.nonce)?;
        let config_str = String::from_utf8(decrypted).map_err(|e| {
            FlowplaneError::internal(format!("Invalid UTF-8 in decrypted secret: {}", e))
        })?;

        // Parse into SecretSpec
        let spec: SecretSpec = serde_json::from_str(&config_str).map_err(|e| {
            FlowplaneError::config(format!("Invalid secret configuration JSON: {}", e))
        })?;

        Ok(spec)
    }

    async fn validate_reference(&self, reference: &str) -> Result<bool> {
        debug!(reference = %reference, "Validating database secret reference");

        // Check if the reference exists
        let exists = if reference.starts_with("sec_") {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM secrets WHERE id = $1")
                .bind(reference)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: "Failed to validate reference".to_string(),
                })?
                > 0
        } else if reference.contains('/') {
            let parts: Vec<&str> = reference.splitn(2, '/').collect();
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM secrets WHERE team = $1 AND name = $2",
            )
            .bind(parts[0])
            .bind(parts[1])
            .fetch_one(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to validate reference".to_string(),
            })? > 0
        } else {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM secrets WHERE name = $1")
                .bind(reference)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: "Failed to validate reference".to_string(),
                })?
                > 0
        };

        Ok(exists)
    }

    fn backend_type(&self) -> SecretBackendType {
        SecretBackendType::Database
    }

    async fn health_check(&self) -> Result<()> {
        // Simple query to check database connectivity
        sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&self.pool).await.map_err(|e| {
            FlowplaneError::Database {
                source: e,
                context: "Database health check failed".to_string(),
            }
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_type() {
        assert_eq!(SecretBackendType::Database.as_str(), "database");
    }

    #[test]
    fn test_reference_parsing() {
        // ID format
        assert!("sec_abc123".starts_with("sec_"));

        // Team/name format
        let ref_with_team = "my-team/my-secret";
        assert!(ref_with_team.contains('/'));
        let parts: Vec<&str> = ref_with_team.splitn(2, '/').collect();
        assert_eq!(parts[0], "my-team");
        assert_eq!(parts[1], "my-secret");

        // Name only format
        let ref_name_only = "my-secret";
        assert!(!ref_name_only.contains('/'));
        assert!(!ref_name_only.starts_with("sec_"));
    }
}
