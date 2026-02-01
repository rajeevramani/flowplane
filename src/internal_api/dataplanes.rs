//! Dataplane Operations for Internal API
//!
//! This module provides the unified dataplane operations layer that sits between
//! HTTP/MCP handlers and the DataplaneRepository. It handles:
//! - Request validation
//! - Team-based access control
//! - Error mapping
//! - Response formatting

use std::sync::Arc;
use tracing::{info, instrument};

use crate::internal_api::auth::InternalAuthContext;
use crate::internal_api::error::InternalError;
use crate::internal_api::types::{
    CreateDataplaneInternalRequest, ListDataplanesInternalRequest, OperationResult,
    UpdateDataplaneInternalRequest,
};
use crate::storage::repositories::{CreateDataplaneRequest, DataplaneData, UpdateDataplaneRequest};
use crate::xds::XdsState;

/// Dataplane operations for the internal API layer
///
/// This struct provides all CRUD operations for dataplanes with unified
/// validation and access control.
pub struct DataplaneOperations {
    xds_state: Arc<XdsState>,
}

impl DataplaneOperations {
    /// Create a new DataplaneOperations instance
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// List dataplanes with team-based filtering
    ///
    /// # Arguments
    /// * `req` - List request with optional pagination
    /// * `auth` - Authentication context for team filtering
    ///
    /// # Returns
    /// * `Ok(Vec<DataplaneData>)` with filtered dataplanes
    #[instrument(skip(self, auth), fields(limit = ?req.limit, offset = ?req.offset))]
    pub async fn list(
        &self,
        req: ListDataplanesInternalRequest,
        auth: &InternalAuthContext,
    ) -> Result<Vec<DataplaneData>, InternalError> {
        // Get pool from cluster_repository (pattern used in handlers)
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository =
            crate::storage::repositories::DataplaneRepository::new(cluster_repo.pool().clone());

        // Admin can see all dataplanes
        let dataplanes = if auth.is_admin {
            repository.list(req.limit, req.offset).await.map_err(InternalError::from)?
        } else {
            // Non-admin users can only see dataplanes from their allowed teams
            repository
                .list_by_teams(&auth.allowed_teams, req.limit, req.offset)
                .await
                .map_err(InternalError::from)?
        };

        Ok(dataplanes)
    }

    /// Get a specific dataplane by name and team
    ///
    /// # Arguments
    /// * `team` - Team that owns the dataplane
    /// * `name` - Dataplane name
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(DataplaneData)` if found and accessible
    /// * `Err(InternalError::NotFound)` if not found or not accessible
    #[instrument(skip(self, auth), fields(team = %team, dataplane_name = %name))]
    pub async fn get(
        &self,
        team: &str,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<DataplaneData, InternalError> {
        // Verify team access
        if !auth.can_access_team(Some(team)) {
            return Err(InternalError::not_found("Dataplane", name));
        }

        // Get pool from cluster_repository (pattern used in handlers)
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository =
            crate::storage::repositories::DataplaneRepository::new(cluster_repo.pool().clone());

        // Get the dataplane
        let dataplane = repository.get_by_name(team, name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Dataplane", name)
            } else {
                InternalError::from(e)
            }
        })?;

        match dataplane {
            Some(dp) => Ok(dp),
            None => Err(InternalError::not_found("Dataplane", name)),
        }
    }

    /// Create a new dataplane
    ///
    /// # Arguments
    /// * `req` - The create request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the created dataplane on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(dataplane_name = %req.name, team = %req.team))]
    pub async fn create(
        &self,
        req: CreateDataplaneInternalRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<DataplaneData>, InternalError> {
        // 1. Validate name is not empty
        if req.name.trim().is_empty() {
            return Err(InternalError::validation("Dataplane name cannot be empty"));
        }

        // 2. Verify team access for creation
        if !auth.can_create_for_team(Some(&req.team)) {
            return Err(InternalError::forbidden(format!(
                "Cannot create dataplane for team '{}'",
                req.team
            )));
        }

        // 3. Get pool from cluster_repository
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository =
            crate::storage::repositories::DataplaneRepository::new(cluster_repo.pool().clone());

        // 4. Check if dataplane already exists
        let existing =
            repository.get_by_name(&req.team, &req.name).await.map_err(InternalError::from)?;

        if existing.is_some() {
            return Err(InternalError::already_exists("Dataplane", &req.name));
        }

        // 5. Create the dataplane via repository
        let create_request = CreateDataplaneRequest {
            team: req.team.clone(),
            name: req.name.clone(),
            gateway_host: req.gateway_host.clone(),
            description: req.description.clone(),
        };

        let created = repository.create(create_request).await.map_err(InternalError::from)?;

        info!(
            dataplane_id = %created.id,
            team = %created.team,
            dataplane_name = %created.name,
            "Dataplane created via internal API"
        );

        Ok(OperationResult::with_message(created, "Dataplane created successfully."))
    }

    /// Update an existing dataplane
    ///
    /// # Arguments
    /// * `team` - Team that owns the dataplane
    /// * `name` - Dataplane name
    /// * `req` - The update request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the updated dataplane on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(team = %team, dataplane_name = %name))]
    pub async fn update(
        &self,
        team: &str,
        name: &str,
        req: UpdateDataplaneInternalRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<DataplaneData>, InternalError> {
        // 1. Get existing dataplane and verify access
        let existing = self.get(team, name, auth).await?;

        // 2. Get pool from cluster_repository
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository =
            crate::storage::repositories::DataplaneRepository::new(cluster_repo.pool().clone());

        // 3. Update via repository
        let update_request = UpdateDataplaneRequest {
            gateway_host: req.gateway_host.map(Some),
            description: req.description.map(Some),
        };

        let updated =
            repository.update(&existing.id, update_request).await.map_err(InternalError::from)?;

        info!(
            dataplane_id = %updated.id,
            team = %updated.team,
            dataplane_name = %updated.name,
            "Dataplane updated via internal API"
        );

        Ok(OperationResult::with_message(updated, "Dataplane updated successfully."))
    }

    /// Delete a dataplane
    ///
    /// # Arguments
    /// * `team` - Team that owns the dataplane
    /// * `name` - Dataplane name
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(team = %team, dataplane_name = %name))]
    pub async fn delete(
        &self,
        team: &str,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // 1. Get existing dataplane and verify access
        let existing = self.get(team, name, auth).await?;

        // 2. Get pool from cluster_repository
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository =
            crate::storage::repositories::DataplaneRepository::new(cluster_repo.pool().clone());

        // 3. Delete via repository
        repository.delete(&existing.id).await.map_err(InternalError::from)?;

        info!(
            dataplane_id = %existing.id,
            team = %existing.team,
            dataplane_name = %existing.name,
            "Dataplane deleted via internal API"
        );

        Ok(OperationResult::with_message((), "Dataplane deleted successfully."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimpleXdsConfig;
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

        // Create dataplanes table
        pool.execute(
            r#"
            CREATE TABLE IF NOT EXISTS dataplanes (
                id TEXT PRIMARY KEY,
                team TEXT NOT NULL,
                name TEXT NOT NULL,
                gateway_host TEXT,
                description TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(team, name)
            )
        "#,
        )
        .await
        .expect("create dataplanes table");

        Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool))
    }

    #[tokio::test]
    async fn test_create_dataplane_admin() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state);
        let auth = InternalAuthContext::admin();

        let req = CreateDataplaneInternalRequest {
            team: "test-team".to_string(),
            name: "production-dp".to_string(),
            gateway_host: Some("https://api.example.com".to_string()),
            description: Some("Production dataplane".to_string()),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());

        let op_result = result.unwrap();
        assert_eq!(op_result.data.team, "test-team");
        assert_eq!(op_result.data.name, "production-dp");
        assert_eq!(op_result.data.gateway_host, Some("https://api.example.com".to_string()));
        assert!(op_result.message.is_some());
    }

    #[tokio::test]
    async fn test_create_dataplane_team_user() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state);
        let auth = InternalAuthContext::for_team("team-a");

        let req = CreateDataplaneInternalRequest {
            team: "team-a".to_string(),
            name: "staging-dp".to_string(),
            gateway_host: None,
            description: None,
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_dataplane_wrong_team() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state);
        let auth = InternalAuthContext::for_team("team-a");

        let req = CreateDataplaneInternalRequest {
            team: "team-b".to_string(), // Different team
            name: "forbidden-dp".to_string(),
            gateway_host: None,
            description: None,
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::Forbidden { .. }));
    }

    #[tokio::test]
    async fn test_create_dataplane_empty_name() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state);
        let auth = InternalAuthContext::admin();

        let req = CreateDataplaneInternalRequest {
            team: "test-team".to_string(),
            name: "   ".to_string(), // Empty after trim
            gateway_host: None,
            description: None,
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::InvalidInput { .. }));
    }

    #[tokio::test]
    async fn test_create_dataplane_already_exists() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create first dataplane
        let req = CreateDataplaneInternalRequest {
            team: "test-team".to_string(),
            name: "duplicate-dp".to_string(),
            gateway_host: None,
            description: None,
        };
        ops.create(req.clone(), &auth).await.expect("create first");

        // Try to create duplicate
        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::AlreadyExists { .. }));
    }

    #[tokio::test]
    async fn test_get_dataplane() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create a dataplane
        let create_req = CreateDataplaneInternalRequest {
            team: "test-team".to_string(),
            name: "my-dp".to_string(),
            gateway_host: Some("https://api.example.com".to_string()),
            description: Some("Test dataplane".to_string()),
        };
        ops.create(create_req, &auth).await.expect("create dataplane");

        // Get it
        let result = ops.get("test-team", "my-dp", &auth).await;
        assert!(result.is_ok());
        let dataplane = result.unwrap();
        assert_eq!(dataplane.name, "my-dp");
        assert_eq!(dataplane.team, "test-team");
    }

    #[tokio::test]
    async fn test_get_dataplane_not_found() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state);
        let auth = InternalAuthContext::admin();

        let result = ops.get("test-team", "nonexistent", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_dataplane_cross_team_returns_not_found() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state.clone());

        // Create dataplane as admin for team-a
        let admin_auth = InternalAuthContext::admin();
        let req = CreateDataplaneInternalRequest {
            team: "team-a".to_string(),
            name: "secret-dp".to_string(),
            gateway_host: None,
            description: None,
        };
        ops.create(req, &admin_auth).await.expect("create dataplane");

        // Try to access from team-b
        let team_b_auth = InternalAuthContext::for_team("team-b");
        let result = ops.get("team-a", "secret-dp", &team_b_auth).await;

        assert!(result.is_err());
        // Should return NotFound to hide existence
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_list_dataplanes_team_filtering() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state.clone());
        let admin_auth = InternalAuthContext::admin();

        // Create dataplanes for different teams
        for (team, count) in [("team-a", 2), ("team-b", 1)] {
            for i in 0..count {
                let req = CreateDataplaneInternalRequest {
                    team: team.to_string(),
                    name: format!("dp-{}", i),
                    gateway_host: None,
                    description: None,
                };
                ops.create(req, &admin_auth).await.expect("create dataplane");
            }
        }

        // List as team-a
        let team_a_auth = InternalAuthContext::for_team("team-a");
        let list_req = ListDataplanesInternalRequest { limit: None, offset: None };
        let result = ops.list(list_req, &team_a_auth).await.expect("list dataplanes");

        // Should only see team-a dataplanes
        assert_eq!(result.len(), 2);
        for dataplane in &result {
            assert_eq!(dataplane.team, "team-a");
        }
    }

    #[tokio::test]
    async fn test_update_dataplane() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create a dataplane
        let create_req = CreateDataplaneInternalRequest {
            team: "test-team".to_string(),
            name: "updatable-dp".to_string(),
            gateway_host: Some("https://old.example.com".to_string()),
            description: Some("Old description".to_string()),
        };
        ops.create(create_req, &auth).await.expect("create dataplane");

        // Update it
        let update_req = UpdateDataplaneInternalRequest {
            gateway_host: Some("https://new.example.com".to_string()),
            description: Some("New description".to_string()),
        };
        let result = ops.update("test-team", "updatable-dp", update_req, &auth).await;
        assert!(result.is_ok());

        let updated = result.unwrap();
        assert_eq!(updated.data.gateway_host, Some("https://new.example.com".to_string()));
        assert_eq!(updated.data.description, Some("New description".to_string()));
    }

    #[tokio::test]
    async fn test_delete_dataplane() {
        let state = setup_state().await;
        let ops = DataplaneOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create a dataplane
        let create_req = CreateDataplaneInternalRequest {
            team: "test-team".to_string(),
            name: "deletable-dp".to_string(),
            gateway_host: None,
            description: None,
        };
        ops.create(create_req, &auth).await.expect("create dataplane");

        // Delete it
        let result = ops.delete("test-team", "deletable-dp", &auth).await;
        assert!(result.is_ok());

        // Verify it's gone
        let get_result = ops.get("test-team", "deletable-dp", &auth).await;
        assert!(get_result.is_err());
    }
}
