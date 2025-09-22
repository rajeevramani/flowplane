//! # Database Connection Pool Management
//!
//! Provides database connection pool creation and management utilities.

use crate::config::DatabaseConfig;
use crate::errors::{MagayaError, Result};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
    Pool, Sqlite,
};
use std::{str::FromStr, time::Duration};

/// Type alias for the database connection pool
pub type DbPool = Pool<Sqlite>;

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Create a database connection pool with the specified configuration
pub async fn create_pool(config: &DatabaseConfig) -> Result<Pool<Sqlite>> {
    // Validate configuration
    validate_config(config)?;

    let pool_options = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(config.connect_timeout())
        .test_before_acquire(true); // Test connections before use

    let pool_options = if let Some(idle_timeout) = config.idle_timeout() {
        pool_options.idle_timeout(idle_timeout)
    } else {
        pool_options
    };

    let pool = if config.is_sqlite() {
        let connect_options = SqliteConnectOptions::from_str(&config.url)
            .map_err(|e| MagayaError::Database {
                source: e,
                context: format!(
                    "Invalid SQLite connection string: {}",
                    sanitize_url(&config.url)
                ),
            })?
            .create_if_missing(true)
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
            .journal_mode(SqliteJournalMode::Wal);

        pool_options
            .connect_with(connect_options)
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    url = %config.url,
                    busy_timeout_ms = SQLITE_BUSY_TIMEOUT.as_millis(),
                    "Failed to create SQLite database pool"
                );
                MagayaError::Database {
                    source: e,
                    context: format!(
                        "Failed to connect to database: {}",
                        sanitize_url(&config.url)
                    ),
                }
            })?
    } else {
        pool_options.connect(&config.url).await.map_err(|e| {
            tracing::error!(error = %e, url = %config.url, "Failed to create database pool");
            MagayaError::Database {
                source: e,
                context: format!(
                    "Failed to connect to database: {}",
                    sanitize_url(&config.url)
                ),
            }
        })?
    };

    tracing::info!(
        database_type = if config.is_sqlite() {
            "sqlite"
        } else {
            "postgresql"
        },
        max_connections = config.max_connections,
        min_connections = config.min_connections,
        connect_timeout_ms = config.connect_timeout().as_millis(),
        idle_timeout_ms = config.idle_timeout().map(|d| d.as_millis()),
        "Database connection pool created"
    );

    // Run migrations if auto_migrate is enabled
    if config.auto_migrate {
        tracing::info!("Auto-migration enabled, running database migrations");
        crate::storage::migrations::run_migrations(&pool).await?;
    }

    Ok(pool)
}

/// Validate database configuration
fn validate_config(config: &DatabaseConfig) -> Result<()> {
    if config.max_connections == 0 {
        return Err(MagayaError::validation(
            "max_connections must be greater than 0",
        ));
    }

    if config.min_connections > config.max_connections {
        return Err(MagayaError::validation(
            "min_connections cannot be greater than max_connections",
        ));
    }

    if config.url.is_empty() {
        return Err(MagayaError::validation("database URL cannot be empty"));
    }

    // Validate URL format
    if !config.url.starts_with("sqlite://") && !config.url.starts_with("postgresql://") {
        return Err(MagayaError::validation(
            "database URL must start with 'sqlite://' or 'postgresql://'",
        ));
    }

    Ok(())
}

/// Sanitize database URL for logging (remove credentials)
fn sanitize_url(url: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url) {
        if parsed.password().is_some() || !parsed.username().is_empty() {
            // Hide credentials in logs
            format!(
                "{}://***:***@{}{}",
                parsed.scheme(),
                parsed.host_str().unwrap_or("unknown"),
                parsed.path()
            )
        } else {
            url.to_string()
        }
    } else {
        url.to_string()
    }
}

/// Get pool statistics for monitoring
pub fn get_pool_stats(pool: &Pool<Sqlite>) -> PoolStats {
    PoolStats {
        size: pool.size(),
        idle: pool.num_idle(),
    }
}

/// Pool statistics for monitoring
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Total connections in the pool
    pub size: u32,
    /// Number of idle connections
    pub idle: usize,
}

impl PoolStats {
    /// Get the number of active connections
    pub fn active(&self) -> u32 {
        self.size.saturating_sub(self.idle as u32)
    }

    /// Check if the pool is healthy (has available connections)
    pub fn is_healthy(&self) -> bool {
        self.size > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_config_valid() {
        let config = DatabaseConfig {
            url: "sqlite://./test.db".to_string(),
            max_connections: 10,
            min_connections: 2,
            ..Default::default()
        };

        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_config_invalid_max_connections() {
        let config = DatabaseConfig {
            url: "sqlite://./test.db".to_string(),
            max_connections: 0,
            ..Default::default()
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_invalid_min_max() {
        let config = DatabaseConfig {
            url: "sqlite://./test.db".to_string(),
            max_connections: 5,
            min_connections: 10,
            ..Default::default()
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_empty_url() {
        let config = DatabaseConfig {
            url: "".to_string(),
            ..Default::default()
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_invalid_url_scheme() {
        let config = DatabaseConfig {
            url: "mysql://localhost/test".to_string(),
            ..Default::default()
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_sanitize_url() {
        assert_eq!(
            sanitize_url("postgresql://user:pass@localhost/db"),
            "postgresql://***:***@localhost/db"
        );

        assert_eq!(sanitize_url("sqlite://./test.db"), "sqlite://./test.db");

        assert_eq!(sanitize_url("invalid-url"), "invalid-url");
    }

    #[test]
    fn test_pool_stats() {
        let stats = PoolStats { size: 10, idle: 3 };

        assert_eq!(stats.active(), 7);
        assert!(stats.is_healthy());

        let empty_stats = PoolStats { size: 0, idle: 0 };

        assert_eq!(empty_stats.active(), 0);
        assert!(!empty_stats.is_healthy());
    }

    #[tokio::test]
    async fn test_create_pool_success() {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            max_connections: 3,
            min_connections: 1,
            auto_migrate: false,
            ..Default::default()
        };

        let pool = create_pool(&config).await.unwrap();
        let stats = get_pool_stats(&pool);
        assert!(stats.is_healthy());
    }

    #[tokio::test]
    async fn test_create_pool_invalid_config() {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            max_connections: 0, // Invalid
            ..Default::default()
        };

        let result = create_pool(&config).await;
        assert!(result.is_err());
    }
}
