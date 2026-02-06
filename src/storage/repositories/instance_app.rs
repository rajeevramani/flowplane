//! Instance App repository for feature enablement management
//!
//! This module provides database access for the instance_apps table,
//! which tracks optional features (apps) that can be enabled/disabled
//! at the instance level by administrators.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;

/// Known app identifiers
pub mod app_ids {
    /// Stats Dashboard app
    pub const STATS_DASHBOARD: &str = "stats_dashboard";
    /// External Secrets app - enables fetching secrets from external backends
    pub const EXTERNAL_SECRETS: &str = "external_secrets";
}

/// Database row structure for instance_apps table
#[derive(Debug, Clone, FromRow)]
struct InstanceAppRow {
    pub app_id: String,
    pub enabled: i32,
    pub config: Option<String>,
    pub enabled_by: Option<String>,
    pub enabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Instance app definition with configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceApp {
    /// App identifier (e.g., "stats_dashboard")
    pub app_id: String,
    /// Whether the app is enabled
    pub enabled: bool,
    /// App-specific configuration (JSON)
    pub config: Option<serde_json::Value>,
    /// User who enabled/disabled the app
    pub enabled_by: Option<String>,
    /// When the app was enabled
    pub enabled_at: Option<DateTime<Utc>>,
    /// When the app record was created
    pub created_at: DateTime<Utc>,
    /// When the app record was last updated
    pub updated_at: DateTime<Utc>,
}

impl From<InstanceAppRow> for InstanceApp {
    fn from(row: InstanceAppRow) -> Self {
        let config = row.config.as_ref().and_then(|c| serde_json::from_str(c).ok());

        Self {
            app_id: row.app_id,
            enabled: row.enabled != 0,
            config,
            enabled_by: row.enabled_by,
            enabled_at: row.enabled_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Request to set an app's status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAppStatusRequest {
    /// Whether to enable or disable the app
    pub enabled: bool,
    /// Optional configuration to set
    pub config: Option<serde_json::Value>,
}

/// Stats Dashboard specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatsDashboardConfig {
    /// Polling interval in seconds (default: 10)
    #[serde(default = "default_poll_interval")]
    pub poll_interval_seconds: u64,
    /// Optional Prometheus endpoint URL for enriched metrics
    pub prometheus_url: Option<String>,
    /// Maximum number of historical data points to keep
    #[serde(default = "default_history_size")]
    pub max_history_size: usize,
}

impl Default for StatsDashboardConfig {
    fn default() -> Self {
        Self {
            poll_interval_seconds: default_poll_interval(),
            prometheus_url: None,
            max_history_size: default_history_size(),
        }
    }
}

fn default_poll_interval() -> u64 {
    10
}

fn default_history_size() -> usize {
    100
}

/// External Secrets specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSecretsConfig {
    /// Backend type: "vault", "aws_secrets_manager", "gcp_secret_manager"
    pub backend_type: String,

    /// Vault-specific settings
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_kv_mount: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_namespace: Option<String>,

    /// AWS-specific settings (uses IAM role by default)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_region: Option<String>,

    /// GCP-specific settings
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gcp_project_id: Option<String>,

    /// Cache TTL in seconds (default: 300)
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_seconds: u64,
}

impl Default for ExternalSecretsConfig {
    fn default() -> Self {
        Self {
            backend_type: "vault".to_string(),
            vault_addr: None,
            vault_kv_mount: None,
            vault_namespace: None,
            aws_region: None,
            gcp_project_id: None,
            cache_ttl_seconds: default_cache_ttl(),
        }
    }
}

fn default_cache_ttl() -> u64 {
    300
}

/// Repository trait for instance app operations
#[async_trait]
pub trait InstanceAppRepository: Send + Sync {
    /// Check if an app is enabled
    async fn is_enabled(&self, app_id: &str) -> Result<bool>;

    /// Get app details
    async fn get_app(&self, app_id: &str) -> Result<Option<InstanceApp>>;

    /// Get all apps
    async fn get_all_apps(&self) -> Result<Vec<InstanceApp>>;

    /// Get all enabled apps
    async fn get_enabled_apps(&self) -> Result<Vec<InstanceApp>>;

    /// Enable an app
    async fn enable_app(
        &self,
        app_id: &str,
        user_id: &str,
        config: Option<serde_json::Value>,
    ) -> Result<InstanceApp>;

    /// Disable an app
    async fn disable_app(&self, app_id: &str, user_id: &str) -> Result<InstanceApp>;

    /// Update app configuration (without changing enabled status)
    async fn update_config(&self, app_id: &str, config: serde_json::Value) -> Result<InstanceApp>;

    /// Get the stats dashboard configuration (convenience method)
    async fn get_stats_dashboard_config(&self) -> Result<Option<StatsDashboardConfig>>;

    /// Get the external secrets configuration (convenience method)
    async fn get_external_secrets_config(&self) -> Result<Option<ExternalSecretsConfig>>;
}

/// SQLx implementation of InstanceAppRepository
pub struct SqlxInstanceAppRepository {
    pool: DbPool,
}

impl SqlxInstanceAppRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl InstanceAppRepository for SqlxInstanceAppRepository {
    async fn is_enabled(&self, app_id: &str) -> Result<bool> {
        let result =
            sqlx::query_scalar::<_, i32>("SELECT enabled FROM instance_apps WHERE app_id = $1")
                .bind(app_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to check if app is enabled: {}", app_id),
                })?;

        Ok(result.map(|e| e != 0).unwrap_or(false))
    }

    async fn get_app(&self, app_id: &str) -> Result<Option<InstanceApp>> {
        let row =
            sqlx::query_as::<_, InstanceAppRow>("SELECT * FROM instance_apps WHERE app_id = $1")
                .bind(app_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to get app: {}", app_id),
                })?;

        Ok(row.map(InstanceApp::from))
    }

    async fn get_all_apps(&self) -> Result<Vec<InstanceApp>> {
        let rows =
            sqlx::query_as::<_, InstanceAppRow>("SELECT * FROM instance_apps ORDER BY app_id")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: "Failed to get all apps".to_string(),
                })?;

        Ok(rows.into_iter().map(InstanceApp::from).collect())
    }

    async fn get_enabled_apps(&self) -> Result<Vec<InstanceApp>> {
        let rows = sqlx::query_as::<_, InstanceAppRow>(
            "SELECT * FROM instance_apps WHERE enabled = 1 ORDER BY app_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to get enabled apps".to_string(),
        })?;

        Ok(rows.into_iter().map(InstanceApp::from).collect())
    }

    async fn enable_app(
        &self,
        app_id: &str,
        user_id: &str,
        config: Option<serde_json::Value>,
    ) -> Result<InstanceApp> {
        let now = Utc::now();
        let config_str = config.as_ref().map(|c| c.to_string());

        // Upsert: insert if not exists, update if exists
        let row = sqlx::query_as::<_, InstanceAppRow>(
            "INSERT INTO instance_apps (app_id, enabled, config, enabled_by, enabled_at, created_at, updated_at)
             VALUES ($1, 1, $2, $3, $4, $5, $5)
             ON CONFLICT(app_id) DO UPDATE SET
                enabled = 1,
                config = COALESCE($2, instance_apps.config),
                enabled_by = $3,
                enabled_at = $4,
                updated_at = $5
             RETURNING *",
        )
        .bind(app_id)
        .bind(config_str.as_deref())
        .bind(user_id)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to enable app: {}", app_id),
        })?;

        Ok(InstanceApp::from(row))
    }

    async fn disable_app(&self, app_id: &str, user_id: &str) -> Result<InstanceApp> {
        let now = Utc::now();

        let row = sqlx::query_as::<_, InstanceAppRow>(
            "INSERT INTO instance_apps (app_id, enabled, enabled_by, created_at, updated_at)
             VALUES ($1, 0, $2, $3, $3)
             ON CONFLICT(app_id) DO UPDATE SET
                enabled = 0,
                enabled_by = $2,
                updated_at = $3
             RETURNING *",
        )
        .bind(app_id)
        .bind(user_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to disable app: {}", app_id),
        })?;

        Ok(InstanceApp::from(row))
    }

    async fn update_config(&self, app_id: &str, config: serde_json::Value) -> Result<InstanceApp> {
        let now = Utc::now();
        let config_str = config.to_string();

        let row = sqlx::query_as::<_, InstanceAppRow>(
            "UPDATE instance_apps SET config = $2, updated_at = $3 WHERE app_id = $1 RETURNING *",
        )
        .bind(app_id)
        .bind(&config_str)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to update config for app: {}", app_id),
        })?;

        match row {
            Some(r) => Ok(InstanceApp::from(r)),
            None => Err(FlowplaneError::not_found("InstanceApp", app_id)),
        }
    }

    async fn get_stats_dashboard_config(&self) -> Result<Option<StatsDashboardConfig>> {
        let app = self.get_app(app_ids::STATS_DASHBOARD).await?;

        match app {
            Some(a) if a.enabled => {
                let config =
                    a.config.and_then(|c| serde_json::from_value(c).ok()).unwrap_or_default();
                Ok(Some(config))
            }
            _ => Ok(None),
        }
    }

    async fn get_external_secrets_config(&self) -> Result<Option<ExternalSecretsConfig>> {
        let app = self.get_app(app_ids::EXTERNAL_SECRETS).await?;

        match app {
            Some(a) if a.enabled => {
                let config =
                    a.config.and_then(|c| serde_json::from_value(c).ok()).unwrap_or_default();
                Ok(Some(config))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    #[tokio::test]
    async fn test_is_enabled_returns_false_for_unknown_app() {
        let _db = TestDatabase::new("instance_app_unknown").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        let enabled = repo.is_enabled("unknown_app").await.expect("is_enabled");
        assert!(!enabled);
    }

    #[tokio::test]
    async fn test_enable_app_creates_record() {
        let _db = TestDatabase::new("instance_app_enable").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        let app =
            repo.enable_app(app_ids::STATS_DASHBOARD, "user-123", None).await.expect("enable_app");

        assert_eq!(app.app_id, app_ids::STATS_DASHBOARD);
        assert!(app.enabled);
        assert_eq!(app.enabled_by.as_deref(), Some("user-123"));
        assert!(app.enabled_at.is_some());
    }

    #[tokio::test]
    async fn test_enable_app_with_config() {
        let _db = TestDatabase::new("instance_app_enable_config").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        let config = serde_json::json!({
            "pollIntervalSeconds": 30,
            "prometheusUrl": "http://prometheus:9090"
        });

        let app = repo
            .enable_app(app_ids::STATS_DASHBOARD, "user-123", Some(config.clone()))
            .await
            .expect("enable_app");

        assert!(app.enabled);
        assert!(app.config.is_some());
        assert_eq!(app.config.unwrap()["pollIntervalSeconds"], 30);
    }

    #[tokio::test]
    async fn test_disable_app() {
        let _db = TestDatabase::new("instance_app_disable").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        // First enable
        repo.enable_app(app_ids::STATS_DASHBOARD, "user-123", None).await.expect("enable_app");

        // Then disable
        let app =
            repo.disable_app(app_ids::STATS_DASHBOARD, "user-456").await.expect("disable_app");

        assert!(!app.enabled);
        assert_eq!(app.enabled_by.as_deref(), Some("user-456"));
    }

    #[tokio::test]
    async fn test_is_enabled_after_enable() {
        let _db = TestDatabase::new("instance_app_enabled_after").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        repo.enable_app(app_ids::STATS_DASHBOARD, "user-123", None).await.expect("enable_app");

        let enabled = repo.is_enabled(app_ids::STATS_DASHBOARD).await.expect("is_enabled");
        assert!(enabled);
    }

    #[tokio::test]
    async fn test_is_enabled_after_disable() {
        let _db = TestDatabase::new("instance_app_disabled_after").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        repo.enable_app(app_ids::STATS_DASHBOARD, "user-123", None).await.expect("enable_app");

        repo.disable_app(app_ids::STATS_DASHBOARD, "user-123").await.expect("disable_app");

        let enabled = repo.is_enabled(app_ids::STATS_DASHBOARD).await.expect("is_enabled");
        assert!(!enabled);
    }

    #[tokio::test]
    async fn test_get_enabled_apps() {
        let _db = TestDatabase::new("instance_app_get_enabled").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        // Enable one app
        repo.enable_app(app_ids::STATS_DASHBOARD, "user-123", None).await.expect("enable_app");

        // Disable another
        repo.disable_app("other_app", "user-123").await.expect("disable_app");

        let enabled = repo.get_enabled_apps().await.expect("get_enabled_apps");
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].app_id, app_ids::STATS_DASHBOARD);
    }

    #[tokio::test]
    async fn test_update_config() {
        let _db = TestDatabase::new("instance_app_update_config").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        // First enable
        repo.enable_app(app_ids::STATS_DASHBOARD, "user-123", None).await.expect("enable_app");

        // Update config
        let new_config = serde_json::json!({
            "pollIntervalSeconds": 60,
            "maxHistorySize": 200
        });

        let app =
            repo.update_config(app_ids::STATS_DASHBOARD, new_config).await.expect("update_config");

        assert_eq!(app.config.unwrap()["pollIntervalSeconds"], 60);
    }

    #[tokio::test]
    async fn test_get_stats_dashboard_config_returns_none_when_disabled() {
        let _db = TestDatabase::new("instance_app_stats_none").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        let config = repo.get_stats_dashboard_config().await.expect("get_stats_dashboard_config");
        assert!(config.is_none());
    }

    #[tokio::test]
    async fn test_get_stats_dashboard_config_returns_default_when_enabled() {
        let _db = TestDatabase::new("instance_app_stats_default").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        repo.enable_app(app_ids::STATS_DASHBOARD, "user-123", None).await.expect("enable_app");

        let config = repo.get_stats_dashboard_config().await.expect("get_stats_dashboard_config");

        assert!(config.is_some());
        let c = config.unwrap();
        assert_eq!(c.poll_interval_seconds, 10); // default
        assert_eq!(c.max_history_size, 100); // default
    }

    #[tokio::test]
    async fn test_get_stats_dashboard_config_with_custom_values() {
        let _db = TestDatabase::new("instance_app_stats_custom").await;
        let pool = _db.pool.clone();
        let repo = SqlxInstanceAppRepository::new(pool);

        let config = serde_json::json!({
            "pollIntervalSeconds": 30,
            "prometheusUrl": "http://prometheus:9090",
            "maxHistorySize": 500
        });

        repo.enable_app(app_ids::STATS_DASHBOARD, "user-123", Some(config))
            .await
            .expect("enable_app");

        let cfg = repo
            .get_stats_dashboard_config()
            .await
            .expect("get_stats_dashboard_config")
            .expect("config exists");

        assert_eq!(cfg.poll_interval_seconds, 30);
        assert_eq!(cfg.prometheus_url.as_deref(), Some("http://prometheus:9090"));
        assert_eq!(cfg.max_history_size, 500);
    }
}
