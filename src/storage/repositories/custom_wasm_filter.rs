//! Custom WASM filter repository for managing user-uploaded WASM filters
//!
//! This module provides CRUD operations for custom WASM filter resources
//! stored in the database with team-scoped access.

use crate::domain::{compute_sha256, validate_wasm_binary, CustomWasmFilterId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;

/// Database row structure for listing (without binary for efficiency)
#[derive(Debug, Clone, FromRow)]
struct CustomWasmFilterListRow {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub wasm_sha256: String,
    pub wasm_size_bytes: i64,
    pub config_schema: String,
    pub per_route_config_schema: Option<String>,
    pub ui_hints: Option<String>,
    pub attachment_points: String,
    pub runtime: String,
    pub failure_policy: String,
    pub version: i64,
    pub team: String,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Custom WASM filter data (metadata without binary for general use)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomWasmFilterData {
    pub id: CustomWasmFilterId,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub wasm_sha256: String,
    pub wasm_size_bytes: i64,
    pub config_schema: serde_json::Value,
    pub per_route_config_schema: Option<serde_json::Value>,
    pub ui_hints: Option<serde_json::Value>,
    pub attachment_points: Vec<String>,
    pub runtime: String,
    pub failure_policy: String,
    pub version: i64,
    pub team: String,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<CustomWasmFilterListRow> for CustomWasmFilterData {
    type Error = FlowplaneError;

    fn try_from(row: CustomWasmFilterListRow) -> Result<Self> {
        let config_schema: serde_json::Value =
            serde_json::from_str(&row.config_schema).map_err(|e| {
                FlowplaneError::internal(format!("Failed to parse config_schema: {}", e))
            })?;

        let per_route_config_schema = row
            .per_route_config_schema
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| {
                FlowplaneError::internal(format!("Failed to parse per_route_config_schema: {}", e))
            })?;

        let ui_hints =
            row.ui_hints.map(|s| serde_json::from_str(&s)).transpose().map_err(|e| {
                FlowplaneError::internal(format!("Failed to parse ui_hints: {}", e))
            })?;

        let attachment_points: Vec<String> =
            serde_json::from_str(&row.attachment_points).map_err(|e| {
                FlowplaneError::internal(format!("Failed to parse attachment_points: {}", e))
            })?;

        Ok(Self {
            id: CustomWasmFilterId::from_string(row.id),
            name: row.name,
            display_name: row.display_name,
            description: row.description,
            wasm_sha256: row.wasm_sha256,
            wasm_size_bytes: row.wasm_size_bytes,
            config_schema,
            per_route_config_schema,
            ui_hints,
            attachment_points,
            runtime: row.runtime,
            failure_policy: row.failure_policy,
            version: row.version,
            team: row.team,
            created_by: row.created_by,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Create custom WASM filter request
#[derive(Debug, Clone)]
pub struct CreateCustomWasmFilterRequest {
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub wasm_binary: Vec<u8>,
    pub config_schema: serde_json::Value,
    pub per_route_config_schema: Option<serde_json::Value>,
    pub ui_hints: Option<serde_json::Value>,
    pub attachment_points: Vec<String>,
    pub runtime: String,
    pub failure_policy: String,
    pub team: String,
    pub created_by: Option<String>,
}

/// Update custom WASM filter request (metadata only, no binary update)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCustomWasmFilterRequest {
    pub display_name: Option<String>,
    pub description: Option<Option<String>>,
    pub config_schema: Option<serde_json::Value>,
    pub per_route_config_schema: Option<Option<serde_json::Value>>,
    pub ui_hints: Option<Option<serde_json::Value>>,
    pub attachment_points: Option<Vec<String>>,
}

/// Repository for custom WASM filter data access
#[derive(Clone, Debug)]
pub struct CustomWasmFilterRepository {
    pool: DbPool,
}

impl CustomWasmFilterRepository {
    /// Create a new custom WASM filter repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Get the database pool reference
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Create a new custom WASM filter
    #[instrument(skip(self, request), fields(filter_name = %request.name, team = %request.team), name = "db_create_custom_wasm_filter")]
    pub async fn create(
        &self,
        request: CreateCustomWasmFilterRequest,
    ) -> Result<CustomWasmFilterData> {
        // Validate the WASM binary
        validate_wasm_binary(&request.wasm_binary)
            .map_err(|e| FlowplaneError::validation(format!("Invalid WASM binary: {}", e)))?;

        let id = CustomWasmFilterId::new();
        let now = chrono::Utc::now();
        let wasm_sha256 = compute_sha256(&request.wasm_binary);
        let wasm_size_bytes = request.wasm_binary.len() as i64;

        let config_schema_str = serde_json::to_string(&request.config_schema).map_err(|e| {
            FlowplaneError::internal(format!("Failed to serialize config_schema: {}", e))
        })?;

        let per_route_config_schema_str = request
            .per_route_config_schema
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| {
                FlowplaneError::internal(format!(
                    "Failed to serialize per_route_config_schema: {}",
                    e
                ))
            })?;

        let ui_hints_str =
            request.ui_hints.as_ref().map(serde_json::to_string).transpose().map_err(|e| {
                FlowplaneError::internal(format!("Failed to serialize ui_hints: {}", e))
            })?;

        let attachment_points_str =
            serde_json::to_string(&request.attachment_points).map_err(|e| {
                FlowplaneError::internal(format!("Failed to serialize attachment_points: {}", e))
            })?;

        let result = sqlx::query(
            "INSERT INTO custom_wasm_filters (id, name, display_name, description, wasm_binary, wasm_sha256, wasm_size_bytes, config_schema, per_route_config_schema, ui_hints, attachment_points, runtime, failure_policy, version, team, created_by, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 1, $14, $15, $16, $17)"
        )
        .bind(id.as_str())
        .bind(&request.name)
        .bind(&request.display_name)
        .bind(&request.description)
        .bind(&request.wasm_binary)
        .bind(&wasm_sha256)
        .bind(wasm_size_bytes)
        .bind(&config_schema_str)
        .bind(&per_route_config_schema_str)
        .bind(&ui_hints_str)
        .bind(&attachment_points_str)
        .bind(&request.runtime)
        .bind(&request.failure_policy)
        .bind(&request.team)
        .bind(&request.created_by)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("UNIQUE constraint failed")
                || err_str.contains("duplicate key value violates unique constraint")
            {
                FlowplaneError::conflict(
                    format!(
                        "Custom WASM filter '{}' already exists for team '{}'",
                        request.name, request.team
                    ),
                    "CustomWasmFilter",
                )
            } else if err_str.contains("FOREIGN KEY constraint failed")
                || err_str.contains("violates foreign key constraint")
            {
                // Team doesn't exist
                FlowplaneError::not_found("Team", &request.team)
            } else {
                tracing::error!(error = %e, filter_name = %request.name, "Failed to create custom WASM filter");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to create custom WASM filter '{}'", request.name),
                }
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create custom WASM filter"));
        }

        tracing::info!(
            filter_id = %id,
            filter_name = %request.name,
            wasm_size = wasm_size_bytes,
            team = %request.team,
            "Created new custom WASM filter"
        );

        self.get_by_id(&id).await
    }

    /// Get custom WASM filter by ID (without binary)
    #[instrument(skip(self), fields(filter_id = %id), name = "db_get_custom_wasm_filter_by_id")]
    pub async fn get_by_id(&self, id: &CustomWasmFilterId) -> Result<CustomWasmFilterData> {
        let row = sqlx::query_as::<sqlx::Postgres, CustomWasmFilterListRow>(
            "SELECT id, name, display_name, description, wasm_sha256, wasm_size_bytes, config_schema, per_route_config_schema, ui_hints, attachment_points, runtime, failure_policy, version, team, created_by, created_at, updated_at \
             FROM custom_wasm_filters WHERE id = $1"
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_id = %id, "Failed to get custom WASM filter by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get custom WASM filter by ID '{}'", id),
            }
        })?;

        match row {
            Some(r) => CustomWasmFilterData::try_from(r),
            None => Err(FlowplaneError::not_found("CustomWasmFilter", id.to_string())),
        }
    }

    /// Get custom WASM filter by name and team (without binary)
    #[instrument(skip(self), fields(filter_name = %name, team = %team), name = "db_get_custom_wasm_filter_by_name")]
    pub async fn get_by_name(&self, team: &str, name: &str) -> Result<CustomWasmFilterData> {
        let row = sqlx::query_as::<sqlx::Postgres, CustomWasmFilterListRow>(
            "SELECT id, name, display_name, description, wasm_sha256, wasm_size_bytes, config_schema, per_route_config_schema, ui_hints, attachment_points, runtime, failure_policy, version, team, created_by, created_at, updated_at \
             FROM custom_wasm_filters WHERE team = $1 AND name = $2"
        )
        .bind(team)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_name = %name, team = %team, "Failed to get custom WASM filter by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get custom WASM filter '{}' for team '{}'", name, team),
            }
        })?;

        match row {
            Some(r) => CustomWasmFilterData::try_from(r),
            None => {
                Err(FlowplaneError::not_found("CustomWasmFilter", format!("{}:{}", team, name)))
            }
        }
    }

    /// Get WASM binary for a filter (separate for lazy loading)
    #[instrument(skip(self), fields(filter_id = %id), name = "db_get_wasm_binary")]
    pub async fn get_wasm_binary(&self, id: &CustomWasmFilterId) -> Result<Vec<u8>> {
        let row: Option<(Vec<u8>,)> =
            sqlx::query_as("SELECT wasm_binary FROM custom_wasm_filters WHERE id = $1")
                .bind(id.as_str())
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, filter_id = %id, "Failed to get WASM binary");
                    FlowplaneError::Database {
                        source: e,
                        context: format!("Failed to get WASM binary for filter '{}'", id),
                    }
                })?;

        match row {
            Some((binary,)) => Ok(binary),
            None => Err(FlowplaneError::not_found("CustomWasmFilter", id.to_string())),
        }
    }

    /// List custom WASM filters for a team
    #[instrument(skip(self), fields(team = %team), name = "db_list_custom_wasm_filters_by_team")]
    pub async fn list_by_team(
        &self,
        team: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CustomWasmFilterData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, CustomWasmFilterListRow>(
            "SELECT id, name, display_name, description, wasm_sha256, wasm_size_bytes, config_schema, per_route_config_schema, ui_hints, attachment_points, runtime, failure_policy, version, team, created_by, created_at, updated_at \
             FROM custom_wasm_filters WHERE team = $1 ORDER BY name LIMIT $2 OFFSET $3"
        )
        .bind(team)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list custom WASM filters by team");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list custom WASM filters for team '{}'", team),
            }
        })?;

        rows.into_iter().map(CustomWasmFilterData::try_from).collect()
    }

    /// List custom WASM filters for multiple teams.
    ///
    /// # Security Note
    ///
    /// Unlike other repositories, this returns an empty list (not all resources)
    /// when teams array is empty. This is the security-conscious pattern that
    /// prevents accidental data leakage.
    #[instrument(skip(self, teams), name = "db_list_custom_wasm_filters_by_teams")]
    pub async fn list_by_teams(
        &self,
        teams: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CustomWasmFilterData>> {
        // SECURITY: Return empty results for empty teams (no admin bypass).
        // This is the secure pattern - empty teams = no results, not all results.
        if teams.is_empty() {
            return Ok(vec![]);
        }

        // Build placeholders for IN clause
        let placeholders: Vec<String> = (1..=teams.len()).map(|i| format!("${}", i)).collect();
        let query = format!(
            "SELECT id, name, display_name, description, wasm_sha256, wasm_size_bytes, config_schema, per_route_config_schema, ui_hints, attachment_points, runtime, failure_policy, version, team, created_by, created_at, updated_at \
             FROM custom_wasm_filters WHERE team IN ({}) ORDER BY team, name LIMIT ${} OFFSET ${}",
            placeholders.join(", "),
            teams.len() + 1,
            teams.len() + 2
        );

        let mut query = sqlx::query_as::<sqlx::Postgres, CustomWasmFilterListRow>(&query);
        for team in teams {
            query = query.bind(team);
        }
        query = query.bind(limit).bind(offset);

        let rows = query.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, "Failed to list custom WASM filters by teams");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list custom WASM filters by teams".to_string(),
            }
        })?;

        rows.into_iter().map(CustomWasmFilterData::try_from).collect()
    }

    /// Count custom WASM filters for a team
    #[instrument(skip(self), fields(team = %team), name = "db_count_custom_wasm_filters")]
    pub async fn count_by_team(&self, team: &str) -> Result<i64> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM custom_wasm_filters WHERE team = $1",
        )
        .bind(team)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to count custom WASM filters");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count custom WASM filters for team '{}'", team),
            }
        })?;

        Ok(row.0)
    }

    /// Check if a filter with the given name exists for a team
    #[instrument(skip(self), fields(filter_name = %name, team = %team), name = "db_exists_custom_wasm_filter")]
    pub async fn exists_by_name(&self, team: &str, name: &str) -> Result<bool> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM custom_wasm_filters WHERE team = $1 AND name = $2",
        )
        .bind(team)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, filter_name = %name, team = %team, "Failed to check filter existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check if filter '{}' exists", name),
            }
        })?;

        Ok(row.0 > 0)
    }

    /// Update custom WASM filter metadata (no binary update)
    #[instrument(skip(self, request), fields(filter_id = %id), name = "db_update_custom_wasm_filter")]
    pub async fn update(
        &self,
        id: &CustomWasmFilterId,
        request: UpdateCustomWasmFilterRequest,
    ) -> Result<CustomWasmFilterData> {
        let now = chrono::Utc::now();

        // Check if any updates are requested
        let has_updates = request.display_name.is_some()
            || request.description.is_some()
            || request.config_schema.is_some()
            || request.per_route_config_schema.is_some()
            || request.ui_hints.is_some()
            || request.attachment_points.is_some();

        if !has_updates {
            // No actual updates, just return current data
            return self.get_by_id(id).await;
        }

        // Apply individual updates
        if let Some(display_name) = &request.display_name {
            sqlx::query("UPDATE custom_wasm_filters SET display_name = $1, updated_at = $2, version = version + 1 WHERE id = $3")
                .bind(display_name)
                .bind(now)
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to update display_name for '{}'", id),
                })?;
        }

        if let Some(description) = &request.description {
            sqlx::query(
                "UPDATE custom_wasm_filters SET description = $1, updated_at = $2 WHERE id = $3",
            )
            .bind(description.as_ref())
            .bind(now)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to update description for '{}'", id),
            })?;
        }

        if let Some(config_schema) = &request.config_schema {
            let config_schema_str = serde_json::to_string(config_schema).map_err(|e| {
                FlowplaneError::internal(format!("Failed to serialize config_schema: {}", e))
            })?;
            sqlx::query(
                "UPDATE custom_wasm_filters SET config_schema = $1, updated_at = $2 WHERE id = $3",
            )
            .bind(&config_schema_str)
            .bind(now)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to update config_schema for '{}'", id),
            })?;
        }

        if let Some(per_route) = &request.per_route_config_schema {
            let per_route_str =
                per_route.as_ref().map(serde_json::to_string).transpose().map_err(|e| {
                    FlowplaneError::internal(format!(
                        "Failed to serialize per_route_config_schema: {}",
                        e
                    ))
                })?;
            sqlx::query("UPDATE custom_wasm_filters SET per_route_config_schema = $1, updated_at = $2 WHERE id = $3")
                .bind(&per_route_str)
                .bind(now)
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to update per_route_config_schema for '{}'", id),
                })?;
        }

        if let Some(ui_hints) = &request.ui_hints {
            let ui_hints_str =
                ui_hints.as_ref().map(serde_json::to_string).transpose().map_err(|e| {
                    FlowplaneError::internal(format!("Failed to serialize ui_hints: {}", e))
                })?;
            sqlx::query(
                "UPDATE custom_wasm_filters SET ui_hints = $1, updated_at = $2 WHERE id = $3",
            )
            .bind(&ui_hints_str)
            .bind(now)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to update ui_hints for '{}'", id),
            })?;
        }

        if let Some(attachment_points) = &request.attachment_points {
            let attachment_points_str = serde_json::to_string(attachment_points).map_err(|e| {
                FlowplaneError::internal(format!("Failed to serialize attachment_points: {}", e))
            })?;
            sqlx::query("UPDATE custom_wasm_filters SET attachment_points = $1, updated_at = $2 WHERE id = $3")
                .bind(&attachment_points_str)
                .bind(now)
                .bind(id.as_str())
                .execute(&self.pool)
                .await
                .map_err(|e| FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to update attachment_points for '{}'", id),
                })?;
        }

        tracing::info!(filter_id = %id, "Updated custom WASM filter");

        self.get_by_id(id).await
    }

    /// Delete a custom WASM filter
    #[instrument(skip(self), fields(filter_id = %id), name = "db_delete_custom_wasm_filter")]
    pub async fn delete(&self, id: &CustomWasmFilterId) -> Result<()> {
        let result = sqlx::query("DELETE FROM custom_wasm_filters WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, filter_id = %id, "Failed to delete custom WASM filter");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete custom WASM filter '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("CustomWasmFilter", id.to_string()));
        }

        tracing::info!(filter_id = %id, "Deleted custom WASM filter");

        Ok(())
    }

    /// List all custom WASM filters (for startup initialization)
    #[instrument(skip(self), name = "db_list_all_custom_wasm_filters")]
    pub async fn list_all(&self) -> Result<Vec<CustomWasmFilterData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, CustomWasmFilterListRow>(
            "SELECT id, name, display_name, description, wasm_sha256, wasm_size_bytes, config_schema, per_route_config_schema, ui_hints, attachment_points, runtime, failure_policy, version, team, created_by, created_at, updated_at \
             FROM custom_wasm_filters ORDER BY team, name"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list all custom WASM filters");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list all custom WASM filters".to_string(),
            }
        })?;

        rows.into_iter().map(CustomWasmFilterData::try_from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::WASM_MAGIC_BYTES;
    use crate::storage::test_helpers::{TestDatabase, TEAM_A_ID, TEAM_B_ID, TEST_TEAM_ID};

    /// Helper to create a non-seeded test team in the database.
    /// Returns the team ID (UUID) for use in data inserts.
    async fn create_test_team(pool: &DbPool, name: &str) -> String {
        let team_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO teams (id, name, display_name, status) VALUES ($1, $2, $3, 'active') ON CONFLICT (name) DO NOTHING",
        )
        .bind(&team_id)
        .bind(name)
        .bind(format!("Test {}", name))
        .execute(pool)
        .await
        .expect("Failed to create test team");
        team_id
    }

    fn create_valid_wasm_binary() -> Vec<u8> {
        // Minimal valid WASM binary (magic bytes + version)
        let mut binary = WASM_MAGIC_BYTES.to_vec();
        binary.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // WASM version 1
        binary
    }

    #[tokio::test]
    async fn test_create_and_get_custom_wasm_filter() {
        let _db = TestDatabase::new("wasm_filter_create_get").await;
        let pool = _db.pool.clone();
        let repo = CustomWasmFilterRepository::new(pool);

        let request = CreateCustomWasmFilterRequest {
            name: "test-filter".to_string(),
            display_name: "Test Filter".to_string(),
            description: Some("A test filter".to_string()),
            wasm_binary: create_valid_wasm_binary(),
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "header_name": {"type": "string"}
                }
            }),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string(), "route".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: TEST_TEAM_ID.to_string(),
            created_by: Some("test-user".to_string()),
        };

        let created = repo.create(request).await.unwrap();

        assert_eq!(created.name, "test-filter");
        assert_eq!(created.display_name, "Test Filter");
        assert_eq!(created.team, TEST_TEAM_ID);
        assert_eq!(created.wasm_size_bytes, 8);
        assert!(!created.wasm_sha256.is_empty());

        // Get by ID
        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.name, created.name);

        // Get by name
        let by_name = repo.get_by_name(TEST_TEAM_ID, "test-filter").await.unwrap();
        assert_eq!(by_name.id, created.id);
    }

    #[tokio::test]
    async fn test_get_wasm_binary() {
        let _db = TestDatabase::new("wasm_filter_get_binary").await;
        let pool = _db.pool.clone();
        let repo = CustomWasmFilterRepository::new(pool);

        let wasm_binary = create_valid_wasm_binary();
        let request = CreateCustomWasmFilterRequest {
            name: "binary-test".to_string(),
            display_name: "Binary Test".to_string(),
            description: None,
            wasm_binary: wasm_binary.clone(),
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: TEST_TEAM_ID.to_string(),
            created_by: None,
        };

        let created = repo.create(request).await.unwrap();

        let fetched_binary = repo.get_wasm_binary(&created.id).await.unwrap();
        assert_eq!(fetched_binary, wasm_binary);
    }

    #[tokio::test]
    async fn test_list_by_team() {
        let _db = TestDatabase::new("wasm_filter_list_by_team").await;
        let pool = _db.pool.clone();
        let team1_id = create_test_team(&pool, "team1").await;
        let team2_id = create_test_team(&pool, "team2").await;
        let repo = CustomWasmFilterRepository::new(pool);

        // Create filters for team1
        for i in 1..=3 {
            let request = CreateCustomWasmFilterRequest {
                name: format!("filter-{}", i),
                display_name: format!("Filter {}", i),
                description: None,
                wasm_binary: create_valid_wasm_binary(),
                config_schema: serde_json::json!({"type": "object"}),
                per_route_config_schema: None,
                ui_hints: None,
                attachment_points: vec!["listener".to_string()],
                runtime: "envoy.wasm.runtime.v8".to_string(),
                failure_policy: "FAIL_CLOSED".to_string(),
                team: team1_id.clone(),
                created_by: None,
            };
            repo.create(request).await.unwrap();
        }

        // Create filter for team2
        let request = CreateCustomWasmFilterRequest {
            name: "filter-other".to_string(),
            display_name: "Other Filter".to_string(),
            description: None,
            wasm_binary: create_valid_wasm_binary(),
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: team2_id.clone(),
            created_by: None,
        };
        repo.create(request).await.unwrap();

        // List team1 filters
        let team1_filters = repo.list_by_team(&team1_id, 100, 0).await.unwrap();
        assert_eq!(team1_filters.len(), 3);

        // List team2 filters
        let team2_filters = repo.list_by_team(&team2_id, 100, 0).await.unwrap();
        assert_eq!(team2_filters.len(), 1);

        // Count
        let count = repo.count_by_team(&team1_id).await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_duplicate_name_conflict() {
        let _db = TestDatabase::new("wasm_filter_dup_name").await;
        let pool = _db.pool.clone();
        let repo = CustomWasmFilterRepository::new(pool);

        let request = CreateCustomWasmFilterRequest {
            name: "unique-filter".to_string(),
            display_name: "Unique Filter".to_string(),
            description: None,
            wasm_binary: create_valid_wasm_binary(),
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: TEST_TEAM_ID.to_string(),
            created_by: None,
        };

        // First create succeeds
        repo.create(request.clone()).await.unwrap();

        // Second create with same name fails
        let result = repo.create(request).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_delete_filter() {
        let _db = TestDatabase::new("wasm_filter_delete").await;
        let pool = _db.pool.clone();
        let repo = CustomWasmFilterRepository::new(pool);

        let request = CreateCustomWasmFilterRequest {
            name: "to-delete".to_string(),
            display_name: "To Delete".to_string(),
            description: None,
            wasm_binary: create_valid_wasm_binary(),
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: TEST_TEAM_ID.to_string(),
            created_by: None,
        };

        let created = repo.create(request).await.unwrap();

        // Verify exists
        assert!(repo.exists_by_name(TEST_TEAM_ID, "to-delete").await.unwrap());

        // Delete
        repo.delete(&created.id).await.unwrap();

        // Verify gone
        assert!(!repo.exists_by_name(TEST_TEAM_ID, "to-delete").await.unwrap());

        // Get returns not found
        let result = repo.get_by_id(&created.id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_filter_metadata() {
        let _db = TestDatabase::new("wasm_filter_update").await;
        let pool = _db.pool.clone();
        let repo = CustomWasmFilterRepository::new(pool);

        let request = CreateCustomWasmFilterRequest {
            name: "update-test".to_string(),
            display_name: "Original Name".to_string(),
            description: Some("Original description".to_string()),
            wasm_binary: create_valid_wasm_binary(),
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: TEST_TEAM_ID.to_string(),
            created_by: None,
        };

        let created = repo.create(request).await.unwrap();
        assert_eq!(created.version, 1);

        // Update display name
        let update_request = UpdateCustomWasmFilterRequest {
            display_name: Some("Updated Name".to_string()),
            description: None,
            config_schema: None,
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: None,
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.display_name, "Updated Name");
        assert_eq!(updated.version, 2);
        // Description should remain unchanged
        assert_eq!(updated.description, Some("Original description".to_string()));
    }

    #[tokio::test]
    async fn test_create_with_nonexistent_team_returns_not_found() {
        let _db = TestDatabase::new("wasm_filter_no_team").await;
        let pool = _db.pool.clone();
        // Intentionally NOT creating the team
        let repo = CustomWasmFilterRepository::new(pool);

        let request = CreateCustomWasmFilterRequest {
            name: "test-filter".to_string(),
            display_name: "Test Filter".to_string(),
            description: None,
            wasm_binary: create_valid_wasm_binary(),
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: "nonexistent-team".to_string(),
            created_by: None,
        };

        let result = repo.create(request).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should get a not_found error for the team, not a database error
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("Team"),
            "Expected 'not found' error for team, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_list_by_teams() {
        let _db = TestDatabase::new("wasm_filter_list_by_teams").await;
        let pool = _db.pool.clone();
        let team_c_id = create_test_team(&pool, "team-c").await;
        let repo = CustomWasmFilterRepository::new(pool);

        // Create filter for team-a (pre-seeded)
        let request_a = CreateCustomWasmFilterRequest {
            name: "filter-a".to_string(),
            display_name: "Filter A".to_string(),
            description: None,
            wasm_binary: create_valid_wasm_binary(),
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: TEAM_A_ID.to_string(),
            created_by: None,
        };
        repo.create(request_a).await.unwrap();

        // Create filter for team-b (pre-seeded)
        let request_b = CreateCustomWasmFilterRequest {
            name: "filter-b".to_string(),
            display_name: "Filter B".to_string(),
            description: None,
            wasm_binary: create_valid_wasm_binary(),
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: TEAM_B_ID.to_string(),
            created_by: None,
        };
        repo.create(request_b).await.unwrap();

        // Create filter for team-c (not in our query)
        let request_c = CreateCustomWasmFilterRequest {
            name: "filter-c".to_string(),
            display_name: "Filter C".to_string(),
            description: None,
            wasm_binary: create_valid_wasm_binary(),
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            team: team_c_id.clone(),
            created_by: None,
        };
        repo.create(request_c).await.unwrap();

        // Query for team-a and team-b only
        let teams = vec![TEAM_A_ID.to_string(), TEAM_B_ID.to_string()];
        let filters = repo.list_by_teams(&teams, 100, 0).await.unwrap();
        assert_eq!(filters.len(), 2);

        // Verify team-c filter is not included
        assert!(filters.iter().all(|f| f.team != team_c_id));
    }
}
