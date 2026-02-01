//! OpenAPI Import Operations for Internal API
//!
//! This module provides the unified OpenAPI import operations layer that sits between
//! HTTP/MCP handlers and the ImportMetadataRepository. It handles:
//! - Request validation
//! - Team-based access control
//! - Error mapping
//! - Response formatting

use std::sync::Arc;
use tracing::instrument;

use crate::internal_api::auth::InternalAuthContext;
use crate::internal_api::error::InternalError;
use crate::internal_api::types::ListOpenApiImportsRequest;
use crate::storage::repositories::import_metadata::ImportMetadataData;
use crate::storage::repositories::ImportMetadataRepository;
use crate::xds::XdsState;

/// OpenAPI import operations for the internal API layer
///
/// This struct provides all read operations for OpenAPI imports with unified
/// validation and access control.
pub struct OpenApiOperations {
    xds_state: Arc<XdsState>,
}

impl OpenApiOperations {
    /// Create a new OpenApiOperations instance
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// List OpenAPI imports with optional filtering
    ///
    /// # Arguments
    /// * `req` - List request with optional pagination
    /// * `auth` - Authentication context for team filtering
    ///
    /// # Returns
    /// * `Ok(Vec<ImportMetadataData>)` with filtered imports
    #[instrument(skip(self, auth), fields(limit = ?req.limit, offset = ?req.offset))]
    pub async fn list(
        &self,
        req: ListOpenApiImportsRequest,
        auth: &InternalAuthContext,
    ) -> Result<Vec<ImportMetadataData>, InternalError> {
        // Get pool from cluster_repository (pattern used in handlers)
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository = ImportMetadataRepository::new(cluster_repo.pool().clone());

        // Admin can see all imports
        let imports = if auth.is_admin {
            let mut all_imports = repository.list_all().await.map_err(InternalError::from)?;

            // Apply pagination if requested
            if let Some(offset) = req.offset {
                let offset_usize = offset as usize;
                if offset_usize < all_imports.len() {
                    all_imports = all_imports.into_iter().skip(offset_usize).collect();
                } else {
                    all_imports = Vec::new();
                }
            }

            if let Some(limit) = req.limit {
                all_imports.truncate(limit as usize);
            }

            all_imports
        } else {
            // Non-admin users can only see imports from their allowed teams
            let mut all_imports = Vec::new();
            for team in &auth.allowed_teams {
                let team_imports =
                    repository.list_by_team(team).await.map_err(InternalError::from)?;
                all_imports.extend(team_imports);
            }

            // Sort by imported_at DESC to match admin behavior
            all_imports.sort_by(|a, b| b.imported_at.cmp(&a.imported_at));

            // Apply pagination if requested
            if let Some(offset) = req.offset {
                let offset_usize = offset as usize;
                if offset_usize < all_imports.len() {
                    all_imports = all_imports.into_iter().skip(offset_usize).collect();
                } else {
                    all_imports = Vec::new();
                }
            }

            if let Some(limit) = req.limit {
                all_imports.truncate(limit as usize);
            }

            all_imports
        };

        Ok(imports)
    }

    /// Get an OpenAPI import by ID
    ///
    /// # Arguments
    /// * `id` - The import ID
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(ImportMetadataData)` if found and accessible
    /// * `Err(InternalError::NotFound)` if not found or not accessible
    #[instrument(skip(self, auth), fields(import_id = %id))]
    pub async fn get(
        &self,
        id: &str,
        auth: &InternalAuthContext,
    ) -> Result<ImportMetadataData, InternalError> {
        // Get pool from cluster_repository (pattern used in handlers)
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository = ImportMetadataRepository::new(cluster_repo.pool().clone());

        // Get the import
        let import = repository
            .get_by_id(id)
            .await
            .map_err(InternalError::from)?
            .ok_or_else(|| InternalError::not_found("OpenAPI import", id))?;

        // Verify team access
        if !auth.can_access_team(Some(&import.team)) {
            return Err(InternalError::not_found("OpenAPI import", id));
        }

        Ok(import)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimpleXdsConfig;
    use crate::storage::repositories::import_metadata::CreateImportMetadataRequest;
    use crate::storage::{create_pool, DatabaseConfig};
    use sqlx::Executor;

    fn create_test_config() -> DatabaseConfig {
        DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        }
    }

    async fn setup_state() -> Arc<XdsState> {
        let pool = create_pool(&create_test_config()).await.expect("pool");

        // Create import_metadata table
        pool.execute(
            r#"
            CREATE TABLE IF NOT EXISTS import_metadata (
                id TEXT PRIMARY KEY,
                spec_name TEXT NOT NULL,
                spec_version TEXT,
                spec_checksum TEXT,
                team TEXT NOT NULL,
                source_content TEXT,
                listener_name TEXT,
                imported_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .await
        .expect("create import_metadata table");

        Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool))
    }

    #[tokio::test]
    async fn test_list_openapi_imports_admin() {
        let state = setup_state().await;
        let ops = OpenApiOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create test imports
        let cluster_repo = state.cluster_repository.as_ref().unwrap();
        let repo = ImportMetadataRepository::new(cluster_repo.pool().clone());

        repo.create(CreateImportMetadataRequest {
            spec_name: "petstore".to_string(),
            spec_version: Some("1.0.0".to_string()),
            spec_checksum: Some("abc123".to_string()),
            team: "team-a".to_string(),
            source_content: None,
            listener_name: Some("main-listener".to_string()),
        })
        .await
        .expect("create import");

        repo.create(CreateImportMetadataRequest {
            spec_name: "orders-api".to_string(),
            spec_version: Some("2.0.0".to_string()),
            spec_checksum: Some("def456".to_string()),
            team: "team-b".to_string(),
            source_content: None,
            listener_name: Some("api-listener".to_string()),
        })
        .await
        .expect("create import");

        // List all imports
        let req = ListOpenApiImportsRequest { limit: None, offset: None };
        let result = ops.list(req, &auth).await.expect("list imports");

        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn test_list_openapi_imports_team_filtering() {
        let state = setup_state().await;
        let ops = OpenApiOperations::new(state.clone());

        // Create test imports for different teams
        let cluster_repo = state.cluster_repository.as_ref().unwrap();
        let repo = ImportMetadataRepository::new(cluster_repo.pool().clone());

        repo.create(CreateImportMetadataRequest {
            spec_name: "petstore".to_string(),
            spec_version: Some("1.0.0".to_string()),
            spec_checksum: None,
            team: "team-a".to_string(),
            source_content: None,
            listener_name: None,
        })
        .await
        .expect("create import");

        repo.create(CreateImportMetadataRequest {
            spec_name: "orders-api".to_string(),
            spec_version: Some("2.0.0".to_string()),
            spec_checksum: None,
            team: "team-b".to_string(),
            source_content: None,
            listener_name: None,
        })
        .await
        .expect("create import");

        // List as team-a
        let team_a_auth = InternalAuthContext::for_team("team-a");
        let req = ListOpenApiImportsRequest { limit: None, offset: None };
        let result = ops.list(req, &team_a_auth).await.expect("list imports");

        // Should only see team-a imports
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].team, "team-a");
        assert_eq!(result[0].spec_name, "petstore");
    }

    #[tokio::test]
    async fn test_list_openapi_imports_pagination() {
        let state = setup_state().await;
        let ops = OpenApiOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create multiple imports
        let cluster_repo = state.cluster_repository.as_ref().unwrap();
        let repo = ImportMetadataRepository::new(cluster_repo.pool().clone());

        for i in 0..5 {
            repo.create(CreateImportMetadataRequest {
                spec_name: format!("api-{}", i),
                spec_version: Some("1.0.0".to_string()),
                spec_checksum: None,
                team: "team-a".to_string(),
                source_content: None,
                listener_name: None,
            })
            .await
            .expect("create import");
        }

        // Test limit
        let req = ListOpenApiImportsRequest { limit: Some(2), offset: None };
        let result = ops.list(req, &auth).await.expect("list imports");
        assert_eq!(result.len(), 2);

        // Test offset
        let req = ListOpenApiImportsRequest { limit: None, offset: Some(3) };
        let result = ops.list(req, &auth).await.expect("list imports");
        assert_eq!(result.len(), 2);

        // Test both
        let req = ListOpenApiImportsRequest { limit: Some(1), offset: Some(1) };
        let result = ops.list(req, &auth).await.expect("list imports");
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn test_get_openapi_import() {
        let state = setup_state().await;
        let ops = OpenApiOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create an import
        let cluster_repo = state.cluster_repository.as_ref().unwrap();
        let repo = ImportMetadataRepository::new(cluster_repo.pool().clone());

        let created = repo
            .create(CreateImportMetadataRequest {
                spec_name: "petstore".to_string(),
                spec_version: Some("1.0.0".to_string()),
                spec_checksum: Some("abc123".to_string()),
                team: "test-team".to_string(),
                source_content: None,
                listener_name: Some("main-listener".to_string()),
            })
            .await
            .expect("create import");

        // Get it back
        let result = ops.get(&created.id, &auth).await.expect("get import");
        assert_eq!(result.id, created.id);
        assert_eq!(result.spec_name, "petstore");
        assert_eq!(result.spec_version, Some("1.0.0".to_string()));
        assert_eq!(result.team, "test-team");
    }

    #[tokio::test]
    async fn test_get_openapi_import_not_found() {
        let state = setup_state().await;
        let ops = OpenApiOperations::new(state);
        let auth = InternalAuthContext::admin();

        let result = ops.get("nonexistent", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_openapi_import_cross_team_returns_not_found() {
        let state = setup_state().await;
        let ops = OpenApiOperations::new(state.clone());

        // Create import for team-a
        let cluster_repo = state.cluster_repository.as_ref().unwrap();
        let repo = ImportMetadataRepository::new(cluster_repo.pool().clone());

        let created = repo
            .create(CreateImportMetadataRequest {
                spec_name: "petstore".to_string(),
                spec_version: Some("1.0.0".to_string()),
                spec_checksum: None,
                team: "team-a".to_string(),
                source_content: None,
                listener_name: None,
            })
            .await
            .expect("create import");

        // Try to access from team-b
        let team_b_auth = InternalAuthContext::for_team("team-b");
        let result = ops.get(&created.id, &team_b_auth).await;

        assert!(result.is_err());
        // Should return NotFound to hide existence
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }
}
