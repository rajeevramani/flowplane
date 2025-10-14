//! Listener repository for managing listener configurations
//!
//! This module provides CRUD operations for listener resources, handling storage,
//! retrieval, and lifecycle management of listener configuration data.

use crate::domain::ListenerId;
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};

/// Database row structure for listeners
#[derive(Debug, Clone, FromRow)]
struct ListenerRow {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: String,
    pub configuration: String,
    pub version: i64,
    pub source: String,
    pub team: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Listener configuration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerData {
    pub id: ListenerId,
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: String,
    pub configuration: String,
    pub version: i64,
    pub source: String,
    pub team: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ListenerRow> for ListenerData {
    fn from(row: ListenerRow) -> Self {
        Self {
            id: ListenerId::from_string(row.id),
            name: row.name,
            address: row.address,
            port: row.port,
            protocol: row.protocol,
            configuration: row.configuration,
            version: row.version,
            source: row.source,
            team: row.team,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Create listener request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateListenerRequest {
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: Option<String>,
    pub configuration: serde_json::Value,
    pub team: Option<String>,
}

/// Update listener request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateListenerRequest {
    pub address: Option<String>,
    pub port: Option<Option<i64>>,
    pub protocol: Option<String>,
    pub configuration: Option<serde_json::Value>,
    pub team: Option<Option<String>>,
}

/// Repository for listener data access
#[derive(Debug, Clone)]
pub struct ListenerRepository {
    pool: DbPool,
}

impl ListenerRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, request: CreateListenerRequest) -> Result<ListenerData> {
        let id = ListenerId::new();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid listener configuration JSON: {}", e))
        })?;
        let now = chrono::Utc::now();
        let protocol = request.protocol.unwrap_or_else(|| "HTTP".to_string());

        let result = sqlx::query(
            "INSERT INTO listeners (id, name, address, port, protocol, configuration, version, team, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, 1, $7, $8, $9)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.address)
        .bind(request.port)
        .bind(&protocol)
        .bind(&configuration_json)
        .bind(&request.team)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_name = %request.name, "Failed to create listener");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create listener '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create listener"));
        }

        tracing::info!(listener_id = %id, listener_name = %request.name, "Created new listener");

        self.get_by_id(&id).await
    }

    pub async fn get_by_id(&self, id: &ListenerId) -> Result<ListenerData> {
        let row = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, source, team, created_at, updated_at FROM listeners WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %id, "Failed to get listener by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get listener with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(ListenerData::from(row)),
            None => {
                Err(FlowplaneError::not_found_msg(format!("Listener with ID '{}' not found", id)))
            }
        }
    }

    pub async fn get_by_name(&self, name: &str) -> Result<ListenerData> {
        let row = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, source, team, created_at, updated_at FROM listeners WHERE name = $1 ORDER BY version DESC LIMIT 1"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_name = %name, "Failed to get listener by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get listener with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(ListenerData::from(row)),
            None => Err(FlowplaneError::not_found_msg(format!(
                "Listener with name '{}' not found",
                name
            ))),
        }
    }

    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<ListenerData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, source, team, created_at, updated_at FROM listeners ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list listeners");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list listeners".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(ListenerData::from).collect())
    }

    /// List listeners filtered by team names (for team-scoped tokens)
    /// If teams list is empty, returns all listeners (for admin:all or resource-level scopes)
    pub async fn list_by_teams(
        &self,
        teams: &[String],
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<ListenerData>> {
        // If no teams specified, return all listeners (admin:all or resource-level scope)
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
            "SELECT id, name, address, port, protocol, configuration, version, source, team, created_at, updated_at \
             FROM listeners \
             WHERE team IN ({}) OR team IS NULL \
             ORDER BY created_at DESC \
             LIMIT ${} OFFSET ${}",
            placeholders,
            teams.len() + 1,
            teams.len() + 2
        );

        let mut query = sqlx::query_as::<Sqlite, ListenerRow>(&query_str);

        // Bind team names
        for team in teams {
            query = query.bind(team);
        }

        // Bind limit and offset
        query = query.bind(limit).bind(offset);

        let rows = query.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to list listeners by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list listeners for teams: {:?}", teams),
            }
        })?;

        Ok(rows.into_iter().map(ListenerData::from).collect())
    }

    pub async fn update(&self, id: &ListenerId, request: UpdateListenerRequest) -> Result<ListenerData> {
        let current = self.get_by_id(id).await?;

        let current_address = current.address.clone();
        let current_protocol = current.protocol.clone();
        let current_configuration = current.configuration.clone();
        let current_name = current.name.clone();

        let new_address = request.address.unwrap_or(current_address);
        let new_port = match request.port {
            Some(value) => value,
            None => current.port,
        };
        let new_protocol = request.protocol.unwrap_or(current_protocol);
        let new_configuration = if let Some(config) = request.configuration {
            serde_json::to_string(&config).map_err(|e| {
                FlowplaneError::validation(format!("Invalid listener configuration JSON: {}", e))
            })?
        } else {
            current_configuration
        };
        let new_team = request.team.unwrap_or(current.team);

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE listeners SET address = $1, port = $2, protocol = $3, configuration = $4, version = $5, team = $6, updated_at = $7 WHERE id = $8"
        )
        .bind(&new_address)
        .bind(new_port)
        .bind(&new_protocol)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(&new_team)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %id, "Failed to update listener");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update listener with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Listener with ID '{}' not found",
                id
            )));
        }

        tracing::info!(listener_id = %id, listener_name = %current_name, new_version = new_version, "Updated listener");

        self.get_by_id(id).await
    }

    pub async fn delete(&self, id: &ListenerId) -> Result<()> {
        let listener = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM listeners WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, listener_id = %id, "Failed to delete listener");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete listener with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Listener with ID '{}' not found",
                id
            )));
        }

        tracing::info!(listener_id = %id, listener_name = %listener.name, "Deleted listener");

        Ok(())
    }

    pub async fn delete_by_name(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM listeners WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
            tracing::error!(error = %e, listener_name = %name, "Failed to delete listener by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to delete listener '{}'", name),
            }
        })?;

        Ok(())
    }

    pub async fn exists_by_name(&self, name: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<Sqlite, i64>("SELECT COUNT(*) FROM listeners WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, listener_name = %name, "Failed to check listener existence");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to check existence of listener '{}'", name),
                }
            })?;

        Ok(count > 0)
    }

    pub async fn count(&self) -> Result<i64> {
        let count = sqlx::query_scalar::<Sqlite, i64>("SELECT COUNT(*) FROM listeners")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get listener count");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to get listener count".to_string(),
                }
            })?;

        Ok(count)
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}
