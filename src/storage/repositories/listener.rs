//! Listener repository for managing listener configurations
//!
//! This module provides CRUD operations for listener resources, handling storage,
//! retrieval, and lifecycle management of listener configuration data.

use crate::domain::ListenerId;
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

/// Internal database row structure for listeners.
///
/// Maps directly to the database schema. This is separate from [`ListenerData`]
/// to handle type conversions (e.g., String to [`ListenerId`]).
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
    pub import_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Listener configuration data returned from the repository.
///
/// Represents a listener with all its configuration including network address,
/// protocol, and xDS-compatible configuration JSON. This is the domain model
/// used throughout the application.
///
/// # Fields
///
/// - `id`: Unique identifier for the listener
/// - `name`: Human-readable name
/// - `address`: Network address (IP or hostname)
/// - `port`: Optional port number
/// - `protocol`: Protocol type (e.g., "HTTP", "HTTPS", "TCP")
/// - `configuration`: JSON-encoded xDS configuration
/// - `version`: Version number for optimistic locking
/// - `source`: API source that created this resource ("native", "gateway", "platform")
/// - `team`: Optional team identifier for multi-tenancy
/// - `created_at`: Timestamp of creation
/// - `updated_at`: Timestamp of last modification
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
    pub import_id: Option<String>,
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
            import_id: row.import_id,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Request to create a new listener.
///
/// # Example
///
/// ```rust,ignore
/// use serde_json::json;
/// use flowplane::storage::repositories::CreateListenerRequest;
///
/// let request = CreateListenerRequest {
///     name: "api-listener".to_string(),
///     address: "0.0.0.0".to_string(),
///     port: Some(8080),
///     protocol: Some("HTTP".to_string()),
///     configuration: json!({"filters": []}),
///     team: Some("team-alpha".to_string()),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateListenerRequest {
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: Option<String>,
    pub configuration: serde_json::Value,
    pub team: Option<String>,
    pub import_id: Option<String>,
}

/// Request to update an existing listener.
///
/// All fields are optional - only provided fields will be updated.
/// Uses `Option<Option<T>>` for nullable fields to distinguish between
/// "don't update" (None) and "set to null" (Some(None)).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateListenerRequest {
    pub address: Option<String>,
    pub port: Option<Option<i64>>,
    pub protocol: Option<String>,
    pub configuration: Option<serde_json::Value>,
    pub team: Option<Option<String>>,
}

/// Repository for listener configuration persistence.
///
/// Provides CRUD operations for listener resources with team-based multi-tenancy support.
/// All operations use optimistic locking via version numbers and include comprehensive
/// error handling with contextual logging.
///
/// # Multi-Tenancy
///
/// Listeners support team-scoped access:
/// - `list()`: Returns all listeners (use with care)
/// - `list_by_teams()`: Returns listeners for specific teams or team-agnostic resources
///
/// # Example
///
/// ```rust,ignore
/// use flowplane::storage::repositories::{ListenerRepository, CreateListenerRequest};
/// use serde_json::json;
///
/// let repo = ListenerRepository::new(pool);
///
/// // Create a listener
/// let listener = repo.create(CreateListenerRequest {
///     name: "api-gateway".to_string(),
///     address: "0.0.0.0".to_string(),
///     port: Some(8080),
///     protocol: Some("HTTP".to_string()),
///     configuration: json!({"filters": ["cors", "jwt"]}),
///     team: Some("team-alpha".to_string()),
/// }).await?;
///
/// // List team-scoped listeners
/// let listeners = repo.list_by_teams(&["team-alpha".to_string()], None, None).await?;
/// ```
#[derive(Debug, Clone)]
pub struct ListenerRepository {
    pool: DbPool,
}

impl ListenerRepository {
    /// Creates a new listener repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Creates a new listener in the database.
    ///
    /// Generates a unique ID, initializes version to 1, and sets timestamps.
    /// The configuration JSON is validated for serializability.
    ///
    /// # Arguments
    ///
    /// * `request` - Listener creation parameters
    ///
    /// # Returns
    ///
    /// The created [`ListenerData`] with generated ID and timestamps.
    ///
    /// # Errors
    ///
    /// - [`FlowplaneError::Validation`] if configuration JSON is invalid
    /// - [`FlowplaneError::Database`] if insertion fails (e.g., duplicate name)
    #[instrument(skip(self, request), fields(listener_name = %request.name, team = ?request.team), name = "db_create_listener")]
    pub async fn create(&self, request: CreateListenerRequest) -> Result<ListenerData> {
        let id = ListenerId::new();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid listener configuration JSON: {}", e))
        })?;
        let now = chrono::Utc::now();
        let protocol = request.protocol.unwrap_or_else(|| "HTTP".to_string());

        let result = sqlx::query(
            "INSERT INTO listeners (id, name, address, port, protocol, configuration, version, team, import_id, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, 1, $7, $8, $9, $10)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.address)
        .bind(request.port)
        .bind(&protocol)
        .bind(&configuration_json)
        .bind(&request.team)
        .bind(&request.import_id)
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

    /// Retrieves a listener by its unique ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The unique listener identifier
    ///
    /// # Returns
    ///
    /// The [`ListenerData`] if found.
    ///
    /// # Errors
    ///
    /// - [`FlowplaneError::NotFound`] if no listener exists with the given ID
    /// - [`FlowplaneError::Database`] if query execution fails
    #[instrument(skip(self), fields(listener_id = %id), name = "db_get_listener_by_id")]
    pub async fn get_by_id(&self, id: &ListenerId) -> Result<ListenerData> {
        let row = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, source, team, import_id, created_at, updated_at FROM listeners WHERE id = $1"
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

    #[instrument(skip(self), fields(listener_name = %name), name = "db_get_listener_by_name")]
    pub async fn get_by_name(&self, name: &str) -> Result<ListenerData> {
        let row = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, source, team, import_id, created_at, updated_at FROM listeners WHERE name = $1 ORDER BY version DESC LIMIT 1"
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

    #[instrument(skip(self), fields(limit = ?limit, offset = ?offset), name = "db_list_listeners")]
    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<ListenerData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, source, team, import_id, created_at, updated_at FROM listeners ORDER BY created_at DESC LIMIT $1 OFFSET $2"
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

    /// Lists listeners filtered by team names for multi-tenancy support.
    ///
    /// This method is critical for enforcing team-based access control.
    /// Returns listeners that belong to any of the specified teams, and
    /// optionally includes team-agnostic listeners (where team is NULL).
    ///
    /// # Arguments
    ///
    /// * `teams` - List of team identifiers to filter by. If empty, returns all listeners.
    /// * `include_default` - If true, also include listeners with team=NULL (default listeners)
    /// * `limit` - Maximum number of results (default: 100, max: 1000)
    /// * `offset` - Number of results to skip for pagination
    ///
    /// # Returns
    ///
    /// A vector of [`ListenerData`] matching the team filter.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Get listeners for specific teams (excluding default listeners)
    /// let listeners = repo.list_by_teams(
    ///     &["team-alpha".to_string(), "team-beta".to_string()],
    ///     false,
    ///     Some(50),
    ///     Some(0)
    /// ).await?;
    ///
    /// // Get listeners including default listeners
    /// let listeners = repo.list_by_teams(
    ///     &["team-alpha".to_string()],
    ///     true,
    ///     None,
    ///     None
    /// ).await?;
    ///
    /// // Get all listeners (admin access)
    /// let all_listeners = repo.list_by_teams(&[], true, None, None).await?;
    /// ```
    ///
    /// # Errors
    ///
    /// - [`FlowplaneError::Database`] if query execution fails
    #[instrument(skip(self), fields(teams = ?teams, limit = ?limit, offset = ?offset), name = "db_list_listeners_by_teams")]
    pub async fn list_by_teams(
        &self,
        teams: &[String],
        _include_default: bool, // Deprecated: always includes default resources
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

        // Always include NULL team listeners (default resources)
        let where_clause = format!("WHERE team IN ({}) OR team IS NULL", placeholders);

        let query_str = format!(
            "SELECT id, name, address, port, protocol, configuration, version, source, team, import_id, created_at, updated_at \
             FROM listeners \
             {} \
             ORDER BY created_at DESC \
             LIMIT ${} OFFSET ${}",
            where_clause,
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

    /// Count listeners created from a specific import (tracked via import_id in the configuration JSON)
    #[instrument(skip(self), fields(import_id = %import_id), name = "db_count_listeners_by_import")]
    pub async fn count_by_import(&self, import_id: &str) -> Result<i64> {
        sqlx::query_scalar::<Sqlite, i64>(
            "SELECT COUNT(*) FROM listeners WHERE import_id = $1",
        )
        .bind(import_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, import_id = %import_id, "Failed to count listeners by import");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count listeners for import '{}'", import_id),
            }
        })
    }

    #[instrument(skip(self, request), fields(listener_id = %id), name = "db_update_listener")]
    pub async fn update(
        &self,
        id: &ListenerId,
        request: UpdateListenerRequest,
    ) -> Result<ListenerData> {
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

    #[instrument(skip(self), fields(listener_id = %id), name = "db_delete_listener")]
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

    #[instrument(skip(self), fields(listener_name = %name), name = "db_delete_listener_by_name")]
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

    #[instrument(skip(self), fields(listener_name = %name), name = "db_exists_listener_by_name")]
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

    #[instrument(skip(self), name = "db_count_listeners")]
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
