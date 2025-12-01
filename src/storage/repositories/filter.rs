use crate::domain::{FilterId, RouteId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

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
    pub async fn update(
        &self,
        id: &FilterId,
        request: UpdateFilterRequest,
    ) -> Result<FilterData> {
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
    pub async fn detach_from_route(
        &self,
        route_id: &RouteId,
        filter_id: &FilterId,
    ) -> Result<()> {
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
}
