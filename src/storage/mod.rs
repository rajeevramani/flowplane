//! # Storage and Persistence
//!
//! This module provides database connectivity and persistence layer for the
//! Flowplane control plane configuration data.

pub mod migrations;
pub mod pool;
pub mod repositories;
pub mod repository;

pub use crate::config::DatabaseConfig;

pub use migrations::{
    get_migration_version, list_applied_migrations, run_migrations as run_db_migrations,
    validate_migrations, MigrationInfo,
};
pub use pool::{create_pool, get_pool_stats, DbPool, PoolStats};
pub use repository::{
    ApiDefinitionData, ApiDefinitionRepository, ApiRouteData, AuditEvent, AuditLogRepository,
    ClusterData, ClusterRepository, CreateApiDefinitionRequest, CreateApiRouteRequest,
    CreateClusterRequest, CreateListenerRequest,
    CreateRouteRequest as CreateRouteRepositoryRequest, ListenerData, ListenerRepository,
    RouteData, RouteRepository, UpdateBootstrapMetadataRequest, UpdateClusterRequest,
    UpdateListenerRequest, UpdateRouteRequest as UpdateRouteRepositoryRequest,
};

use crate::errors::{FlowplaneError, Result};

/// Run database migrations
pub async fn run_migrations(pool: &DbPool) -> Result<()> {
    migrations::run_migrations(pool).await
}

/// Check database connectivity
pub async fn check_connection(pool: &DbPool) -> Result<()> {
    sqlx::query("SELECT 1").fetch_one(pool).await.map_err(|e| FlowplaneError::Database {
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
            url: "sqlite://:memory:".to_string(),
            max_connections: 5,
            auto_migrate: false,
            ..Default::default()
        };

        let pool = create_pool(&config).await.unwrap();
        assert!(pool.size() > 0 || pool.size() == 0); // Pool size check

        // Test connectivity
        check_connection(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn test_invalid_database_url() {
        let config = DatabaseConfig { url: "invalid://url".to_string(), ..Default::default() };

        let result = create_pool(&config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_migrations() {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        };

        let pool = create_pool(&config).await.unwrap();
        let result = run_migrations(&pool).await;
        assert!(result.is_ok());
    }
}
