//! # Storage and Persistence
//!
//! This module provides database connectivity and persistence layer for the
//! Magaya control plane configuration data.

pub mod pool;

pub use pool::{create_pool, DbPool};

use crate::config::DatabaseConfig;
use crate::errors::{MagayaError, Result};
use sqlx::{Any, Pool};

/// Type alias for the database connection pool
pub type DbPool = Pool<Any>;

/// Create a database connection pool based on configuration
pub async fn create_pool(config: &DatabaseConfig) -> Result<DbPool> {
    let pool_options = sqlx::any::AnyPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(config.connect_timeout());

    let pool_options = if let Some(idle_timeout) = config.idle_timeout() {
        pool_options.idle_timeout(idle_timeout)
    } else {
        pool_options
    };

    let pool = pool_options
        .connect(&config.url)
        .await
        .map_err(|e| MagayaError::Database {
            source: e,
            context: format!("Failed to create database pool for URL: {}", config.url),
        })?;

    // Run migrations if enabled
    if config.auto_migrate {
        run_migrations(&pool).await?;
    }

    tracing::info!(
        database_type = if config.is_sqlite() { "sqlite" } else { "postgresql" },
        max_connections = config.max_connections,
        url_prefix = if config.url.len() > 20 { &config.url[..20] } else { &config.url },
        "Database connection pool created successfully"
    );

    Ok(pool)
}

/// Run database migrations
pub async fn run_migrations(pool: &DbPool) -> Result<()> {
    // For now, this is a placeholder for future migration implementation
    // In a full implementation, you would use sqlx::migrate! macro or
    // implement a custom migration system

    tracing::info!("Database migrations completed (placeholder implementation)");
    Ok(())
}

/// Check database connectivity
pub async fn check_connection(pool: &DbPool) -> Result<()> {
    sqlx::query("SELECT 1")
        .fetch_one(pool)
        .await
        .map_err(|e| MagayaError::Database {
            source: e,
            context: "Database connectivity check failed".to_string(),
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_sqlite_pool() {
        let config = DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            max_connections: 5,
            auto_migrate: false,
            ..Default::default()
        };

        let pool = create_pool(&config).await.unwrap();
        assert_eq!(pool.size(), 0); // No connections created yet

        // Test connectivity
        check_connection(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn test_invalid_database_url() {
        let config = DatabaseConfig {
            url: "invalid://url".to_string(),
            ..Default::default()
        };

        let result = create_pool(&config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_migrations() {
        let config = DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        };

        let pool = create_pool(&config).await.unwrap();
        let result = run_migrations(&pool).await;
        assert!(result.is_ok());
    }
}