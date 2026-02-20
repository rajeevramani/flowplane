//! Cluster Operations for Internal API
//!
//! This module provides the unified cluster operations layer that sits between
//! HTTP/MCP handlers and the ClusterService. It handles:
//! - Request validation
//! - Team-based access control
//! - Error mapping
//! - Response formatting

use std::sync::Arc;
use tracing::{info, instrument};

use crate::internal_api::auth::{verify_team_access, InternalAuthContext};
use crate::internal_api::error::InternalError;
use crate::internal_api::types::{
    CreateClusterRequest, ListClustersRequest, ListClustersResponse, OperationResult,
    UpdateClusterRequest,
};
use crate::services::ClusterService;
use crate::storage::ClusterData;
use crate::xds::{ClusterSpec, XdsState};

// Note: TeamOwned for ClusterData is implemented in storage/repositories/cluster.rs

/// Cluster operations for the internal API layer
///
/// This struct provides all CRUD operations for clusters with unified
/// validation and access control.
pub struct ClusterOperations {
    xds_state: Arc<XdsState>,
}

impl ClusterOperations {
    /// Create a new ClusterOperations instance
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Create a new cluster
    ///
    /// # Arguments
    /// * `req` - The create cluster request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the created cluster on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(cluster_name = %req.name, team = ?req.team))]
    pub async fn create(
        &self,
        req: CreateClusterRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<ClusterData>, InternalError> {
        // 1. Validate the cluster spec
        req.config.validate_model().map_err(|e| InternalError::validation(e.to_string()))?;

        // Resolve team name to UUID for database storage
        let resolved_team = self
            .xds_state
            .resolve_optional_team(req.team.as_deref())
            .await
            .map_err(InternalError::from)?;

        // 2. Verify team access (can create in this team?)
        if !auth.can_create_for_team(resolved_team.as_deref()) {
            return Err(InternalError::forbidden(format!(
                "Cannot create cluster for team '{}'",
                req.team.as_deref().unwrap_or("global")
            )));
        }

        // 3. Call service layer
        let service = ClusterService::new(self.xds_state.clone());
        let created = service
            .create_cluster(req.name.clone(), req.service_name, req.config, resolved_team)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("already exists")
                    || err_str.contains("UNIQUE constraint")
                    || err_str.contains("unique constraint")
                {
                    InternalError::already_exists("Cluster", &req.name)
                } else {
                    InternalError::from(e)
                }
            })?;

        info!(
            cluster_id = %created.id,
            cluster_name = %created.name,
            "Cluster created via internal API"
        );

        Ok(OperationResult::with_message(
            created,
            "Cluster created successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Get a cluster by name
    ///
    /// # Arguments
    /// * `name` - The cluster name
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(ClusterData)` if found and accessible
    /// * `Err(InternalError::NotFound)` if not found or not accessible
    #[instrument(skip(self, auth), fields(cluster_name = %name))]
    pub async fn get(
        &self,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<ClusterData, InternalError> {
        let service = ClusterService::new(self.xds_state.clone());

        // Get the cluster
        let cluster = service.get_cluster(name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Cluster", name)
            } else {
                InternalError::from(e)
            }
        })?;

        // Verify team access
        verify_team_access(cluster, auth).await
    }

    /// List clusters with pagination
    ///
    /// # Arguments
    /// * `req` - List request with pagination options
    /// * `auth` - Authentication context for filtering
    ///
    /// # Returns
    /// * `Ok(ListClustersResponse)` with filtered clusters
    #[instrument(skip(self, auth), fields(limit = ?req.limit, offset = ?req.offset))]
    pub async fn list(
        &self,
        req: ListClustersRequest,
        auth: &InternalAuthContext,
    ) -> Result<ListClustersResponse, InternalError> {
        let repository =
            self.xds_state.cluster_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Cluster repository unavailable")
            })?;

        // Use team filtering - empty allowed_teams means admin access to all
        let clusters = repository
            .list_by_teams(&auth.allowed_teams, req.include_defaults, req.limit, req.offset)
            .await
            .map_err(|e| InternalError::database(e.to_string()))?;

        let count = clusters.len();

        Ok(ListClustersResponse { clusters, count, limit: req.limit, offset: req.offset })
    }

    /// Update an existing cluster
    ///
    /// # Arguments
    /// * `name` - The cluster name to update
    /// * `req` - The update request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the updated cluster on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(cluster_name = %name))]
    pub async fn update(
        &self,
        name: &str,
        req: UpdateClusterRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<ClusterData>, InternalError> {
        // 1. Get existing cluster and verify access
        let existing = self.get(name, auth).await?;

        // 2. Validate the new config
        req.config.validate_model().map_err(|e| InternalError::validation(e.to_string()))?;

        // 3. Determine service name (use existing if not provided)
        let service_name = req.service_name.unwrap_or_else(|| existing.service_name.clone());

        // 4. Update via service layer
        let service = ClusterService::new(self.xds_state.clone());
        let updated = service
            .update_cluster(name, service_name, req.config)
            .await
            .map_err(InternalError::from)?;

        info!(
            cluster_id = %updated.id,
            cluster_name = %updated.name,
            "Cluster updated via internal API"
        );

        Ok(OperationResult::with_message(
            updated,
            "Cluster updated successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Delete a cluster
    ///
    /// # Arguments
    /// * `name` - The cluster name to delete
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(cluster_name = %name))]
    pub async fn delete(
        &self,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // 1. Get existing cluster and verify access
        let _existing = self.get(name, auth).await?;

        // 2. Delete via service layer (includes dependency check)
        let service = ClusterService::new(self.xds_state.clone());
        service.delete_cluster(name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("in use") {
                // Extract dependency info from error message
                InternalError::in_use("Cluster", name, vec![err_str])
            } else if err_str.contains("default gateway") || err_str.contains("cannot be deleted") {
                InternalError::forbidden(err_str)
            } else {
                InternalError::from(e)
            }
        })?;

        info!(cluster_name = %name, "Cluster deleted via internal API");

        Ok(OperationResult::with_message(
            (),
            "Cluster deleted successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Parse cluster configuration from stored data
    ///
    /// This is a utility method to convert stored JSON config to ClusterSpec.
    pub fn parse_config(&self, data: &ClusterData) -> Result<ClusterSpec, InternalError> {
        let service = ClusterService::new(self.xds_state.clone());
        service.parse_config(data).map_err(|e| InternalError::internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimpleXdsConfig;
    use crate::storage::test_helpers::{TestDatabase, TEAM_A_ID, TEAM_B_ID, TEST_TEAM_ID};
    use crate::xds::EndpointSpec;

    async fn setup_state() -> (TestDatabase, Arc<XdsState>) {
        let test_db = TestDatabase::new("internal_api_clusters").await;
        let pool = test_db.pool.clone();
        let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool));
        (test_db, state)
    }

    fn sample_config() -> ClusterSpec {
        ClusterSpec {
            endpoints: vec![EndpointSpec::Address { host: "10.0.0.1".to_string(), port: 8080 }],
            connect_timeout_seconds: Some(5),
            use_tls: Some(false),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_create_cluster_admin() {
        let (_db, state) = setup_state().await;
        let ops = ClusterOperations::new(state);
        let auth = InternalAuthContext::for_team(TEST_TEAM_ID);

        let req = CreateClusterRequest {
            name: "new-test-cluster".to_string(),
            service_name: "test-service".to_string(),
            team: Some(TEST_TEAM_ID.to_string()),
            config: sample_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());

        let op_result = result.unwrap();
        assert_eq!(op_result.data.name, "new-test-cluster");
        assert_eq!(op_result.data.service_name, "test-service");
        assert!(op_result.message.is_some());
    }

    #[tokio::test]
    async fn test_create_cluster_team_user() {
        let (_db, state) = setup_state().await;
        let ops = ClusterOperations::new(state);
        let auth = InternalAuthContext::for_team(TEAM_A_ID);

        let req = CreateClusterRequest {
            name: "team-cluster".to_string(),
            service_name: "team-service".to_string(),
            team: Some(TEAM_A_ID.to_string()),
            config: sample_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_cluster_wrong_team() {
        let (_db, state) = setup_state().await;
        let ops = ClusterOperations::new(state);
        let auth = InternalAuthContext::for_team(TEAM_A_ID);

        let req = CreateClusterRequest {
            name: "wrong-team-cluster".to_string(),
            service_name: "service".to_string(),
            team: Some(TEAM_B_ID.to_string()), // Different team
            config: sample_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::Forbidden { .. }));
    }

    #[tokio::test]
    async fn test_get_cluster_not_found() {
        let (_db, state) = setup_state().await;
        let ops = ClusterOperations::new(state);
        let auth = InternalAuthContext::for_team(TEST_TEAM_ID);

        let result = ops.get("nonexistent", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_cluster_cross_team_returns_not_found() {
        let (_db, state) = setup_state().await;
        let ops = ClusterOperations::new(state.clone());

        // Create cluster as team-a user
        let team_a_auth = InternalAuthContext::for_team(TEAM_A_ID);
        let req = CreateClusterRequest {
            name: "team-a-cluster".to_string(),
            service_name: "service".to_string(),
            team: Some(TEAM_A_ID.to_string()),
            config: sample_config(),
        };
        ops.create(req, &team_a_auth).await.expect("create cluster");

        // Try to access from team-b
        let team_b_auth = InternalAuthContext::for_team(TEAM_B_ID);
        let result = ops.get("team-a-cluster", &team_b_auth).await;

        assert!(result.is_err());
        // Should return NotFound to hide existence
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_list_clusters_team_filtering() {
        let (_db, state) = setup_state().await;
        let ops = ClusterOperations::new(state.clone());
        let multi_team_auth =
            InternalAuthContext::for_teams(vec![TEAM_A_ID.to_string(), TEAM_B_ID.to_string()]);

        // Create clusters for different teams (use unique names to avoid seed data conflicts)
        for (name, team) in
            [("filtered-a", TEAM_A_ID), ("filtered-b", TEAM_B_ID), ("filtered-a2", TEAM_A_ID)]
        {
            let req = CreateClusterRequest {
                name: name.to_string(),
                service_name: name.to_string(),
                team: Some(team.to_string()),
                config: sample_config(),
            };
            ops.create(req, &multi_team_auth).await.expect("create cluster");
        }

        // List as team-a
        let team_a_auth = InternalAuthContext::for_team(TEAM_A_ID);
        let list_req = ListClustersRequest { include_defaults: true, ..Default::default() };
        let result = ops.list(list_req, &team_a_auth).await.expect("list clusters");

        // Should see team-a clusters (plus global seed clusters)
        let team_a_names: Vec<&str> = result
            .clusters
            .iter()
            .filter(|c| c.team.as_deref() == Some(TEAM_A_ID))
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(team_a_names.len(), 2);
        assert!(team_a_names.contains(&"filtered-a"));
        assert!(team_a_names.contains(&"filtered-a2"));
    }

    #[tokio::test]
    async fn test_update_cluster() {
        let (_db, state) = setup_state().await;
        let ops = ClusterOperations::new(state);
        let auth = InternalAuthContext::for_team(TEST_TEAM_ID);

        // Create a cluster
        let create_req = CreateClusterRequest {
            name: "update-test".to_string(),
            service_name: "original".to_string(),
            team: Some(TEST_TEAM_ID.to_string()),
            config: sample_config(),
        };
        ops.create(create_req, &auth).await.expect("create cluster");

        // Update it
        let update_req = UpdateClusterRequest {
            service_name: Some("updated".to_string()),
            config: sample_config(),
        };
        let result = ops.update("update-test", update_req, &auth).await;

        assert!(result.is_ok());
        let updated = result.unwrap().data;
        assert_eq!(updated.service_name, "updated");
    }

    #[tokio::test]
    async fn test_delete_cluster() {
        let (_db, state) = setup_state().await;
        let ops = ClusterOperations::new(state.clone());
        let auth = InternalAuthContext::for_team(TEST_TEAM_ID);

        // Create a cluster
        let create_req = CreateClusterRequest {
            name: "delete-test".to_string(),
            service_name: "service".to_string(),
            team: Some(TEST_TEAM_ID.to_string()),
            config: sample_config(),
        };
        ops.create(create_req, &auth).await.expect("create cluster");

        // Delete it
        let result = ops.delete("delete-test", &auth).await;
        assert!(result.is_ok());

        // Verify it's gone
        let get_result = ops.get("delete-test", &auth).await;
        assert!(get_result.is_err());
    }
}
