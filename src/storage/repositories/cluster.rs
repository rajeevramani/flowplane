//! Cluster repository for managing cluster configurations
//!
//! This module provides CRUD operations for cluster resources, handling storage,
//! retrieval, and lifecycle management of cluster configuration data.

use crate::domain::ClusterId;
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;

/// Database row structure for clusters
#[derive(Debug, Clone, FromRow)]
struct ClusterRow {
    pub id: String,
    pub name: String,
    pub service_name: String,
    pub configuration: String,
    pub version: i64,
    pub source: String,
    pub team: Option<String>,
    pub import_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Cluster configuration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterData {
    pub id: ClusterId,
    pub name: String,
    pub service_name: String,
    pub configuration: String, // JSON serialized
    pub version: i64,
    pub source: String,
    pub team: Option<String>,
    pub import_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ClusterRow> for ClusterData {
    fn from(row: ClusterRow) -> Self {
        Self {
            id: ClusterId::from_string(row.id),
            name: row.name,
            service_name: row.service_name,
            configuration: row.configuration,
            version: row.version,
            source: row.source,
            team: row.team,
            import_id: row.import_id,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl crate::api::handlers::TeamOwned for ClusterData {
    fn team(&self) -> Option<&str> {
        self.team.as_deref()
    }

    fn resource_name(&self) -> &str {
        &self.name
    }

    fn resource_type() -> &'static str {
        "Cluster"
    }

    fn resource_type_metric() -> &'static str {
        "clusters"
    }
}

/// Create cluster request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateClusterRequest {
    pub name: String,
    pub service_name: String,
    pub configuration: serde_json::Value,
    pub team: Option<String>,
    pub import_id: Option<String>,
}

/// Update cluster request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateClusterRequest {
    pub service_name: Option<String>,
    pub configuration: Option<serde_json::Value>,
    pub team: Option<Option<String>>,
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
    #[instrument(skip(self, request), fields(cluster_name = %request.name), name = "db_create_cluster")]
    pub async fn create(&self, request: CreateClusterRequest) -> Result<ClusterData> {
        let id = ClusterId::new();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid configuration JSON: {}", e))
        })?;
        let now = chrono::Utc::now();

        // Use parameterized query with positional parameters
        let result = sqlx::query(
            "INSERT INTO clusters (id, name, service_name, configuration, version, team, import_id, created_at, updated_at) VALUES ($1, $2, $3, $4, 1, $5, $6, $7, $8)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.service_name)
        .bind(&configuration_json)
        .bind(&request.team)
        .bind(&request.import_id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_name = %request.name, "Failed to create cluster");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create cluster '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create cluster"));
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
    #[instrument(skip(self), fields(cluster_id = %id), name = "db_get_cluster_by_id")]
    pub async fn get_by_id(&self, id: &ClusterId) -> Result<ClusterData> {
        let row = sqlx::query_as::<sqlx::Postgres, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, source, team, import_id, created_at, updated_at FROM clusters WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %id, "Failed to get cluster by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get cluster with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(ClusterData::from(row)),
            None => {
                Err(FlowplaneError::not_found_msg(format!("Cluster with ID '{}' not found", id)))
            }
        }
    }

    /// Get cluster by name
    #[instrument(skip(self), fields(cluster_name = %name), name = "db_get_cluster_by_name")]
    pub async fn get_by_name(&self, name: &str) -> Result<ClusterData> {
        let row = sqlx::query_as::<sqlx::Postgres, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, source, team, import_id, created_at, updated_at FROM clusters WHERE name = $1 ORDER BY version DESC LIMIT 1"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_name = %name, "Failed to get cluster by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get cluster with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(ClusterData::from(row)),
            None => Err(FlowplaneError::not_found_msg(format!(
                "Cluster with name '{}' not found",
                name
            ))),
        }
    }

    /// List all clusters
    #[instrument(skip(self), name = "db_list_clusters")]
    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<ClusterData>> {
        let limit = limit.unwrap_or(100).min(1000); // Max 1000 results
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<sqlx::Postgres, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, source, team, import_id, created_at, updated_at FROM clusters ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list clusters");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list clusters".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(ClusterData::from).collect())
    }

    /// List clusters filtered by team names (for team-scoped tokens)
    /// If teams list is empty, returns all clusters (for admin:all or resource-level scopes)
    ///
    /// # Security Note
    ///
    /// Empty teams array returns ALL resources. This is intentional for admin:all
    /// scope but could be a security issue if authorization logic has bugs.
    /// A warning is logged when this occurs for auditing purposes.
    #[instrument(skip(self), fields(teams = ?teams, limit = ?limit, offset = ?offset), name = "db_list_clusters_by_teams")]
    pub async fn list_by_teams(
        &self,
        teams: &[String],
        _include_default: bool, // Deprecated: always includes default resources
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<ClusterData>> {
        // SECURITY: Empty teams array returns ALL resources (admin scope).
        // Log warning for audit trail - this should only happen for admin:all scope.
        if teams.is_empty() {
            tracing::warn!(
                resource = "clusters",
                "list_by_teams called with empty teams array - returning all resources (admin scope)"
            );
            return self.list(limit, offset).await;
        }

        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        // Build the query with IN clause for team filtering
        // Use positional parameters for the limit and offset,
        // and bind team names dynamically
        let placeholders = teams
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        // Always include NULL team clusters (default resources)
        let where_clause = format!("WHERE team IN ({}) OR team IS NULL", placeholders);

        let query_str = format!(
            "SELECT id, name, service_name, configuration, version, source, team, import_id, created_at, updated_at \
             FROM clusters \
             {} \
             ORDER BY created_at DESC \
             LIMIT ${} OFFSET ${}",
            where_clause,
            teams.len() + 1,
            teams.len() + 2
        );

        let mut query = sqlx::query_as::<sqlx::Postgres, ClusterRow>(&query_str);

        // Bind team names
        for team in teams {
            query = query.bind(team);
        }

        // Bind limit and offset
        query = query.bind(limit).bind(offset);

        let rows = query.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to list clusters by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list clusters for teams: {:?}", teams),
            }
        })?;

        Ok(rows.into_iter().map(ClusterData::from).collect())
    }

    /// List only default/shared clusters (team IS NULL)
    ///
    /// Used for Allowlist scope where clients should only see shared infrastructure,
    /// not team-specific resources.
    #[instrument(skip(self), fields(limit = ?limit, offset = ?offset), name = "db_list_default_clusters")]
    pub async fn list_default_only(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<ClusterData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<sqlx::Postgres, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, source, team, import_id, created_at, updated_at \
             FROM clusters \
             WHERE team IS NULL \
             ORDER BY created_at DESC \
             LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list default clusters");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list default clusters".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(ClusterData::from).collect())
    }

    /// Update cluster
    #[instrument(skip(self, request), fields(cluster_id = %id), name = "db_update_cluster")]
    pub async fn update(
        &self,
        id: &ClusterId,
        request: UpdateClusterRequest,
    ) -> Result<ClusterData> {
        // Get current cluster to check if it exists and get current values
        let current = self.get_by_id(id).await?;

        let new_service_name = request.service_name.unwrap_or(current.service_name);
        let new_configuration = if let Some(config) = request.configuration {
            serde_json::to_string(&config).map_err(|e| {
                FlowplaneError::validation(format!("Invalid configuration JSON: {}", e))
            })?
        } else {
            current.configuration
        };
        let new_team = request.team.unwrap_or(current.team);

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE clusters SET service_name = $1, configuration = $2, version = $3, team = $4, updated_at = $5 WHERE id = $6"
        )
        .bind(&new_service_name)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(&new_team)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %id, "Failed to update cluster");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update cluster with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
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
    #[instrument(skip(self), fields(cluster_id = %id), name = "db_delete_cluster")]
    pub async fn delete(&self, id: &ClusterId) -> Result<()> {
        // Check if cluster exists first
        let cluster = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM clusters WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, cluster_id = %id, "Failed to delete cluster");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete cluster with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
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
    #[instrument(skip(self), fields(cluster_name = %name), name = "db_exists_cluster_by_name")]
    pub async fn exists_by_name(&self, name: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clusters WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, cluster_name = %name, "Failed to check cluster existence");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to check existence of cluster '{}'", name),
                }
            })?;

        Ok(count > 0)
    }

    #[instrument(skip(self), fields(cluster_name = %name), name = "db_delete_cluster_by_name")]
    pub async fn delete_by_name(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM clusters WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, cluster_name = %name, "Failed to delete cluster by name");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete cluster '{}'", name),
                }
            })?;

        Ok(())
    }

    /// Get cluster count
    #[instrument(skip(self), name = "db_count_clusters")]
    pub async fn count(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clusters")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get cluster count");
                FlowplaneError::Database {
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
