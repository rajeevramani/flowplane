use crate::domain::{FilterId, ListenerId, RouteConfigId, RouteId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

// ============================================================================
// Installation/Configuration Types (for Install/Configure Redesign)
// ============================================================================

/// Scope type for filter configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FilterScopeType {
    /// Apply to entire route configuration
    RouteConfig,
    /// Apply to specific virtual host
    VirtualHost,
    /// Apply to specific route
    Route,
}

impl std::fmt::Display for FilterScopeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterScopeType::RouteConfig => write!(f, "route-config"),
            FilterScopeType::VirtualHost => write!(f, "virtual-host"),
            FilterScopeType::Route => write!(f, "route"),
        }
    }
}

/// Single installation item showing where a filter is installed
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterInstallation {
    pub listener_id: String,
    pub listener_name: String,
    pub listener_address: String,
    pub order: i64,
}

/// Single configuration item showing where a filter is configured
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterConfiguration {
    pub scope_type: FilterScopeType,
    pub scope_id: String,
    pub scope_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

#[derive(Debug, Clone, FromRow)]
struct FilterRow {
    pub id: String,
    pub name: String,
    pub filter_type: String,
    pub description: Option<String>,
    pub configuration: String,
    pub version: i64,
    pub source: String,
    pub team: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterData {
    pub id: FilterId,
    pub name: String,
    pub filter_type: String,
    pub description: Option<String>,
    pub configuration: String,
    pub version: i64,
    pub source: String,
    pub team: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<FilterRow> for FilterData {
    fn from(row: FilterRow) -> Self {
        Self {
            id: FilterId::from_string(row.id),
            name: row.name,
            filter_type: row.filter_type,
            description: row.description,
            configuration: row.configuration,
            version: row.version,
            source: row.source,
            team: row.team,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl crate::api::handlers::TeamOwned for FilterData {
    fn team(&self) -> Option<&str> {
        Some(&self.team)
    }

    fn resource_name(&self) -> &str {
        &self.name
    }

    fn resource_type() -> &'static str {
        "Filter"
    }

    fn resource_type_metric() -> &'static str {
        "filters"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFilterRequest {
    pub name: String,
    pub filter_type: String,
    pub description: Option<String>,
    pub configuration: String,
    pub team: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFilterRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub configuration: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FilterRepository {
    pool: DbPool,
}

impl FilterRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    #[instrument(skip(self, request), fields(filter_name = %request.name), name = "db_create_filter")]
    pub async fn create(&self, request: CreateFilterRequest) -> Result<FilterData> {
        let id = FilterId::new();
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "INSERT INTO filters (id, name, filter_type, description, configuration, team, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(id.as_str())
        .bind(&request.name)
        .bind(&request.filter_type)
        .bind(&request.description)
        .bind(&request.configuration)
        .bind(&request.team)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_name = %request.name, "Failed to create filter");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create filter: {}", request.name),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::internal("Failed to create filter"));
        }

        self.get_by_id(&id).await
    }

    #[instrument(skip(self), fields(filter_id = %id), name = "db_get_filter_by_id")]
    pub async fn get_by_id(&self, id: &FilterId) -> Result<FilterData> {
        let row = sqlx::query_as::<Sqlite, FilterRow>(
            "SELECT id, name, filter_type, description, configuration, version, source, team, created_at, updated_at \
             FROM filters WHERE id = $1"
        )
        .bind(id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                FlowplaneError::not_found("Filter", id.as_str())
            }
            _ => {
                tracing::error!(error = %e, filter_id = %id, "Failed to get filter by ID");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to get filter by ID: {}", id.as_str()),
                }
            }
        })?;

        Ok(FilterData::from(row))
    }

    #[instrument(skip(self), fields(filter_name = %name), name = "db_get_filter_by_name")]
    pub async fn get_by_name(&self, name: &str) -> Result<FilterData> {
        let row = sqlx::query_as::<Sqlite, FilterRow>(
            "SELECT id, name, filter_type, description, configuration, version, source, team, created_at, updated_at \
             FROM filters WHERE name = $1"
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                FlowplaneError::not_found("Filter", name)
            }
            _ => {
                tracing::error!(error = %e, filter_name = %name, "Failed to get filter by name");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to get filter by name: {}", name),
                }
            }
        })?;

        Ok(FilterData::from(row))
    }

    #[instrument(skip(self), fields(limit = ?limit, offset = ?offset), name = "db_list_filters")]
    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<FilterData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, FilterRow>(
            "SELECT id, name, filter_type, description, configuration, version, source, team, created_at, updated_at \
             FROM filters ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list filters");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list filters".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(FilterData::from).collect())
    }

    #[instrument(skip(self), fields(teams = ?teams, limit = ?limit, offset = ?offset), name = "db_list_filters_by_teams")]
    pub async fn list_by_teams(
        &self,
        teams: &[String],
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<FilterData>> {
        if teams.is_empty() {
            return self.list(limit, offset).await;
        }

        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let placeholders = teams
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let where_clause = format!("WHERE team IN ({})", placeholders);

        let query_str = format!(
            "SELECT id, name, filter_type, description, configuration, version, source, team, created_at, updated_at \
             FROM filters \
             {} \
             ORDER BY created_at DESC \
             LIMIT ${} OFFSET ${}",
            where_clause,
            teams.len() + 1,
            teams.len() + 2
        );

        let mut query = sqlx::query_as::<Sqlite, FilterRow>(&query_str);

        for team in teams {
            query = query.bind(team);
        }

        query = query.bind(limit).bind(offset);

        let rows = query.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to list filters by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list filters for teams: {:?}", teams),
            }
        })?;

        Ok(rows.into_iter().map(FilterData::from).collect())
    }

    #[instrument(skip(self, request), fields(filter_id = %id), name = "db_update_filter")]
    pub async fn update(&self, id: &FilterId, request: UpdateFilterRequest) -> Result<FilterData> {
        let current = self.get_by_id(id).await?;

        let new_name = request.name.unwrap_or(current.name);
        let new_description = request.description.or(current.description);
        let new_configuration = request.configuration.unwrap_or(current.configuration);

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE filters SET name = $1, description = $2, configuration = $3, version = $4, updated_at = $5 WHERE id = $6"
        )
        .bind(&new_name)
        .bind(&new_description)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(now)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %id, "Failed to update filter");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update filter: {}", id.as_str()),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("Filter", id.as_str()));
        }

        self.get_by_id(id).await
    }

    #[instrument(skip(self), fields(filter_id = %id), name = "db_delete_filter")]
    pub async fn delete(&self, id: &FilterId) -> Result<()> {
        let result = sqlx::query("DELETE FROM filters WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, filter_id = %id, "Failed to delete filter");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete filter: {}", id.as_str()),
                }
            })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("Filter", id.as_str()));
        }

        Ok(())
    }

    #[instrument(skip(self), fields(team = %team, filter_name = %name), name = "db_exists_filter_by_name")]
    pub async fn exists_by_name(&self, team: &str, name: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM filters WHERE team = $1 AND name = $2"
        )
        .bind(team)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, filter_name = %name, "Failed to check filter existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check filter existence: {}", name),
            }
        })?;

        Ok(count > 0)
    }

    // Filter attachment methods

    #[instrument(skip(self), fields(route_id = %route_id, filter_id = %filter_id, order = %order), name = "db_attach_filter_to_route")]
    pub async fn attach_to_route(
        &self,
        route_id: &RouteId,
        filter_id: &FilterId,
        order: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now();

        sqlx::query(
            "INSERT INTO route_filters (route_id, filter_id, filter_order, created_at) VALUES ($1, $2, $3, $4)"
        )
        .bind(route_id.as_str())
        .bind(filter_id.as_str())
        .bind(order)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %route_id, filter_id = %filter_id, "Failed to attach filter to route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to attach filter {} to route {}", filter_id.as_str(), route_id.as_str()),
            }
        })?;

        Ok(())
    }

    #[instrument(skip(self), fields(route_id = %route_id, filter_id = %filter_id), name = "db_detach_filter_from_route")]
    pub async fn detach_from_route(&self, route_id: &RouteId, filter_id: &FilterId) -> Result<()> {
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
                context: format!("Failed to detach filter {} from route {}", filter_id.as_str(), route_id.as_str()),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("Route filter attachment", ""));
        }

        Ok(())
    }

    #[instrument(skip(self), fields(route_id = %route_id), name = "db_list_route_filters")]
    pub async fn list_route_filters(&self, route_id: &RouteId) -> Result<Vec<FilterData>> {
        let rows = sqlx::query_as::<Sqlite, FilterRow>(
            "SELECT f.id, f.name, f.filter_type, f.description, f.configuration, f.version, f.source, f.team, f.created_at, f.updated_at \
             FROM filters f \
             INNER JOIN route_filters rf ON f.id = rf.filter_id \
             WHERE rf.route_id = $1 \
             ORDER BY rf.filter_order ASC"
        )
        .bind(route_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %route_id, "Failed to list route filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list filters for route: {}", route_id.as_str()),
            }
        })?;

        Ok(rows.into_iter().map(FilterData::from).collect())
    }

    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_list_filter_routes")]
    pub async fn list_filter_routes(&self, filter_id: &FilterId) -> Result<Vec<RouteId>> {
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
                context: format!("Failed to list routes for filter: {}", filter_id.as_str()),
            }
        })?;

        Ok(route_ids.into_iter().map(RouteId::from_string).collect())
    }

    #[instrument(skip(self), name = "db_count_filters")]
    pub async fn count(&self) -> Result<i64> {
        let count = sqlx::query_scalar("SELECT COUNT(*) FROM filters")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to count filters");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to count filters".to_string(),
                }
            })?;

        Ok(count)
    }

    // Route Config filter attachment methods (for RouteConfiguration-level filters)

    #[instrument(skip(self, settings), fields(route_config_id = %route_config_id, filter_id = %filter_id, order = %order), name = "db_attach_filter_to_route_config")]
    pub async fn attach_to_route_config(
        &self,
        route_config_id: &RouteConfigId,
        filter_id: &FilterId,
        order: i64,
        settings: Option<serde_json::Value>,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let settings_json = settings.as_ref().map(|s| s.to_string());

        sqlx::query(
            "INSERT INTO route_config_filters (route_config_id, filter_id, filter_order, created_at, settings) VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(route_config_id.as_str())
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
                                route_config_id = %route_config_id,
                                filter_id = %filter_id,
                                order = %order,
                                "Filter order already in use on route config"
                            );
                            return FlowplaneError::Conflict {
                                message: format!(
                                    "Filter order {} is already in use on this route config. Choose a different order value.",
                                    order
                                ),
                                resource_type: "route_config_filter".to_string(),
                            };
                        } else if msg.contains("filter_id") {
                            tracing::warn!(
                                route_config_id = %route_config_id,
                                filter_id = %filter_id,
                                "Filter already attached to route config"
                            );
                            return FlowplaneError::Conflict {
                                message: "This filter is already attached to the route config.".to_string(),
                                resource_type: "route_config_filter".to_string(),
                            };
                        }
                    }
                }
            }
            tracing::error!(error = %e, route_config_id = %route_config_id, filter_id = %filter_id, "Failed to attach filter to route config");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to attach filter {} to route config {}", filter_id.as_str(), route_config_id.as_str()),
            }
        })?;

        Ok(())
    }

    #[instrument(skip(self), fields(route_config_id = %route_config_id, filter_id = %filter_id), name = "db_detach_filter_from_route_config")]
    pub async fn detach_from_route_config(
        &self,
        route_config_id: &RouteConfigId,
        filter_id: &FilterId,
    ) -> Result<()> {
        let result = sqlx::query(
            "DELETE FROM route_config_filters WHERE route_config_id = $1 AND filter_id = $2"
        )
        .bind(route_config_id.as_str())
        .bind(filter_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %route_config_id, filter_id = %filter_id, "Failed to detach filter from route config");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to detach filter {} from route config {}", filter_id.as_str(), route_config_id.as_str()),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("Route config filter attachment", ""));
        }

        Ok(())
    }

    /// Get the next available order for a route config's filter chain.
    #[instrument(skip(self), fields(route_config_id = %route_config_id), name = "db_next_route_config_filter_order")]
    pub async fn get_next_route_config_filter_order(
        &self,
        route_config_id: &RouteConfigId,
    ) -> Result<i64> {
        let max_order: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(filter_order) FROM route_config_filters WHERE route_config_id = $1",
        )
        .bind(route_config_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %route_config_id, "Failed to get max filter order");
            FlowplaneError::Database {
                source: e,
                context: format!(
                    "Failed to get max filter order for route config '{}'",
                    route_config_id
                ),
            }
        })?;

        Ok(max_order.unwrap_or(0) + 1)
    }

    #[instrument(skip(self), fields(route_config_id = %route_config_id), name = "db_list_route_config_filters")]
    pub async fn list_route_config_filters(
        &self,
        route_config_id: &RouteConfigId,
    ) -> Result<Vec<FilterData>> {
        let rows = sqlx::query_as::<Sqlite, FilterRow>(
            "SELECT f.id, f.name, f.filter_type, f.description, f.configuration, f.version, f.source, f.team, f.created_at, f.updated_at \
             FROM filters f \
             INNER JOIN route_config_filters rcf ON f.id = rcf.filter_id \
             WHERE rcf.route_config_id = $1 \
             ORDER BY rcf.filter_order ASC"
        )
        .bind(route_config_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %route_config_id, "Failed to list route config filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list filters for route config: {}", route_config_id.as_str()),
            }
        })?;

        Ok(rows.into_iter().map(FilterData::from).collect())
    }

    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_list_filter_route_configs")]
    pub async fn list_filter_route_configs(
        &self,
        filter_id: &FilterId,
    ) -> Result<Vec<RouteConfigId>> {
        let route_config_ids: Vec<String> = sqlx::query_scalar(
            "SELECT route_config_id FROM route_config_filters WHERE filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to list route configs using filter");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list route configs for filter: {}", filter_id.as_str()),
            }
        })?;

        Ok(route_config_ids.into_iter().map(RouteConfigId::from_string).collect())
    }

    // Listener filter attachment methods

    #[instrument(skip(self), fields(listener_id = %listener_id, filter_id = %filter_id, order = %order), name = "db_attach_filter_to_listener")]
    pub async fn attach_to_listener(
        &self,
        listener_id: &ListenerId,
        filter_id: &FilterId,
        order: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now();

        sqlx::query(
            "INSERT INTO listener_filters (listener_id, filter_id, filter_order, created_at) VALUES ($1, $2, $3, $4)"
        )
        .bind(listener_id.as_str())
        .bind(filter_id.as_str())
        .bind(order)
        .bind(now)
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
                                listener_id = %listener_id,
                                filter_id = %filter_id,
                                order = %order,
                                "Filter order already in use on listener"
                            );
                            return FlowplaneError::Conflict {
                                message: format!(
                                    "Filter order {} is already in use on this listener. Choose a different order value.",
                                    order
                                ),
                                resource_type: "listener_filter".to_string(),
                            };
                        } else if msg.contains("filter_id") {
                            tracing::warn!(
                                listener_id = %listener_id,
                                filter_id = %filter_id,
                                "Filter already attached to listener"
                            );
                            return FlowplaneError::Conflict {
                                message: "This filter is already attached to the listener.".to_string(),
                                resource_type: "listener_filter".to_string(),
                            };
                        }
                    }
                }
            }
            tracing::error!(error = %e, listener_id = %listener_id, filter_id = %filter_id, "Failed to attach filter to listener");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to attach filter {} to listener {}", filter_id.as_str(), listener_id.as_str()),
            }
        })?;

        Ok(())
    }

    #[instrument(skip(self), fields(listener_id = %listener_id, filter_id = %filter_id), name = "db_detach_filter_from_listener")]
    pub async fn detach_from_listener(
        &self,
        listener_id: &ListenerId,
        filter_id: &FilterId,
    ) -> Result<()> {
        let result = sqlx::query(
            "DELETE FROM listener_filters WHERE listener_id = $1 AND filter_id = $2"
        )
        .bind(listener_id.as_str())
        .bind(filter_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, filter_id = %filter_id, "Failed to detach filter from listener");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to detach filter {} from listener {}", filter_id.as_str(), listener_id.as_str()),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("Listener filter attachment", ""));
        }

        Ok(())
    }

    #[instrument(skip(self), fields(listener_id = %listener_id), name = "db_list_listener_filters")]
    pub async fn list_listener_filters(&self, listener_id: &ListenerId) -> Result<Vec<FilterData>> {
        let rows = sqlx::query_as::<Sqlite, FilterRow>(
            "SELECT f.id, f.name, f.filter_type, f.description, f.configuration, f.version, f.source, f.team, f.created_at, f.updated_at \
             FROM filters f \
             INNER JOIN listener_filters lf ON f.id = lf.filter_id \
             WHERE lf.listener_id = $1 \
             ORDER BY lf.filter_order ASC"
        )
        .bind(listener_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, "Failed to list listener filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list filters for listener: {}", listener_id.as_str()),
            }
        })?;

        Ok(rows.into_iter().map(FilterData::from).collect())
    }

    /// Get the next available order for a listener's filter chain.
    #[instrument(skip(self), fields(listener_id = %listener_id), name = "db_next_listener_filter_order")]
    pub async fn get_next_listener_filter_order(&self, listener_id: &ListenerId) -> Result<i64> {
        let max_order: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(filter_order) FROM listener_filters WHERE listener_id = $1",
        )
        .bind(listener_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %listener_id, "Failed to get max filter order");
            FlowplaneError::Database {
                source: e,
                context: format!(
                    "Failed to get max filter order for listener '{}'",
                    listener_id
                ),
            }
        })?;

        Ok(max_order.unwrap_or(0) + 1)
    }

    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_list_filter_listeners")]
    pub async fn list_filter_listeners(&self, filter_id: &FilterId) -> Result<Vec<ListenerId>> {
        let listener_ids: Vec<String> = sqlx::query_scalar(
            "SELECT listener_id FROM listener_filters WHERE filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to list listeners using filter");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list listeners for filter: {}", filter_id.as_str()),
            }
        })?;

        Ok(listener_ids.into_iter().map(ListenerId::from_string).collect())
    }

    /// Count total attachments (routes + listeners) for a filter
    /// Used to prevent deletion of attached filters
    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_count_filter_attachments")]
    pub async fn count_attachments(&self, filter_id: &FilterId) -> Result<i64> {
        let route_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM route_filters WHERE filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to count route attachments");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count route attachments for filter: {}", filter_id.as_str()),
            }
        })?;

        let listener_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM listener_filters WHERE filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to count listener attachments");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count listener attachments for filter: {}", filter_id.as_str()),
            }
        })?;

        Ok(route_count + listener_count)
    }

    /// Find all filters that reference a specific cluster in their configuration.
    ///
    /// Scans filter configuration JSON for cluster references in:
    /// - OAuth2: `token_endpoint.cluster`
    /// - JWT: `providers[].jwks.http_uri.cluster`
    /// - ExtAuthz: `service.http.server_uri.cluster`
    ///
    /// Returns tuples of (FilterData, reference_path) for each match.
    #[instrument(skip(self), fields(cluster_name = %cluster_name), name = "db_find_filters_by_cluster")]
    pub async fn find_by_cluster(&self, cluster_name: &str) -> Result<Vec<(FilterData, String)>> {
        // Get all filters and check their JSON configuration for cluster references
        let rows = sqlx::query_as::<Sqlite, FilterRow>(
            "SELECT id, name, filter_type, description, configuration, version, source, team, created_at, updated_at \
             FROM filters WHERE filter_type IN ('oauth2', 'jwt_auth', 'ext_authz')"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_name = %cluster_name, "Failed to query filters for cluster references");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to find filters using cluster '{}'", cluster_name),
            }
        })?;

        let mut results = Vec::new();

        for row in rows {
            let filter = FilterData::from(row.clone());

            // Parse the configuration JSON and check for cluster references
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&filter.configuration) {
                let reference_path = match filter.filter_type.as_str() {
                    "oauth2" => {
                        // Check token_endpoint.cluster
                        if let Some(token_endpoint) =
                            config.get("config").and_then(|c| c.get("token_endpoint"))
                        {
                            if let Some(cluster) =
                                token_endpoint.get("cluster").and_then(|c| c.as_str())
                            {
                                if cluster == cluster_name {
                                    Some("token_endpoint.cluster".to_string())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    "jwt_auth" => {
                        // Check providers[].jwks.http_uri.cluster
                        if let Some(providers) =
                            config.get("config").and_then(|c| c.get("providers"))
                        {
                            if let Some(providers_map) = providers.as_object() {
                                for (provider_name, provider_config) in providers_map {
                                    if let Some(jwks) = provider_config.get("jwks") {
                                        if let Some(http_uri) = jwks.get("http_uri") {
                                            if let Some(cluster) =
                                                http_uri.get("cluster").and_then(|c| c.as_str())
                                            {
                                                if cluster == cluster_name {
                                                    return Ok(vec![(
                                                        filter,
                                                        format!(
                                                            "providers.{}.jwks.http_uri.cluster",
                                                            provider_name
                                                        ),
                                                    )]);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        None
                    }
                    "ext_authz" => {
                        // Check service.http.server_uri.cluster
                        if let Some(service) = config.get("config").and_then(|c| c.get("service")) {
                            if let Some(http) = service.get("http") {
                                if let Some(server_uri) = http.get("server_uri") {
                                    if let Some(cluster) =
                                        server_uri.get("cluster").and_then(|c| c.as_str())
                                    {
                                        if cluster == cluster_name {
                                            Some("service.http.server_uri.cluster".to_string())
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                if let Some(path) = reference_path {
                    results.push((filter, path));
                }
            }
        }

        Ok(results)
    }

    // ============================================================================
    // Filter Installation/Configuration Methods (Install/Configure Redesign)
    // ============================================================================

    /// List all listeners a filter is installed on with metadata.
    ///
    /// Returns installation details including listener name, address, and order.
    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_list_filter_installations")]
    pub async fn list_filter_installations(
        &self,
        filter_id: &FilterId,
    ) -> Result<Vec<FilterInstallation>> {
        #[derive(FromRow)]
        struct InstallationRow {
            listener_id: String,
            listener_name: String,
            listener_address: String,
            filter_order: i64,
        }

        let rows = sqlx::query_as::<Sqlite, InstallationRow>(
            "SELECT l.id as listener_id, l.name as listener_name, l.address as listener_address, lf.filter_order \
             FROM listener_filters lf \
             INNER JOIN listeners l ON lf.listener_id = l.id \
             WHERE lf.filter_id = $1 \
             ORDER BY l.name ASC"
        )
        .bind(filter_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to list filter installations");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list installations for filter: {}", filter_id.as_str()),
            }
        })?;

        Ok(rows
            .into_iter()
            .map(|row| FilterInstallation {
                listener_id: row.listener_id,
                listener_name: row.listener_name,
                listener_address: row.listener_address,
                order: row.filter_order,
            })
            .collect())
    }

    /// List all configurations (scopes) for a filter.
    ///
    /// Returns configurations from route_config_filters, virtual_host_filters, and route_filters.
    #[instrument(skip(self), fields(filter_id = %filter_id), name = "db_list_filter_configurations")]
    pub async fn list_filter_configurations(
        &self,
        filter_id: &FilterId,
    ) -> Result<Vec<FilterConfiguration>> {
        let mut configurations = Vec::new();

        // Get route config configurations
        #[derive(FromRow)]
        struct RouteConfigRow {
            #[allow(dead_code)]
            route_config_id: String,
            route_config_name: String,
            settings: Option<String>,
        }

        let route_config_rows = sqlx::query_as::<Sqlite, RouteConfigRow>(
            "SELECT rc.id as route_config_id, rc.name as route_config_name, rcf.settings \
             FROM route_config_filters rcf \
             INNER JOIN route_configs rc ON rcf.route_config_id = rc.id \
             WHERE rcf.filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to list route config configurations");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list route config configurations for filter: {}", filter_id.as_str()),
            }
        })?;

        for row in route_config_rows {
            configurations.push(FilterConfiguration {
                scope_type: FilterScopeType::RouteConfig,
                scope_id: row.route_config_name.clone(),
                scope_name: row.route_config_name,
                settings: row.settings.and_then(|s| serde_json::from_str(&s).ok()),
            });
        }

        // Get virtual host configurations
        // Join through virtual_hosts to get the route config and virtual host name
        #[derive(FromRow)]
        struct VirtualHostRow {
            #[allow(dead_code)]
            route_config_id: String,
            route_config_name: String,
            virtual_host_name: String,
            settings: Option<String>,
        }

        let vhost_rows = sqlx::query_as::<Sqlite, VirtualHostRow>(
            "SELECT vh.route_config_id, rc.name as route_config_name, vh.name as virtual_host_name, vhf.settings \
             FROM virtual_host_filters vhf \
             INNER JOIN virtual_hosts vh ON vhf.virtual_host_id = vh.id \
             INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
             WHERE vhf.filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to list virtual host configurations");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list virtual host configurations for filter: {}", filter_id.as_str()),
            }
        })?;

        for row in vhost_rows {
            configurations.push(FilterConfiguration {
                scope_type: FilterScopeType::VirtualHost,
                scope_id: format!("{}/{}", row.route_config_name, row.virtual_host_name),
                scope_name: row.virtual_host_name,
                settings: row.settings.and_then(|s| serde_json::from_str(&s).ok()),
            });
        }

        // Get route configurations
        // Join through routes and virtual_hosts to get the full hierarchy
        #[derive(FromRow)]
        struct RouteRow {
            #[allow(dead_code)]
            route_config_id: String,
            route_config_name: String,
            virtual_host_name: String,
            route_name: String,
            settings: Option<String>,
        }

        let route_rows = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT vh.route_config_id, rc.name as route_config_name, \
                    vh.name as virtual_host_name, r.name as route_name, rf.settings \
             FROM route_filters rf \
             INNER JOIN routes r ON rf.route_id = r.id \
             INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id \
             INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
             WHERE rf.filter_id = $1"
        )
        .bind(filter_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %filter_id, "Failed to list route configurations");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list route configurations for filter: {}", filter_id.as_str()),
            }
        })?;

        for row in route_rows {
            configurations.push(FilterConfiguration {
                scope_type: FilterScopeType::Route,
                scope_id: format!(
                    "{}/{}/{}",
                    row.route_config_name, row.virtual_host_name, row.route_name
                ),
                scope_name: row.route_name,
                settings: row.settings.and_then(|s| serde_json::from_str(&s).ok()),
            });
        }

        Ok(configurations)
    }
}
