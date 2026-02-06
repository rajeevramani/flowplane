//! Cluster Endpoint Repository
//!
//! This module provides operations for cluster endpoints extracted from cluster configuration.
//! Endpoints are synchronized when clusters are created/updated.

use crate::domain::{ClusterId, EndpointHealthStatus, EndpointId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;

/// Internal database row structure for cluster_endpoints.
#[derive(Debug, Clone, FromRow)]
struct ClusterEndpointRow {
    pub id: String,
    pub cluster_id: String,
    pub address: String,
    pub port: i32,
    pub weight: i32,
    pub health_status: String,
    pub priority: i32,
    pub metadata: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Cluster endpoint data returned from the repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterEndpointData {
    pub id: EndpointId,
    pub cluster_id: ClusterId,
    pub address: String,
    pub port: u16,
    pub weight: u32,
    pub health_status: EndpointHealthStatus,
    pub priority: u32,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<ClusterEndpointRow> for ClusterEndpointData {
    type Error = FlowplaneError;

    fn try_from(row: ClusterEndpointRow) -> Result<Self> {
        let health_status: EndpointHealthStatus =
            row.health_status.parse().map_err(|e: String| {
                FlowplaneError::internal(format!("Failed to parse health status: {}", e))
            })?;

        let metadata = if let Some(ref json) = row.metadata {
            Some(serde_json::from_str(json).map_err(|e| {
                FlowplaneError::internal(format!("Failed to parse endpoint metadata: {}", e))
            })?)
        } else {
            None
        };

        Ok(Self {
            id: EndpointId::from_string(row.id),
            cluster_id: ClusterId::from_string(row.cluster_id),
            address: row.address,
            port: row.port as u16,
            weight: row.weight as u32,
            health_status,
            priority: row.priority as u32,
            metadata,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Request to create a new cluster endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEndpointRequest {
    pub cluster_id: ClusterId,
    pub address: String,
    pub port: u16,
    pub weight: Option<u32>,
    pub priority: Option<u32>,
    pub metadata: Option<serde_json::Value>,
}

/// Request to update a cluster endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateEndpointRequest {
    pub weight: Option<u32>,
    pub health_status: Option<EndpointHealthStatus>,
    pub priority: Option<u32>,
    pub metadata: Option<serde_json::Value>,
}

/// Repository for cluster endpoint operations.
#[derive(Debug, Clone)]
pub struct ClusterEndpointRepository {
    pool: DbPool,
}

impl ClusterEndpointRepository {
    /// Creates a new repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new cluster endpoint.
    #[instrument(skip(self, request), fields(cluster_id = %request.cluster_id, address = %request.address, port = request.port), name = "db_create_endpoint")]
    pub async fn create(&self, request: CreateEndpointRequest) -> Result<ClusterEndpointData> {
        let id = EndpointId::new();
        let now = Utc::now();
        let weight = request.weight.unwrap_or(1);
        let priority = request.priority.unwrap_or(0);
        let metadata_json = request.metadata.as_ref().map(|m| m.to_string());

        sqlx::query(
            "INSERT INTO cluster_endpoints (id, cluster_id, address, port, weight, health_status, priority, metadata, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"
        )
        .bind(id.as_str())
        .bind(request.cluster_id.as_str())
        .bind(&request.address)
        .bind(request.port as i32)
        .bind(weight as i32)
        .bind(EndpointHealthStatus::Unknown.as_str())
        .bind(priority as i32)
        .bind(&metadata_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %request.cluster_id, address = %request.address, "Failed to create endpoint");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create endpoint for cluster '{}'", request.cluster_id),
            }
        })?;

        tracing::info!(
            id = %id,
            cluster_id = %request.cluster_id,
            address = %request.address,
            port = request.port,
            "Created cluster endpoint"
        );

        Ok(ClusterEndpointData {
            id,
            cluster_id: request.cluster_id,
            address: request.address,
            port: request.port,
            weight,
            health_status: EndpointHealthStatus::Unknown,
            priority,
            metadata: request.metadata,
            created_at: now,
            updated_at: now,
        })
    }

    /// Get an endpoint by ID.
    #[instrument(skip(self), fields(id = %id), name = "db_get_endpoint_by_id")]
    pub async fn get_by_id(&self, id: &EndpointId) -> Result<ClusterEndpointData> {
        let row = sqlx::query_as::<sqlx::Postgres, ClusterEndpointRow>(
            "SELECT id, cluster_id, address, port, weight, health_status, priority, metadata, created_at, updated_at \
             FROM cluster_endpoints WHERE id = $1"
        )
        .bind(id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => FlowplaneError::not_found("ClusterEndpoint", id.as_str()),
            _ => {
                tracing::error!(error = %e, id = %id, "Failed to get endpoint by ID");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to get endpoint by ID: {}", id),
                }
            }
        })?;

        ClusterEndpointData::try_from(row)
    }

    /// Get an endpoint by cluster, address and port.
    #[instrument(skip(self), fields(cluster_id = %cluster_id, address = %address, port = port), name = "db_get_endpoint_by_address")]
    pub async fn get_by_address(
        &self,
        cluster_id: &ClusterId,
        address: &str,
        port: u16,
    ) -> Result<ClusterEndpointData> {
        let row = sqlx::query_as::<sqlx::Postgres, ClusterEndpointRow>(
            "SELECT id, cluster_id, address, port, weight, health_status, priority, metadata, created_at, updated_at \
             FROM cluster_endpoints WHERE cluster_id = $1 AND address = $2 AND port = $3"
        )
        .bind(cluster_id.as_str())
        .bind(address)
        .bind(port as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                FlowplaneError::not_found("ClusterEndpoint", format!("{}:{}:{}", cluster_id, address, port))
            }
            _ => {
                tracing::error!(error = %e, cluster_id = %cluster_id, address = %address, "Failed to get endpoint by address");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to get endpoint for cluster '{}'", cluster_id),
                }
            }
        })?;

        ClusterEndpointData::try_from(row)
    }

    /// List all endpoints for a cluster.
    #[instrument(skip(self), fields(cluster_id = %cluster_id), name = "db_list_endpoints_by_cluster")]
    pub async fn list_by_cluster(
        &self,
        cluster_id: &ClusterId,
    ) -> Result<Vec<ClusterEndpointData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, ClusterEndpointRow>(
            "SELECT id, cluster_id, address, port, weight, health_status, priority, metadata, created_at, updated_at \
             FROM cluster_endpoints WHERE cluster_id = $1 ORDER BY priority ASC, address ASC, port ASC"
        )
        .bind(cluster_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %cluster_id, "Failed to list endpoints");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list endpoints for cluster '{}'", cluster_id),
            }
        })?;

        rows.into_iter().map(ClusterEndpointData::try_from).collect()
    }

    /// List healthy endpoints for a cluster.
    #[instrument(skip(self), fields(cluster_id = %cluster_id), name = "db_list_healthy_endpoints")]
    pub async fn list_healthy_by_cluster(
        &self,
        cluster_id: &ClusterId,
    ) -> Result<Vec<ClusterEndpointData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, ClusterEndpointRow>(
            "SELECT id, cluster_id, address, port, weight, health_status, priority, metadata, created_at, updated_at \
             FROM cluster_endpoints WHERE cluster_id = $1 AND health_status IN ('healthy', 'degraded') \
             ORDER BY priority ASC, address ASC, port ASC"
        )
        .bind(cluster_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %cluster_id, "Failed to list healthy endpoints");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list healthy endpoints for cluster '{}'", cluster_id),
            }
        })?;

        rows.into_iter().map(ClusterEndpointData::try_from).collect()
    }

    /// Update an endpoint.
    #[instrument(skip(self, request), fields(id = %id), name = "db_update_endpoint")]
    pub async fn update(
        &self,
        id: &EndpointId,
        request: UpdateEndpointRequest,
    ) -> Result<ClusterEndpointData> {
        let current = self.get_by_id(id).await?;
        let now = Utc::now();

        let new_weight = request.weight.unwrap_or(current.weight);
        let new_health_status = request.health_status.unwrap_or(current.health_status);
        let new_priority = request.priority.unwrap_or(current.priority);
        let new_metadata = request.metadata.or(current.metadata);
        let metadata_json = new_metadata.as_ref().map(|m| m.to_string());

        let result = sqlx::query(
            "UPDATE cluster_endpoints SET weight = $1, health_status = $2, priority = $3, metadata = $4, updated_at = $5 WHERE id = $6"
        )
        .bind(new_weight as i32)
        .bind(new_health_status.as_str())
        .bind(new_priority as i32)
        .bind(&metadata_json)
        .bind(now)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, id = %id, "Failed to update endpoint");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update endpoint '{}'", id),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("ClusterEndpoint", id.as_str()));
        }

        self.get_by_id(id).await
    }

    /// Update health status for an endpoint.
    #[instrument(skip(self), fields(id = %id, status = %status), name = "db_update_endpoint_health")]
    pub async fn update_health_status(
        &self,
        id: &EndpointId,
        status: EndpointHealthStatus,
    ) -> Result<()> {
        let now = Utc::now();

        let result = sqlx::query(
            "UPDATE cluster_endpoints SET health_status = $1, updated_at = $2 WHERE id = $3"
        )
        .bind(status.as_str())
        .bind(now)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, id = %id, status = %status, "Failed to update endpoint health");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update health for endpoint '{}'", id),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("ClusterEndpoint", id.as_str()));
        }

        tracing::debug!(id = %id, status = %status, "Updated endpoint health status");
        Ok(())
    }

    /// Delete an endpoint by ID.
    #[instrument(skip(self), fields(id = %id), name = "db_delete_endpoint")]
    pub async fn delete(&self, id: &EndpointId) -> Result<()> {
        let result = sqlx::query("DELETE FROM cluster_endpoints WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, id = %id, "Failed to delete endpoint");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete endpoint '{}'", id),
                }
            })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("ClusterEndpoint", id.as_str()));
        }

        tracing::info!(id = %id, "Deleted cluster endpoint");
        Ok(())
    }

    /// Delete all endpoints for a cluster.
    /// Used during cluster sync to clear old data.
    #[instrument(skip(self), fields(cluster_id = %cluster_id), name = "db_delete_endpoints_by_cluster")]
    pub async fn delete_by_cluster(&self, cluster_id: &ClusterId) -> Result<u64> {
        let result = sqlx::query("DELETE FROM cluster_endpoints WHERE cluster_id = $1")
            .bind(cluster_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, cluster_id = %cluster_id, "Failed to delete endpoints by cluster");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete endpoints for cluster '{}'", cluster_id),
                }
            })?;

        let deleted = result.rows_affected();
        if deleted > 0 {
            tracing::info!(cluster_id = %cluster_id, deleted = deleted, "Deleted cluster endpoints");
        }

        Ok(deleted)
    }

    /// Check if an endpoint exists by address.
    #[instrument(skip(self), fields(cluster_id = %cluster_id, address = %address, port = port), name = "db_exists_endpoint")]
    pub async fn exists(&self, cluster_id: &ClusterId, address: &str, port: u16) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM cluster_endpoints WHERE cluster_id = $1 AND address = $2 AND port = $3"
        )
        .bind(cluster_id.as_str())
        .bind(address)
        .bind(port as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %cluster_id, address = %address, "Failed to check endpoint existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check endpoint existence for cluster '{}'", cluster_id),
            }
        })?;

        Ok(count > 0)
    }

    /// Count endpoints for a cluster.
    #[instrument(skip(self), fields(cluster_id = %cluster_id), name = "db_count_endpoints")]
    pub async fn count_by_cluster(&self, cluster_id: &ClusterId) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM cluster_endpoints WHERE cluster_id = $1",
        )
        .bind(cluster_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %cluster_id, "Failed to count endpoints");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count endpoints for cluster '{}'", cluster_id),
            }
        })?;

        Ok(count)
    }
}
