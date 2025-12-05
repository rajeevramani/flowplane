//! Route Configuration repository for managing route configurations
//!
//! This module provides CRUD operations for route configuration resources, handling storage,
//! retrieval, and lifecycle management of route configuration data.

use crate::domain::RouteConfigId;
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

/// Internal database row structure for route_configs.
///
/// Maps directly to the database schema. Separate from [`RouteConfigData`]
/// to handle type conversions.
#[derive(Debug, Clone, FromRow)]
struct RouteConfigRow {
    pub id: String,
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: String,
    pub version: i64,
    pub source: String,
    pub team: Option<String>,
    pub import_id: Option<String>,
    pub route_order: Option<i64>,
    pub headers: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Route configuration data returned from the repository.
///
/// Represents an Envoy RouteConfiguration that matches incoming requests to backend clusters.
/// RouteConfigs contain virtual hosts which in turn contain routes.
///
/// # Fields
///
/// - `id`: Unique identifier
/// - `name`: Human-readable name
/// - `path_prefix`: Path prefix for request matching (e.g., "/api/v1")
/// - `cluster_name`: Target cluster to forward matched requests
/// - `configuration`: JSON-encoded route configuration (filters, retry policy, etc.)
/// - `version`: Version number for optimistic locking
/// - `source`: API source ("native", "gateway", "platform")
/// - `team`: Optional team identifier
/// - `import_id`: Optional import metadata ID (for OpenAPI imports)
/// - `route_order`: Order for deterministic Envoy route matching
/// - `headers`: Optional JSON-encoded header matching rules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfigData {
    pub id: RouteConfigId,
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: String,
    pub version: i64,
    pub source: String,
    pub team: Option<String>,
    pub import_id: Option<String>,
    pub route_order: Option<i64>,
    pub headers: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<RouteConfigRow> for RouteConfigData {
    fn from(row: RouteConfigRow) -> Self {
        Self {
            id: RouteConfigId::from_string(row.id),
            name: row.name,
            path_prefix: row.path_prefix,
            cluster_name: row.cluster_name,
            configuration: row.configuration,
            version: row.version,
            source: row.source,
            team: row.team,
            import_id: row.import_id,
            route_order: row.route_order,
            headers: row.headers,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Create route config request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRouteConfigRequest {
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: serde_json::Value,
    pub team: Option<String>,
    pub import_id: Option<String>,
    pub route_order: Option<i64>,
    pub headers: Option<serde_json::Value>,
}

/// Update route config request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRouteConfigRequest {
    pub path_prefix: Option<String>,
    pub cluster_name: Option<String>,
    pub configuration: Option<serde_json::Value>,
    pub team: Option<Option<String>>,
}

/// Repository for route configuration persistence.
///
/// Provides CRUD operations for route config resources with team-based access control.
/// Route configs define how incoming requests are matched and forwarded to backend clusters.
///
/// # Example
///
/// ```rust,ignore
/// use flowplane::storage::repositories::{RouteConfigRepository, CreateRouteConfigRequest};
/// use serde_json::json;
///
/// let repo = RouteConfigRepository::new(pool);
///
/// // Create a route config
/// let route_config = repo.create(CreateRouteConfigRequest {
///     name: "api-route".to_string(),
///     path_prefix: "/api/v1".to_string(),
///     cluster_name: "backend-cluster".to_string(),
///     configuration: json!({"retry_policy": {"num_retries": 3}}),
///     team: Some("team-alpha".to_string()),
/// }).await?;
/// ```
#[derive(Debug, Clone)]
pub struct RouteConfigRepository {
    pool: DbPool,
}

impl RouteConfigRepository {
    /// Creates a new route config repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Creates a new route config in the database.
    ///
    /// # Arguments
    ///
    /// * `request` - Route config creation parameters
    ///
    /// # Returns
    ///
    /// The created [`RouteConfigData`] with generated ID and timestamps.
    ///
    /// # Errors
    ///
    /// - [`FlowplaneError::Validation`] if configuration JSON is invalid
    /// - [`FlowplaneError::Database`] if insertion fails
    #[instrument(skip(self, request), fields(route_config_name = %request.name), name = "db_create_route_config")]
    pub async fn create(&self, request: CreateRouteConfigRequest) -> Result<RouteConfigData> {
        let id = RouteConfigId::new();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid route configuration JSON: {}", e))
        })?;
        let headers_json = request
            .headers
            .as_ref()
            .map(|h| {
                serde_json::to_string(h)
                    .map_err(|e| FlowplaneError::validation(format!("Invalid headers JSON: {}", e)))
            })
            .transpose()?;
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "INSERT INTO route_configs (id, name, path_prefix, cluster_name, configuration, version, team, import_id, route_order, headers, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, 1, $6, $7, $8, $9, $10, $11)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.path_prefix)
        .bind(&request.cluster_name)
        .bind(&configuration_json)
        .bind(&request.team)
        .bind(&request.import_id)
        .bind(request.route_order)
        .bind(&headers_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_name = %request.name, "Failed to create route config");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create route config '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create route config"));
        }

        tracing::info!(route_config_id = %id, route_config_name = %request.name, "Created new route config");

        self.get_by_id(&id).await
    }

    #[instrument(skip(self), fields(route_config_id = %id), name = "db_get_route_config_by_id")]
    pub async fn get_by_id(&self, id: &RouteConfigId) -> Result<RouteConfigData> {
        let row = sqlx::query_as::<Sqlite, RouteConfigRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, team, import_id, route_order, headers, created_at, updated_at FROM route_configs WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %id, "Failed to get route config by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get route config with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(RouteConfigData::from(row)),
            None => Err(FlowplaneError::not_found_msg(format!(
                "Route config with ID '{}' not found",
                id
            ))),
        }
    }

    #[instrument(skip(self), fields(route_config_name = %name), name = "db_get_route_config_by_name")]
    pub async fn get_by_name(&self, name: &str) -> Result<RouteConfigData> {
        let row = sqlx::query_as::<Sqlite, RouteConfigRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, team, import_id, route_order, headers, created_at, updated_at FROM route_configs WHERE name = $1 ORDER BY version DESC LIMIT 1"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_name = %name, "Failed to get route config by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get route config with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(RouteConfigData::from(row)),
            None => Err(FlowplaneError::not_found_msg(format!(
                "Route config with name '{}' not found",
                name
            ))),
        }
    }

    #[instrument(skip(self), fields(route_config_name = %name), name = "db_exists_route_config_by_name")]
    pub async fn exists_by_name(&self, name: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<Sqlite, i64>(
            "SELECT COUNT(*) FROM route_configs WHERE name = $1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_name = %name, "Failed to check route config existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check existence of route config '{}'", name),
            }
        })?;

        Ok(count > 0)
    }

    #[instrument(skip(self), name = "db_list_route_configs")]
    pub async fn list(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<RouteConfigData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, RouteConfigRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, team, import_id, route_order, headers, created_at, updated_at FROM route_configs ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list route configs");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list route configs".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(RouteConfigData::from).collect())
    }

    /// Lists route configs filtered by team names for multi-tenancy support.
    ///
    /// Critical for team-based access control. Returns route configs for specified
    /// teams and optionally includes team-agnostic route configs (where team is NULL).
    ///
    /// # Arguments
    ///
    /// * `teams` - Team identifiers to filter by. Empty list returns all route configs.
    /// * `include_default` - If true, also include route configs with team=NULL (default resources)
    /// * `limit` - Maximum results (default: 100, max: 1000)
    /// * `offset` - Pagination offset
    ///
    /// # Errors
    ///
    /// - [`FlowplaneError::Database`] if query execution fails
    #[instrument(skip(self), fields(teams = ?teams, limit = ?limit, offset = ?offset), name = "db_list_route_configs_by_teams")]
    pub async fn list_by_teams(
        &self,
        teams: &[String],
        _include_default: bool, // Deprecated: always includes default resources
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<RouteConfigData>> {
        // If no teams specified, return all route configs (admin:all or resource-level scope)
        if teams.is_empty() {
            return self.list(limit, offset).await;
        }

        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        // Build the query with IN clause for team filtering
        let placeholders = teams
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        // Always include NULL team route configs (default resources)
        let where_clause = format!("WHERE team IN ({}) OR team IS NULL", placeholders);

        let query_str = format!(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, team, import_id, route_order, headers, created_at, updated_at \
             FROM route_configs \
             {} \
             ORDER BY created_at DESC \
             LIMIT ${} OFFSET ${}",
            where_clause,
            teams.len() + 1,
            teams.len() + 2
        );

        let mut query = sqlx::query_as::<Sqlite, RouteConfigRow>(&query_str);

        // Bind team names
        for team in teams {
            query = query.bind(team);
        }

        // Bind limit and offset
        query = query.bind(limit).bind(offset);

        let rows = query.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to list route configs by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list route configs for teams: {:?}", teams),
            }
        })?;

        Ok(rows.into_iter().map(RouteConfigData::from).collect())
    }

    /// Lists route configs filtered by import_id for OpenAPI import tracking.
    ///
    /// Returns all route configs associated with a specific OpenAPI import,
    /// used for import details and cascade delete operations.
    ///
    /// # Arguments
    ///
    /// * `import_id` - Import metadata ID to filter by
    ///
    /// # Errors
    ///
    /// - [`FlowplaneError::Database`] if query execution fails
    #[instrument(skip(self), name = "db_list_route_configs_by_import")]
    pub async fn list_by_import(&self, import_id: &str) -> Result<Vec<RouteConfigData>> {
        let rows = sqlx::query_as::<Sqlite, RouteConfigRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, team, import_id, route_order, headers, created_at, updated_at \
             FROM route_configs \
             WHERE import_id = $1 \
             ORDER BY route_order ASC, created_at ASC"
        )
        .bind(import_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, import_id = %import_id, "Failed to list route configs by import_id");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list route configs for import_id: {}", import_id),
            }
        })?;

        Ok(rows.into_iter().map(RouteConfigData::from).collect())
    }

    #[instrument(skip(self, request), fields(route_config_id = %id), name = "db_update_route_config")]
    pub async fn update(
        &self,
        id: &RouteConfigId,
        request: UpdateRouteConfigRequest,
    ) -> Result<RouteConfigData> {
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
        let new_team = request.team.unwrap_or(current.team);

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE route_configs SET path_prefix = $1, cluster_name = $2, configuration = $3, version = $4, team = $5, updated_at = $6 WHERE id = $7"
        )
        .bind(&new_path_prefix)
        .bind(&new_cluster_name)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(&new_team)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %id, "Failed to update route config");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update route config with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Route config with ID '{}' not found",
                id
            )));
        }

        tracing::info!(route_config_id = %id, route_config_name = %current.name, new_version = new_version, "Updated route config");

        self.get_by_id(id).await
    }

    #[instrument(skip(self), fields(route_config_id = %id), name = "db_delete_route_config")]
    pub async fn delete(&self, id: &RouteConfigId) -> Result<()> {
        let route_config = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM route_configs WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, route_config_id = %id, "Failed to delete route config");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete route config with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Route config with ID '{}' not found",
                id
            )));
        }

        tracing::info!(route_config_id = %id, route_config_name = %route_config.name, "Deleted route config");

        Ok(())
    }

    #[instrument(skip(self), fields(route_config_name = %name), name = "db_delete_route_config_by_name")]
    pub async fn delete_by_name(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM route_configs WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, route_config_name = %name, "Failed to delete route config by name");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete route config '{}'", name),
                }
            })?;

        Ok(())
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}
