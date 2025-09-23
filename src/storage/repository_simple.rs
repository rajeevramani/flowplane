//! # Repository Pattern for Data Access
//!
//! Provides repository implementations using runtime queries with structured types
//! for development velocity while maintaining type safety.

use crate::errors::{MagayaError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use uuid::Uuid;

/// Database row structure for clusters
#[derive(Debug, Clone, FromRow)]
struct ClusterRow {
    pub id: String,
    pub name: String,
    pub service_name: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Cluster configuration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterData {
    pub id: String,
    pub name: String,
    pub service_name: String,
    pub configuration: String, // JSON serialized
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ClusterRow> for ClusterData {
    fn from(row: ClusterRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            service_name: row.service_name,
            configuration: row.configuration,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Create cluster request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateClusterRequest {
    pub name: String,
    pub service_name: String,
    pub configuration: serde_json::Value,
}

/// Update cluster request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateClusterRequest {
    pub service_name: Option<String>,
    pub configuration: Option<serde_json::Value>,
}

/// Repository for cluster data access (simplified version)
#[derive(Debug, Clone)]
pub struct ClusterRepository {
    pool: DbPool,
}

impl ClusterRepository {
    /// Create a new cluster repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new cluster
    pub async fn create(&self, request: CreateClusterRequest) -> Result<ClusterData> {
        let id = Uuid::new_v4().to_string();
        let configuration_json = serde_json::to_string(&request.configuration)
            .map_err(|e| MagayaError::validation(format!("Invalid configuration JSON: {}", e)))?;
        let now = chrono::Utc::now();

        // Use parameterized query with positional parameters (works with both SQLite and PostgreSQL)
        let result = sqlx::query(
            "INSERT INTO clusters (id, name, service_name, configuration, version, created_at, updated_at) VALUES ($1, $2, $3, $4, 1, $5, $6)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.service_name)
        .bind(&configuration_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_name = %request.name, "Failed to create cluster");
            MagayaError::Database {
                source: e,
                context: format!("Failed to create cluster '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(MagayaError::validation("Failed to create cluster"));
        }

        tracing::info!(
            cluster_id = %id,
            cluster_name = %request.name,
            service_name = %request.service_name,
            "Created new cluster"
        );

        // Return the created cluster
        self.get_by_id(&id).await
    }

    /// Get cluster by ID
    pub async fn get_by_id(&self, id: &str) -> Result<ClusterData> {
        let row = sqlx::query_as::<Sqlite, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, created_at, updated_at FROM clusters WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %id, "Failed to get cluster by ID");
            MagayaError::Database {
                source: e,
                context: format!("Failed to get cluster with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(ClusterData::from(row)),
            None => Err(MagayaError::not_found(format!(
                "Cluster with ID '{}' not found",
                id
            ))),
        }
    }

    /// Get cluster by name
    pub async fn get_by_name(&self, name: &str) -> Result<ClusterData> {
        let row = sqlx::query_as::<Sqlite, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, created_at, updated_at FROM clusters WHERE name = $1 ORDER BY version DESC LIMIT 1"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_name = %name, "Failed to get cluster by name");
            MagayaError::Database {
                source: e,
                context: format!("Failed to get cluster with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(ClusterData::from(row)),
            None => Err(MagayaError::not_found(format!(
                "Cluster with name '{}' not found",
                name
            ))),
        }
    }

    /// List all clusters
    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<ClusterData>> {
        let limit = limit.unwrap_or(100).min(1000); // Max 1000 results
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, created_at, updated_at FROM clusters ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list clusters");
            MagayaError::Database {
                source: e,
                context: "Failed to list clusters".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(ClusterData::from).collect())
    }

    /// Update cluster
    pub async fn update(&self, id: &str, request: UpdateClusterRequest) -> Result<ClusterData> {
        // Get current cluster to check if it exists and get current values
        let current = self.get_by_id(id).await?;

        let new_service_name = request.service_name.unwrap_or(current.service_name);
        let new_configuration = if let Some(config) = request.configuration {
            serde_json::to_string(&config).map_err(|e| {
                MagayaError::validation(format!("Invalid configuration JSON: {}", e))
            })?
        } else {
            current.configuration
        };

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE clusters SET service_name = $1, configuration = $2, version = $3, updated_at = $4 WHERE id = $5"
        )
        .bind(&new_service_name)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %id, "Failed to update cluster");
            MagayaError::Database {
                source: e,
                context: format!("Failed to update cluster with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(MagayaError::not_found(format!(
                "Cluster with ID '{}' not found",
                id
            )));
        }

        tracing::info!(
            cluster_id = %id,
            cluster_name = %current.name,
            new_version = new_version,
            "Updated cluster"
        );

        // Return the updated cluster
        self.get_by_id(id).await
    }

    /// Delete cluster
    pub async fn delete(&self, id: &str) -> Result<()> {
        // Check if cluster exists first
        let cluster = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM clusters WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, cluster_id = %id, "Failed to delete cluster");
                MagayaError::Database {
                    source: e,
                    context: format!("Failed to delete cluster with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(MagayaError::not_found(format!(
                "Cluster with ID '{}' not found",
                id
            )));
        }

        tracing::info!(
            cluster_id = %id,
            cluster_name = %cluster.name,
            "Deleted cluster"
        );

        Ok(())
    }

    /// Check if cluster exists by name
    pub async fn exists_by_name(&self, name: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clusters WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, cluster_name = %name, "Failed to check cluster existence");
                MagayaError::Database {
                    source: e,
                    context: format!("Failed to check existence of cluster '{}'", name),
                }
            })?;

        Ok(count > 0)
    }

    /// Get cluster count
    pub async fn count(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clusters")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get cluster count");
                MagayaError::Database {
                    source: e,
                    context: "Failed to get cluster count".to_string(),
                }
            })?;

        Ok(count)
    }

    /// Access the underlying pool (used by background watchers)
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseConfig;
    use crate::storage::create_pool;

    async fn create_test_pool() -> crate::storage::DbPool {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        };
        let pool = create_pool(&config).await.unwrap();

        // Run basic schema creation for testing
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS clusters (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                service_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(name, version)
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    #[tokio::test]
    async fn test_cluster_crud_operations() {
        let pool = create_test_pool().await;
        let repo = ClusterRepository::new(pool);

        // Create a test cluster
        let create_request = CreateClusterRequest {
            name: "test_cluster".to_string(),
            service_name: "test_service".to_string(),
            configuration: serde_json::json!({
                "type": "EDS",
                "endpoints": ["127.0.0.1:8080"]
            }),
        };

        let created = repo.create(create_request).await.unwrap();
        assert_eq!(created.name, "test_cluster");
        assert_eq!(created.service_name, "test_service");
        assert_eq!(created.version, 1);

        // Get by ID
        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, created.name);

        // Get by name
        let fetched_by_name = repo.get_by_name("test_cluster").await.unwrap();
        assert_eq!(fetched_by_name.id, created.id);

        // Update cluster
        let update_request = UpdateClusterRequest {
            service_name: Some("updated_service".to_string()),
            configuration: Some(serde_json::json!({
                "type": "EDS",
                "endpoints": ["127.0.0.1:9090"]
            })),
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.service_name, "updated_service");
        assert_eq!(updated.version, 2);

        // List clusters
        let clusters = repo.list(None, None).await.unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].id, created.id);

        // Check existence
        assert!(repo.exists_by_name("test_cluster").await.unwrap());
        assert!(!repo.exists_by_name("nonexistent").await.unwrap());

        // Get count
        let count = repo.count().await.unwrap();
        assert_eq!(count, 1);

        // Delete cluster
        repo.delete(&created.id).await.unwrap();

        // Verify deletion
        assert!(repo.get_by_id(&created.id).await.is_err());
        let count_after_delete = repo.count().await.unwrap();
        assert_eq!(count_after_delete, 0);
    }

    #[tokio::test]
    async fn test_cluster_not_found() {
        let pool = create_test_pool().await;
        let repo = ClusterRepository::new(pool);

        let result = repo.get_by_id("nonexistent-id").await;
        assert!(result.is_err());

        let result = repo.get_by_name("nonexistent-name").await;
        assert!(result.is_err());
    }
}
