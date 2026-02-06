//! Dataplane repository for managing dataplane configurations
//!
//! This module provides CRUD operations for dataplane resources, representing Envoy
//! instances with their gateway_host for MCP tool execution.

use crate::domain::DataplaneId;
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;

/// Internal database row structure for dataplanes.
#[derive(Debug, Clone, FromRow)]
struct DataplaneRow {
    pub id: String,
    pub team: String,
    pub name: String,
    pub gateway_host: Option<String>,
    pub description: Option<String>,
    pub certificate_serial: Option<String>,
    pub certificate_expires_at: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Dataplane configuration data returned from the repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataplaneData {
    pub id: DataplaneId,
    pub team: String,
    pub name: String,
    pub gateway_host: Option<String>,
    pub description: Option<String>,
    /// Serial number of the currently issued certificate (if any)
    pub certificate_serial: Option<String>,
    /// Expiration time of the current certificate (if any)
    pub certificate_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<DataplaneRow> for DataplaneData {
    fn from(row: DataplaneRow) -> Self {
        // Parse certificate_expires_at from RFC3339 string to DateTime
        let certificate_expires_at = row.certificate_expires_at.and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(&s).map(|dt| dt.with_timezone(&chrono::Utc)).ok()
        });

        Self {
            id: DataplaneId::from_string(row.id),
            team: row.team,
            name: row.name,
            gateway_host: row.gateway_host,
            description: row.description,
            certificate_serial: row.certificate_serial,
            certificate_expires_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl crate::api::handlers::TeamOwned for DataplaneData {
    fn team(&self) -> Option<&str> {
        Some(&self.team)
    }

    fn resource_name(&self) -> &str {
        &self.name
    }

    fn resource_type() -> &'static str {
        "Dataplane"
    }

    fn resource_type_metric() -> &'static str {
        "dataplanes"
    }
}

/// Request to create a new dataplane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDataplaneRequest {
    pub team: String,
    pub name: String,
    pub gateway_host: Option<String>,
    pub description: Option<String>,
}

/// Request to update an existing dataplane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDataplaneRequest {
    pub gateway_host: Option<Option<String>>,
    pub description: Option<Option<String>>,
}

/// Repository for dataplane configuration persistence.
#[derive(Debug, Clone)]
pub struct DataplaneRepository {
    pool: DbPool,
}

impl DataplaneRepository {
    /// Creates a new dataplane repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Creates a new dataplane in the database.
    #[instrument(skip(self, request), fields(dataplane_name = %request.name, team = %request.team), name = "db_create_dataplane")]
    pub async fn create(&self, request: CreateDataplaneRequest) -> Result<DataplaneData> {
        let id = DataplaneId::new();
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "INSERT INTO dataplanes (id, team, name, gateway_host, description, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7)"
        )
        .bind(&id)
        .bind(&request.team)
        .bind(&request.name)
        .bind(&request.gateway_host)
        .bind(&request.description)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, dataplane_name = %request.name, "Failed to create dataplane");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create dataplane '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create dataplane"));
        }

        tracing::info!(dataplane_id = %id, dataplane_name = %request.name, "Created new dataplane");

        self.get_by_id(&id).await
    }

    /// Retrieves a dataplane by its unique ID.
    #[instrument(skip(self), fields(dataplane_id = %id), name = "db_get_dataplane_by_id")]
    pub async fn get_by_id(&self, id: &DataplaneId) -> Result<DataplaneData> {
        let row = sqlx::query_as::<sqlx::Postgres, DataplaneRow>(
            "SELECT id, team, name, gateway_host, description, certificate_serial, certificate_expires_at, created_at, updated_at FROM dataplanes WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, dataplane_id = %id, "Failed to get dataplane by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get dataplane with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(DataplaneData::from(row)),
            None => {
                Err(FlowplaneError::not_found_msg(format!("Dataplane with ID '{}' not found", id)))
            }
        }
    }

    /// Retrieves a dataplane by name and team.
    #[instrument(skip(self), fields(dataplane_name = %name, team = %team), name = "db_get_dataplane_by_name")]
    pub async fn get_by_name(&self, team: &str, name: &str) -> Result<Option<DataplaneData>> {
        let row = sqlx::query_as::<sqlx::Postgres, DataplaneRow>(
            "SELECT id, team, name, gateway_host, description, certificate_serial, certificate_expires_at, created_at, updated_at FROM dataplanes WHERE team = $1 AND name = $2"
        )
        .bind(team)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, dataplane_name = %name, team = %team, "Failed to get dataplane by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get dataplane '{}' for team '{}'", name, team),
            }
        })?;

        Ok(row.map(DataplaneData::from))
    }

    /// Lists dataplanes for a specific team.
    #[instrument(skip(self), fields(team = %team, limit = ?limit, offset = ?offset), name = "db_list_dataplanes_by_team")]
    pub async fn list_by_team(
        &self,
        team: &str,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<DataplaneData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<sqlx::Postgres, DataplaneRow>(
            "SELECT id, team, name, gateway_host, description, certificate_serial, certificate_expires_at, created_at, updated_at FROM dataplanes WHERE team = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )
        .bind(team)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list dataplanes by team");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list dataplanes for team '{}'", team),
            }
        })?;

        Ok(rows.into_iter().map(DataplaneData::from).collect())
    }

    /// Lists dataplanes filtered by teams for multi-tenancy support.
    ///
    /// # Security Note
    ///
    /// Empty teams array returns ALL resources. This is intentional for admin:all
    /// scope but could be a security issue if authorization logic has bugs.
    /// A warning is logged when this occurs for auditing purposes.
    #[instrument(skip(self), fields(teams = ?teams, limit = ?limit, offset = ?offset), name = "db_list_dataplanes_by_teams")]
    pub async fn list_by_teams(
        &self,
        teams: &[String],
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<DataplaneData>> {
        // SECURITY: Empty teams array returns ALL resources (admin scope).
        // Log warning for audit trail - this should only happen for admin:all scope.
        if teams.is_empty() {
            tracing::warn!(
                resource = "dataplanes",
                "list_by_teams called with empty teams array - returning all resources (admin scope)"
            );
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

        let query_str = format!(
            "SELECT id, team, name, gateway_host, description, certificate_serial, certificate_expires_at, created_at, updated_at \
             FROM dataplanes \
             WHERE team IN ({}) \
             ORDER BY created_at DESC \
             LIMIT ${} OFFSET ${}",
            placeholders,
            teams.len() + 1,
            teams.len() + 2
        );

        let mut query = sqlx::query_as::<sqlx::Postgres, DataplaneRow>(&query_str);

        for team in teams {
            query = query.bind(team);
        }

        query = query.bind(limit).bind(offset);

        let rows = query.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to list dataplanes by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list dataplanes for teams: {:?}", teams),
            }
        })?;

        Ok(rows.into_iter().map(DataplaneData::from).collect())
    }

    /// Lists all dataplanes.
    #[instrument(skip(self), fields(limit = ?limit, offset = ?offset), name = "db_list_dataplanes")]
    pub async fn list(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<DataplaneData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<sqlx::Postgres, DataplaneRow>(
            "SELECT id, team, name, gateway_host, description, certificate_serial, certificate_expires_at, created_at, updated_at FROM dataplanes ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list dataplanes");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list dataplanes".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(DataplaneData::from).collect())
    }

    /// Updates an existing dataplane.
    #[instrument(skip(self, request), fields(dataplane_id = %id), name = "db_update_dataplane")]
    pub async fn update(
        &self,
        id: &DataplaneId,
        request: UpdateDataplaneRequest,
    ) -> Result<DataplaneData> {
        let current = self.get_by_id(id).await?;

        let new_gateway_host = request.gateway_host.unwrap_or(current.gateway_host);
        let new_description = request.description.unwrap_or(current.description);
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "UPDATE dataplanes SET gateway_host = $1, description = $2, updated_at = $3 WHERE id = $4"
        )
        .bind(&new_gateway_host)
        .bind(&new_description)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, dataplane_id = %id, "Failed to update dataplane");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update dataplane with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Dataplane with ID '{}' not found",
                id
            )));
        }

        tracing::info!(dataplane_id = %id, dataplane_name = %current.name, "Updated dataplane");

        self.get_by_id(id).await
    }

    /// Updates only the gateway_host of a dataplane.
    #[instrument(skip(self), fields(dataplane_id = %id, gateway_host = %gateway_host), name = "db_update_dataplane_gateway_host")]
    pub async fn update_gateway_host(&self, id: &DataplaneId, gateway_host: &str) -> Result<()> {
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "UPDATE dataplanes SET gateway_host = $1, updated_at = $2 WHERE id = $3",
        )
        .bind(gateway_host)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, dataplane_id = %id, "Failed to update dataplane gateway_host");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update gateway_host for dataplane '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Dataplane with ID '{}' not found",
                id
            )));
        }

        tracing::info!(dataplane_id = %id, gateway_host = %gateway_host, "Updated dataplane gateway_host");

        Ok(())
    }

    /// Deletes a dataplane.
    #[instrument(skip(self), fields(dataplane_id = %id), name = "db_delete_dataplane")]
    pub async fn delete(&self, id: &DataplaneId) -> Result<()> {
        let dataplane = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM dataplanes WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, dataplane_id = %id, "Failed to delete dataplane");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete dataplane with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Dataplane with ID '{}' not found",
                id
            )));
        }

        tracing::info!(dataplane_id = %id, dataplane_name = %dataplane.name, "Deleted dataplane");

        Ok(())
    }

    /// Checks if a dataplane name exists for a team.
    #[instrument(skip(self), fields(dataplane_name = %name, team = %team), name = "db_exists_dataplane_by_name")]
    pub async fn exists_by_name(&self, team: &str, name: &str) -> Result<bool> {
        let count =
            sqlx::query_scalar::<sqlx::Postgres, i64>("SELECT COUNT(*) FROM dataplanes WHERE team = $1 AND name = $2")
                .bind(team)
                .bind(name)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, dataplane_name = %name, team = %team, "Failed to check dataplane existence");
                    FlowplaneError::Database {
                        source: e,
                        context: format!("Failed to check existence of dataplane '{}' for team '{}'", name, team),
                    }
                })?;

        Ok(count > 0)
    }

    /// Updates certificate information for a dataplane after certificate issuance.
    ///
    /// # Arguments
    ///
    /// * `id` - The dataplane ID
    /// * `serial` - Certificate serial number from the PKI backend
    /// * `expires_at` - Certificate expiration time
    #[instrument(skip(self), fields(dataplane_id = %id, serial = %serial, expires_at = %expires_at), name = "db_update_dataplane_certificate_info")]
    pub async fn update_certificate_info(
        &self,
        id: &DataplaneId,
        serial: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let expires_at_str = expires_at.to_rfc3339();

        let result = sqlx::query(
            "UPDATE dataplanes SET certificate_serial = $1, certificate_expires_at = $2, updated_at = $3 WHERE id = $4"
        )
        .bind(serial)
        .bind(&expires_at_str)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, dataplane_id = %id, "Failed to update certificate info");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update certificate info for dataplane '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Dataplane with ID '{}' not found",
                id
            )));
        }

        tracing::info!(dataplane_id = %id, serial = %serial, expires_at = %expires_at, "Updated dataplane certificate info");

        Ok(())
    }

    /// Lists dataplanes with certificates expiring within the specified number of days.
    ///
    /// Used for monitoring and automated renewal workflows.
    ///
    /// # Arguments
    ///
    /// * `within_days` - Return dataplanes with certificates expiring within this many days
    /// * `limit` - Maximum number of results (default: 100, max: 1000)
    #[instrument(skip(self), fields(within_days = within_days, limit = ?limit), name = "db_list_expiring_certificates")]
    pub async fn list_expiring_certificates(
        &self,
        within_days: i64,
        limit: Option<i32>,
    ) -> Result<Vec<DataplaneData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let threshold = chrono::Utc::now() + chrono::Duration::days(within_days);
        let threshold_str = threshold.to_rfc3339();

        let rows = sqlx::query_as::<sqlx::Postgres, DataplaneRow>(
            "SELECT id, team, name, gateway_host, description, certificate_serial, certificate_expires_at, created_at, updated_at \
             FROM dataplanes \
             WHERE certificate_expires_at IS NOT NULL \
               AND certificate_expires_at <= $1 \
             ORDER BY certificate_expires_at ASC \
             LIMIT $2"
        )
        .bind(&threshold_str)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, within_days = within_days, "Failed to list expiring certificates");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list dataplanes with expiring certificates".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(DataplaneData::from).collect())
    }

    /// Clears certificate information for a dataplane (e.g., after revocation).
    #[instrument(skip(self), fields(dataplane_id = %id), name = "db_clear_dataplane_certificate_info")]
    pub async fn clear_certificate_info(&self, id: &DataplaneId) -> Result<()> {
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "UPDATE dataplanes SET certificate_serial = NULL, certificate_expires_at = NULL, updated_at = $1 WHERE id = $2"
        )
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, dataplane_id = %id, "Failed to clear certificate info");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to clear certificate info for dataplane '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "Dataplane with ID '{}' not found",
                id
            )));
        }

        tracing::info!(dataplane_id = %id, "Cleared dataplane certificate info");

        Ok(())
    }

    /// Returns the database pool.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}
