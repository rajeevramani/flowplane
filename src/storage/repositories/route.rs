//! Route repository for managing route configurations
//!
//! This module provides CRUD operations for route resources, handling storage,
//! retrieval, and lifecycle management of route configuration data.

use crate::domain::RouteId;
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

/// Internal database row structure for routes.
///
/// Maps directly to the database schema. Separate from [`RouteData`]
/// to handle type conversions.
#[derive(Debug, Clone, FromRow)]
struct RouteRow {
    pub id: String,
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: String,
    pub version: i64,
    pub source: String,
    pub team: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Route configuration data returned from the repository.
///
/// Represents a route that matches incoming requests to backend clusters.
/// Routes define path-based matching and cluster selection for request forwarding.
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteData {
    pub id: RouteId,
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: String,
    pub version: i64,
    pub source: String,
    pub team: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<RouteRow> for RouteData {
    fn from(row: RouteRow) -> Self {
        Self {
            id: RouteId::from_string(row.id),
            name: row.name,
            path_prefix: row.path_prefix,
            cluster_name: row.cluster_name,
            configuration: row.configuration,
            version: row.version,
            source: row.source,
            team: row.team,
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
    pub team: Option<String>,
}

/// Update route request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRouteRequest {
    pub path_prefix: Option<String>,
    pub cluster_name: Option<String>,
    pub configuration: Option<serde_json::Value>,
    pub team: Option<Option<String>>,
}

/// Repository for route configuration persistence.
///
/// Provides CRUD operations for route resources with team-based access control.
/// Routes define how incoming requests are matched and forwarded to backend clusters.
///
/// # Example
///
/// ```rust,ignore
/// use flowplane::storage::repositories::{RouteRepository, CreateRouteRequest};
/// use serde_json::json;
///
/// let repo = RouteRepository::new(pool);
///
/// // Create a route
/// let route = repo.create(CreateRouteRequest {
///     name: "api-route".to_string(),
///     path_prefix: "/api/v1".to_string(),
///     cluster_name: "backend-cluster".to_string(),
///     configuration: json!({"retry_policy": {"num_retries": 3}}),
///     team: Some("team-alpha".to_string()),
/// }).await?;
/// ```
#[derive(Debug, Clone)]
pub struct RouteRepository {
    pool: DbPool,
}

impl RouteRepository {
    /// Creates a new route repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Creates a new route in the database.
    ///
    /// # Arguments
    ///
    /// * `request` - Route creation parameters
    ///
    /// # Returns
    ///
    /// The created [`RouteData`] with generated ID and timestamps.
    ///
    /// # Errors
    ///
    /// - [`FlowplaneError::Validation`] if configuration JSON is invalid
    /// - [`FlowplaneError::Database`] if insertion fails
    #[instrument(skip(self, request), fields(route_name = %request.name), name = "db_create_route")]
    pub async fn create(&self, request: CreateRouteRequest) -> Result<RouteData> {
        let id = RouteId::new();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid route configuration JSON: {}", e))
        })?;
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "INSERT INTO routes (id, name, path_prefix, cluster_name, configuration, version, team, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, 1, $6, $7, $8)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.path_prefix)
        .bind(&request.cluster_name)
        .bind(&configuration_json)
        .bind(&request.team)
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

    pub async fn get_by_id(&self, id: &RouteId) -> Result<RouteData> {
        let row = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, team, created_at, updated_at FROM routes WHERE id = $1"
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
            None => Err(FlowplaneError::not_found_msg(format!("Route with ID '{}' not found", id))),
        }
    }

    pub async fn get_by_name(&self, name: &str) -> Result<RouteData> {
        let row = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, team, created_at, updated_at FROM routes WHERE name = $1 ORDER BY version DESC LIMIT 1"
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
            None => {
                Err(FlowplaneError::not_found_msg(format!("Route with name '{}' not found", name)))
            }
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

    #[instrument(skip(self), name = "db_list_routes")]
    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<RouteData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, team, created_at, updated_at FROM routes ORDER BY created_at DESC LIMIT $1 OFFSET $2"
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

    /// Lists routes filtered by team names for multi-tenancy support.
    ///
    /// Critical for team-based access control. Returns routes for specified
    /// teams plus any team-agnostic routes.
    ///
    /// # Arguments
    ///
    /// * `teams` - Team identifiers to filter by. Empty list returns all routes.
    /// * `limit` - Maximum results (default: 100, max: 1000)
    /// * `offset` - Pagination offset
    ///
    /// # Errors
    ///
    /// - [`FlowplaneError::Database`] if query execution fails
    pub async fn list_by_teams(
        &self,
        teams: &[String],
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<RouteData>> {
        // If no teams specified, return all routes (admin:all or resource-level scope)
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

        let query_str = format!(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, source, team, created_at, updated_at \
             FROM routes \
             WHERE team IN ({}) OR team IS NULL \
             ORDER BY created_at DESC \
             LIMIT ${} OFFSET ${}",
            placeholders,
            teams.len() + 1,
            teams.len() + 2
        );

        let mut query = sqlx::query_as::<Sqlite, RouteRow>(&query_str);

        // Bind team names
        for team in teams {
            query = query.bind(team);
        }

        // Bind limit and offset
        query = query.bind(limit).bind(offset);

        let rows = query.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to list routes by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list routes for teams: {:?}", teams),
            }
        })?;

        Ok(rows.into_iter().map(RouteData::from).collect())
    }

    pub async fn update(&self, id: &RouteId, request: UpdateRouteRequest) -> Result<RouteData> {
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
            "UPDATE routes SET path_prefix = $1, cluster_name = $2, configuration = $3, version = $4, team = $5, updated_at = $6 WHERE id = $7"
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
            tracing::error!(error = %e, route_id = %id, "Failed to update route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update route with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!("Route with ID '{}' not found", id)));
        }

        tracing::info!(route_id = %id, route_name = %current.name, new_version = new_version, "Updated route");

        self.get_by_id(id).await
    }

    pub async fn delete(&self, id: &RouteId) -> Result<()> {
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
            return Err(FlowplaneError::not_found_msg(format!("Route with ID '{}' not found", id)));
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
