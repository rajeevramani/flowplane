//! Filter Operations for Internal API
//!
//! This module provides the unified filter operations layer that sits between
//! HTTP/MCP handlers and the FilterService. It handles:
//! - Request validation
//! - Team-based access control
//! - Error mapping
//! - Response formatting

use std::sync::Arc;
use tracing::{info, instrument};

use crate::domain::FilterId;
use crate::internal_api::auth::{verify_team_access, InternalAuthContext};
use crate::internal_api::error::InternalError;
use crate::internal_api::types::{
    CreateFilterRequest, FilterInstallation, FilterWithInstallations, ListFiltersRequest,
    ListFiltersResponse, OperationResult, UpdateFilterRequest,
};
use crate::services::FilterService;
use crate::storage::FilterData;
use crate::storage::FilterScopeType;
use crate::xds::XdsState;

/// Filter operations for the internal API layer
///
/// This struct provides all CRUD operations for filters with unified
/// validation and access control.
pub struct FilterOperations {
    xds_state: Arc<XdsState>,
}

impl FilterOperations {
    /// Create a new FilterOperations instance
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Create a new filter
    ///
    /// # Arguments
    /// * `req` - The create filter request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the created filter on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(filter_name = %req.name, filter_type = %req.filter_type, team = ?req.team))]
    pub async fn create(
        &self,
        req: CreateFilterRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<FilterData>, InternalError> {
        // 1. Verify team access (can create in this team?)
        if !auth.can_create_for_team(req.team.as_deref()) {
            return Err(InternalError::forbidden(format!(
                "Cannot create filter for team '{}'",
                req.team.as_deref().unwrap_or("global")
            )));
        }

        // 2. Validate that we have a team (filters require team)
        let team = req.team.ok_or_else(|| {
            InternalError::validation("Filter must be associated with a team".to_string())
        })?;

        // 3. Call service layer
        let service = FilterService::new(self.xds_state.clone());
        let created = service
            .create_filter(req.name.clone(), req.filter_type, req.description, req.config, team)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("already exists")
                    || err_str.contains("UNIQUE constraint")
                    || err_str.contains("unique constraint")
                {
                    InternalError::already_exists("Filter", &req.name)
                } else if err_str.contains("do not match") || err_str.contains("validation") {
                    InternalError::validation(err_str)
                } else {
                    InternalError::from(e)
                }
            })?;

        info!(
            filter_id = %created.id,
            filter_name = %created.name,
            filter_type = %created.filter_type,
            "Filter created via internal API"
        );

        Ok(OperationResult::with_message(
            created,
            "Filter created successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Get a filter by ID or name
    ///
    /// # Arguments
    /// * `id_or_name` - The filter ID or name
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(FilterData)` if found and accessible
    /// * `Err(InternalError::NotFound)` if not found or not accessible
    #[instrument(skip(self, auth), fields(filter_id_or_name = %id_or_name))]
    pub async fn get(
        &self,
        id_or_name: &str,
        auth: &InternalAuthContext,
    ) -> Result<FilterData, InternalError> {
        let repository =
            self.xds_state.filter_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Filter repository unavailable")
            })?;

        // Try to get by ID first, then by name
        let filter = if id_or_name.contains('-') && id_or_name.len() > 30 {
            // Looks like a UUID
            let filter_id = FilterId::from_string(id_or_name.to_string());
            repository.get_by_id(&filter_id).await
        } else {
            repository.get_by_name(id_or_name).await
        }
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Filter", id_or_name)
            } else {
                InternalError::from(e)
            }
        })?;

        // Verify team access
        verify_team_access(filter, auth).await
    }

    /// Get a filter with its installations
    ///
    /// # Arguments
    /// * `id_or_name` - The filter ID or name
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(FilterWithInstallations)` if found and accessible
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(filter_id_or_name = %id_or_name))]
    pub async fn get_with_installations(
        &self,
        id_or_name: &str,
        auth: &InternalAuthContext,
    ) -> Result<FilterWithInstallations, InternalError> {
        let filter = self.get(id_or_name, auth).await?;

        let repository =
            self.xds_state.filter_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Filter repository unavailable")
            })?;

        // Get listener installations
        let listener_installations = repository
            .list_filter_installations(&filter.id)
            .await
            .map_err(|e| InternalError::database(e.to_string()))?
            .into_iter()
            .map(|i| FilterInstallation {
                resource_id: i.listener_id.to_string(),
                resource_name: i.listener_name,
                order: i.order,
            })
            .collect();

        // Get route config installations
        let route_config_installations = repository
            .list_filter_configurations(&filter.id)
            .await
            .map_err(|e| InternalError::database(e.to_string()))?
            .into_iter()
            .filter(|c| c.scope_type == FilterScopeType::RouteConfig)
            .map(|c| FilterInstallation {
                resource_id: c.scope_id.clone(),
                resource_name: c.scope_name.clone(),
                order: 0,
            })
            .collect();

        Ok(FilterWithInstallations { filter, listener_installations, route_config_installations })
    }

    /// List filters with pagination
    ///
    /// # Arguments
    /// * `req` - List request with pagination options
    /// * `auth` - Authentication context for filtering
    ///
    /// # Returns
    /// * `Ok(ListFiltersResponse)` with filtered filters
    #[instrument(skip(self, auth), fields(limit = ?req.limit, offset = ?req.offset, filter_type = ?req.filter_type))]
    pub async fn list(
        &self,
        req: ListFiltersRequest,
        auth: &InternalAuthContext,
    ) -> Result<ListFiltersResponse, InternalError> {
        let repository =
            self.xds_state.filter_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Filter repository unavailable")
            })?;

        // Use team filtering - empty allowed_teams means admin access to all
        let mut filters = repository
            .list_by_teams(&auth.allowed_teams, req.limit, req.offset)
            .await
            .map_err(|e| InternalError::database(e.to_string()))?;

        // Apply type filter if specified
        if let Some(ref filter_type) = req.filter_type {
            filters.retain(|f| f.filter_type == *filter_type);
        }

        let count = filters.len();

        Ok(ListFiltersResponse { filters, count, limit: req.limit, offset: req.offset })
    }

    /// Update an existing filter
    ///
    /// # Arguments
    /// * `id_or_name` - The filter ID or name to update
    /// * `req` - The update request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the updated filter on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(filter_id_or_name = %id_or_name))]
    pub async fn update(
        &self,
        id_or_name: &str,
        req: UpdateFilterRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<FilterData>, InternalError> {
        // 1. Get existing filter and verify access
        let existing = self.get(id_or_name, auth).await?;

        // 2. Update via service layer
        let service = FilterService::new(self.xds_state.clone());
        let updated = service
            .update_filter(&existing.id, req.name, req.description, req.config)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    InternalError::not_found("Filter", id_or_name)
                } else if err_str.contains("already exists") {
                    InternalError::already_exists("Filter", id_or_name)
                } else if err_str.contains("mismatch") || err_str.contains("validation") {
                    InternalError::validation(err_str)
                } else {
                    InternalError::from(e)
                }
            })?;

        info!(
            filter_id = %updated.id,
            filter_name = %updated.name,
            "Filter updated via internal API"
        );

        Ok(OperationResult::with_message(
            updated,
            "Filter updated successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Delete a filter
    ///
    /// # Arguments
    /// * `id_or_name` - The filter ID or name to delete
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(filter_id_or_name = %id_or_name))]
    pub async fn delete(
        &self,
        id_or_name: &str,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // 1. Get existing filter and verify access
        let existing = self.get(id_or_name, auth).await?;

        // 2. Delete via service layer
        let service = FilterService::new(self.xds_state.clone());
        service.delete_filter(&existing.id).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("attached") || err_str.contains("in use") {
                InternalError::conflict(format!(
                    "Cannot delete filter '{}': {}",
                    existing.name, err_str
                ))
            } else {
                InternalError::from(e)
            }
        })?;

        info!(
            filter_id = %existing.id,
            filter_name = %existing.name,
            "Filter deleted via internal API"
        );

        Ok(OperationResult::with_message(
            (),
            "Filter deleted successfully. xDS configuration has been refreshed.",
        ))
    }

    // =========================================================================
    // Filter Attachment Operations
    // =========================================================================

    /// Attach a filter to a listener
    #[instrument(skip(self, auth), fields(filter_id = %filter_id, listener_name = %listener_name))]
    pub async fn attach_to_listener(
        &self,
        filter_id: &str,
        listener_name: &str,
        order: Option<i64>,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // Get and verify filter access
        let filter = self.get(filter_id, auth).await?;

        // Resolve listener name to ID
        let listener_repository =
            self.xds_state.listener_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Listener repository unavailable")
            })?;

        let listener = listener_repository.get_by_name(listener_name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Listener", listener_name)
            } else {
                InternalError::from(e)
            }
        })?;

        // Attach via service
        let service = FilterService::new(self.xds_state.clone());
        service.attach_filter_to_listener(&listener.id, &filter.id, order).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("already attached")
                || err_str.contains("UNIQUE constraint")
                || err_str.contains("unique constraint")
            {
                InternalError::conflict(format!(
                    "Filter '{}' is already attached to listener '{}'",
                    filter.name, listener_name
                ))
            } else {
                InternalError::from(e)
            }
        })?;

        info!(
            filter_id = %filter.id,
            filter_name = %filter.name,
            listener_name = %listener_name,
            "Filter attached to listener via internal API"
        );

        Ok(OperationResult::with_message((), "Filter attached to listener successfully."))
    }

    /// Detach a filter from a listener
    #[instrument(skip(self, auth), fields(filter_id = %filter_id, listener_name = %listener_name))]
    pub async fn detach_from_listener(
        &self,
        filter_id: &str,
        listener_name: &str,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // Get and verify filter access
        let filter = self.get(filter_id, auth).await?;

        // Resolve listener name to ID
        let listener_repository =
            self.xds_state.listener_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Listener repository unavailable")
            })?;

        let listener = listener_repository.get_by_name(listener_name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Listener", listener_name)
            } else {
                InternalError::from(e)
            }
        })?;

        // Detach via service
        let service = FilterService::new(self.xds_state.clone());
        service.detach_filter_from_listener(&listener.id, &filter.id).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not attached") || err_str.contains("not found") {
                InternalError::not_found(
                    "Filter attachment",
                    format!("{}/{}", filter_id, listener_name),
                )
            } else {
                InternalError::from(e)
            }
        })?;

        info!(
            filter_id = %filter.id,
            filter_name = %filter.name,
            listener_name = %listener_name,
            "Filter detached from listener via internal API"
        );

        Ok(OperationResult::with_message((), "Filter detached from listener successfully."))
    }

    /// Attach a filter to a route config
    #[instrument(skip(self, auth), fields(filter_id = %filter_id, route_config_name = %route_config_name))]
    pub async fn attach_to_route_config(
        &self,
        filter_id: &str,
        route_config_name: &str,
        order: Option<i64>,
        settings: Option<serde_json::Value>,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // Get and verify filter access
        let filter = self.get(filter_id, auth).await?;

        // Resolve route config name to ID
        let route_config_repository =
            self.xds_state.route_config_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Route config repository unavailable")
            })?;

        let route_config =
            route_config_repository.get_by_name(route_config_name).await.map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    InternalError::not_found("Route config", route_config_name)
                } else {
                    InternalError::from(e)
                }
            })?;

        // Attach via service
        let service = FilterService::new(self.xds_state.clone());
        service
            .attach_filter_to_route_config(&route_config.id, &filter.id, order, settings)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("already attached")
                    || err_str.contains("UNIQUE constraint")
                    || err_str.contains("unique constraint")
                {
                    InternalError::conflict(format!(
                        "Filter '{}' is already attached to route config '{}'",
                        filter.name, route_config_name
                    ))
                } else {
                    InternalError::from(e)
                }
            })?;

        info!(
            filter_id = %filter.id,
            filter_name = %filter.name,
            route_config_name = %route_config_name,
            "Filter attached to route config via internal API"
        );

        Ok(OperationResult::with_message((), "Filter attached to route config successfully."))
    }

    /// Detach a filter from a route config
    #[instrument(skip(self, auth), fields(filter_id = %filter_id, route_config_name = %route_config_name))]
    pub async fn detach_from_route_config(
        &self,
        filter_id: &str,
        route_config_name: &str,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // Get and verify filter access
        let filter = self.get(filter_id, auth).await?;

        // Resolve route config name to ID
        let route_config_repository =
            self.xds_state.route_config_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Route config repository unavailable")
            })?;

        let route_config =
            route_config_repository.get_by_name(route_config_name).await.map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    InternalError::not_found("Route config", route_config_name)
                } else {
                    InternalError::from(e)
                }
            })?;

        // Detach via service
        let service = FilterService::new(self.xds_state.clone());
        service.detach_filter_from_route_config(&route_config.id, &filter.id).await.map_err(
            |e| {
                let err_str = e.to_string();
                if err_str.contains("not attached") || err_str.contains("not found") {
                    InternalError::not_found(
                        "Filter attachment",
                        format!("{}/{}", filter_id, route_config_name),
                    )
                } else {
                    InternalError::from(e)
                }
            },
        )?;

        info!(
            filter_id = %filter.id,
            filter_name = %filter.name,
            route_config_name = %route_config_name,
            "Filter detached from route config via internal API"
        );

        Ok(OperationResult::with_message((), "Filter detached from route config successfully."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimpleXdsConfig;
    use crate::domain::FilterConfig;
    use crate::storage::test_helpers::TestDatabase;
    use crate::xds::filters::http::cors::{CorsConfig, CorsPolicyConfig};

    async fn setup_state() -> (TestDatabase, Arc<XdsState>) {
        let test_db = TestDatabase::new("internal_api_filters").await;
        let pool = test_db.pool.clone();
        let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool));
        (test_db, state)
    }

    fn sample_cors_config() -> FilterConfig {
        FilterConfig::Cors(CorsConfig {
            policy: CorsPolicyConfig {
                allow_origin: vec![],
                allow_methods: vec!["GET".to_string(), "POST".to_string()],
                allow_headers: vec![],
                expose_headers: vec![],
                max_age: Some(3600),
                allow_credentials: Some(false),
                allow_private_network_access: Some(false),
                forward_not_matching_preflights: Some(false),
                filter_enabled: None,
                shadow_enabled: None,
            },
        })
    }

    #[tokio::test]
    async fn test_create_filter_admin() {
        let (_db, state) = setup_state().await;
        let ops = FilterOperations::new(state);
        let auth = InternalAuthContext::admin();

        let req = CreateFilterRequest {
            name: "test-cors".to_string(),
            filter_type: "cors".to_string(),
            description: Some("Test CORS filter".to_string()),
            team: Some("test-team".to_string()),
            config: sample_cors_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());

        let op_result = result.unwrap();
        assert_eq!(op_result.data.name, "test-cors");
        assert_eq!(op_result.data.filter_type, "cors");
        assert!(op_result.message.is_some());
    }

    #[tokio::test]
    async fn test_create_filter_team_user() {
        let (_db, state) = setup_state().await;
        let ops = FilterOperations::new(state);
        let auth = InternalAuthContext::for_team("team-a");

        let req = CreateFilterRequest {
            name: "team-cors".to_string(),
            filter_type: "cors".to_string(),
            description: None,
            team: Some("team-a".to_string()),
            config: sample_cors_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_filter_wrong_team() {
        let (_db, state) = setup_state().await;
        let ops = FilterOperations::new(state);
        let auth = InternalAuthContext::for_team("team-a");

        let req = CreateFilterRequest {
            name: "wrong-team-filter".to_string(),
            filter_type: "cors".to_string(),
            description: None,
            team: Some("team-b".to_string()), // Different team
            config: sample_cors_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::Forbidden { .. }));
    }

    #[tokio::test]
    async fn test_get_filter_not_found() {
        let (_db, state) = setup_state().await;
        let ops = FilterOperations::new(state);
        let auth = InternalAuthContext::admin();

        let result = ops.get("nonexistent", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_filter_cross_team_returns_not_found() {
        let (_db, state) = setup_state().await;
        let ops = FilterOperations::new(state.clone());

        // Create filter as admin for team-a
        let admin_auth = InternalAuthContext::admin();
        let req = CreateFilterRequest {
            name: "team-a-filter".to_string(),
            filter_type: "cors".to_string(),
            description: None,
            team: Some("team-a".to_string()),
            config: sample_cors_config(),
        };
        ops.create(req, &admin_auth).await.expect("create filter");

        // Try to access from team-b
        let team_b_auth = InternalAuthContext::for_team("team-b");
        let result = ops.get("team-a-filter", &team_b_auth).await;

        assert!(result.is_err());
        // Should return NotFound to hide existence
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_list_filters_team_filtering() {
        let (_db, state) = setup_state().await;
        let ops = FilterOperations::new(state.clone());
        let admin_auth = InternalAuthContext::admin();

        // Create filters for different teams
        for (name, team) in
            [("filter-a1", "team-a"), ("filter-b1", "team-b"), ("filter-a2", "team-a")]
        {
            let req = CreateFilterRequest {
                name: name.to_string(),
                filter_type: "cors".to_string(),
                description: None,
                team: Some(team.to_string()),
                config: sample_cors_config(),
            };
            ops.create(req, &admin_auth).await.expect("create filter");
        }

        // List as team-a
        let team_a_auth = InternalAuthContext::for_team("team-a");
        let list_req = ListFiltersRequest { include_defaults: true, ..Default::default() };
        let result = ops.list(list_req, &team_a_auth).await.expect("list filters");

        // Should only see team-a filters
        assert_eq!(result.count, 2);
        for filter in &result.filters {
            assert_eq!(filter.team, "team-a");
        }
    }

    #[tokio::test]
    async fn test_list_filters_by_type() {
        let (_db, state) = setup_state().await;
        let ops = FilterOperations::new(state.clone());
        let admin_auth = InternalAuthContext::admin();

        // Create a filter
        let req = CreateFilterRequest {
            name: "test-cors".to_string(),
            filter_type: "cors".to_string(),
            description: None,
            team: Some("test-team".to_string()),
            config: sample_cors_config(),
        };
        ops.create(req, &admin_auth).await.expect("create filter");

        // List with type filter
        let list_req = ListFiltersRequest {
            filter_type: Some("cors".to_string()),
            include_defaults: true,
            ..Default::default()
        };
        let result = ops.list(list_req, &admin_auth).await.expect("list filters");
        assert_eq!(result.count, 1);
        assert_eq!(result.filters[0].filter_type, "cors");

        // List with different type filter
        let list_req = ListFiltersRequest {
            filter_type: Some("jwt_auth".to_string()),
            include_defaults: true,
            ..Default::default()
        };
        let result = ops.list(list_req, &admin_auth).await.expect("list filters");
        assert_eq!(result.count, 0);
    }

    #[tokio::test]
    async fn test_update_filter() {
        let (_db, state) = setup_state().await;
        let ops = FilterOperations::new(state);
        let auth = InternalAuthContext::admin();

        // Create a filter
        let create_req = CreateFilterRequest {
            name: "update-test".to_string(),
            filter_type: "cors".to_string(),
            description: Some("Original".to_string()),
            team: Some("test-team".to_string()),
            config: sample_cors_config(),
        };
        ops.create(create_req, &auth).await.expect("create filter");

        // Update it
        let update_req = UpdateFilterRequest {
            name: Some("update-test-renamed".to_string()),
            description: Some("Updated".to_string()),
            config: None,
        };
        let result = ops.update("update-test", update_req, &auth).await;

        assert!(result.is_ok());
        let updated = result.unwrap().data;
        assert_eq!(updated.name, "update-test-renamed");
        assert_eq!(updated.description, Some("Updated".to_string()));
    }

    #[tokio::test]
    async fn test_delete_filter() {
        let (_db, state) = setup_state().await;
        let ops = FilterOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create a filter
        let create_req = CreateFilterRequest {
            name: "delete-test".to_string(),
            filter_type: "cors".to_string(),
            description: None,
            team: Some("test-team".to_string()),
            config: sample_cors_config(),
        };
        ops.create(create_req, &auth).await.expect("create filter");

        // Delete it
        let result = ops.delete("delete-test", &auth).await;
        assert!(result.is_ok());

        // Verify it's gone
        let get_result = ops.get("delete-test", &auth).await;
        assert!(get_result.is_err());
    }
}
