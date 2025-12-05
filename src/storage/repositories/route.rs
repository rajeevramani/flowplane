//! Route Repository
//!
//! This module provides CRUD operations for routes extracted from virtual hosts.
//! Routes are synchronized when route configs are created/updated.

use crate::domain::{RouteId, RouteMatchType, VirtualHostId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

/// Internal database row structure for routes.
#[derive(Debug, Clone, FromRow)]
struct RouteRow {
    pub id: String,
    pub virtual_host_id: String,
    pub name: String,
    pub path_pattern: String,
    pub match_type: String,
    pub rule_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Route data returned from the repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteData {
    pub id: RouteId,
    pub virtual_host_id: VirtualHostId,
    pub name: String,
    pub path_pattern: String,
    pub match_type: RouteMatchType,
    pub rule_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<RouteRow> for RouteData {
    type Error = FlowplaneError;

    fn try_from(row: RouteRow) -> Result<Self> {
        let match_type: RouteMatchType = row.match_type.parse().map_err(|e: String| {
            FlowplaneError::internal(format!("Failed to parse match type: {}", e))
        })?;

        Ok(Self {
            id: RouteId::from_string(row.id),
            virtual_host_id: VirtualHostId::from_string(row.virtual_host_id),
            name: row.name,
            path_pattern: row.path_pattern,
            match_type,
            rule_order: row.rule_order,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Request to create a new route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRouteRequest {
    pub virtual_host_id: VirtualHostId,
    pub name: String,
    pub path_pattern: String,
    pub match_type: RouteMatchType,
    pub rule_order: i32,
}

/// Request to update a route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRouteRequest {
    pub path_pattern: Option<String>,
    pub match_type: Option<RouteMatchType>,
    pub rule_order: Option<i32>,
}

/// Repository for route operations.
#[derive(Debug, Clone)]
pub struct RouteRepository {
    pool: DbPool,
}

impl RouteRepository {
    /// Creates a new repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new route.
    #[instrument(skip(self, request), fields(vh_id = %request.virtual_host_id, name = %request.name), name = "db_create_route")]
    pub async fn create(&self, request: CreateRouteRequest) -> Result<RouteData> {
        let id = RouteId::new();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO routes (id, virtual_host_id, name, path_pattern, match_type, rule_order, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(id.as_str())
        .bind(request.virtual_host_id.as_str())
        .bind(&request.name)
        .bind(&request.path_pattern)
        .bind(request.match_type.as_str())
        .bind(request.rule_order)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, vh_id = %request.virtual_host_id, name = %request.name, "Failed to create route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create route '{}' for virtual host '{}'", request.name, request.virtual_host_id),
            }
        })?;

        tracing::info!(
            id = %id,
            vh_id = %request.virtual_host_id,
            name = %request.name,
            "Created route"
        );

        Ok(RouteData {
            id,
            virtual_host_id: request.virtual_host_id,
            name: request.name,
            path_pattern: request.path_pattern,
            match_type: request.match_type,
            rule_order: request.rule_order,
            created_at: now,
            updated_at: now,
        })
    }

    /// Get a route by ID.
    #[instrument(skip(self), fields(id = %id), name = "db_get_route_by_id")]
    pub async fn get_by_id(&self, id: &RouteId) -> Result<RouteData> {
        let row = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, virtual_host_id, name, path_pattern, match_type, rule_order, created_at, updated_at \
             FROM routes WHERE id = $1"
        )
        .bind(id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => FlowplaneError::not_found("Route", id.as_str()),
            _ => {
                tracing::error!(error = %e, id = %id, "Failed to get route by ID");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to get route by ID: {}", id),
                }
            }
        })?;

        RouteData::try_from(row)
    }

    /// Get a route by virtual host ID and name.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id, name = %name), name = "db_get_route_by_name")]
    pub async fn get_by_vh_and_name(
        &self,
        virtual_host_id: &VirtualHostId,
        name: &str,
    ) -> Result<RouteData> {
        let row = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, virtual_host_id, name, path_pattern, match_type, rule_order, created_at, updated_at \
             FROM routes WHERE virtual_host_id = $1 AND name = $2"
        )
        .bind(virtual_host_id.as_str())
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                FlowplaneError::not_found("Route", format!("{}:{}", virtual_host_id, name))
            }
            _ => {
                tracing::error!(error = %e, vh_id = %virtual_host_id, name = %name, "Failed to get route by name");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to get route '{}' for virtual host '{}'", name, virtual_host_id),
                }
            }
        })?;

        RouteData::try_from(row)
    }

    /// List all routes for a virtual host.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id), name = "db_list_routes_by_vh")]
    pub async fn list_by_virtual_host(
        &self,
        virtual_host_id: &VirtualHostId,
    ) -> Result<Vec<RouteData>> {
        let rows = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, virtual_host_id, name, path_pattern, match_type, rule_order, created_at, updated_at \
             FROM routes WHERE virtual_host_id = $1 ORDER BY rule_order ASC"
        )
        .bind(virtual_host_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, vh_id = %virtual_host_id, "Failed to list routes");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list routes for virtual host '{}'", virtual_host_id),
            }
        })?;

        rows.into_iter().map(RouteData::try_from).collect()
    }

    /// Update a route.
    #[instrument(skip(self, request), fields(id = %id), name = "db_update_route")]
    pub async fn update(&self, id: &RouteId, request: UpdateRouteRequest) -> Result<RouteData> {
        let current = self.get_by_id(id).await?;
        let now = Utc::now();

        let new_path_pattern = request.path_pattern.unwrap_or(current.path_pattern);
        let new_match_type = request.match_type.unwrap_or(current.match_type);
        let new_rule_order = request.rule_order.unwrap_or(current.rule_order);

        let result = sqlx::query(
            "UPDATE routes SET path_pattern = $1, match_type = $2, rule_order = $3, updated_at = $4 WHERE id = $5"
        )
        .bind(&new_path_pattern)
        .bind(new_match_type.as_str())
        .bind(new_rule_order)
        .bind(now)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, id = %id, "Failed to update route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update route '{}'", id),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("Route", id.as_str()));
        }

        self.get_by_id(id).await
    }

    /// Delete a route by ID.
    #[instrument(skip(self), fields(id = %id), name = "db_delete_route")]
    pub async fn delete(&self, id: &RouteId) -> Result<()> {
        let result = sqlx::query("DELETE FROM routes WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, id = %id, "Failed to delete route");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete route '{}'", id),
                }
            })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("Route", id.as_str()));
        }

        tracing::info!(id = %id, "Deleted route");
        Ok(())
    }

    /// Delete all routes for a virtual host.
    /// Used during sync to clear old data before re-populating.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id), name = "db_delete_routes_by_vh")]
    pub async fn delete_by_virtual_host(&self, virtual_host_id: &VirtualHostId) -> Result<u64> {
        let result = sqlx::query("DELETE FROM routes WHERE virtual_host_id = $1")
            .bind(virtual_host_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, vh_id = %virtual_host_id, "Failed to delete routes by virtual host");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete routes for virtual host '{}'", virtual_host_id),
                }
            })?;

        let deleted = result.rows_affected();
        if deleted > 0 {
            tracing::info!(vh_id = %virtual_host_id, deleted = deleted, "Deleted routes");
        }

        Ok(deleted)
    }

    /// Check if a route exists by virtual host and name.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id, name = %name), name = "db_exists_route")]
    pub async fn exists(&self, virtual_host_id: &VirtualHostId, name: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM routes WHERE virtual_host_id = $1 AND name = $2"
        )
        .bind(virtual_host_id.as_str())
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, vh_id = %virtual_host_id, name = %name, "Failed to check route existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check route existence for virtual host '{}'", virtual_host_id),
            }
        })?;

        Ok(count > 0)
    }
}
