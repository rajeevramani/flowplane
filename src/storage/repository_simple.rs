//! # Repository Pattern for Data Access
//!
//! Provides repository implementations using runtime queries with structured types
//! for development velocity while maintaining type safety.

use crate::errors::{FlowplaneError, Result};
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

/// Database row structure for listeners
#[derive(Debug, Clone, FromRow)]
struct ListenerRow {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Listener configuration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerData {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ListenerRow> for ListenerData {
    fn from(row: ListenerRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            address: row.address,
            port: row.port,
            protocol: row.protocol,
            configuration: row.configuration,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
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

/// Database row structure for routes
#[derive(Debug, Clone, FromRow)]
struct RouteRow {
    pub id: String,
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Route configuration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteData {
    pub id: String,
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<RouteRow> for RouteData {
    fn from(row: RouteRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            path_prefix: row.path_prefix,
            cluster_name: row.cluster_name,
            configuration: row.configuration,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Create route request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRouteRequest {
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: serde_json::Value,
}

/// Update route request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRouteRequest {
    pub path_prefix: Option<String>,
    pub cluster_name: Option<String>,
    pub configuration: Option<serde_json::Value>,
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

/// Create listener request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateListenerRequest {
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: Option<String>,
    pub configuration: serde_json::Value,
}

/// Update listener request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateListenerRequest {
    pub address: Option<String>,
    pub port: Option<Option<i64>>,
    pub protocol: Option<String>,
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
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid configuration JSON: {}", e))
        })?;
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
    pub async fn get_by_id(&self, id: &str) -> Result<ClusterData> {
        let row = sqlx::query_as::<Sqlite, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, created_at, updated_at FROM clusters WHERE id = $1"
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
            None => Err(FlowplaneError::not_found(format!(
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
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get cluster with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(ClusterData::from(row)),
            None => Err(FlowplaneError::not_found(format!(
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
            FlowplaneError::Database {
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
                FlowplaneError::validation(format!("Invalid configuration JSON: {}", e))
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
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update cluster with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!(
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
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete cluster with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!(
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
                FlowplaneError::Database {
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

/// Repository for listener data access
#[derive(Debug, Clone)]
pub struct ListenerRepository {
    pool: DbPool,
}

impl ListenerRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, request: CreateListenerRequest) -> Result<ListenerData> {
        let id = Uuid::new_v4().to_string();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid listener configuration JSON: {}", e))
        })?;
        let now = chrono::Utc::now();
        let protocol = request.protocol.unwrap_or_else(|| "HTTP".to_string());

        let result = sqlx::query(
            "INSERT INTO listeners (id, name, address, port, protocol, configuration, version, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, 1, $7, $8)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.address)
        .bind(request.port)
        .bind(&protocol)
        .bind(&configuration_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_name = %request.name, "Failed to create listener");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create listener '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create listener"));
        }

        tracing::info!(listener_id = %id, listener_name = %request.name, "Created new listener");

        self.get_by_id(&id).await
    }

    pub async fn get_by_id(&self, id: &str) -> Result<ListenerData> {
        let row = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, created_at, updated_at FROM listeners WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %id, "Failed to get listener by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get listener with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(ListenerData::from(row)),
            None => Err(FlowplaneError::not_found(format!(
                "Listener with ID '{}' not found",
                id
            ))),
        }
    }

    pub async fn get_by_name(&self, name: &str) -> Result<ListenerData> {
        let row = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, created_at, updated_at FROM listeners WHERE name = $1 ORDER BY version DESC LIMIT 1"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_name = %name, "Failed to get listener by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get listener with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(ListenerData::from(row)),
            None => Err(FlowplaneError::not_found(format!(
                "Listener with name '{}' not found",
                name
            ))),
        }
    }

    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<ListenerData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, created_at, updated_at FROM listeners ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list listeners");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list listeners".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(ListenerData::from).collect())
    }

    pub async fn update(&self, id: &str, request: UpdateListenerRequest) -> Result<ListenerData> {
        let current = self.get_by_id(id).await?;

        let current_address = current.address.clone();
        let current_protocol = current.protocol.clone();
        let current_configuration = current.configuration.clone();
        let current_name = current.name.clone();

        let new_address = request.address.unwrap_or(current_address);
        let new_port = match request.port {
            Some(value) => value,
            None => current.port,
        };
        let new_protocol = request.protocol.unwrap_or(current_protocol);
        let new_configuration = if let Some(config) = request.configuration {
            serde_json::to_string(&config).map_err(|e| {
                FlowplaneError::validation(format!("Invalid listener configuration JSON: {}", e))
            })?
        } else {
            current_configuration
        };

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE listeners SET address = $1, port = $2, protocol = $3, configuration = $4, version = $5, updated_at = $6 WHERE id = $7"
        )
        .bind(&new_address)
        .bind(new_port)
        .bind(&new_protocol)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %id, "Failed to update listener");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update listener with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!(
                "Listener with ID '{}' not found",
                id
            )));
        }

        tracing::info!(listener_id = %id, listener_name = %current_name, new_version = new_version, "Updated listener");

        self.get_by_id(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let listener = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM listeners WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, listener_id = %id, "Failed to delete listener");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete listener with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!(
                "Listener with ID '{}' not found",
                id
            )));
        }

        tracing::info!(listener_id = %id, listener_name = %listener.name, "Deleted listener");

        Ok(())
    }

    pub async fn exists_by_name(&self, name: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<Sqlite, i64>("SELECT COUNT(*) FROM listeners WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, listener_name = %name, "Failed to check listener existence");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to check existence of listener '{}'", name),
                }
            })?;

        Ok(count > 0)
    }

    pub async fn count(&self) -> Result<i64> {
        let count = sqlx::query_scalar::<Sqlite, i64>("SELECT COUNT(*) FROM listeners")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get listener count");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to get listener count".to_string(),
                }
            })?;

        Ok(count)
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}

/// Repository for route data access
#[derive(Debug, Clone)]
pub struct RouteRepository {
    pool: DbPool,
}

impl RouteRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, request: CreateRouteRequest) -> Result<RouteData> {
        let id = Uuid::new_v4().to_string();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid route configuration JSON: {}", e))
        })?;
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "INSERT INTO routes (id, name, path_prefix, cluster_name, configuration, version, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, 1, $6, $7)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.path_prefix)
        .bind(&request.cluster_name)
        .bind(&configuration_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_name = %request.name, "Failed to create route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create route '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create route"));
        }

        tracing::info!(route_id = %id, route_name = %request.name, "Created new route");

        self.get_by_id(&id).await
    }

    pub async fn get_by_id(&self, id: &str) -> Result<RouteData> {
        let row = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, created_at, updated_at FROM routes WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %id, "Failed to get route by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get route with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(RouteData::from(row)),
            None => Err(FlowplaneError::not_found(format!(
                "Route with ID '{}' not found",
                id
            ))),
        }
    }

    pub async fn get_by_name(&self, name: &str) -> Result<RouteData> {
        let row = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, created_at, updated_at FROM routes WHERE name = $1 ORDER BY version DESC LIMIT 1"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_name = %name, "Failed to get route by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get route with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(RouteData::from(row)),
            None => Err(FlowplaneError::not_found(format!(
                "Route with name '{}' not found",
                name
            ))),
        }
    }

    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<RouteData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, created_at, updated_at FROM routes ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list routes");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list routes".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(RouteData::from).collect())
    }

    pub async fn update(&self, id: &str, request: UpdateRouteRequest) -> Result<RouteData> {
        let current = self.get_by_id(id).await?;

        let new_path_prefix = request.path_prefix.unwrap_or(current.path_prefix);
        let new_cluster_name = request.cluster_name.unwrap_or(current.cluster_name);
        let new_configuration = if let Some(config) = request.configuration {
            serde_json::to_string(&config).map_err(|e| {
                FlowplaneError::validation(format!("Invalid route configuration JSON: {}", e))
            })?
        } else {
            current.configuration
        };

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE routes SET path_prefix = $1, cluster_name = $2, configuration = $3, version = $4, updated_at = $5 WHERE id = $6"
        )
        .bind(&new_path_prefix)
        .bind(&new_cluster_name)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %id, "Failed to update route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update route with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!(
                "Route with ID '{}' not found",
                id
            )));
        }

        tracing::info!(route_id = %id, route_name = %current.name, new_version = new_version, "Updated route");

        self.get_by_id(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let route = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM routes WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, route_id = %id, "Failed to delete route");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete route with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!(
                "Route with ID '{}' not found",
                id
            )));
        }

        tracing::info!(route_id = %id, route_name = %route.name, "Deleted route");

        Ok(())
    }

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

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS routes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                path_prefix TEXT NOT NULL,
                cluster_name TEXT NOT NULL,
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

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS listeners (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                address TEXT NOT NULL,
                port INTEGER,
                protocol TEXT NOT NULL DEFAULT 'HTTP',
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

    #[tokio::test]
    async fn test_route_crud_operations() {
        let pool = create_test_pool().await;
        let repo = RouteRepository::new(pool.clone());

        let create_request = CreateRouteRequest {
            name: "test_route".to_string(),
            path_prefix: "/api".to_string(),
            cluster_name: "cluster-a".to_string(),
            configuration: serde_json::json!({
                "name": "test_route",
                "virtualHosts": [
                    {
                        "name": "default",
                        "domains": ["*"],
                        "routes": [
                            {
                                "name": "api",
                                "match": {
                                    "path": { "Prefix": "/api" }
                                },
                                "action": {
                                    "Cluster": {
                                        "name": "cluster-a"
                                    }
                                }
                            }
                        ]
                    }
                ]
            }),
        };

        let created = repo.create(create_request).await.unwrap();
        assert_eq!(created.name, "test_route");
        assert_eq!(created.version, 1);

        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);

        let fetched_by_name = repo.get_by_name("test_route").await.unwrap();
        assert_eq!(fetched_by_name.id, created.id);

        let update_request = UpdateRouteRequest {
            path_prefix: Some("/api/v2".to_string()),
            cluster_name: Some("cluster-b".to_string()),
            configuration: Some(serde_json::json!({
                "name": "test_route",
                "virtualHosts": [
                    {
                        "name": "default",
                        "domains": ["*"],
                        "routes": [
                            {
                                "name": "api",
                                "match": {
                                    "path": { "Prefix": "/api/v2" }
                                },
                                "action": {
                                    "WeightedClusters": {
                                        "clusters": [
                                            {"name": "cluster-b", "weight": 70},
                                            {"name": "cluster-c", "weight": 30}
                                        ]
                                    }
                                }
                            }
                        ]
                    }
                ]
            })),
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.path_prefix, "/api/v2");
        assert_eq!(updated.cluster_name, "cluster-b");

        let listed = repo.list(None, None).await.unwrap();
        assert_eq!(listed.len(), 1);

        repo.delete(&created.id).await.unwrap();
        assert!(repo.get_by_id(&created.id).await.is_err());
    }

    #[tokio::test]
    async fn test_listener_crud_operations() {
        let pool = create_test_pool().await;
        let repo = ListenerRepository::new(pool);

        let create_request = CreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8080),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({
                "name": "test-listener",
                "address": "0.0.0.0",
                "port": 8080
            }),
        };

        let created = repo.create(create_request).await.unwrap();
        assert_eq!(created.name, "test-listener");
        assert_eq!(created.port, Some(8080));
        assert_eq!(created.protocol, "HTTP");
        assert_eq!(created.version, 1);

        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);

        let fetched_by_name = repo.get_by_name("test-listener").await.unwrap();
        assert_eq!(fetched_by_name.id, created.id);

        let update_request = UpdateListenerRequest {
            address: Some("127.0.0.1".to_string()),
            port: Some(Some(9090)),
            protocol: Some("TCP".to_string()),
            configuration: Some(serde_json::json!({
                "name": "test-listener",
                "address": "127.0.0.1",
                "port": 9090
            })),
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.address, "127.0.0.1");
        assert_eq!(updated.port, Some(9090));
        assert_eq!(updated.protocol, "TCP");
        assert_eq!(updated.version, 2);

        let listeners = repo.list(None, None).await.unwrap();
        assert_eq!(listeners.len(), 1);

        assert!(repo.exists_by_name("test-listener").await.unwrap());
        assert!(!repo.exists_by_name("missing").await.unwrap());

        assert_eq!(repo.count().await.unwrap(), 1);

        repo.delete(&created.id).await.unwrap();
        assert!(repo.get_by_id(&created.id).await.is_err());
        assert_eq!(repo.count().await.unwrap(), 0);
    }
}
