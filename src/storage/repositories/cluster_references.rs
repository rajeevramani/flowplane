//! Cluster References repository for tracking cross-import cluster deduplication
//!
//! This module manages the many-to-many relationship between clusters and imports,
//! enabling efficient cluster sharing across multiple OpenAPI imports.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

/// Database row structure for cluster_references
#[derive(Debug, Clone, FromRow)]
struct ClusterReferenceRow {
    pub cluster_id: String,
    pub import_id: String,
    pub route_count: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Cluster reference data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterReferenceData {
    pub cluster_id: String,
    pub import_id: String,
    pub route_count: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<ClusterReferenceRow> for ClusterReferenceData {
    fn from(row: ClusterReferenceRow) -> Self {
        Self {
            cluster_id: row.cluster_id,
            import_id: row.import_id,
            route_count: row.route_count,
            created_at: row.created_at,
        }
    }
}

/// Repository for cluster references data access
#[derive(Debug, Clone)]
pub struct ClusterReferencesRepository {
    pool: DbPool,
}

impl ClusterReferencesRepository {
    /// Create a new cluster references repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Add or increment reference count for a cluster-import pair
    #[instrument(skip(self), name = "db_add_cluster_reference")]
    pub async fn add_reference(&self, cluster_id: &str, import_id: &str, count: i64) -> Result<()> {
        let now = chrono::Utc::now();

        // Use INSERT OR REPLACE to handle both new refs and increments
        sqlx::query(
            "INSERT INTO cluster_references (cluster_id, import_id, route_count, created_at)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT(cluster_id, import_id)
             DO UPDATE SET route_count = route_count + $3",
        )
        .bind(cluster_id)
        .bind(import_id)
        .bind(count)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| FlowplaneError::database(e, "Failed to add cluster reference".to_string()))?;

        Ok(())
    }

    /// Decrement reference count for a cluster-import pair
    #[instrument(skip(self), name = "db_decrement_cluster_reference")]
    pub async fn decrement_reference(
        &self,
        cluster_id: &str,
        import_id: &str,
        count: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE cluster_references
             SET route_count = route_count - $1
             WHERE cluster_id = $2 AND import_id = $3",
        )
        .bind(count)
        .bind(cluster_id)
        .bind(import_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            FlowplaneError::database(e, "Failed to decrement cluster reference".to_string())
        })?;

        Ok(())
    }

    /// Delete all references for an import (called when deleting an import)
    #[instrument(skip(self), name = "db_delete_import_references")]
    pub async fn delete_by_import(&self, import_id: &str) -> Result<Vec<String>> {
        // First, get all cluster IDs that will become orphaned (route_count will be 0)
        let orphaned_clusters = sqlx::query_scalar::<_, String>(
            "SELECT cluster_id FROM cluster_references
             WHERE import_id = $1
             AND route_count <= (
                 SELECT route_count FROM cluster_references cr2
                 WHERE cr2.cluster_id = cluster_references.cluster_id AND cr2.import_id = $1
             )
             AND (SELECT COUNT(*) FROM cluster_references cr3
                  WHERE cr3.cluster_id = cluster_references.cluster_id) = 1",
        )
        .bind(import_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::database(e, "Failed to find orphaned clusters".to_string()))?;

        // Delete all references for this import (CASCADE will handle cleanup)
        sqlx::query("DELETE FROM cluster_references WHERE import_id = $1")
            .bind(import_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                FlowplaneError::database(e, "Failed to delete cluster references".to_string())
            })?;

        Ok(orphaned_clusters)
    }

    /// Get all references for a specific cluster
    #[instrument(skip(self), name = "db_get_cluster_references")]
    pub async fn get_by_cluster(&self, cluster_id: &str) -> Result<Vec<ClusterReferenceData>> {
        let rows = sqlx::query_as::<Sqlite, ClusterReferenceRow>(
            "SELECT cluster_id, import_id, route_count, created_at
             FROM cluster_references WHERE cluster_id = $1",
        )
        .bind(cluster_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            FlowplaneError::database(e, "Failed to fetch cluster references".to_string())
        })?;

        Ok(rows.into_iter().map(ClusterReferenceData::from).collect())
    }

    /// Get all references for a specific import
    #[instrument(skip(self), name = "db_get_import_references")]
    pub async fn get_by_import(&self, import_id: &str) -> Result<Vec<ClusterReferenceData>> {
        let rows = sqlx::query_as::<Sqlite, ClusterReferenceRow>(
            "SELECT cluster_id, import_id, route_count, created_at
             FROM cluster_references WHERE import_id = $1",
        )
        .bind(import_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            FlowplaneError::database(e, "Failed to fetch import references".to_string())
        })?;

        Ok(rows.into_iter().map(ClusterReferenceData::from).collect())
    }

    /// Check if a cluster is referenced by any imports
    #[instrument(skip(self), name = "db_is_cluster_referenced")]
    pub async fn is_cluster_referenced(&self, cluster_id: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM cluster_references WHERE cluster_id = $1",
        )
        .bind(cluster_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            FlowplaneError::database(e, "Failed to check cluster references".to_string())
        })?;

        Ok(count > 0)
    }
}
