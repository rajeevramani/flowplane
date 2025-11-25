//! Import Metadata repository for tracking OpenAPI spec imports
//!
//! This module provides CRUD operations for import metadata, handling storage,
//! retrieval, and lifecycle management of OpenAPI import tracking data.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;
use uuid::Uuid;

/// Database row structure for import_metadata
#[derive(Debug, Clone, FromRow)]
struct ImportMetadataRow {
    pub id: String,
    pub spec_name: String,
    pub spec_version: Option<String>,
    pub spec_checksum: Option<String>,
    pub team: String,
    pub source_content: Option<String>,
    pub listener_name: Option<String>,
    pub imported_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Import metadata data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportMetadataData {
    pub id: String,
    pub spec_name: String,
    pub spec_version: Option<String>,
    pub spec_checksum: Option<String>,
    pub team: String,
    pub source_content: Option<String>,
    pub listener_name: Option<String>,
    pub imported_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ImportMetadataRow> for ImportMetadataData {
    fn from(row: ImportMetadataRow) -> Self {
        Self {
            id: row.id,
            spec_name: row.spec_name,
            spec_version: row.spec_version,
            spec_checksum: row.spec_checksum,
            team: row.team,
            source_content: row.source_content,
            listener_name: row.listener_name,
            imported_at: row.imported_at,
            updated_at: row.updated_at,
        }
    }
}

/// Create import metadata request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateImportMetadataRequest {
    pub spec_name: String,
    pub spec_version: Option<String>,
    pub spec_checksum: Option<String>,
    pub team: String,
    pub source_content: Option<String>,
    pub listener_name: Option<String>,
}

/// Repository for import metadata data access
#[derive(Debug, Clone)]
pub struct ImportMetadataRepository {
    pool: DbPool,
}

impl ImportMetadataRepository {
    /// Create a new import metadata repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new import metadata record
    #[instrument(skip(self, request), fields(spec_name = %request.spec_name, team = %request.team), name = "db_create_import_metadata")]
    pub async fn create(&self, request: CreateImportMetadataRequest) -> Result<ImportMetadataData> {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now();

        sqlx::query(
            "INSERT INTO import_metadata (id, spec_name, spec_version, spec_checksum, team, source_content, listener_name, imported_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&id)
        .bind(&request.spec_name)
        .bind(&request.spec_version)
        .bind(&request.spec_checksum)
        .bind(&request.team)
        .bind(&request.source_content)
        .bind(&request.listener_name)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| FlowplaneError::database(e, "Failed to create import metadata".to_string()))?;

        self.get_by_id(&id)
            .await?
            .ok_or_else(|| FlowplaneError::not_found("import_metadata", id.clone()))
    }

    /// Get import metadata by ID
    #[instrument(skip(self), name = "db_get_import_metadata")]
    pub async fn get_by_id(&self, id: &str) -> Result<Option<ImportMetadataData>> {
        let row = sqlx::query_as::<Sqlite, ImportMetadataRow>(
            "SELECT id, spec_name, spec_version, spec_checksum, team, source_content, listener_name, imported_at, updated_at
             FROM import_metadata WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::database(e, "Failed to fetch import metadata".to_string()))?;

        Ok(row.map(ImportMetadataData::from))
    }

    /// Get import metadata by team and spec name
    #[instrument(skip(self), name = "db_get_import_metadata_by_spec")]
    pub async fn get_by_team_and_spec(
        &self,
        team: &str,
        spec_name: &str,
    ) -> Result<Option<ImportMetadataData>> {
        let row = sqlx::query_as::<Sqlite, ImportMetadataRow>(
            "SELECT id, spec_name, spec_version, spec_checksum, team, source_content, listener_name, imported_at, updated_at
             FROM import_metadata WHERE team = $1 AND spec_name = $2",
        )
        .bind(team)
        .bind(spec_name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::database(e, "Failed to fetch import metadata".to_string()))?;

        Ok(row.map(ImportMetadataData::from))
    }

    /// List all import metadata for a team
    #[instrument(skip(self), name = "db_list_import_metadata")]
    pub async fn list_by_team(&self, team: &str) -> Result<Vec<ImportMetadataData>> {
        let rows = sqlx::query_as::<Sqlite, ImportMetadataRow>(
            "SELECT id, spec_name, spec_version, spec_checksum, team, source_content, listener_name, imported_at, updated_at
             FROM import_metadata WHERE team = $1 ORDER BY imported_at DESC",
        )
        .bind(team)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::database(e, "Failed to list import metadata".to_string()))?;

        Ok(rows.into_iter().map(ImportMetadataData::from).collect())
    }

    /// List all import metadata across all teams (for admin users)
    #[instrument(skip(self), name = "db_list_all_import_metadata")]
    pub async fn list_all(&self) -> Result<Vec<ImportMetadataData>> {
        let rows = sqlx::query_as::<Sqlite, ImportMetadataRow>(
            "SELECT id, spec_name, spec_version, spec_checksum, team, source_content, listener_name, imported_at, updated_at
             FROM import_metadata ORDER BY imported_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::database(e, "Failed to list all import metadata".to_string()))?;

        Ok(rows.into_iter().map(ImportMetadataData::from).collect())
    }

    /// Delete import metadata by ID (cascades to resources)
    #[instrument(skip(self), name = "db_delete_import_metadata")]
    pub async fn delete(&self, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM import_metadata WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                FlowplaneError::database(e, "Failed to delete import metadata".to_string())
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found("import_metadata", id));
        }

        Ok(())
    }
}
