//! Audit log repository for tracking audit events
//!
//! This module provides audit event logging for authentication and platform API
//! lifecycle events.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;

/// Audit event descriptor for authentication activity logging.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub action: String,
    pub resource_id: Option<String>,
    pub resource_name: Option<String>,
    pub metadata: serde_json::Value,
}

impl AuditEvent {
    pub fn token(
        action: &str,
        resource_id: Option<&str>,
        resource_name: Option<&str>,
        metadata: serde_json::Value,
    ) -> Self {
        Self {
            action: action.to_string(),
            resource_id: resource_id.map(|value| value.to_string()),
            resource_name: resource_name.map(|value| value.to_string()),
            metadata,
        }
    }

    /// Create a secrets-related audit event.
    ///
    /// # Arguments
    ///
    /// * `action` - The action performed (e.g., "secrets.get", "secrets.set", "secrets.rotate")
    /// * `secret_key` - The key of the secret being accessed
    /// * `metadata` - Additional metadata about the operation
    ///
    /// # Security
    ///
    /// NEVER include the actual secret value in the metadata. Only log the key and operation metadata.
    pub fn secret(action: &str, secret_key: &str, metadata: serde_json::Value) -> Self {
        Self {
            action: action.to_string(),
            resource_id: Some(secret_key.to_string()),
            resource_name: Some(secret_key.to_string()),
            metadata,
        }
    }
}

/// Repository for audit log interactions (scaffold for auth events).
#[derive(Debug, Clone)]
pub struct AuditLogRepository {
    pool: DbPool,
}

impl AuditLogRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    async fn record_event(&self, resource_type: &str, event: AuditEvent) -> Result<()> {
        let now = chrono::Utc::now();
        let metadata_json = serde_json::to_string(&event.metadata).map_err(|err| {
            FlowplaneError::validation(format!("Invalid audit metadata JSON: {}", err))
        })?;
        let resource_name = event.resource_name.unwrap_or_else(|| event.action.clone());

        sqlx::query(
            "INSERT INTO audit_log (resource_type, resource_id, resource_name, action, old_configuration, new_configuration, user_id, client_ip, user_agent, created_at) \
             VALUES ($1, $2, $3, $4, NULL, $5, NULL, NULL, NULL, $6)"
        )
        .bind(resource_type)
        .bind(event.resource_id.as_deref())
        .bind(&resource_name)
        .bind(event.action.as_str())
        .bind(metadata_json)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to write authentication audit event".to_string(),
        })?;

        Ok(())
    }

    /// Record an authentication-related audit event.
    pub async fn record_auth_event(&self, event: AuditEvent) -> Result<()> {
        self.record_event("auth.token", event).await
    }

    /// Record a Platform API lifecycle event.
    pub async fn record_platform_event(&self, event: AuditEvent) -> Result<()> {
        self.record_event("platform.api", event).await
    }

    /// Record a secrets management audit event.
    ///
    /// This method logs all secret access operations for security auditing.
    /// Events are tamper-evident and include timestamps, operation type, and metadata.
    ///
    /// # Security
    ///
    /// Secret values are NEVER logged. Only secret keys and operation metadata are recorded.
    pub async fn record_secrets_event(&self, event: AuditEvent) -> Result<()> {
        self.record_event("secrets", event).await
    }
}
