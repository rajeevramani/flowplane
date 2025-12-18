//! Route Filter Repository
//!
//! This module provides operations for attaching filters to routes.
//! Filters attached at the route level apply only to that specific route,
//! overriding filters attached at the virtual host or route config level.

use crate::domain::{FilterId, RouteId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

/// Internal database row structure for route_filters.
#[derive(Debug, Clone, FromRow)]
struct RouteFilterRow {
    pub route_id: String,
    pub filter_id: String,
    pub filter_order: i32,
    pub created_at: DateTime<Utc>,
    pub settings: Option<String>,
}

/// Route filter attachment data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteFilterData {
    pub route_id: RouteId,
    pub filter_id: FilterId,
    pub filter_order: i32,
    pub created_at: DateTime<Utc>,
    /// Per-scope settings (behavior, config overrides, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

impl From<RouteFilterRow> for RouteFilterData {
    fn from(row: RouteFilterRow) -> Self {
        Self {
            route_id: RouteId::from_string(row.route_id),
            filter_id: FilterId::from_string(row.filter_id),
            filter_order: row.filter_order,
            created_at: row.created_at,
            settings: row.settings.and_then(|s| serde_json::from_str(&s).ok()),
        }
    }
}

/// Repository for route filter attachment operations.
#[derive(Debug, Clone)]
pub struct RouteFilterRepository {
    pool: DbPool,
}

impl RouteFilterRepository {
    /// Creates a new repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Attach a filter to a route.
    #[instrument(skip(self, settings), fields(route_id = %route_id, filter_id = %filter_id, order = order), name = "db_attach_filter_to_route")]
    pub async fn attach(
        &self,
        route_id: &RouteId,
        filter_id: &FilterId,
        order: i32,
        settings: Option<serde_json::Value>,
    ) -> Result<RouteFilterData> {
        let now = Utc::now();
        let settings_json = settings.as_ref().map(|s| s.to_string());

        sqlx::query(
            "INSERT INTO route_filters (route_id, filter_id, filter_order, created_at, settings) \
             VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(route_id.as_str())
        .bind(filter_id.as_str())
        .bind(order)
        .bind(now)
        .bind(&settings_json)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            // Check for unique constraint violations and provide helpful error messages
            if let Some(db_err) = e.as_database_error() {
                if let Some(code) = db_err.code() {
                    // SQLite UNIQUE constraint violation code is 2067
                    if code.as_ref() == "2067" {
                        let msg = db_err.message();
                        // Check which constraint was violated
                        if msg.contains("filter_order") {
                            tracing::warn!(
                                route_id = %route_id,
                                filter_id = %filter_id,
                                order = order,
                                "Filter order already in use on route"
                            );
                            return FlowplaneError::Conflict {
                                message: format!(
                                    "Filter order {} is already in use on this route. Choose a different order value.",
                                    order
                                ),
                                resource_type: "route_filter".to_string(),
                            };
                        } else if msg.contains("filter_id") {
                            tracing::warn!(
                                route_id = %route_id,
                                filter_id = %filter_id,
                                "Filter already attached to route"
                            );
                            return FlowplaneError::Conflict {
                                message: "This filter is already attached to the route.".to_string(),
                                resource_type: "route_filter".to_string(),
                            };
                        }
                    }
                }
            }
            tracing::error!(error = %e, route_id = %route_id, filter_id = %filter_id, "Failed to attach filter to route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to attach filter '{}' to route '{}'", filter_id, route_id),
            }
        })?;

        tracing::info!(
            route_id = %route_id,
            filter_id = %filter_id,
            order = order,
            "Attached filter to route"
        );

        Ok(RouteFilterData {
            route_id: route_id.clone(),
            filter_id: filter_id.clone(),
            filter_order: order,
            created_at: now,
            settings,
        })
    }

    /// Detach a filter from a route.
    #[instrument(skip(self), fields(route_id = %route_id, filter_id = %filter_id), name = "db_detach_filter_from_route")]
    pub async fn detach(&self, route_id: &RouteId, filter_id: &FilterId) -> Result<()> {
        let result = sqlx::query(
            "DELETE FROM route_filters WHERE route_id = $1 AND filter_id = $2"
        )
        .bind(route_id.as_str())
        .bind(filter_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %route_id, filter_id = %filter_id, "Failed to detach filter from route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to detach filter '{}' from route '{}'", filter_id, route_id),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found(
                "RouteFilterAttachment",
                format!("{}:{}", route_id, filter_id),
            ));
        }

        tracing::info!(
            route_id = %route_id,
            filter_id = %filter_id,
            "Detached filter from route"
        );

        Ok(())
    }

    /// List all filter attachments for a route.
    #[instrument(skip(self), fields(route_id = %route_id), name = "db_list_route_filters")]
    pub async fn list_by_route(&self, route_id: &RouteId) -> Result<Vec<RouteFilterData>> {
        let rows = sqlx::query_as::<Sqlite, RouteFilterRow>(
            "SELECT route_id, filter_id, filter_order, created_at, settings \
             FROM route_filters WHERE route_id = $1 ORDER BY filter_order ASC",
        )
        .bind(route_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %route_id, "Failed to list route filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list filters for route '{}'", route_id),
            }
        })?;

        Ok(rows.into_iter().map(RouteFilterData::from).collect())
    }

    /// List all routes using a specific filter.
    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_list_filter_routes")]
    pub async fn list_by_filter(&self, filter_id: &FilterId) -> Result<Vec<RouteId>> {
        let route_ids: Vec<String> = sqlx::query_scalar(
            "SELECT route_id FROM route_filters WHERE filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to list routes using filter");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list routes for filter '{}'", filter_id),
            }
        })?;

        Ok(route_ids.into_iter().map(RouteId::from_string).collect())
    }

    /// Check if a filter is attached to a route.
    #[instrument(skip(self), fields(route_id = %route_id, filter_id = %filter_id), name = "db_exists_route_filter")]
    pub async fn exists(&self, route_id: &RouteId, filter_id: &FilterId) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM route_filters WHERE route_id = $1 AND filter_id = $2"
        )
        .bind(route_id.as_str())
        .bind(filter_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %route_id, filter_id = %filter_id, "Failed to check filter attachment");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check filter attachment for route '{}'", route_id),
            }
        })?;

        Ok(count > 0)
    }

    /// Get the next available order for a route.
    #[instrument(skip(self), fields(route_id = %route_id), name = "db_next_route_filter_order")]
    pub async fn get_next_order(&self, route_id: &RouteId) -> Result<i32> {
        let max_order: Option<i32> = sqlx::query_scalar(
            "SELECT MAX(filter_order) FROM route_filters WHERE route_id = $1",
        )
        .bind(route_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %route_id, "Failed to get max filter order");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get max filter order for route '{}'", route_id),
            }
        })?;

        Ok(max_order.unwrap_or(0) + 1)
    }

    /// Count total attachments for a filter (used to prevent deletion of attached filters).
    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_count_route_filter_attachments")]
    pub async fn count_by_filter(&self, filter_id: &FilterId) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM route_filters WHERE filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to count filter attachments");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count attachments for filter '{}'", filter_id),
            }
        })?;

        Ok(count)
    }

    /// Count filters attached to a route.
    #[instrument(skip(self), fields(route_id = %route_id), name = "db_count_filters_by_route")]
    pub async fn count_by_route(&self, route_id: &RouteId) -> Result<i64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM route_filters WHERE route_id = $1")
                .bind(route_id.as_str())
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, route_id = %route_id, "Failed to count filters");
                    FlowplaneError::Database {
                        source: e,
                        context: format!("Failed to count filters for route '{}'", route_id),
                    }
                })?;

        Ok(count)
    }
}
