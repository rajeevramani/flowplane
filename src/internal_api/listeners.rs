//! Listener Operations for Internal API
//!
//! This module provides the unified listener operations layer that sits between
//! HTTP/MCP handlers and the ListenerService. It handles:
//! - Request validation
//! - Team-based access control
//! - Error mapping
//! - Response formatting

use std::sync::Arc;
use tracing::{info, instrument};

use crate::internal_api::auth::{verify_team_access, InternalAuthContext};
use crate::internal_api::error::InternalError;
use crate::internal_api::types::{
    CreateListenerRequest, ListListenersRequest, ListListenersResponse, OperationResult,
    UpdateListenerRequest,
};
use crate::openapi::defaults::is_default_gateway_listener;
use crate::services::ListenerService;
use crate::storage::ListenerData;
use crate::xds::XdsState;

/// Listener operations for the internal API layer
///
/// This struct provides all CRUD operations for listeners with unified
/// validation and access control.
pub struct ListenerOperations {
    xds_state: Arc<XdsState>,
}

impl ListenerOperations {
    /// Create a new ListenerOperations instance
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Create a new listener
    ///
    /// # Arguments
    /// * `req` - The create listener request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the created listener on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(listener_name = %req.name, team = ?req.team))]
    pub async fn create(
        &self,
        req: CreateListenerRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<ListenerData>, InternalError> {
        // 1. Verify team access (can create in this team?)
        if !auth.can_create_for_team(req.team.as_deref()) {
            return Err(InternalError::forbidden(format!(
                "Cannot create listener for team '{}'",
                req.team.as_deref().unwrap_or("global")
            )));
        }

        // 2. Call service layer
        let service = ListenerService::new(self.xds_state.clone());
        let created = service
            .create_listener(
                req.name.clone(),
                req.address,
                req.port,
                req.protocol.unwrap_or_else(|| "HTTP".to_string()),
                req.config,
                req.team,
            )
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("already exists") || err_str.contains("UNIQUE constraint") {
                    InternalError::already_exists("Listener", &req.name)
                } else {
                    InternalError::from(e)
                }
            })?;

        info!(
            listener_id = %created.id,
            listener_name = %created.name,
            "Listener created via internal API"
        );

        Ok(OperationResult::with_message(
            created,
            "Listener created successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Get a listener by name
    ///
    /// # Arguments
    /// * `name` - The listener name
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(ListenerData)` if found and accessible
    /// * `Err(InternalError::NotFound)` if not found or not accessible
    #[instrument(skip(self, auth), fields(listener_name = %name))]
    pub async fn get(
        &self,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<ListenerData, InternalError> {
        let service = ListenerService::new(self.xds_state.clone());

        // Get the listener
        let listener = service.get_listener(name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Listener", name)
            } else {
                InternalError::from(e)
            }
        })?;

        // Verify team access
        verify_team_access(listener, auth).await
    }

    /// List listeners with pagination
    ///
    /// # Arguments
    /// * `req` - List request with pagination options
    /// * `auth` - Authentication context for filtering
    ///
    /// # Returns
    /// * `Ok(ListListenersResponse)` with filtered listeners
    #[instrument(skip(self, auth), fields(limit = ?req.limit, offset = ?req.offset))]
    pub async fn list(
        &self,
        req: ListListenersRequest,
        auth: &InternalAuthContext,
    ) -> Result<ListListenersResponse, InternalError> {
        let repository =
            self.xds_state.listener_repository.as_ref().ok_or_else(|| {
                InternalError::service_unavailable("Listener repository unavailable")
            })?;

        // Use team filtering - empty allowed_teams means admin access to all
        let listeners = repository
            .list_by_teams(&auth.allowed_teams, req.include_defaults, req.limit, req.offset)
            .await
            .map_err(|e| InternalError::database(e.to_string()))?;

        let count = listeners.len();

        Ok(ListListenersResponse { listeners, count, limit: req.limit, offset: req.offset })
    }

    /// Update an existing listener
    ///
    /// # Arguments
    /// * `name` - The listener name to update
    /// * `req` - The update request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the updated listener on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(listener_name = %name))]
    pub async fn update(
        &self,
        name: &str,
        req: UpdateListenerRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<ListenerData>, InternalError> {
        // 1. Get existing listener and verify access
        let existing = self.get(name, auth).await?;

        // 2. Determine values (use existing if not provided)
        let address = req.address.unwrap_or_else(|| existing.address.clone());
        let port = req.port.unwrap_or_else(|| existing.port.unwrap_or(80) as u16);
        let protocol = req.protocol.unwrap_or_else(|| existing.protocol.clone());

        // 3. Update via service layer
        let service = ListenerService::new(self.xds_state.clone());
        let updated = service
            .update_listener(name, address, port, protocol, req.config)
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    InternalError::not_found("Listener", name)
                } else {
                    InternalError::from(e)
                }
            })?;

        info!(
            listener_id = %updated.id,
            listener_name = %updated.name,
            "Listener updated via internal API"
        );

        Ok(OperationResult::with_message(
            updated,
            "Listener updated successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Delete a listener
    ///
    /// # Arguments
    /// * `name` - The listener name to delete
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(listener_name = %name))]
    pub async fn delete(
        &self,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // 1. Check for default listener protection
        if is_default_gateway_listener(name) {
            return Err(InternalError::forbidden(
                "The default gateway listener cannot be deleted".to_string(),
            ));
        }

        // 2. Get existing listener and verify access
        let _existing = self.get(name, auth).await?;

        // 3. Delete via service layer
        let service = ListenerService::new(self.xds_state.clone());
        service.delete_listener(name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("default gateway") || err_str.contains("cannot be deleted") {
                InternalError::forbidden(err_str)
            } else {
                InternalError::from(e)
            }
        })?;

        info!(listener_name = %name, "Listener deleted via internal API");

        Ok(OperationResult::with_message(
            (),
            "Listener deleted successfully. xDS configuration has been refreshed.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimpleXdsConfig;
    use crate::storage::{create_pool, DatabaseConfig};
    use crate::xds::listener::{FilterChainConfig, FilterConfig, FilterType, ListenerConfig};
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

        // Create listeners table for repository usage
        pool.execute(
            r#"
            CREATE TABLE IF NOT EXISTS listeners (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                address TEXT NOT NULL,
                port INTEGER,
                protocol TEXT NOT NULL DEFAULT 'HTTP',
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'openapi_import')),
                team TEXT,
                import_id TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .await
        .expect("create table");

        Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool))
    }

    fn sample_config() -> ListenerConfig {
        ListenerConfig {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            filter_chains: vec![FilterChainConfig {
                name: Some("default".to_string()),
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: FilterType::HttpConnectionManager {
                        route_config_name: Some("test-routes".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: vec![],
                    },
                }],
                tls_context: None,
            }],
        }
    }

    #[tokio::test]
    async fn test_create_listener_admin() {
        let state = setup_state().await;
        let ops = ListenerOperations::new(state);
        let auth = InternalAuthContext::admin();

        let req = CreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            protocol: Some("HTTP".to_string()),
            team: Some("test-team".to_string()),
            config: sample_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());

        let op_result = result.unwrap();
        assert_eq!(op_result.data.name, "test-listener");
        assert_eq!(op_result.data.address, "0.0.0.0");
        assert_eq!(op_result.data.port, Some(8080));
        assert!(op_result.message.is_some());
    }

    #[tokio::test]
    async fn test_create_listener_team_user() {
        let state = setup_state().await;
        let ops = ListenerOperations::new(state);
        let auth = InternalAuthContext::for_team("team-a");

        let req = CreateListenerRequest {
            name: "team-listener".to_string(),
            address: "127.0.0.1".to_string(),
            port: 9090,
            protocol: Some("HTTPS".to_string()),
            team: Some("team-a".to_string()),
            config: sample_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_listener_wrong_team() {
        let state = setup_state().await;
        let ops = ListenerOperations::new(state);
        let auth = InternalAuthContext::for_team("team-a");

        let req = CreateListenerRequest {
            name: "wrong-team-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            protocol: Some("HTTP".to_string()),
            team: Some("team-b".to_string()), // Different team
            config: sample_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::Forbidden { .. }));
    }

    #[tokio::test]
    async fn test_get_listener_not_found() {
        let state = setup_state().await;
        let ops = ListenerOperations::new(state);
        let auth = InternalAuthContext::admin();

        let result = ops.get("nonexistent", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_listener_cross_team_returns_not_found() {
        let state = setup_state().await;
        let ops = ListenerOperations::new(state.clone());

        // Create listener as admin for team-a
        let admin_auth = InternalAuthContext::admin();
        let req = CreateListenerRequest {
            name: "team-a-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            protocol: Some("HTTP".to_string()),
            team: Some("team-a".to_string()),
            config: sample_config(),
        };
        ops.create(req, &admin_auth).await.expect("create listener");

        // Try to access from team-b
        let team_b_auth = InternalAuthContext::for_team("team-b");
        let result = ops.get("team-a-listener", &team_b_auth).await;

        assert!(result.is_err());
        // Should return NotFound to hide existence
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_list_listeners_team_filtering() {
        let state = setup_state().await;
        let ops = ListenerOperations::new(state.clone());
        let admin_auth = InternalAuthContext::admin();

        // Create listeners for different teams
        for (name, port, team) in [
            ("listener-a1", 8081, "team-a"),
            ("listener-b1", 8082, "team-b"),
            ("listener-a2", 8083, "team-a"),
        ] {
            let req = CreateListenerRequest {
                name: name.to_string(),
                address: "0.0.0.0".to_string(),
                port,
                protocol: Some("HTTP".to_string()),
                team: Some(team.to_string()),
                config: sample_config(),
            };
            ops.create(req, &admin_auth).await.expect("create listener");
        }

        // List as team-a
        let team_a_auth = InternalAuthContext::for_team("team-a");
        let list_req = ListListenersRequest { include_defaults: true, ..Default::default() };
        let result = ops.list(list_req, &team_a_auth).await.expect("list listeners");

        // Should only see team-a listeners
        assert_eq!(result.count, 2);
        for listener in &result.listeners {
            assert_eq!(listener.team.as_deref(), Some("team-a"));
        }
    }

    #[tokio::test]
    async fn test_update_listener() {
        let state = setup_state().await;
        let ops = ListenerOperations::new(state);
        let auth = InternalAuthContext::admin();

        // Create a listener
        let create_req = CreateListenerRequest {
            name: "update-test".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            protocol: Some("HTTP".to_string()),
            team: Some("test-team".to_string()),
            config: sample_config(),
        };
        ops.create(create_req, &auth).await.expect("create listener");

        // Update it
        let mut updated_config = sample_config();
        updated_config.address = "127.0.0.1".to_string();
        updated_config.port = 9090;

        let update_req = UpdateListenerRequest {
            address: Some("127.0.0.1".to_string()),
            port: Some(9090),
            protocol: Some("HTTPS".to_string()),
            config: updated_config,
        };
        let result = ops.update("update-test", update_req, &auth).await;

        assert!(result.is_ok());
        let updated = result.unwrap().data;
        assert_eq!(updated.address, "127.0.0.1");
        assert_eq!(updated.port, Some(9090));
    }

    #[tokio::test]
    async fn test_delete_listener() {
        let state = setup_state().await;
        let ops = ListenerOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create a listener
        let create_req = CreateListenerRequest {
            name: "delete-test".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            protocol: Some("HTTP".to_string()),
            team: Some("test-team".to_string()),
            config: sample_config(),
        };
        ops.create(create_req, &auth).await.expect("create listener");

        // Delete it
        let result = ops.delete("delete-test", &auth).await;
        assert!(result.is_ok());

        // Verify it's gone
        let get_result = ops.get("delete-test", &auth).await;
        assert!(get_result.is_err());
    }

    #[tokio::test]
    async fn test_delete_default_listener_blocked() {
        let state = setup_state().await;
        let ops = ListenerOperations::new(state);
        let auth = InternalAuthContext::admin();

        // Try to delete the default gateway listener
        let result = ops.delete("default-gateway-listener", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::Forbidden { .. }));
    }
}
