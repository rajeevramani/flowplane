//! Listener auto-filter repository for tracking auto-added HTTP filters
//!
//! This module tracks HTTP filters that are automatically added to listeners
//! when filter resources are attached to route configs, virtual hosts, or routes.
//! It enables reference counting so filters are only removed when no attachments need them.
//!
//! The enhanced schema supports hierarchical filter attachment:
//! - RouteConfig level: applies to all vhosts and routes in the route config
//! - VirtualHost level: applies to all routes in that virtual host
//! - Route level: applies to that specific route only

use crate::domain::{AttachmentLevel, FilterId, ListenerId, RouteConfigId, RouteId, VirtualHostId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;
use uuid::Uuid;

/// Internal database row structure for listener_auto_filters.
#[derive(Debug, Clone, FromRow)]
struct ListenerAutoFilterRow {
    pub id: String,
    pub listener_id: String,
    pub http_filter_name: String,
    pub source_filter_id: String,
    pub route_config_id: String,
    pub attachment_level: String,
    pub source_virtual_host_id: Option<String>,
    pub source_route_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Listener auto-filter tracking data.
///
/// Represents a record tracking that an HTTP filter was auto-added to a listener
/// because a filter resource was attached at some level of the route hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerAutoFilterData {
    pub id: String,
    pub listener_id: ListenerId,
    pub http_filter_name: String,
    pub source_filter_id: FilterId,
    pub route_config_id: RouteConfigId,
    pub attachment_level: AttachmentLevel,
    pub source_virtual_host_id: Option<VirtualHostId>,
    pub source_route_id: Option<RouteId>,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<ListenerAutoFilterRow> for ListenerAutoFilterData {
    type Error = FlowplaneError;

    fn try_from(row: ListenerAutoFilterRow) -> Result<Self> {
        let attachment_level: AttachmentLevel =
            row.attachment_level.parse().map_err(|e: String| {
                FlowplaneError::internal(format!("Failed to parse attachment level: {}", e))
            })?;

        Ok(Self {
            id: row.id,
            listener_id: ListenerId::from_string(row.listener_id),
            http_filter_name: row.http_filter_name,
            source_filter_id: FilterId::from_string(row.source_filter_id),
            route_config_id: RouteConfigId::from_string(row.route_config_id),
            attachment_level,
            source_virtual_host_id: row.source_virtual_host_id.map(VirtualHostId::from_string),
            source_route_id: row.source_route_id.map(RouteId::from_string),
            created_at: row.created_at,
        })
    }
}

/// Request to create a route config-level auto-filter tracking record.
#[derive(Debug, Clone)]
pub struct CreateRouteConfigAutoFilterRequest {
    pub listener_id: ListenerId,
    pub http_filter_name: String,
    pub source_filter_id: FilterId,
    pub route_config_id: RouteConfigId,
}

/// Request to create a virtual host-level auto-filter tracking record.
#[derive(Debug, Clone)]
pub struct CreateVirtualHostAutoFilterRequest {
    pub listener_id: ListenerId,
    pub http_filter_name: String,
    pub source_filter_id: FilterId,
    pub route_config_id: RouteConfigId,
    pub source_virtual_host_id: VirtualHostId,
}

/// Request to create a route-level auto-filter tracking record.
#[derive(Debug, Clone)]
pub struct CreateRouteAutoFilterRequest {
    pub listener_id: ListenerId,
    pub http_filter_name: String,
    pub source_filter_id: FilterId,
    pub route_config_id: RouteConfigId,
    pub source_route_id: RouteId,
}

/// Repository for tracking auto-added listener HTTP filters.
///
/// This repository enables automatic listener filter chain management:
/// - When a filter is attached to a route config/vhost/route, track the auto-added HTTP filter
/// - When a filter is detached, check if any other attachments still need the HTTP filter
/// - Remove the HTTP filter from the listener only when no attachments need it
#[derive(Debug, Clone)]
pub struct ListenerAutoFilterRepository {
    pool: DbPool,
}

impl ListenerAutoFilterRepository {
    /// Creates a new repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a route config-level auto-filter tracking record.
    #[instrument(skip(self, request), fields(listener_id = %request.listener_id, http_filter_name = %request.http_filter_name), name = "db_create_route_config_auto_filter")]
    pub async fn create_for_route_config(
        &self,
        request: CreateRouteConfigAutoFilterRequest,
    ) -> Result<ListenerAutoFilterData> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO listener_auto_filters (id, listener_id, http_filter_name, source_filter_id, route_config_id, attachment_level, source_virtual_host_id, source_route_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, NULL, NULL, $7)"
        )
        .bind(&id)
        .bind(request.listener_id.as_str())
        .bind(&request.http_filter_name)
        .bind(request.source_filter_id.as_str())
        .bind(request.route_config_id.as_str())
        .bind(AttachmentLevel::RouteConfig.as_str())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %request.listener_id, "Failed to create route config-level auto-filter");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create auto-filter for listener '{}'", request.listener_id),
            }
        })?;

        tracing::info!(
            listener_id = %request.listener_id,
            http_filter_name = %request.http_filter_name,
            route_config_id = %request.route_config_id,
            level = "route_config",
            "Created route config-level auto-filter tracking record"
        );

        Ok(ListenerAutoFilterData {
            id,
            listener_id: request.listener_id,
            http_filter_name: request.http_filter_name,
            source_filter_id: request.source_filter_id,
            route_config_id: request.route_config_id,
            attachment_level: AttachmentLevel::RouteConfig,
            source_virtual_host_id: None,
            source_route_id: None,
            created_at: now,
        })
    }

    /// Create a virtual host-level auto-filter tracking record.
    #[instrument(skip(self, request), fields(listener_id = %request.listener_id, vh_id = %request.source_virtual_host_id), name = "db_create_vh_auto_filter")]
    pub async fn create_for_virtual_host(
        &self,
        request: CreateVirtualHostAutoFilterRequest,
    ) -> Result<ListenerAutoFilterData> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO listener_auto_filters (id, listener_id, http_filter_name, source_filter_id, route_config_id, attachment_level, source_virtual_host_id, source_route_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, $8)"
        )
        .bind(&id)
        .bind(request.listener_id.as_str())
        .bind(&request.http_filter_name)
        .bind(request.source_filter_id.as_str())
        .bind(request.route_config_id.as_str())
        .bind(AttachmentLevel::VirtualHost.as_str())
        .bind(request.source_virtual_host_id.as_str())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %request.listener_id, vh_id = %request.source_virtual_host_id, "Failed to create VH-level auto-filter");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create auto-filter for listener '{}'", request.listener_id),
            }
        })?;

        tracing::info!(
            listener_id = %request.listener_id,
            http_filter_name = %request.http_filter_name,
            vh_id = %request.source_virtual_host_id,
            level = "virtual_host",
            "Created VH-level auto-filter tracking record"
        );

        Ok(ListenerAutoFilterData {
            id,
            listener_id: request.listener_id,
            http_filter_name: request.http_filter_name,
            source_filter_id: request.source_filter_id,
            route_config_id: request.route_config_id,
            attachment_level: AttachmentLevel::VirtualHost,
            source_virtual_host_id: Some(request.source_virtual_host_id),
            source_route_id: None,
            created_at: now,
        })
    }

    /// Create a route-level auto-filter tracking record.
    #[instrument(skip(self, request), fields(listener_id = %request.listener_id, route_id = %request.source_route_id), name = "db_create_route_auto_filter")]
    pub async fn create_for_route(
        &self,
        request: CreateRouteAutoFilterRequest,
    ) -> Result<ListenerAutoFilterData> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO listener_auto_filters (id, listener_id, http_filter_name, source_filter_id, route_config_id, attachment_level, source_virtual_host_id, source_route_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8)"
        )
        .bind(&id)
        .bind(request.listener_id.as_str())
        .bind(&request.http_filter_name)
        .bind(request.source_filter_id.as_str())
        .bind(request.route_config_id.as_str())
        .bind(AttachmentLevel::Route.as_str())
        .bind(request.source_route_id.as_str())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %request.listener_id, route_id = %request.source_route_id, "Failed to create route-level auto-filter");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create auto-filter for listener '{}'", request.listener_id),
            }
        })?;

        tracing::info!(
            listener_id = %request.listener_id,
            http_filter_name = %request.http_filter_name,
            route_id = %request.source_route_id,
            level = "route",
            "Created route-level auto-filter tracking record"
        );

        Ok(ListenerAutoFilterData {
            id,
            listener_id: request.listener_id,
            http_filter_name: request.http_filter_name,
            source_filter_id: request.source_filter_id,
            route_config_id: request.route_config_id,
            attachment_level: AttachmentLevel::Route,
            source_virtual_host_id: None,
            source_route_id: Some(request.source_route_id),
            created_at: now,
        })
    }

    /// Check if a route config-level tracking record already exists (for idempotency).
    #[instrument(skip(self), fields(listener_id = %listener_id, route_config_id = %route_config_id), name = "db_exists_route_config_auto_filter")]
    pub async fn exists_for_route_config(
        &self,
        listener_id: &ListenerId,
        http_filter_name: &str,
        source_filter_id: &FilterId,
        route_config_id: &RouteConfigId,
    ) -> Result<bool> {
        let count = sqlx::query_scalar::<sqlx::Postgres, i64>(
            "SELECT COUNT(*) FROM listener_auto_filters \
             WHERE listener_id = $1 AND http_filter_name = $2 AND source_filter_id = $3 \
             AND route_config_id = $4 AND attachment_level = 'route_config'"
        )
        .bind(listener_id.as_str())
        .bind(http_filter_name)
        .bind(source_filter_id.as_str())
        .bind(route_config_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, "Failed to check route config auto-filter existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check auto-filter existence for listener '{}'", listener_id),
            }
        })?;

        Ok(count > 0)
    }

    /// Check if a VH-level tracking record already exists.
    #[instrument(skip(self), fields(listener_id = %listener_id, vh_id = %source_virtual_host_id), name = "db_exists_vh_auto_filter")]
    pub async fn exists_for_virtual_host(
        &self,
        listener_id: &ListenerId,
        http_filter_name: &str,
        source_filter_id: &FilterId,
        source_virtual_host_id: &VirtualHostId,
    ) -> Result<bool> {
        let count = sqlx::query_scalar::<sqlx::Postgres, i64>(
            "SELECT COUNT(*) FROM listener_auto_filters \
             WHERE listener_id = $1 AND http_filter_name = $2 AND source_filter_id = $3 \
             AND source_virtual_host_id = $4 AND attachment_level = 'virtual_host'"
        )
        .bind(listener_id.as_str())
        .bind(http_filter_name)
        .bind(source_filter_id.as_str())
        .bind(source_virtual_host_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, vh_id = %source_virtual_host_id, "Failed to check VH auto-filter existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check auto-filter existence for listener '{}'", listener_id),
            }
        })?;

        Ok(count > 0)
    }

    /// Check if a route-level tracking record already exists.
    #[instrument(skip(self), fields(listener_id = %listener_id, route_id = %source_route_id), name = "db_exists_route_auto_filter")]
    pub async fn exists_for_route(
        &self,
        listener_id: &ListenerId,
        http_filter_name: &str,
        source_filter_id: &FilterId,
        source_route_id: &RouteId,
    ) -> Result<bool> {
        let count = sqlx::query_scalar::<sqlx::Postgres, i64>(
            "SELECT COUNT(*) FROM listener_auto_filters \
             WHERE listener_id = $1 AND http_filter_name = $2 AND source_filter_id = $3 \
             AND source_route_id = $4 AND attachment_level = 'route'"
        )
        .bind(listener_id.as_str())
        .bind(http_filter_name)
        .bind(source_filter_id.as_str())
        .bind(source_route_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, route_id = %source_route_id, "Failed to check route auto-filter existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check auto-filter existence for listener '{}'", listener_id),
            }
        })?;

        Ok(count > 0)
    }

    /// Delete route config-level auto-filter records.
    #[instrument(skip(self), fields(source_filter_id = %source_filter_id, route_config_id = %route_config_id), name = "db_delete_route_config_auto_filters")]
    pub async fn delete_for_route_config(
        &self,
        source_filter_id: &FilterId,
        route_config_id: &RouteConfigId,
    ) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM listener_auto_filters \
             WHERE source_filter_id = $1 AND route_config_id = $2 AND attachment_level = 'route_config'"
        )
        .bind(source_filter_id.as_str())
        .bind(route_config_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, source_filter_id = %source_filter_id, route_config_id = %route_config_id, "Failed to delete route config auto-filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to delete auto-filters for route config '{}'", route_config_id),
            }
        })?;

        let deleted = result.rows_affected();
        if deleted > 0 {
            tracing::info!(route_config_id = %route_config_id, deleted = deleted, "Deleted route config-level auto-filter records");
        }

        Ok(deleted)
    }

    /// Delete VH-level auto-filter records.
    #[instrument(skip(self), fields(source_filter_id = %source_filter_id, vh_id = %source_virtual_host_id), name = "db_delete_vh_auto_filters")]
    pub async fn delete_for_virtual_host(
        &self,
        source_filter_id: &FilterId,
        source_virtual_host_id: &VirtualHostId,
    ) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM listener_auto_filters \
             WHERE source_filter_id = $1 AND source_virtual_host_id = $2 AND attachment_level = 'virtual_host'"
        )
        .bind(source_filter_id.as_str())
        .bind(source_virtual_host_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, source_filter_id = %source_filter_id, vh_id = %source_virtual_host_id, "Failed to delete VH auto-filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to delete auto-filters for virtual host '{}'", source_virtual_host_id),
            }
        })?;

        let deleted = result.rows_affected();
        if deleted > 0 {
            tracing::info!(vh_id = %source_virtual_host_id, deleted = deleted, "Deleted VH-level auto-filter records");
        }

        Ok(deleted)
    }

    /// Delete route-level auto-filter records.
    #[instrument(skip(self), fields(source_filter_id = %source_filter_id, route_id = %source_route_id), name = "db_delete_route_auto_filters")]
    pub async fn delete_for_route(
        &self,
        source_filter_id: &FilterId,
        source_route_id: &RouteId,
    ) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM listener_auto_filters \
             WHERE source_filter_id = $1 AND source_route_id = $2 AND attachment_level = 'route'"
        )
        .bind(source_filter_id.as_str())
        .bind(source_route_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, source_filter_id = %source_filter_id, route_id = %source_route_id, "Failed to delete route auto-filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to delete auto-filters for route '{}'", source_route_id),
            }
        })?;

        let deleted = result.rows_affected();
        if deleted > 0 {
            tracing::info!(route_id = %source_route_id, deleted = deleted, "Deleted route-level auto-filter records");
        }

        Ok(deleted)
    }

    /// Count how many tracking records exist for a listener and HTTP filter name.
    ///
    /// This is used to determine if an HTTP filter should be removed from a listener.
    /// If count is 0, no attachments need the filter and it can be removed.
    #[instrument(skip(self), fields(listener_id = %listener_id, http_filter_name = %http_filter_name), name = "db_count_listener_auto_filters")]
    pub async fn count_by_listener_and_http_filter(
        &self,
        listener_id: &ListenerId,
        http_filter_name: &str,
    ) -> Result<i64> {
        let count = sqlx::query_scalar::<sqlx::Postgres, i64>(
            "SELECT COUNT(*) FROM listener_auto_filters WHERE listener_id = $1 AND http_filter_name = $2"
        )
        .bind(listener_id.as_str())
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
    #[instrument(skip(self), fields(listener_id = %listener_id), name = "db_list_listener_auto_filters")]
    pub async fn list_by_listener(
        &self,
        listener_id: &ListenerId,
    ) -> Result<Vec<ListenerAutoFilterData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, ListenerAutoFilterRow>(
            "SELECT id, listener_id, http_filter_name, source_filter_id, route_config_id, attachment_level, \
             source_virtual_host_id, source_route_id, created_at \
             FROM listener_auto_filters WHERE listener_id = $1 ORDER BY created_at ASC"
        )
        .bind(listener_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, "Failed to list listener auto-filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list auto-filters for listener '{}'", listener_id),
            }
        })?;

        rows.into_iter().map(ListenerAutoFilterData::try_from).collect()
    }

    /// Get auto-filter records by route config.
    #[instrument(skip(self), fields(route_config_id = %route_config_id), name = "db_get_auto_filters_by_route_config")]
    pub async fn get_by_route_config(
        &self,
        route_config_id: &RouteConfigId,
    ) -> Result<Vec<ListenerAutoFilterData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, ListenerAutoFilterRow>(
            "SELECT id, listener_id, http_filter_name, source_filter_id, route_config_id, attachment_level, \
             source_virtual_host_id, source_route_id, created_at \
             FROM listener_auto_filters WHERE route_config_id = $1"
        )
        .bind(route_config_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %route_config_id, "Failed to get auto-filters by route config");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get auto-filters for route config '{}'", route_config_id),
            }
        })?;

        rows.into_iter().map(ListenerAutoFilterData::try_from).collect()
    }

    /// Get listener IDs that have auto-filters for a specific route config.
    #[instrument(skip(self), fields(route_config_id = %route_config_id), name = "db_get_listeners_by_route_config")]
    pub async fn get_listener_ids_by_route_config(
        &self,
        route_config_id: &RouteConfigId,
    ) -> Result<Vec<ListenerId>> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT listener_id FROM listener_auto_filters WHERE route_config_id = $1"
        )
        .bind(route_config_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %route_config_id, "Failed to get listener IDs by route config");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get listeners for route config '{}'", route_config_id),
            }
        })?;

        Ok(ids.into_iter().map(ListenerId::from_string).collect())
    }
}
