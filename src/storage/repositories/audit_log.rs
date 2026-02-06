//! Audit log repository for tracking audit events
//!
//! This module provides audit event logging for authentication and platform API
//! lifecycle events.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;
use utoipa::ToSchema;

/// Audit event descriptor for authentication activity logging.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub action: String,
    pub resource_id: Option<String>,
    pub resource_name: Option<String>,
    pub metadata: serde_json::Value,
    pub user_id: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
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
            user_id: None,
            client_ip: None,
            user_agent: None,
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
            user_id: None,
            client_ip: None,
            user_agent: None,
        }
    }

    /// Set user context for this audit event.
    ///
    /// This is typically called after creating the event via a factory method
    /// to add authentication and request context.
    pub fn with_user_context(
        mut self,
        user_id: Option<String>,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        self.user_id = user_id;
        self.client_ip = client_ip;
        self.user_agent = user_agent;
        self
    }
}

/// Audit log entry returned from database queries.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogEntry {
    pub id: i64,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub resource_name: Option<String>,
    pub action: String,
    pub old_configuration: Option<String>,
    pub new_configuration: Option<String>,
    pub user_id: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Internal row type for database query
#[derive(FromRow)]
struct AuditLogRow {
    pub id: i64,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub resource_name: Option<String>,
    pub action: String,
    pub old_configuration: Option<String>,
    pub new_configuration: Option<String>,
    pub user_id: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
}

impl From<AuditLogRow> for AuditLogEntry {
    fn from(row: AuditLogRow) -> Self {
        Self {
            id: row.id,
            resource_type: row.resource_type,
            resource_id: row.resource_id,
            resource_name: row.resource_name,
            action: row.action,
            old_configuration: row.old_configuration,
            new_configuration: row.new_configuration,
            user_id: row.user_id,
            client_ip: row.client_ip,
            user_agent: row.user_agent,
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        }
    }
}

/// Query filters for audit log entries.
#[derive(Debug, Clone, Default)]
pub struct AuditLogFilters {
    pub resource_type: Option<String>,
    pub action: Option<String>,
    pub user_id: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
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

    #[instrument(skip(self, event), fields(resource_type = %resource_type, action = %event.action), name = "db_record_audit_event")]
    async fn record_event(&self, resource_type: &str, event: AuditEvent) -> Result<()> {
        let now = chrono::Utc::now();
        let metadata_json = serde_json::to_string(&event.metadata).map_err(|err| {
            FlowplaneError::validation(format!("Invalid audit metadata JSON: {}", err))
        })?;
        let resource_name = event.resource_name.unwrap_or_else(|| event.action.clone());

        sqlx::query(
            "INSERT INTO audit_log (resource_type, resource_id, resource_name, action, old_configuration, new_configuration, user_id, client_ip, user_agent, created_at) \
             VALUES ($1, $2, $3, $4, NULL, $5, $6, $7, $8, $9)"
        )
        .bind(resource_type)
        .bind(event.resource_id.as_deref())
        .bind(&resource_name)
        .bind(event.action.as_str())
        .bind(metadata_json)
        .bind(event.user_id.as_deref())
        .bind(event.client_ip.as_deref())
        .bind(event.user_agent.as_deref())
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
    #[instrument(skip(self, event), fields(action = %event.action), name = "db_record_auth_event")]
    pub async fn record_auth_event(&self, event: AuditEvent) -> Result<()> {
        self.record_event("auth.token", event).await
    }

    /// Record a Platform API lifecycle event.
    #[instrument(skip(self, event), fields(action = %event.action), name = "db_record_platform_event")]
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
    #[instrument(skip(self, event), fields(action = %event.action), name = "db_record_secrets_event")]
    pub async fn record_secrets_event(&self, event: AuditEvent) -> Result<()> {
        self.record_event("secrets", event).await
    }

    /// Query audit log entries with filtering and pagination.
    ///
    /// # Arguments
    ///
    /// * `filters` - Optional filters for resource_type, action, user_id, and date range
    /// * `limit` - Maximum number of results (default: 50, max: 1000)
    /// * `offset` - Number of results to skip for pagination
    ///
    /// # Returns
    ///
    /// A vector of [`AuditLogEntry`] matching the filters, ordered by created_at DESC.
    #[instrument(skip(self, filters), fields(limit = ?limit, offset = ?offset), name = "db_query_audit_logs")]
    pub async fn query_logs(
        &self,
        filters: Option<AuditLogFilters>,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<AuditLogEntry>> {
        let limit = limit.unwrap_or(50).min(1000);
        let offset = offset.unwrap_or(0);

        let mut query_builder = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT id, resource_type, resource_id, resource_name, action, \
             old_configuration, new_configuration, user_id, client_ip, user_agent, created_at \
             FROM audit_log WHERE 1=1",
        );

        // Apply filters
        if let Some(filters) = filters {
            if let Some(resource_type) = filters.resource_type {
                query_builder.push(" AND resource_type = ");
                query_builder.push_bind(resource_type);
            }
            if let Some(action) = filters.action {
                query_builder.push(" AND action = ");
                query_builder.push_bind(action);
            }
            if let Some(user_id) = filters.user_id {
                query_builder.push(" AND user_id = ");
                query_builder.push_bind(user_id);
            }
            if let Some(start_date) = filters.start_date {
                query_builder.push(" AND created_at >= ");
                query_builder.push_bind(start_date.to_rfc3339());
            }
            if let Some(end_date) = filters.end_date {
                query_builder.push(" AND created_at <= ");
                query_builder.push_bind(end_date.to_rfc3339());
            }
        }

        query_builder.push(" ORDER BY created_at DESC LIMIT ");
        query_builder.push_bind(limit);
        query_builder.push(" OFFSET ");
        query_builder.push_bind(offset);

        let rows =
            query_builder.build_query_as::<AuditLogRow>().fetch_all(&self.pool).await.map_err(
                |e| {
                    tracing::error!(error = %e, "Failed to query audit logs");
                    FlowplaneError::Database {
                        source: e,
                        context: "Failed to query audit logs".to_string(),
                    }
                },
            )?;

        let entries = rows.into_iter().map(|row| row.into()).collect();
        Ok(entries)
    }

    /// Get the total count of audit log entries matching the filters.
    ///
    /// This is useful for pagination to know the total number of pages.
    #[instrument(skip(self, filters), name = "db_count_audit_logs")]
    pub async fn count_logs(&self, filters: Option<AuditLogFilters>) -> Result<i64> {
        let mut query_builder = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT COUNT(*) as count FROM audit_log WHERE 1=1",
        );

        // Apply filters (same as query_logs)
        if let Some(filters) = filters {
            if let Some(resource_type) = filters.resource_type {
                query_builder.push(" AND resource_type = ");
                query_builder.push_bind(resource_type);
            }
            if let Some(action) = filters.action {
                query_builder.push(" AND action = ");
                query_builder.push_bind(action);
            }
            if let Some(user_id) = filters.user_id {
                query_builder.push(" AND user_id = ");
                query_builder.push_bind(user_id);
            }
            if let Some(start_date) = filters.start_date {
                query_builder.push(" AND created_at >= ");
                query_builder.push_bind(start_date.to_rfc3339());
            }
            if let Some(end_date) = filters.end_date {
                query_builder.push(" AND created_at <= ");
                query_builder.push_bind(end_date.to_rfc3339());
            }
        }

        let row: (i64,) =
            query_builder.build_query_as().fetch_one(&self.pool).await.map_err(|e| {
                tracing::error!(error = %e, "Failed to count audit logs");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to count audit logs".to_string(),
                }
            })?;

        Ok(row.0)
    }
}
