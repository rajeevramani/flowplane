//! Virtual Host Filter Repository
//!
//! This module provides operations for attaching filters to virtual hosts.
//! Filters attached at the virtual host level apply to all route rules within that host.

use crate::domain::{FilterId, VirtualHostId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

/// Internal database row structure for virtual_host_filters.
#[derive(Debug, Clone, FromRow)]
struct VirtualHostFilterRow {
    pub virtual_host_id: String,
    pub filter_id: String,
    pub filter_order: i32,
    pub created_at: DateTime<Utc>,
}

/// Virtual host filter attachment data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualHostFilterData {
    pub virtual_host_id: VirtualHostId,
    pub filter_id: FilterId,
    pub filter_order: i32,
    pub created_at: DateTime<Utc>,
}

impl From<VirtualHostFilterRow> for VirtualHostFilterData {
    fn from(row: VirtualHostFilterRow) -> Self {
        Self {
            virtual_host_id: VirtualHostId::from_string(row.virtual_host_id),
            filter_id: FilterId::from_string(row.filter_id),
            filter_order: row.filter_order,
            created_at: row.created_at,
        }
    }
}

/// Repository for virtual host filter attachment operations.
#[derive(Debug, Clone)]
pub struct VirtualHostFilterRepository {
    pool: DbPool,
}

impl VirtualHostFilterRepository {
    /// Creates a new repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Attach a filter to a virtual host.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id, filter_id = %filter_id, order = order), name = "db_attach_filter_to_vh")]
    pub async fn attach(
        &self,
        virtual_host_id: &VirtualHostId,
        filter_id: &FilterId,
        order: i32,
    ) -> Result<VirtualHostFilterData> {
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO virtual_host_filters (virtual_host_id, filter_id, filter_order, created_at) \
             VALUES ($1, $2, $3, $4)"
        )
        .bind(virtual_host_id.as_str())
        .bind(filter_id.as_str())
        .bind(order)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, vh_id = %virtual_host_id, filter_id = %filter_id, "Failed to attach filter to virtual host");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to attach filter '{}' to virtual host '{}'", filter_id, virtual_host_id),
            }
        })?;

        tracing::info!(
            vh_id = %virtual_host_id,
            filter_id = %filter_id,
            order = order,
            "Attached filter to virtual host"
        );

        Ok(VirtualHostFilterData {
            virtual_host_id: virtual_host_id.clone(),
            filter_id: filter_id.clone(),
            filter_order: order,
            created_at: now,
        })
    }

    /// Detach a filter from a virtual host.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id, filter_id = %filter_id), name = "db_detach_filter_from_vh")]
    pub async fn detach(
        &self,
        virtual_host_id: &VirtualHostId,
        filter_id: &FilterId,
    ) -> Result<()> {
        let result = sqlx::query(
            "DELETE FROM virtual_host_filters WHERE virtual_host_id = $1 AND filter_id = $2"
        )
        .bind(virtual_host_id.as_str())
        .bind(filter_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, vh_id = %virtual_host_id, filter_id = %filter_id, "Failed to detach filter from virtual host");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to detach filter '{}' from virtual host '{}'", filter_id, virtual_host_id),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found(
                "VirtualHostFilterAttachment",
                format!("{}:{}", virtual_host_id, filter_id),
            ));
        }

        tracing::info!(
            vh_id = %virtual_host_id,
            filter_id = %filter_id,
            "Detached filter from virtual host"
        );

        Ok(())
    }

    /// List all filter attachments for a virtual host.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id), name = "db_list_vh_filters")]
    pub async fn list_by_virtual_host(
        &self,
        virtual_host_id: &VirtualHostId,
    ) -> Result<Vec<VirtualHostFilterData>> {
        let rows = sqlx::query_as::<Sqlite, VirtualHostFilterRow>(
            "SELECT virtual_host_id, filter_id, filter_order, created_at \
             FROM virtual_host_filters WHERE virtual_host_id = $1 ORDER BY filter_order ASC"
        )
        .bind(virtual_host_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, vh_id = %virtual_host_id, "Failed to list virtual host filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list filters for virtual host '{}'", virtual_host_id),
            }
        })?;

        Ok(rows.into_iter().map(VirtualHostFilterData::from).collect())
    }

    /// List all virtual hosts using a specific filter.
    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_list_filter_vhs")]
    pub async fn list_by_filter(&self, filter_id: &FilterId) -> Result<Vec<VirtualHostId>> {
        let vh_ids: Vec<String> = sqlx::query_scalar(
            "SELECT virtual_host_id FROM virtual_host_filters WHERE filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to list virtual hosts using filter");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list virtual hosts for filter '{}'", filter_id),
            }
        })?;

        Ok(vh_ids.into_iter().map(VirtualHostId::from_string).collect())
    }

    /// Check if a filter is attached to a virtual host.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id, filter_id = %filter_id), name = "db_exists_vh_filter")]
    pub async fn exists(
        &self,
        virtual_host_id: &VirtualHostId,
        filter_id: &FilterId,
    ) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM virtual_host_filters WHERE virtual_host_id = $1 AND filter_id = $2"
        )
        .bind(virtual_host_id.as_str())
        .bind(filter_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, vh_id = %virtual_host_id, filter_id = %filter_id, "Failed to check filter attachment");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check filter attachment for virtual host '{}'", virtual_host_id),
            }
        })?;

        Ok(count > 0)
    }

    /// Get the next available order for a virtual host.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id), name = "db_next_vh_filter_order")]
    pub async fn get_next_order(&self, virtual_host_id: &VirtualHostId) -> Result<i32> {
        let max_order: Option<i32> = sqlx::query_scalar(
            "SELECT MAX(filter_order) FROM virtual_host_filters WHERE virtual_host_id = $1",
        )
        .bind(virtual_host_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, vh_id = %virtual_host_id, "Failed to get max filter order");
            FlowplaneError::Database {
                source: e,
                context: format!(
                    "Failed to get max filter order for virtual host '{}'",
                    virtual_host_id
                ),
            }
        })?;

        Ok(max_order.unwrap_or(0) + 1)
    }

    /// Count total attachments for a filter (used to prevent deletion of attached filters).
    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_count_vh_filter_attachments")]
    pub async fn count_by_filter(&self, filter_id: &FilterId) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM virtual_host_filters WHERE filter_id = $1"
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
}
