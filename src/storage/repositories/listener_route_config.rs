//! Listener Route Config Repository
//!
//! This module provides operations for the explicit listener-route config relationship.
//! This replaces runtime JSON parsing with direct database lookups.

use crate::domain::{ListenerId, RouteConfigId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;

/// Internal database row structure for listener_route_configs.
#[derive(Debug, Clone, FromRow)]
struct ListenerRouteConfigRow {
    pub listener_id: String,
    pub route_config_id: String,
    pub route_order: i64,
    pub created_at: DateTime<Utc>,
}

/// Listener-route config relationship data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerRouteConfigData {
    pub listener_id: ListenerId,
    pub route_config_id: RouteConfigId,
    pub route_order: i64,
    pub created_at: DateTime<Utc>,
}

impl From<ListenerRouteConfigRow> for ListenerRouteConfigData {
    fn from(row: ListenerRouteConfigRow) -> Self {
        Self {
            listener_id: ListenerId::from_string(row.listener_id),
            route_config_id: RouteConfigId::from_string(row.route_config_id),
            route_order: row.route_order,
            created_at: row.created_at,
        }
    }
}

/// Repository for listener-route config relationship operations.
#[derive(Debug, Clone)]
pub struct ListenerRouteConfigRepository {
    pool: DbPool,
}

impl ListenerRouteConfigRepository {
    /// Creates a new repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a listener-route config relationship.
    #[instrument(skip(self), fields(listener_id = %listener_id, route_config_id = %route_config_id, order = order), name = "db_create_listener_route_config")]
    pub async fn create(
        &self,
        listener_id: &ListenerId,
        route_config_id: &RouteConfigId,
        order: i64,
    ) -> Result<ListenerRouteConfigData> {
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO listener_route_configs (listener_id, route_config_id, route_order, created_at) \
             VALUES ($1, $2, $3, $4)"
        )
        .bind(listener_id.as_str())
        .bind(route_config_id.as_str())
        .bind(order)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, route_config_id = %route_config_id, "Failed to create listener-route config relationship");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to link listener '{}' to route config '{}'", listener_id, route_config_id),
            }
        })?;

        tracing::info!(
            listener_id = %listener_id,
            route_config_id = %route_config_id,
            order = order,
            "Created listener-route config relationship"
        );

        Ok(ListenerRouteConfigData {
            listener_id: listener_id.clone(),
            route_config_id: route_config_id.clone(),
            route_order: order,
            created_at: now,
        })
    }

    /// Delete a specific listener-route config relationship.
    #[instrument(skip(self), fields(listener_id = %listener_id, route_config_id = %route_config_id), name = "db_delete_listener_route_config")]
    pub async fn delete(
        &self,
        listener_id: &ListenerId,
        route_config_id: &RouteConfigId,
    ) -> Result<()> {
        let result = sqlx::query(
            "DELETE FROM listener_route_configs WHERE listener_id = $1 AND route_config_id = $2"
        )
        .bind(listener_id.as_str())
        .bind(route_config_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, route_config_id = %route_config_id, "Failed to delete listener-route config relationship");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to unlink listener '{}' from route config '{}'", listener_id, route_config_id),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found(
                "ListenerRouteConfigRelationship",
                format!("{}:{}", listener_id, route_config_id),
            ));
        }

        tracing::info!(
            listener_id = %listener_id,
            route_config_id = %route_config_id,
            "Deleted listener-route config relationship"
        );

        Ok(())
    }

    /// Delete all route config relationships for a listener.
    /// Used during listener sync to clear old relationships.
    #[instrument(skip(self), fields(listener_id = %listener_id), name = "db_delete_listener_route_configs_by_listener")]
    pub async fn delete_by_listener(&self, listener_id: &ListenerId) -> Result<u64> {
        let result = sqlx::query("DELETE FROM listener_route_configs WHERE listener_id = $1")
            .bind(listener_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, listener_id = %listener_id, "Failed to delete listener route configs");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete route configs for listener '{}'", listener_id),
                }
            })?;

        let deleted = result.rows_affected();
        if deleted > 0 {
            tracing::info!(listener_id = %listener_id, deleted = deleted, "Deleted listener-route config relationships");
        }

        Ok(deleted)
    }

    /// List all route configs for a listener.
    #[instrument(skip(self), fields(listener_id = %listener_id), name = "db_list_route_configs_by_listener")]
    pub async fn list_by_listener(
        &self,
        listener_id: &ListenerId,
    ) -> Result<Vec<ListenerRouteConfigData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, ListenerRouteConfigRow>(
            "SELECT listener_id, route_config_id, route_order, created_at \
             FROM listener_route_configs WHERE listener_id = $1 ORDER BY route_order ASC"
        )
        .bind(listener_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, "Failed to list listener route configs");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list route configs for listener '{}'", listener_id),
            }
        })?;

        Ok(rows.into_iter().map(ListenerRouteConfigData::from).collect())
    }

    /// List all route config IDs for a listener (lightweight query).
    #[instrument(skip(self), fields(listener_id = %listener_id), name = "db_list_route_config_ids_by_listener")]
    pub async fn list_route_config_ids_by_listener(
        &self,
        listener_id: &ListenerId,
    ) -> Result<Vec<RouteConfigId>> {
        let route_config_ids: Vec<String> = sqlx::query_scalar(
            "SELECT route_config_id FROM listener_route_configs WHERE listener_id = $1 ORDER BY route_order ASC",
        )
        .bind(listener_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, "Failed to list route config IDs");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list route config IDs for listener '{}'", listener_id),
            }
        })?;

        Ok(route_config_ids.into_iter().map(RouteConfigId::from_string).collect())
    }

    /// List all listeners using a specific route config.
    #[instrument(skip(self), fields(route_config_id = %route_config_id), name = "db_list_listeners_by_route_config")]
    pub async fn list_by_route_config(
        &self,
        route_config_id: &RouteConfigId,
    ) -> Result<Vec<ListenerRouteConfigData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, ListenerRouteConfigRow>(
            "SELECT listener_id, route_config_id, route_order, created_at \
             FROM listener_route_configs WHERE route_config_id = $1"
        )
        .bind(route_config_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %route_config_id, "Failed to list listeners using route config");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list listeners for route config '{}'", route_config_id),
            }
        })?;

        Ok(rows.into_iter().map(ListenerRouteConfigData::from).collect())
    }

    /// List all listener IDs using a specific route config (lightweight query).
    #[instrument(skip(self), fields(route_config_id = %route_config_id), name = "db_list_listener_ids_by_route_config")]
    pub async fn list_listener_ids_by_route_config(
        &self,
        route_config_id: &RouteConfigId,
    ) -> Result<Vec<ListenerId>> {
        let listener_ids: Vec<String> = sqlx::query_scalar(
            "SELECT listener_id FROM listener_route_configs WHERE route_config_id = $1",
        )
        .bind(route_config_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %route_config_id, "Failed to list listener IDs");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list listener IDs for route config '{}'", route_config_id),
            }
        })?;

        Ok(listener_ids.into_iter().map(ListenerId::from_string).collect())
    }

    /// Check if a listener-route config relationship exists.
    #[instrument(skip(self), fields(listener_id = %listener_id, route_config_id = %route_config_id), name = "db_exists_listener_route_config")]
    pub async fn exists(
        &self,
        listener_id: &ListenerId,
        route_config_id: &RouteConfigId,
    ) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM listener_route_configs WHERE listener_id = $1 AND route_config_id = $2"
        )
        .bind(listener_id.as_str())
        .bind(route_config_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, route_config_id = %route_config_id, "Failed to check listener-route config existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check relationship for listener '{}'", listener_id),
            }
        })?;

        Ok(count > 0)
    }
}
