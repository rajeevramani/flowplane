//! Listener auto-filter repository for tracking auto-added HTTP filters
//!
//! This module tracks HTTP filters that are automatically added to listeners
//! when filter resources are attached to routes. It enables reference counting
//! so filters are only removed when no routes need them.

use crate::domain::{FilterId, ListenerId, RouteId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;
use uuid::Uuid;

/// Internal database row structure for listener_auto_filters.
#[derive(Debug, Clone, FromRow)]
struct ListenerAutoFilterRow {
    pub id: String,
    pub listener_id: String,
    pub http_filter_name: String,
    pub source_filter_id: String,
    pub source_route_id: String,
    pub created_at: DateTime<Utc>,
}

/// Listener auto-filter tracking data.
///
/// Represents a record tracking that an HTTP filter was auto-added to a listener
/// because a filter resource was attached to a route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerAutoFilterData {
    pub id: String,
    pub listener_id: ListenerId,
    pub http_filter_name: String,
    pub source_filter_id: FilterId,
    pub source_route_id: RouteId,
    pub created_at: DateTime<Utc>,
}

impl From<ListenerAutoFilterRow> for ListenerAutoFilterData {
    fn from(row: ListenerAutoFilterRow) -> Self {
        Self {
            id: row.id,
            listener_id: ListenerId::from_string(row.listener_id),
            http_filter_name: row.http_filter_name,
            source_filter_id: FilterId::from_string(row.source_filter_id),
            source_route_id: RouteId::from_string(row.source_route_id),
            created_at: row.created_at,
        }
    }
}

/// Repository for tracking auto-added listener HTTP filters.
///
/// This repository enables automatic listener filter chain management:
/// - When a filter is attached to a route, track the auto-added HTTP filter
/// - When a filter is detached, check if any other routes still need the HTTP filter
/// - Remove the HTTP filter from the listener only when no routes need it
#[derive(Debug, Clone)]
pub struct ListenerAutoFilterRepository {
    pool: DbPool,
}

impl ListenerAutoFilterRepository {
    /// Creates a new repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new auto-filter tracking record.
    ///
    /// This is called when a filter is attached to a route and the corresponding
    /// HTTP filter is added to a listener.
    #[instrument(skip(self), fields(listener_id = %listener_id, http_filter_name = %http_filter_name), name = "db_create_listener_auto_filter")]
    pub async fn create(
        &self,
        listener_id: &ListenerId,
        http_filter_name: &str,
        source_filter_id: &FilterId,
        source_route_id: &RouteId,
    ) -> Result<ListenerAutoFilterData> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO listener_auto_filters (id, listener_id, http_filter_name, source_filter_id, source_route_id, created_at) VALUES ($1, $2, $3, $4, $5, $6)"
        )
        .bind(&id)
        .bind(listener_id)
        .bind(http_filter_name)
        .bind(source_filter_id)
        .bind(source_route_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, "Failed to create listener auto-filter tracking record");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create auto-filter tracking for listener '{}'", listener_id),
            }
        })?;

        tracing::info!(
            listener_id = %listener_id,
            http_filter_name = %http_filter_name,
            source_filter_id = %source_filter_id,
            source_route_id = %source_route_id,
            "Created listener auto-filter tracking record"
        );

        Ok(ListenerAutoFilterData {
            id,
            listener_id: listener_id.clone(),
            http_filter_name: http_filter_name.to_string(),
            source_filter_id: source_filter_id.clone(),
            source_route_id: source_route_id.clone(),
            created_at: now,
        })
    }

    /// Check if a tracking record already exists (for idempotency).
    #[instrument(skip(self), fields(listener_id = %listener_id, http_filter_name = %http_filter_name), name = "db_exists_listener_auto_filter")]
    pub async fn exists(
        &self,
        listener_id: &ListenerId,
        http_filter_name: &str,
        source_filter_id: &FilterId,
        source_route_id: &RouteId,
    ) -> Result<bool> {
        let count = sqlx::query_scalar::<Sqlite, i64>(
            "SELECT COUNT(*) FROM listener_auto_filters WHERE listener_id = $1 AND http_filter_name = $2 AND source_filter_id = $3 AND source_route_id = $4"
        )
        .bind(listener_id)
        .bind(http_filter_name)
        .bind(source_filter_id)
        .bind(source_route_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, "Failed to check listener auto-filter existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check auto-filter existence for listener '{}'", listener_id),
            }
        })?;

        Ok(count > 0)
    }

    /// Get all tracking records for a specific filter/route source.
    ///
    /// This is used when detaching a filter from a route to find which listeners
    /// were affected.
    #[instrument(skip(self), fields(source_filter_id = %source_filter_id, source_route_id = %source_route_id), name = "db_get_listener_auto_filters_by_source")]
    pub async fn get_by_source(
        &self,
        source_filter_id: &FilterId,
        source_route_id: &RouteId,
    ) -> Result<Vec<ListenerAutoFilterData>> {
        let rows = sqlx::query_as::<Sqlite, ListenerAutoFilterRow>(
            "SELECT id, listener_id, http_filter_name, source_filter_id, source_route_id, created_at FROM listener_auto_filters WHERE source_filter_id = $1 AND source_route_id = $2"
        )
        .bind(source_filter_id)
        .bind(source_route_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, source_filter_id = %source_filter_id, source_route_id = %source_route_id, "Failed to get listener auto-filters by source");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get auto-filters for filter '{}' and route '{}'", source_filter_id, source_route_id),
            }
        })?;

        Ok(rows.into_iter().map(ListenerAutoFilterData::from).collect())
    }

    /// Delete all tracking records for a specific filter/route source.
    ///
    /// This is called when a filter is detached from a route.
    #[instrument(skip(self), fields(source_filter_id = %source_filter_id, source_route_id = %source_route_id), name = "db_delete_listener_auto_filters_by_source")]
    pub async fn delete_by_source(
        &self,
        source_filter_id: &FilterId,
        source_route_id: &RouteId,
    ) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM listener_auto_filters WHERE source_filter_id = $1 AND source_route_id = $2"
        )
        .bind(source_filter_id)
        .bind(source_route_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, source_filter_id = %source_filter_id, source_route_id = %source_route_id, "Failed to delete listener auto-filters by source");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to delete auto-filters for filter '{}' and route '{}'", source_filter_id, source_route_id),
            }
        })?;

        let deleted = result.rows_affected();
        if deleted > 0 {
            tracing::info!(
                source_filter_id = %source_filter_id,
                source_route_id = %source_route_id,
                deleted_count = deleted,
                "Deleted listener auto-filter tracking records"
            );
        }

        Ok(deleted)
    }

    /// Count how many tracking records exist for a listener and HTTP filter name.
    ///
    /// This is used to determine if an HTTP filter should be removed from a listener.
    /// If count is 0, no routes need the filter and it can be removed.
    #[instrument(skip(self), fields(listener_id = %listener_id, http_filter_name = %http_filter_name), name = "db_count_listener_auto_filters")]
    pub async fn count_by_listener_and_http_filter(
        &self,
        listener_id: &ListenerId,
        http_filter_name: &str,
    ) -> Result<i64> {
        let count = sqlx::query_scalar::<Sqlite, i64>(
            "SELECT COUNT(*) FROM listener_auto_filters WHERE listener_id = $1 AND http_filter_name = $2"
        )
        .bind(listener_id)
        .bind(http_filter_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, http_filter_name = %http_filter_name, "Failed to count listener auto-filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count auto-filters for listener '{}' and filter '{}'", listener_id, http_filter_name),
            }
        })?;

        Ok(count)
    }

    /// List all auto-filter records for a listener.
    ///
    /// Useful for debugging and inspection.
    #[instrument(skip(self), fields(listener_id = %listener_id), name = "db_list_listener_auto_filters")]
    pub async fn list_by_listener(
        &self,
        listener_id: &ListenerId,
    ) -> Result<Vec<ListenerAutoFilterData>> {
        let rows = sqlx::query_as::<Sqlite, ListenerAutoFilterRow>(
            "SELECT id, listener_id, http_filter_name, source_filter_id, source_route_id, created_at FROM listener_auto_filters WHERE listener_id = $1 ORDER BY created_at ASC"
        )
        .bind(listener_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, "Failed to list listener auto-filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list auto-filters for listener '{}'", listener_id),
            }
        })?;

        Ok(rows.into_iter().map(ListenerAutoFilterData::from).collect())
    }
}
