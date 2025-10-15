//! Audited secrets client wrapper.
//!
//! This module provides an audited wrapper around any [`SecretsClient`] implementation
//! that automatically logs all secret access operations to the audit log for security
//! tracking and compliance.
//!
//! # Security
//!
//! The audited client logs:
//! - Operation type (get, set, rotate, list, delete)
//! - Secret key (NEVER the secret value)
//! - Timestamp
//! - Success/failure status
//! - Operation metadata
//!
//! Secret values are NEVER logged to maintain security.
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::secrets::{AuditedSecretsClient, VaultSecretsClient};
//! use flowplane::storage::repositories::AuditLogRepository;
//!
//! let vault_client = VaultSecretsClient::new(config).await?;
//! let audit_repo = AuditLogRepository::new(pool);
//!
//! // Wrap the Vault client with auditing
//! let client = AuditedSecretsClient::new(vault_client, audit_repo);
//!
//! // All operations are now automatically audited
//! let secret = client.get_secret("jwt_secret").await?;
//! ```

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use super::client::{SecretMetadata, SecretsClient};
use super::error::Result;
use crate::storage::repository::{AuditEvent, AuditLogRepository};

/// Audited wrapper for SecretsClient implementations.
///
/// This wrapper automatically logs all secret operations to the audit log
/// for security tracking and compliance. It transparently wraps any
/// [`SecretsClient`] implementation.
///
/// # Security
///
/// - Secret values are NEVER logged
/// - All operations are logged with timestamps
/// - Failed operations are logged with error details
/// - Audit logs are tamper-evident (stored in database)
pub struct AuditedSecretsClient<T: SecretsClient> {
    inner: T,
    audit_repository: Arc<AuditLogRepository>,
}

impl<T: SecretsClient> AuditedSecretsClient<T> {
    /// Creates a new audited secrets client.
    ///
    /// # Arguments
    ///
    /// * `inner` - The underlying secrets client to wrap
    /// * `audit_repository` - Repository for storing audit events
    pub fn new(inner: T, audit_repository: Arc<AuditLogRepository>) -> Self {
        Self { inner, audit_repository }
    }

    /// Record a successful secret operation to the audit log.
    async fn record_success(&self, action: &str, key: &str, metadata: serde_json::Value) {
        let event = AuditEvent::secret(action, key, metadata);
        if let Err(e) = self.audit_repository.record_secrets_event(event).await {
            tracing::error!(
                error = %e,
                action = %action,
                key = %key,
                "Failed to record secrets audit event"
            );
        }
    }

    /// Record a failed secret operation to the audit log.
    async fn record_failure(&self, action: &str, key: &str, error: &str) {
        let event = AuditEvent::secret(
            action,
            key,
            json!({
                "success": false,
                "error": error,
                "timestamp": chrono::Utc::now()
            }),
        );
        if let Err(e) = self.audit_repository.record_secrets_event(event).await {
            tracing::error!(
                error = %e,
                action = %action,
                key = %key,
                "Failed to record secrets audit event for failure"
            );
        }
    }
}

#[async_trait]
impl<T: SecretsClient> SecretsClient for AuditedSecretsClient<T> {
    async fn get_secret(&self, key: &str) -> Result<String> {
        match self.inner.get_secret(key).await {
            Ok(value) => {
                self.record_success(
                    "secrets.get",
                    key,
                    json!({
                        "success": true,
                        "timestamp": chrono::Utc::now()
                    }),
                )
                .await;
                Ok(value)
            }
            Err(e) => {
                self.record_failure("secrets.get", key, &e.to_string()).await;
                Err(e)
            }
        }
    }

    async fn set_secret(&self, key: &str, value: &str) -> Result<()> {
        match self.inner.set_secret(key, value).await {
            Ok(()) => {
                self.record_success(
                    "secrets.set",
                    key,
                    json!({
                        "success": true,
                        "timestamp": chrono::Utc::now()
                    }),
                )
                .await;
                Ok(())
            }
            Err(e) => {
                self.record_failure("secrets.set", key, &e.to_string()).await;
                Err(e)
            }
        }
    }

    async fn rotate_secret(&self, key: &str) -> Result<String> {
        match self.inner.rotate_secret(key).await {
            Ok(new_value) => {
                self.record_success(
                    "secrets.rotate",
                    key,
                    json!({
                        "success": true,
                        "timestamp": chrono::Utc::now()
                    }),
                )
                .await;
                Ok(new_value)
            }
            Err(e) => {
                self.record_failure("secrets.rotate", key, &e.to_string()).await;
                Err(e)
            }
        }
    }

    async fn list_secrets(&self) -> Result<Vec<SecretMetadata>> {
        match self.inner.list_secrets().await {
            Ok(secrets) => {
                self.record_success(
                    "secrets.list",
                    "*",
                    json!({
                        "success": true,
                        "count": secrets.len(),
                        "timestamp": chrono::Utc::now()
                    }),
                )
                .await;
                Ok(secrets)
            }
            Err(e) => {
                self.record_failure("secrets.list", "*", &e.to_string()).await;
                Err(e)
            }
        }
    }

    async fn delete_secret(&self, key: &str) -> Result<()> {
        match self.inner.delete_secret(key).await {
            Ok(()) => {
                self.record_success(
                    "secrets.delete",
                    key,
                    json!({
                        "success": true,
                        "timestamp": chrono::Utc::now()
                    }),
                )
                .await;
                Ok(())
            }
            Err(e) => {
                self.record_failure("secrets.delete", key, &e.to_string()).await;
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::EnvVarSecretsClient;
    use crate::storage::DbPool;

    #[tokio::test]
    async fn test_audited_client_logs_operations() {
        // Set up test environment
        std::env::set_var("FLOWPLANE_SECRET_TEST", "test-value");

        // Create test database pool (in-memory SQLite for testing)
        let pool =
            DbPool::connect("sqlite::memory:").await.expect("Failed to create test database");

        // Run migrations to create audit_log table
        sqlx::migrate!().run(&pool).await.expect("Failed to run migrations");

        let env_client = EnvVarSecretsClient::new();
        let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));
        let audited_client = AuditedSecretsClient::new(env_client, audit_repo);

        // Test get_secret with auditing
        let result = audited_client.get_secret("test").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-value");

        // Verify audit log was created (query the audit_log table)
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = 'secrets.get'")
                .fetch_one(&pool)
                .await
                .expect("Failed to query audit log");

        assert_eq!(count, 1);

        std::env::remove_var("FLOWPLANE_SECRET_TEST");
    }

    #[tokio::test]
    async fn test_audited_client_logs_failures() {
        let pool =
            DbPool::connect("sqlite::memory:").await.expect("Failed to create test database");

        sqlx::migrate!().run(&pool).await.expect("Failed to run migrations");

        let env_client = EnvVarSecretsClient::new();
        let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));
        let audited_client = AuditedSecretsClient::new(env_client, audit_repo);

        // Try to get non-existent secret
        let result = audited_client.get_secret("nonexistent").await;
        assert!(result.is_err());

        // Verify failure was logged
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE action = 'secrets.get' AND new_configuration LIKE '%\"success\":false%'"
        )
        .fetch_one(&pool)
        .await
        .expect("Failed to query audit log");

        assert_eq!(count, 1);
    }
}
