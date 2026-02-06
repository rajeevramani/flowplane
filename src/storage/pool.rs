//! # Database Connection Pool Management
//!
//! Provides database connection pool creation and management utilities.
//! Uses PostgreSQL as the sole database backend.

use crate::config::DatabaseConfig;
use crate::errors::{FlowplaneError, Result};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::str::FromStr;

/// Type alias for the database connection pool.
/// Uses PostgreSQL as the sole database backend.
pub type DbPool = PgPool;

/// Create a database connection pool with the specified configuration.
pub async fn create_pool(config: &DatabaseConfig) -> Result<PgPool> {
    // Validate configuration
    validate_config(config)?;

    let pool_options = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(config.connect_timeout())
        .test_before_acquire(true);

    let pool_options = if let Some(idle_timeout) = config.idle_timeout() {
        pool_options.idle_timeout(idle_timeout)
    } else {
        pool_options
    };

    let connect_options =
        PgConnectOptions::from_str(&config.url).map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Invalid PostgreSQL connection string: {}", sanitize_url(&config.url)),
        })?;

    let pool = pool_options.connect_with(connect_options).await.map_err(|e| {
        tracing::error!(
            error = %e,
            url = %sanitize_url(&config.url),
            "Failed to create PostgreSQL database pool"
        );
        FlowplaneError::Database {
            source: e,
            context: format!("Failed to connect to database: {}", sanitize_url(&config.url)),
        }
    })?;

    tracing::info!(
        database_type = "postgresql",
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
        return Err(FlowplaneError::validation("max_connections must be greater than 0"));
    }

    if config.min_connections > config.max_connections {
        return Err(FlowplaneError::validation(
            "min_connections cannot be greater than max_connections",
        ));
    }

    if config.url.is_empty() {
        return Err(FlowplaneError::validation("database URL cannot be empty"));
    }

    // Validate URL format - PostgreSQL only
    if !config.url.starts_with("postgresql://") && !config.url.starts_with("postgres://") {
        return Err(FlowplaneError::validation(
            "database URL must start with 'postgresql://' or 'postgres://'",
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
pub fn get_pool_stats(pool: &PgPool) -> PoolStats {
    PoolStats { size: pool.size(), idle: pool.num_idle() }
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
            url: "postgresql://localhost/test".to_string(),
            max_connections: 10,
            min_connections: 2,
            ..Default::default()
        };

        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_config_postgres_scheme() {
        let config = DatabaseConfig {
            url: "postgres://localhost/test".to_string(),
            max_connections: 10,
            min_connections: 2,
            ..Default::default()
        };

        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_config_invalid_max_connections() {
        let config = DatabaseConfig {
            url: "postgresql://localhost/test".to_string(),
            max_connections: 0,
            ..Default::default()
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_invalid_min_max() {
        let config = DatabaseConfig {
            url: "postgresql://localhost/test".to_string(),
            max_connections: 5,
            min_connections: 10,
            ..Default::default()
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_empty_url() {
        let config = DatabaseConfig { url: "".to_string(), ..Default::default() };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_invalid_url_scheme() {
        let config = DatabaseConfig { url: "sqlite://./test.db".to_string(), ..Default::default() };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_config_mysql_rejected() {
        let config =
            DatabaseConfig { url: "mysql://localhost/test".to_string(), ..Default::default() };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_sanitize_url() {
        assert_eq!(
            sanitize_url("postgresql://user:pass@localhost/db"),
            "postgresql://***:***@localhost/db"
        );

        assert_eq!(sanitize_url("postgresql://localhost/test"), "postgresql://localhost/test");

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
}
