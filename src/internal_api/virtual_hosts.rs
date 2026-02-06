//! Virtual Host Operations for Internal API
//!
//! This module provides the unified virtual host operations layer that sits between
//! HTTP/MCP handlers and the VirtualHostRepository. It handles:
//! - Request validation
//! - Team-based access control (via route config ownership)
//! - Error mapping
//! - Response formatting

use std::sync::Arc;
use tracing::{info, instrument};

use crate::internal_api::auth::{verify_team_access, InternalAuthContext};
use crate::internal_api::error::InternalError;
use crate::internal_api::types::{
    CreateVirtualHostRequest, ListVirtualHostsRequest, ListVirtualHostsResponse, OperationResult,
    UpdateVirtualHostRequest,
};
use crate::storage::{RouteConfigData, VirtualHostData};
use crate::xds::XdsState;

/// Virtual host operations for the internal API layer
///
/// This struct provides all CRUD operations for virtual hosts with unified
/// validation and access control.
pub struct VirtualHostOperations {
    xds_state: Arc<XdsState>,
}

impl VirtualHostOperations {
    /// Create a new VirtualHostOperations instance
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Create a new virtual host
    ///
    /// # Arguments
    /// * `req` - The create virtual host request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the created virtual host on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(route_config = %req.route_config, name = %req.name))]
    pub async fn create(
        &self,
        req: CreateVirtualHostRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<VirtualHostData>, InternalError> {
        // 1. Verify access to the route config (team check)
        let route_config = self.get_route_config(&req.route_config, auth).await?;

        // 2. Create virtual host via repository
        let repository = self.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Virtual host repository unavailable")
        })?;

        let create_request = crate::storage::CreateVirtualHostRequest {
            route_config_id: route_config.id.clone(),
            name: req.name.clone(),
            domains: req.domains.clone(),
            rule_order: req.rule_order.unwrap_or(0),
        };

        let created = repository.create(create_request).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("already exists")
                || err_str.contains("UNIQUE constraint")
                || err_str.contains("unique constraint")
            {
                InternalError::already_exists(
                    "Virtual host",
                    format!("{}:{}", req.route_config, req.name),
                )
            } else {
                InternalError::from(e)
            }
        })?;

        info!(
            virtual_host_id = %created.id,
            route_config_id = %created.route_config_id,
            name = %created.name,
            "Virtual host created via internal API"
        );

        Ok(OperationResult::with_message(created, "Virtual host created successfully."))
    }

    /// Get a virtual host by route config and name
    ///
    /// # Arguments
    /// * `route_config` - The route config name
    /// * `name` - The virtual host name
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(VirtualHostData)` if found and accessible
    /// * `Err(InternalError::NotFound)` if not found or not accessible
    #[instrument(skip(self, auth), fields(route_config = %route_config, name = %name))]
    pub async fn get(
        &self,
        route_config: &str,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<VirtualHostData, InternalError> {
        // 1. Verify access to the route config (team check)
        let route_config_data = self.get_route_config(route_config, auth).await?;

        // 2. Get the virtual host
        let repository = self.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Virtual host repository unavailable")
        })?;

        let virtual_host = repository
            .get_by_route_config_and_name(&route_config_data.id, name)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    InternalError::not_found("Virtual host", format!("{}:{}", route_config, name))
                } else {
                    InternalError::from(e)
                }
            })?;

        Ok(virtual_host)
    }

    /// List virtual hosts with pagination
    ///
    /// # Arguments
    /// * `req` - List request with pagination options
    /// * `auth` - Authentication context for filtering
    ///
    /// # Returns
    /// * `Ok(ListVirtualHostsResponse)` with filtered virtual hosts
    #[instrument(skip(self, auth), fields(route_config = ?req.route_config, limit = ?req.limit, offset = ?req.offset))]
    pub async fn list(
        &self,
        req: ListVirtualHostsRequest,
        auth: &InternalAuthContext,
    ) -> Result<ListVirtualHostsResponse, InternalError> {
        let repository = self.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Virtual host repository unavailable")
        })?;

        // If route_config is specified, verify access and list for that route config only
        if let Some(ref route_config_name) = req.route_config {
            let route_config_data = self.get_route_config(route_config_name, auth).await?;

            let virtual_hosts = repository
                .list_by_route_config(&route_config_data.id)
                .await
                .map_err(|e| InternalError::database(e.to_string()))?;

            // Apply pagination manually since repository doesn't support it for single route config
            let offset = req.offset.unwrap_or(0) as usize;
            let limit = req.limit.map(|l| l as usize);

            let paginated: Vec<VirtualHostData> =
                virtual_hosts.into_iter().skip(offset).take(limit.unwrap_or(usize::MAX)).collect();

            let count = paginated.len();

            return Ok(ListVirtualHostsResponse {
                virtual_hosts: paginated,
                count,
                limit: req.limit,
                offset: req.offset,
            });
        }

        // List all virtual hosts across accessible route configs
        // This is more complex as we need to join with route_configs to filter by team
        let route_config_repo =
            self.xds_state.route_config_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Route config repository unavailable")
            })?;

        // Get all accessible route configs
        let route_configs = route_config_repo
            .list_by_teams(&auth.allowed_teams, false, None, None)
            .await
            .map_err(|e| InternalError::database(e.to_string()))?;

        // Collect virtual hosts from all accessible route configs
        let mut all_virtual_hosts = Vec::new();
        for route_config in route_configs {
            let vhosts = repository
                .list_by_route_config(&route_config.id)
                .await
                .map_err(|e| InternalError::database(e.to_string()))?;
            all_virtual_hosts.extend(vhosts);
        }

        // Apply pagination
        let offset = req.offset.unwrap_or(0) as usize;
        let limit = req.limit.map(|l| l as usize);

        let paginated: Vec<VirtualHostData> =
            all_virtual_hosts.into_iter().skip(offset).take(limit.unwrap_or(usize::MAX)).collect();

        let count = paginated.len();

        Ok(ListVirtualHostsResponse {
            virtual_hosts: paginated,
            count,
            limit: req.limit,
            offset: req.offset,
        })
    }

    /// Update an existing virtual host
    ///
    /// # Arguments
    /// * `route_config` - The route config name
    /// * `name` - The virtual host name to update
    /// * `req` - The update request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the updated virtual host on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(route_config = %route_config, name = %name))]
    pub async fn update(
        &self,
        route_config: &str,
        name: &str,
        req: UpdateVirtualHostRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<VirtualHostData>, InternalError> {
        // 1. Get existing virtual host and verify access
        let existing = self.get(route_config, name, auth).await?;

        // 2. Update via repository
        let repository = self.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Virtual host repository unavailable")
        })?;

        let update_request = crate::storage::UpdateVirtualHostRequest {
            domains: req.domains,
            rule_order: req.rule_order,
        };

        let updated = repository.update(&existing.id, update_request).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Virtual host", format!("{}:{}", route_config, name))
            } else {
                InternalError::from(e)
            }
        })?;

        info!(
            virtual_host_id = %updated.id,
            name = %updated.name,
            "Virtual host updated via internal API"
        );

        Ok(OperationResult::with_message(updated, "Virtual host updated successfully."))
    }

    /// Delete a virtual host
    ///
    /// # Arguments
    /// * `route_config` - The route config name
    /// * `name` - The virtual host name to delete
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(route_config = %route_config, name = %name))]
    pub async fn delete(
        &self,
        route_config: &str,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // 1. Get existing virtual host and verify access
        let existing = self.get(route_config, name, auth).await?;

        // 2. Delete via repository
        let repository = self.xds_state.virtual_host_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Virtual host repository unavailable")
        })?;

        repository.delete(&existing.id).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Virtual host", format!("{}:{}", route_config, name))
            } else {
                InternalError::from(e)
            }
        })?;

        info!(
            route_config = %route_config,
            name = %name,
            "Virtual host deleted via internal API"
        );

        Ok(OperationResult::with_message((), "Virtual host deleted successfully."))
    }

    /// Helper to get and verify access to a route config
    async fn get_route_config(
        &self,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<RouteConfigData, InternalError> {
        let repo = self.xds_state.route_config_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Route config repository unavailable")
        })?;

        let route_config = repo.get_by_name(name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Route config", name)
            } else {
                InternalError::from(e)
            }
        })?;

        // Verify team access to the route config
        verify_team_access(route_config, auth).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimpleXdsConfig;
    use crate::storage::test_helpers::TestDatabase;

    async fn setup_state() -> (TestDatabase, Arc<XdsState>) {
        let test_db = TestDatabase::new("internal_api_virtual_hosts").await;
        let pool = test_db.pool.clone();
        let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool));
        (test_db, state)
    }

    async fn create_test_route_config(
        state: &Arc<XdsState>,
        name: &str,
        team: Option<&str>,
    ) -> RouteConfigData {
        let repo = state.route_config_repository.as_ref().expect("route config repo");

        let req = crate::storage::repositories::route_config::CreateRouteConfigRequest {
            name: name.to_string(),
            path_prefix: "/".to_string(),
            cluster_name: "test-cluster".to_string(),
            configuration: serde_json::json!({}),
            team: team.map(String::from),
            import_id: None,
            route_order: None,
            headers: None,
        };

        repo.create(req).await.expect("create route config")
    }

    #[tokio::test]
    async fn test_create_virtual_host_admin() {
        let (_db, state) = setup_state().await;
        let ops = VirtualHostOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create route config first
        let _route_config =
            create_test_route_config(&state, "test-routes", Some("test-team")).await;

        let req = CreateVirtualHostRequest {
            route_config: "test-routes".to_string(),
            name: "api".to_string(),
            domains: vec!["api.example.com".to_string()],
            rule_order: Some(10),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());

        let op_result = result.unwrap();
        assert_eq!(op_result.data.name, "api");
        assert_eq!(op_result.data.domains, vec!["api.example.com"]);
        assert!(op_result.message.is_some());
    }

    #[tokio::test]
    async fn test_create_virtual_host_team_user() {
        let (_db, state) = setup_state().await;
        let ops = VirtualHostOperations::new(state.clone());
        let auth = InternalAuthContext::for_team("team-a");

        // Create route config for team-a
        let _route_config = create_test_route_config(&state, "team-a-routes", Some("team-a")).await;

        let req = CreateVirtualHostRequest {
            route_config: "team-a-routes".to_string(),
            name: "web".to_string(),
            domains: vec!["*.example.com".to_string()],
            rule_order: None,
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_virtual_host_wrong_team() {
        let (_db, state) = setup_state().await;
        let ops = VirtualHostOperations::new(state.clone());
        let auth = InternalAuthContext::for_team("team-a");

        // Create route config for team-b
        let _route_config = create_test_route_config(&state, "team-b-routes", Some("team-b")).await;

        let req = CreateVirtualHostRequest {
            route_config: "team-b-routes".to_string(),
            name: "forbidden".to_string(),
            domains: vec!["forbidden.example.com".to_string()],
            rule_order: None,
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_virtual_host() {
        let (_db, state) = setup_state().await;
        let ops = VirtualHostOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create route config and virtual host
        let _route_config =
            create_test_route_config(&state, "test-routes", Some("test-team")).await;

        let create_req = CreateVirtualHostRequest {
            route_config: "test-routes".to_string(),
            name: "api".to_string(),
            domains: vec!["api.example.com".to_string()],
            rule_order: Some(5),
        };
        ops.create(create_req, &auth).await.expect("create virtual host");

        // Get the virtual host
        let result = ops.get("test-routes", "api", &auth).await;
        assert!(result.is_ok());

        let vhost = result.unwrap();
        assert_eq!(vhost.name, "api");
        assert_eq!(vhost.domains, vec!["api.example.com"]);
        assert_eq!(vhost.rule_order, 5);
    }

    #[tokio::test]
    async fn test_get_virtual_host_not_found() {
        let (_db, state) = setup_state().await;
        let ops = VirtualHostOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        let _route_config = create_test_route_config(&state, "test-routes", None).await;

        let result = ops.get("test-routes", "nonexistent", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_virtual_host_cross_team_returns_not_found() {
        let (_db, state) = setup_state().await;
        let ops = VirtualHostOperations::new(state.clone());

        // Create route config for team-a
        let _route_config = create_test_route_config(&state, "team-a-routes", Some("team-a")).await;

        // Create virtual host as admin
        let admin_auth = InternalAuthContext::admin();
        let create_req = CreateVirtualHostRequest {
            route_config: "team-a-routes".to_string(),
            name: "secret".to_string(),
            domains: vec!["secret.example.com".to_string()],
            rule_order: None,
        };
        ops.create(create_req, &admin_auth).await.expect("create virtual host");

        // Try to access from team-b
        let team_b_auth = InternalAuthContext::for_team("team-b");
        let result = ops.get("team-a-routes", "secret", &team_b_auth).await;

        assert!(result.is_err());
        // Should return NotFound to hide existence
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_list_virtual_hosts_by_route_config() {
        let (_db, state) = setup_state().await;
        let ops = VirtualHostOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create route config
        let _route_config =
            create_test_route_config(&state, "test-routes", Some("test-team")).await;

        // Create multiple virtual hosts
        for (name, domains) in
            [("api", vec!["api.example.com"]), ("web", vec!["www.example.com", "example.com"])]
        {
            let req = CreateVirtualHostRequest {
                route_config: "test-routes".to_string(),
                name: name.to_string(),
                domains: domains.into_iter().map(String::from).collect(),
                rule_order: None,
            };
            ops.create(req, &auth).await.expect("create virtual host");
        }

        // List virtual hosts for this route config
        let list_req = ListVirtualHostsRequest {
            route_config: Some("test-routes".to_string()),
            ..Default::default()
        };
        let result = ops.list(list_req, &auth).await.expect("list virtual hosts");

        assert_eq!(result.count, 2);
        assert_eq!(result.virtual_hosts.len(), 2);
    }

    #[tokio::test]
    async fn test_update_virtual_host() {
        let (_db, state) = setup_state().await;
        let ops = VirtualHostOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create route config and virtual host
        let _route_config =
            create_test_route_config(&state, "test-routes", Some("test-team")).await;

        let create_req = CreateVirtualHostRequest {
            route_config: "test-routes".to_string(),
            name: "api".to_string(),
            domains: vec!["old.example.com".to_string()],
            rule_order: Some(1),
        };
        ops.create(create_req, &auth).await.expect("create virtual host");

        // Update it
        let update_req = UpdateVirtualHostRequest {
            domains: Some(vec!["new.example.com".to_string(), "api.example.com".to_string()]),
            rule_order: Some(10),
        };
        let result = ops.update("test-routes", "api", update_req, &auth).await;

        assert!(result.is_ok());
        let updated = result.unwrap().data;
        assert_eq!(updated.domains, vec!["new.example.com", "api.example.com"]);
        assert_eq!(updated.rule_order, 10);
    }

    #[tokio::test]
    async fn test_delete_virtual_host() {
        let (_db, state) = setup_state().await;
        let ops = VirtualHostOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create route config and virtual host
        let _route_config =
            create_test_route_config(&state, "test-routes", Some("test-team")).await;

        let create_req = CreateVirtualHostRequest {
            route_config: "test-routes".to_string(),
            name: "temp".to_string(),
            domains: vec!["temp.example.com".to_string()],
            rule_order: None,
        };
        ops.create(create_req, &auth).await.expect("create virtual host");

        // Delete it
        let result = ops.delete("test-routes", "temp", &auth).await;
        assert!(result.is_ok());

        // Verify it's gone
        let get_result = ops.get("test-routes", "temp", &auth).await;
        assert!(get_result.is_err());
    }
}
