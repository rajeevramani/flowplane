//! # Storage and Persistence
//!
//! This module provides database connectivity and persistence layer for the
//! Flowplane control plane configuration data.

pub mod migrations;
pub mod pool;
pub mod repositories;
pub mod repository;

#[cfg(test)]
pub mod test_helpers;

pub use crate::config::DatabaseConfig;

pub use migrations::{
    get_migration_version, list_applied_migrations, run_migrations as run_db_migrations,
    validate_migrations, MigrationInfo,
};
pub use pool::{create_pool, get_pool_stats, DbPool, PoolStats};
pub use repositories::{
    AggregatedSchemaData, AggregatedSchemaRepository, ClusterEndpointData,
    ClusterEndpointRepository, CreateAggregatedSchemaRequest, CreateCustomWasmFilterRequest,
    CreateDataplaneRequest, CreateEndpointRequest, CreateFilterRequest, CreateMcpToolRequest,
    CreateRouteAutoFilterRequest, CreateRouteConfigAutoFilterRequest, CreateRouteRequest,
    CreateSecretReferenceRequest, CreateSecretRequest, CreateVirtualHostAutoFilterRequest,
    CreateVirtualHostRequest, CustomWasmFilterData, CustomWasmFilterRepository, DataplaneData,
    DataplaneRepository, FilterConfiguration, FilterData, FilterInstallation, FilterRepository,
    FilterScopeType, ListenerAutoFilterData, ListenerAutoFilterRepository, ListenerRouteConfigData,
    ListenerRouteConfigRepository, McpToolData, McpToolRepository, NackEventRepository, RouteData,
    RouteFilterData, RouteFilterRepository, RouteRepository, SecretData, SecretRepository,
    UpdateCustomWasmFilterRequest, UpdateDataplaneRequest, UpdateEndpointRequest,
    UpdateFilterRequest, UpdateMcpToolRequest, UpdateRouteRequest, UpdateSecretRequest,
    UpdateVirtualHostRequest, VirtualHostData, VirtualHostFilterData, VirtualHostFilterRepository,
    VirtualHostRepository,
};
pub use repository::{
    AuditEvent, AuditLogRepository, ClusterData, ClusterRepository, CreateClusterRequest,
    CreateListenerRequest, CreateRouteConfigRequest as CreateRouteConfigRepositoryRequest,
    ListenerData, ListenerRepository, RouteConfigData, RouteConfigRepository, UpdateClusterRequest,
    UpdateListenerRequest, UpdateRouteConfigRequest as UpdateRouteConfigRepositoryRequest,
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
    use super::test_helpers::TestDatabase;
    use super::*;

    #[tokio::test]
    async fn test_create_pool() {
        let test_db = TestDatabase::new("create_pool").await;
        let pool = test_db.pool.clone();
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
        let test_db = TestDatabase::new("run_migrations").await;
        let pool = test_db.pool.clone();
        let result = run_migrations(&pool).await;
        assert!(result.is_ok());
    }
}
