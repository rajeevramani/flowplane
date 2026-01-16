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

/// Internal database row structure for routes with related data (joined query).
#[derive(Debug, Clone, FromRow)]
struct RouteWithRelatedDataRow {
    // Route fields
    pub route_id: String,
    pub route_name: String,
    pub path_pattern: String,
    pub match_type: String,
    pub rule_order: i32,
    pub route_created_at: DateTime<Utc>,
    pub route_updated_at: DateTime<Utc>,
    // Virtual host fields
    pub virtual_host_id: String,
    pub virtual_host_name: String,
    pub domains: String, // JSON array
    // Route config fields
    pub route_config_id: String,
    pub route_config_name: String,
    pub configuration: String, // JSON for extraction
    // MCP tool fields (optional, from LEFT JOIN)
    pub mcp_enabled: i32, // SQLite uses integer for boolean
    pub mcp_tool_name: Option<String>,
    // Filter count (computed)
    pub filter_count: i64,
}

/// Route data with related entities (virtual host, route config, MCP tool, filter count).
/// Used by the route views endpoint to avoid N+1 queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteWithRelatedData {
    // Route fields
    pub route_id: RouteId,
    pub route_name: String,
    pub path_pattern: String,
    pub match_type: RouteMatchType,
    pub rule_order: i32,
    pub route_created_at: DateTime<Utc>,
    pub route_updated_at: DateTime<Utc>,
    // Virtual host fields
    pub virtual_host_id: VirtualHostId,
    pub virtual_host_name: String,
    pub domains: Vec<String>,
    // Route config fields
    pub route_config_id: crate::domain::RouteConfigId,
    pub route_config_name: String,
    pub configuration: String,
    // MCP tool fields
    pub mcp_enabled: bool,
    pub mcp_tool_name: Option<String>,
    // Filter count
    pub filter_count: i32,
}

impl TryFrom<RouteWithRelatedDataRow> for RouteWithRelatedData {
    type Error = FlowplaneError;

    fn try_from(row: RouteWithRelatedDataRow) -> Result<Self> {
        let match_type: RouteMatchType = row.match_type.parse().map_err(|e: String| {
            FlowplaneError::internal(format!("Failed to parse match type: {}", e))
        })?;

        let domains: Vec<String> = serde_json::from_str(&row.domains).map_err(|e| {
            FlowplaneError::internal(format!("Failed to parse domains JSON: {}", e))
        })?;

        Ok(Self {
            route_id: RouteId::from_string(row.route_id),
            route_name: row.route_name,
            path_pattern: row.path_pattern,
            match_type,
            rule_order: row.rule_order,
            route_created_at: row.route_created_at,
            route_updated_at: row.route_updated_at,
            virtual_host_id: VirtualHostId::from_string(row.virtual_host_id),
            virtual_host_name: row.virtual_host_name,
            domains,
            route_config_id: crate::domain::RouteConfigId::from_string(row.route_config_id),
            route_config_name: row.route_config_name,
            configuration: row.configuration,
            mcp_enabled: row.mcp_enabled != 0,
            mcp_tool_name: row.mcp_tool_name,
            filter_count: row.filter_count as i32,
        })
    }
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

    /// Count routes for a virtual host.
    #[instrument(skip(self), fields(vh_id = %virtual_host_id), name = "db_count_routes_by_vh")]
    pub async fn count_by_virtual_host(&self, virtual_host_id: &VirtualHostId) -> Result<i64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM routes WHERE virtual_host_id = $1")
                .bind(virtual_host_id.as_str())
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, vh_id = %virtual_host_id, "Failed to count routes");
                    FlowplaneError::Database {
                        source: e,
                        context: format!(
                            "Failed to count routes for virtual host '{}'",
                            virtual_host_id
                        ),
                    }
                })?;
        Ok(count)
    }

    /// List all routes for a team (joins through virtual_host and route_config).
    #[instrument(skip(self), fields(team = %team), name = "db_list_routes_by_team")]
    pub async fn list_by_team(&self, team: &str) -> Result<Vec<RouteData>> {
        let rows = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT r.id, r.virtual_host_id, r.name, r.path_pattern, r.match_type, r.rule_order, r.created_at, r.updated_at \
             FROM routes r \
             INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
             INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
             WHERE rc.team = $1 \
             ORDER BY r.name"
        )
        .bind(team)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list routes by team");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list routes for team '{}'", team),
            }
        })?;

        rows.into_iter().map(RouteData::try_from).collect()
    }

    /// Count all routes for a team (joins through virtual_host and route_config).
    #[instrument(skip(self), fields(team = %team), name = "db_count_routes_by_team")]
    pub async fn count_by_team(&self, team: &str) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) \
             FROM routes r \
             INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
             INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
             WHERE rc.team = $1",
        )
        .bind(team)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to count routes by team");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count routes for team '{}'", team),
            }
        })?;

        Ok(count)
    }

    /// List routes for a team with pagination (joins through virtual_host and route_config).
    ///
    /// # Arguments
    ///
    /// * `team` - Team identifier
    /// * `limit` - Maximum number of results (default: 20, max: 100)
    /// * `offset` - Number of results to skip
    /// * `search` - Optional search term (matches name, path_pattern)
    #[instrument(skip(self), fields(team = %team, limit = ?limit, offset = ?offset), name = "db_list_routes_by_team_paginated")]
    pub async fn list_by_team_paginated(
        &self,
        team: &str,
        limit: Option<i32>,
        offset: Option<i32>,
        search: Option<&str>,
    ) -> Result<Vec<RouteData>> {
        let limit = limit.unwrap_or(20).min(100);
        let offset = offset.unwrap_or(0);

        let rows = if let Some(search_term) = search {
            let search_pattern = format!("%{}%", search_term);
            sqlx::query_as::<Sqlite, RouteRow>(
                "SELECT r.id, r.virtual_host_id, r.name, r.path_pattern, r.match_type, r.rule_order, r.created_at, r.updated_at \
                 FROM routes r \
                 INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
                 INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
                 WHERE rc.team = $1 \
                   AND (r.name LIKE $2 OR r.path_pattern LIKE $2 OR vh.name LIKE $2) \
                 ORDER BY r.created_at DESC \
                 LIMIT $3 OFFSET $4"
            )
            .bind(team)
            .bind(&search_pattern)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<Sqlite, RouteRow>(
                "SELECT r.id, r.virtual_host_id, r.name, r.path_pattern, r.match_type, r.rule_order, r.created_at, r.updated_at \
                 FROM routes r \
                 INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
                 INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
                 WHERE rc.team = $1 \
                 ORDER BY r.created_at DESC \
                 LIMIT $2 OFFSET $3"
            )
            .bind(team)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list routes by team paginated");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list routes for team '{}' with pagination", team),
            }
        })?;

        rows.into_iter().map(RouteData::try_from).collect()
    }

    /// Count routes for a team with optional search filter.
    #[instrument(skip(self), fields(team = %team, search = ?search), name = "db_count_routes_by_team_filtered")]
    pub async fn count_by_team_filtered(&self, team: &str, search: Option<&str>) -> Result<i64> {
        let count: i64 = if let Some(search_term) = search {
            let search_pattern = format!("%{}%", search_term);
            sqlx::query_scalar(
                "SELECT COUNT(*) \
                 FROM routes r \
                 INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
                 INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
                 WHERE rc.team = $1 \
                   AND (r.name LIKE $2 OR r.path_pattern LIKE $2 OR vh.name LIKE $2)",
            )
            .bind(team)
            .bind(&search_pattern)
            .fetch_one(&self.pool)
            .await
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) \
                 FROM routes r \
                 INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
                 INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
                 WHERE rc.team = $1",
            )
            .bind(team)
            .fetch_one(&self.pool)
            .await
        }
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to count routes by team filtered");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count routes for team '{}'", team),
            }
        })?;

        Ok(count)
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

    /// List routes with all related data in a single query (optimized for route views).
    ///
    /// This method uses JOINs to fetch routes along with their virtual host, route config,
    /// MCP tool status, and filter count in a single database query. This avoids the N+1
    /// query problem when building route view DTOs.
    ///
    /// # Arguments
    ///
    /// * `team` - Team identifier
    /// * `limit` - Maximum number of results (default: 20, max: 100)
    /// * `offset` - Number of results to skip
    /// * `search` - Optional search term (matches route name, path_pattern, virtual host name)
    /// * `mcp_filter` - Optional MCP filter ("enabled" or "disabled")
    #[instrument(skip(self), fields(team = %team, limit = ?limit, offset = ?offset), name = "db_list_routes_with_related")]
    pub async fn list_by_team_paginated_with_related(
        &self,
        team: &str,
        limit: Option<i32>,
        offset: Option<i32>,
        search: Option<&str>,
        mcp_filter: Option<&str>,
    ) -> Result<Vec<RouteWithRelatedData>> {
        let limit = limit.unwrap_or(20).min(100);
        let offset = offset.unwrap_or(0);

        // Build the query with optional search and MCP filter
        let base_query = r#"
            SELECT
                r.id as route_id,
                r.name as route_name,
                r.path_pattern,
                r.match_type,
                r.rule_order,
                r.created_at as route_created_at,
                r.updated_at as route_updated_at,
                vh.id as virtual_host_id,
                vh.name as virtual_host_name,
                vh.domains,
                rc.id as route_config_id,
                rc.name as route_config_name,
                rc.configuration,
                COALESCE(mt.enabled, 0) as mcp_enabled,
                mt.name as mcp_tool_name,
                COALESCE((SELECT COUNT(*) FROM route_filters rf WHERE rf.route_id = r.id), 0) as filter_count
            FROM routes r
            INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
            INNER JOIN route_configs rc ON vh.route_config_id = rc.id
            LEFT JOIN mcp_tools mt ON mt.route_id = r.id
            WHERE rc.team = $1
        "#;

        let rows = match (search, mcp_filter) {
            (Some(search_term), Some("enabled")) => {
                let search_pattern = format!("%{}%", search_term);
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND (r.name LIKE $2 OR r.path_pattern LIKE $2 OR vh.name LIKE $2) \
                     AND COALESCE(mt.enabled, 0) = 1 \
                     ORDER BY r.created_at DESC LIMIT $3 OFFSET $4",
                    base_query
                ))
                .bind(team)
                .bind(&search_pattern)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
            }
            (Some(search_term), Some("disabled")) => {
                let search_pattern = format!("%{}%", search_term);
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND (r.name LIKE $2 OR r.path_pattern LIKE $2 OR vh.name LIKE $2) \
                     AND COALESCE(mt.enabled, 0) = 0 \
                     ORDER BY r.created_at DESC LIMIT $3 OFFSET $4",
                    base_query
                ))
                .bind(team)
                .bind(&search_pattern)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
            }
            (Some(search_term), _) => {
                let search_pattern = format!("%{}%", search_term);
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND (r.name LIKE $2 OR r.path_pattern LIKE $2 OR vh.name LIKE $2) \
                     ORDER BY r.created_at DESC LIMIT $3 OFFSET $4",
                    base_query
                ))
                .bind(team)
                .bind(&search_pattern)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
            }
            (None, Some("enabled")) => {
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND COALESCE(mt.enabled, 0) = 1 \
                     ORDER BY r.created_at DESC LIMIT $2 OFFSET $3",
                    base_query
                ))
                .bind(team)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
            }
            (None, Some("disabled")) => {
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND COALESCE(mt.enabled, 0) = 0 \
                     ORDER BY r.created_at DESC LIMIT $2 OFFSET $3",
                    base_query
                ))
                .bind(team)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
            }
            (None, _) => {
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} ORDER BY r.created_at DESC LIMIT $2 OFFSET $3",
                    base_query
                ))
                .bind(team)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list routes with related data");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list routes with related data for team '{}'", team),
            }
        })?;

        rows.into_iter().map(RouteWithRelatedData::try_from).collect()
    }

    /// Count routes for a team with optional search and MCP filter.
    ///
    /// This method counts routes matching the given filters, which is needed for
    /// pagination when using the optimized route views query.
    #[instrument(skip(self), fields(team = %team, search = ?search, mcp_filter = ?mcp_filter), name = "db_count_routes_with_filters")]
    pub async fn count_by_team_with_mcp_filter(
        &self,
        team: &str,
        search: Option<&str>,
        mcp_filter: Option<&str>,
    ) -> Result<i64> {
        let base_query = r#"
            SELECT COUNT(*)
            FROM routes r
            INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
            INNER JOIN route_configs rc ON vh.route_config_id = rc.id
            LEFT JOIN mcp_tools mt ON mt.route_id = r.id
            WHERE rc.team = $1
        "#;

        let count: i64 = match (search, mcp_filter) {
            (Some(search_term), Some("enabled")) => {
                let search_pattern = format!("%{}%", search_term);
                sqlx::query_scalar(&format!(
                    "{} AND (r.name LIKE $2 OR r.path_pattern LIKE $2 OR vh.name LIKE $2) \
                     AND COALESCE(mt.enabled, 0) = 1",
                    base_query
                ))
                .bind(team)
                .bind(&search_pattern)
                .fetch_one(&self.pool)
                .await
            }
            (Some(search_term), Some("disabled")) => {
                let search_pattern = format!("%{}%", search_term);
                sqlx::query_scalar(&format!(
                    "{} AND (r.name LIKE $2 OR r.path_pattern LIKE $2 OR vh.name LIKE $2) \
                     AND COALESCE(mt.enabled, 0) = 0",
                    base_query
                ))
                .bind(team)
                .bind(&search_pattern)
                .fetch_one(&self.pool)
                .await
            }
            (Some(search_term), _) => {
                let search_pattern = format!("%{}%", search_term);
                sqlx::query_scalar(&format!(
                    "{} AND (r.name LIKE $2 OR r.path_pattern LIKE $2 OR vh.name LIKE $2)",
                    base_query
                ))
                .bind(team)
                .bind(&search_pattern)
                .fetch_one(&self.pool)
                .await
            }
            (None, Some("enabled")) => {
                sqlx::query_scalar(&format!("{} AND COALESCE(mt.enabled, 0) = 1", base_query))
                    .bind(team)
                    .fetch_one(&self.pool)
                    .await
            }
            (None, Some("disabled")) => {
                sqlx::query_scalar(&format!("{} AND COALESCE(mt.enabled, 0) = 0", base_query))
                    .bind(team)
                    .fetch_one(&self.pool)
                    .await
            }
            (None, _) => sqlx::query_scalar(base_query).bind(team).fetch_one(&self.pool).await,
        }
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to count routes with filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count routes for team '{}'", team),
            }
        })?;

        Ok(count)
    }

    /// List routes with related data for multiple teams (supports admin bypass).
    ///
    /// If `teams` is empty, returns ALL routes across all teams (admin bypass).
    /// This is the multi-team equivalent of `list_by_team_paginated_with_related`.
    ///
    /// # Arguments
    /// * `teams` - Team names to filter by (empty = all teams / admin bypass)
    /// * `limit` - Maximum number of results (default: 20, max: 100)
    /// * `offset` - Number of results to skip
    /// * `search` - Optional search term (matches route name, path_pattern, virtual host name)
    /// * `mcp_filter` - Optional MCP filter ("enabled" or "disabled")
    #[instrument(skip(self), fields(teams = ?teams.len(), limit = ?limit, offset = ?offset), name = "db_list_routes_by_teams_with_related")]
    pub async fn list_by_teams_paginated_with_related(
        &self,
        teams: &[String],
        limit: Option<i32>,
        offset: Option<i32>,
        search: Option<&str>,
        mcp_filter: Option<&str>,
    ) -> Result<Vec<RouteWithRelatedData>> {
        let limit_val = limit.unwrap_or(20).min(100);
        let offset_val = offset.unwrap_or(0);

        // Admin bypass: empty teams = query all routes
        let base_query = if teams.is_empty() {
            r#"
            SELECT
                r.id as route_id,
                r.name as route_name,
                r.path_pattern,
                r.match_type,
                r.rule_order,
                r.created_at as route_created_at,
                r.updated_at as route_updated_at,
                vh.id as virtual_host_id,
                vh.name as virtual_host_name,
                vh.domains,
                rc.id as route_config_id,
                rc.name as route_config_name,
                rc.configuration,
                COALESCE(mt.enabled, 0) as mcp_enabled,
                mt.name as mcp_tool_name,
                COALESCE((SELECT COUNT(*) FROM route_filters rf WHERE rf.route_id = r.id), 0) as filter_count
            FROM routes r
            INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
            INNER JOIN route_configs rc ON vh.route_config_id = rc.id
            LEFT JOIN mcp_tools mt ON mt.route_id = r.id
            WHERE 1=1
            "#
        } else {
            // Note: We can't use a runtime-built IN clause with the static query.
            // For simplicity, if there's only one team, use the existing single-team method.
            if teams.len() == 1 {
                return self
                    .list_by_team_paginated_with_related(
                        &teams[0], limit, offset, search, mcp_filter,
                    )
                    .await;
            }

            // For multi-team case, we need to build a dynamic query
            // This is a simplified approach - in production you'd use a more robust query builder
            return self
                .list_by_teams_paginated_with_related_dynamic(
                    teams, limit_val, offset_val, search, mcp_filter,
                )
                .await;
        };

        // Admin bypass case (teams is empty) - query all routes
        let rows = match (search, mcp_filter) {
            (Some(search_term), Some("enabled")) => {
                let search_pattern = format!("%{}%", search_term);
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND (r.name LIKE $1 OR r.path_pattern LIKE $1 OR vh.name LIKE $1) \
                     AND COALESCE(mt.enabled, 0) = 1 \
                     ORDER BY r.created_at DESC LIMIT $2 OFFSET $3",
                    base_query
                ))
                .bind(&search_pattern)
                .bind(limit_val)
                .bind(offset_val)
                .fetch_all(&self.pool)
                .await
            }
            (Some(search_term), Some("disabled")) => {
                let search_pattern = format!("%{}%", search_term);
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND (r.name LIKE $1 OR r.path_pattern LIKE $1 OR vh.name LIKE $1) \
                     AND COALESCE(mt.enabled, 0) = 0 \
                     ORDER BY r.created_at DESC LIMIT $2 OFFSET $3",
                    base_query
                ))
                .bind(&search_pattern)
                .bind(limit_val)
                .bind(offset_val)
                .fetch_all(&self.pool)
                .await
            }
            (Some(search_term), _) => {
                let search_pattern = format!("%{}%", search_term);
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND (r.name LIKE $1 OR r.path_pattern LIKE $1 OR vh.name LIKE $1) \
                     ORDER BY r.created_at DESC LIMIT $2 OFFSET $3",
                    base_query
                ))
                .bind(&search_pattern)
                .bind(limit_val)
                .bind(offset_val)
                .fetch_all(&self.pool)
                .await
            }
            (None, Some("enabled")) => {
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND COALESCE(mt.enabled, 0) = 1 \
                     ORDER BY r.created_at DESC LIMIT $1 OFFSET $2",
                    base_query
                ))
                .bind(limit_val)
                .bind(offset_val)
                .fetch_all(&self.pool)
                .await
            }
            (None, Some("disabled")) => {
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} AND COALESCE(mt.enabled, 0) = 0 \
                     ORDER BY r.created_at DESC LIMIT $1 OFFSET $2",
                    base_query
                ))
                .bind(limit_val)
                .bind(offset_val)
                .fetch_all(&self.pool)
                .await
            }
            (None, _) => {
                sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&format!(
                    "{} ORDER BY r.created_at DESC LIMIT $1 OFFSET $2",
                    base_query
                ))
                .bind(limit_val)
                .bind(offset_val)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list all routes with related data (admin bypass)");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list all routes with related data".to_string(),
            }
        })?;

        rows.into_iter().map(RouteWithRelatedData::try_from).collect()
    }

    /// Internal helper for multi-team queries (more than one team).
    async fn list_by_teams_paginated_with_related_dynamic(
        &self,
        teams: &[String],
        limit: i32,
        offset: i32,
        search: Option<&str>,
        mcp_filter: Option<&str>,
    ) -> Result<Vec<RouteWithRelatedData>> {
        // Build placeholders for IN clause
        let placeholders: Vec<String> = (1..=teams.len()).map(|i| format!("${}", i)).collect();
        let team_placeholder_count = teams.len();

        let base_query = format!(
            r#"
            SELECT
                r.id as route_id,
                r.name as route_name,
                r.path_pattern,
                r.match_type,
                r.rule_order,
                r.created_at as route_created_at,
                r.updated_at as route_updated_at,
                vh.id as virtual_host_id,
                vh.name as virtual_host_name,
                vh.domains,
                rc.id as route_config_id,
                rc.name as route_config_name,
                rc.configuration,
                COALESCE(mt.enabled, 0) as mcp_enabled,
                mt.name as mcp_tool_name,
                COALESCE((SELECT COUNT(*) FROM route_filters rf WHERE rf.route_id = r.id), 0) as filter_count
            FROM routes r
            INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
            INNER JOIN route_configs rc ON vh.route_config_id = rc.id
            LEFT JOIN mcp_tools mt ON mt.route_id = r.id
            WHERE rc.team IN ({})
            "#,
            placeholders.join(", ")
        );

        let query_str = match (search, mcp_filter) {
            (Some(_), Some("enabled")) => {
                format!(
                    "{} AND (r.name LIKE ${} OR r.path_pattern LIKE ${} OR vh.name LIKE ${}) \
                     AND COALESCE(mt.enabled, 0) = 1 \
                     ORDER BY r.created_at DESC LIMIT ${} OFFSET ${}",
                    base_query,
                    team_placeholder_count + 1,
                    team_placeholder_count + 1,
                    team_placeholder_count + 1,
                    team_placeholder_count + 2,
                    team_placeholder_count + 3
                )
            }
            (Some(_), Some("disabled")) => {
                format!(
                    "{} AND (r.name LIKE ${} OR r.path_pattern LIKE ${} OR vh.name LIKE ${}) \
                     AND COALESCE(mt.enabled, 0) = 0 \
                     ORDER BY r.created_at DESC LIMIT ${} OFFSET ${}",
                    base_query,
                    team_placeholder_count + 1,
                    team_placeholder_count + 1,
                    team_placeholder_count + 1,
                    team_placeholder_count + 2,
                    team_placeholder_count + 3
                )
            }
            (Some(_), _) => {
                format!(
                    "{} AND (r.name LIKE ${} OR r.path_pattern LIKE ${} OR vh.name LIKE ${}) \
                     ORDER BY r.created_at DESC LIMIT ${} OFFSET ${}",
                    base_query,
                    team_placeholder_count + 1,
                    team_placeholder_count + 1,
                    team_placeholder_count + 1,
                    team_placeholder_count + 2,
                    team_placeholder_count + 3
                )
            }
            (None, Some("enabled")) => {
                format!(
                    "{} AND COALESCE(mt.enabled, 0) = 1 \
                     ORDER BY r.created_at DESC LIMIT ${} OFFSET ${}",
                    base_query,
                    team_placeholder_count + 1,
                    team_placeholder_count + 2
                )
            }
            (None, Some("disabled")) => {
                format!(
                    "{} AND COALESCE(mt.enabled, 0) = 0 \
                     ORDER BY r.created_at DESC LIMIT ${} OFFSET ${}",
                    base_query,
                    team_placeholder_count + 1,
                    team_placeholder_count + 2
                )
            }
            (None, _) => {
                format!(
                    "{} ORDER BY r.created_at DESC LIMIT ${} OFFSET ${}",
                    base_query,
                    team_placeholder_count + 1,
                    team_placeholder_count + 2
                )
            }
        };

        let mut query = sqlx::query_as::<Sqlite, RouteWithRelatedDataRow>(&query_str);

        // Bind team parameters
        for team in teams {
            query = query.bind(team);
        }

        // Bind search, limit, offset based on the match case
        let rows = match (search, mcp_filter) {
            (Some(search_term), Some(_)) | (Some(search_term), None) => {
                let search_pattern = format!("%{}%", search_term);
                query.bind(&search_pattern).bind(limit).bind(offset).fetch_all(&self.pool).await
            }
            (None, _) => query.bind(limit).bind(offset).fetch_all(&self.pool).await,
        }
        .map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to list routes by teams with related data");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list routes for teams {:?}", teams),
            }
        })?;

        rows.into_iter().map(RouteWithRelatedData::try_from).collect()
    }

    /// Count routes for multiple teams with optional search and MCP filter (supports admin bypass).
    ///
    /// If `teams` is empty, counts ALL routes across all teams (admin bypass).
    #[instrument(skip(self), fields(teams = ?teams.len(), search = ?search, mcp_filter = ?mcp_filter), name = "db_count_routes_by_teams")]
    pub async fn count_by_teams_with_mcp_filter(
        &self,
        teams: &[String],
        search: Option<&str>,
        mcp_filter: Option<&str>,
    ) -> Result<i64> {
        // Single team: use existing optimized method
        if teams.len() == 1 {
            return self.count_by_team_with_mcp_filter(&teams[0], search, mcp_filter).await;
        }

        // Admin bypass: empty teams = count all routes
        let base_query = if teams.is_empty() {
            r#"
            SELECT COUNT(*)
            FROM routes r
            INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
            INNER JOIN route_configs rc ON vh.route_config_id = rc.id
            LEFT JOIN mcp_tools mt ON mt.route_id = r.id
            WHERE 1=1
            "#
            .to_string()
        } else {
            // Build IN clause for team filtering
            let placeholders: Vec<String> = (1..=teams.len()).map(|i| format!("${}", i)).collect();
            format!(
                r#"
                SELECT COUNT(*)
                FROM routes r
                INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
                INNER JOIN route_configs rc ON vh.route_config_id = rc.id
                LEFT JOIN mcp_tools mt ON mt.route_id = r.id
                WHERE rc.team IN ({})
                "#,
                placeholders.join(", ")
            )
        };

        let team_count = teams.len();
        let query_str = if teams.is_empty() {
            // Admin bypass queries (no team binding)
            match (search, mcp_filter) {
                (Some(_), Some("enabled")) => format!(
                    "{} AND (r.name LIKE $1 OR r.path_pattern LIKE $1 OR vh.name LIKE $1) \
                     AND COALESCE(mt.enabled, 0) = 1",
                    base_query
                ),
                (Some(_), Some("disabled")) => format!(
                    "{} AND (r.name LIKE $1 OR r.path_pattern LIKE $1 OR vh.name LIKE $1) \
                     AND COALESCE(mt.enabled, 0) = 0",
                    base_query
                ),
                (Some(_), _) => format!(
                    "{} AND (r.name LIKE $1 OR r.path_pattern LIKE $1 OR vh.name LIKE $1)",
                    base_query
                ),
                (None, Some("enabled")) => {
                    format!("{} AND COALESCE(mt.enabled, 0) = 1", base_query)
                }
                (None, Some("disabled")) => {
                    format!("{} AND COALESCE(mt.enabled, 0) = 0", base_query)
                }
                (None, _) => base_query,
            }
        } else {
            // Multi-team queries
            match (search, mcp_filter) {
                (Some(_), Some("enabled")) => format!(
                    "{} AND (r.name LIKE ${} OR r.path_pattern LIKE ${} OR vh.name LIKE ${}) \
                     AND COALESCE(mt.enabled, 0) = 1",
                    base_query,
                    team_count + 1,
                    team_count + 1,
                    team_count + 1
                ),
                (Some(_), Some("disabled")) => format!(
                    "{} AND (r.name LIKE ${} OR r.path_pattern LIKE ${} OR vh.name LIKE ${}) \
                     AND COALESCE(mt.enabled, 0) = 0",
                    base_query,
                    team_count + 1,
                    team_count + 1,
                    team_count + 1
                ),
                (Some(_), _) => format!(
                    "{} AND (r.name LIKE ${} OR r.path_pattern LIKE ${} OR vh.name LIKE ${})",
                    base_query,
                    team_count + 1,
                    team_count + 1,
                    team_count + 1
                ),
                (None, Some("enabled")) => {
                    format!("{} AND COALESCE(mt.enabled, 0) = 1", base_query)
                }
                (None, Some("disabled")) => {
                    format!("{} AND COALESCE(mt.enabled, 0) = 0", base_query)
                }
                (None, _) => base_query,
            }
        };

        let mut query = sqlx::query_scalar::<Sqlite, i64>(&query_str);

        // Bind team parameters (if any)
        for team in teams {
            query = query.bind(team);
        }

        // Bind search pattern if present
        let count: i64 = if let Some(search_term) = search {
            let search_pattern = format!("%{}%", search_term);
            query.bind(&search_pattern).fetch_one(&self.pool).await
        } else {
            query.fetch_one(&self.pool).await
        }
        .map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to count routes by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count routes for teams {:?}", teams),
            }
        })?;

        Ok(count)
    }

    /// Count all routes for multiple teams (supports admin bypass).
    ///
    /// If `teams` is empty, counts ALL routes across all teams (admin bypass).
    #[instrument(skip(self), fields(teams = ?teams.len()), name = "db_count_routes_by_teams")]
    pub async fn count_by_teams(&self, teams: &[String]) -> Result<i64> {
        // Single team: use existing optimized method
        if teams.len() == 1 {
            return self.count_by_team(&teams[0]).await;
        }

        // Admin bypass: empty teams = count all routes
        if teams.is_empty() {
            let count: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)
                FROM routes r
                INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
                INNER JOIN route_configs rc ON vh.route_config_id = rc.id
                "#,
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to count all routes (admin bypass)");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to count all routes".to_string(),
                }
            })?;
            return Ok(count);
        }

        // Build IN clause for team filtering
        let placeholders: Vec<String> = (1..=teams.len()).map(|i| format!("${}", i)).collect();
        let query_str = format!(
            r#"
            SELECT COUNT(*)
            FROM routes r
            INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
            INNER JOIN route_configs rc ON vh.route_config_id = rc.id
            WHERE rc.team IN ({})
            "#,
            placeholders.join(", ")
        );

        let mut query = sqlx::query_scalar::<Sqlite, i64>(&query_str);
        for team in teams {
            query = query.bind(team);
        }

        let count: i64 = query.fetch_one(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to count routes by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count routes for teams {:?}", teams),
            }
        })?;

        Ok(count)
    }
}
