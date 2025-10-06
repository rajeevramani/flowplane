//! Route repository for managing route configurations
//!
//! This module provides CRUD operations for route resources, handling storage,
//! retrieval, and lifecycle management of route configuration data.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use uuid::Uuid;

/// Database row structure for routes
#[derive(Debug, Clone, FromRow)]
struct RouteRow {
    pub id: String,
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: String,
    pub version: i64,
    pub source: String,
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
    pub source: String,
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
            source: row.source,
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

/// Repository for route configuration persistence
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
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, created_at, updated_at FROM routes WHERE id = $1"
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
            None => Err(FlowplaneError::not_found(format!("Route with ID '{}' not found", id))),
        }
    }

    pub async fn get_by_name(&self, name: &str) -> Result<RouteData> {
        let row = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, created_at, updated_at FROM routes WHERE name = $1 ORDER BY version DESC LIMIT 1"
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
            None => Err(FlowplaneError::not_found(format!("Route with name '{}' not found", name))),
        }
    }

    pub async fn exists_by_name(&self, name: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<Sqlite, i64>(
            "SELECT COUNT(*) FROM routes WHERE name = $1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_name = %name, "Failed to check route existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check existence of route '{}'", name),
            }
        })?;

        Ok(count > 0)
    }

    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<RouteData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, created_at, updated_at FROM routes ORDER BY created_at DESC LIMIT $1 OFFSET $2"
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
            return Err(FlowplaneError::not_found(format!("Route with ID '{}' not found", id)));
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
            return Err(FlowplaneError::not_found(format!("Route with ID '{}' not found", id)));
        }

        tracing::info!(route_id = %id, route_name = %route.name, "Deleted route");

        Ok(())
    }

    pub async fn delete_by_name(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM routes WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, route_name = %name, "Failed to delete route by name");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete route '{}'", name),
                }
            })?;

        Ok(())
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}
