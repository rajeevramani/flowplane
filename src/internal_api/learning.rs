//! Learning Session Operations for Internal API
//!
//! This module provides the unified learning session operations layer that sits between
//! HTTP/MCP handlers and the LearningSessionService. It handles:
//! - Request validation
//! - Team-based access control
//! - Error mapping
//! - Response formatting

use std::sync::Arc;
use tracing::{info, instrument};

use crate::internal_api::auth::InternalAuthContext;
use crate::internal_api::error::InternalError;
use crate::internal_api::types::{
    CreateLearningSessionInternalRequest, ListLearningSessionsRequest, OperationResult,
};
use crate::services::LearningSessionService;
use crate::storage::repositories::{
    CreateLearningSessionRequest, LearningSessionData, LearningSessionStatus,
};
use crate::xds::XdsState;

/// Learning session operations for the internal API layer
///
/// This struct provides all CRUD operations for learning sessions with unified
/// validation and access control.
pub struct LearningSessionOperations {
    xds_state: Arc<XdsState>,
}

impl LearningSessionOperations {
    /// Create a new LearningSessionOperations instance
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// List learning sessions with optional filtering
    ///
    /// # Arguments
    /// * `req` - List request with optional status filter and pagination
    /// * `auth` - Authentication context for team filtering
    ///
    /// # Returns
    /// * `Ok(Vec<LearningSessionData>)` with filtered sessions
    #[instrument(skip(self, auth), fields(limit = ?req.limit, offset = ?req.offset, status = ?req.status))]
    pub async fn list(
        &self,
        req: ListLearningSessionsRequest,
        auth: &InternalAuthContext,
    ) -> Result<Vec<LearningSessionData>, InternalError> {
        // Get pool from cluster_repository (pattern used in handlers)
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository = crate::storage::repositories::LearningSessionRepository::new(
            cluster_repo.pool().clone(),
        );

        // Parse status filter if provided
        let status_filter = if let Some(status_str) = req.status {
            let status: LearningSessionStatus =
                status_str.parse().map_err(|e: crate::errors::FlowplaneError| {
                    InternalError::validation(format!("Invalid status: {}", e))
                })?;
            Some(status)
        } else {
            None
        };

        // Admin can see all sessions
        let sessions = if auth.is_admin {
            repository
                .list_all(status_filter, req.limit.map(|l| l as i32), req.offset.map(|o| o as i32))
                .await
                .map_err(InternalError::from)?
        } else {
            // Non-admin users can only see sessions from their allowed teams
            let mut all_sessions = Vec::new();
            for team in &auth.allowed_teams {
                let team_sessions = repository
                    .list_by_team(
                        team,
                        status_filter.clone(),
                        req.limit.map(|l| l as i32),
                        req.offset.map(|o| o as i32),
                    )
                    .await
                    .map_err(InternalError::from)?;
                all_sessions.extend(team_sessions);
            }
            all_sessions
        };

        Ok(sessions)
    }

    /// Get a learning session by ID
    ///
    /// # Arguments
    /// * `id` - The session ID
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(LearningSessionData)` if found and accessible
    /// * `Err(InternalError::NotFound)` if not found or not accessible
    #[instrument(skip(self, auth), fields(session_id = %id))]
    pub async fn get(
        &self,
        id: &str,
        auth: &InternalAuthContext,
    ) -> Result<LearningSessionData, InternalError> {
        // Get pool from cluster_repository (pattern used in handlers)
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository = crate::storage::repositories::LearningSessionRepository::new(
            cluster_repo.pool().clone(),
        );

        // Get the session
        let session = repository.get_by_id(id).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Learning session", id)
            } else {
                InternalError::from(e)
            }
        })?;

        // Verify team access
        if !auth.can_access_team(Some(&session.team)) {
            return Err(InternalError::not_found("Learning session", id));
        }

        Ok(session)
    }

    /// Create a new learning session
    ///
    /// # Arguments
    /// * `req` - The create request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the created session on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(route_pattern = %req.route_pattern, team = ?req.team, auto_start = ?req.auto_start))]
    pub async fn create(
        &self,
        req: CreateLearningSessionInternalRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<LearningSessionData>, InternalError> {
        // 1. Determine team - if provided, verify access; otherwise use auth team
        let team = if let Some(team) = req.team {
            // Verify team access
            if !auth.can_create_for_team(Some(&team)) {
                return Err(InternalError::forbidden(format!(
                    "Cannot create learning session for team '{}'",
                    team
                )));
            }
            team
        } else {
            // Use the primary team from auth context
            auth.team.clone().ok_or_else(|| {
                InternalError::validation(
                    "Learning session must be associated with a team".to_string(),
                )
            })?
        };

        // 2. Validate target sample count
        if req.target_sample_count <= 0 {
            return Err(InternalError::validation(
                "target_sample_count must be greater than 0".to_string(),
            ));
        }

        // 3. Create the session via repository
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository = crate::storage::repositories::LearningSessionRepository::new(
            cluster_repo.pool().clone(),
        );

        let create_request = CreateLearningSessionRequest {
            team: team.clone(),
            route_pattern: req.route_pattern.clone(),
            cluster_name: req.cluster_name,
            http_methods: req.http_methods,
            target_sample_count: req.target_sample_count,
            max_duration_seconds: None, // Could be added to the request if needed
            triggered_by: None,
            deployment_version: None,
            configuration_snapshot: None,
        };

        let created = repository.create(create_request).await.map_err(InternalError::from)?;

        info!(
            session_id = %created.id,
            team = %team,
            route_pattern = %created.route_pattern,
            "Learning session created via internal API"
        );

        // 4. Auto-start if requested
        let session = if req.auto_start.unwrap_or(false) {
            // Use the existing learning session service from XdsState if available
            if let Some(service) = self.xds_state.get_learning_session_service() {
                // Activate the session
                service.activate_session(&created.id).await.map_err(|e| {
                    InternalError::internal(format!("Failed to activate session: {}", e))
                })?
            } else {
                // If no service is configured, create a minimal one for activation
                let service = LearningSessionService::new(repository.clone());

                // Wire up the service dependencies if available
                let service = if let Some(access_log_service) = &self.xds_state.access_log_service {
                    service.with_access_log_service(access_log_service.clone())
                } else {
                    service
                };

                let service = if let Some(ext_proc_service) = &self.xds_state.ext_proc_service {
                    service.with_ext_proc_service(ext_proc_service.clone())
                } else {
                    service
                };

                let service = service.with_xds_state(self.xds_state.clone());

                // Activate the session
                service.activate_session(&created.id).await.map_err(|e| {
                    InternalError::internal(format!("Failed to activate session: {}", e))
                })?
            }
        } else {
            created
        };

        let message = if req.auto_start.unwrap_or(false) {
            "Learning session created and activated successfully."
        } else {
            "Learning session created successfully. Use activate to start collecting samples."
        };

        Ok(OperationResult::with_message(session, message))
    }

    /// Delete (cancel) a learning session
    ///
    /// # Arguments
    /// * `id` - The session ID to delete
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(session_id = %id))]
    pub async fn delete(
        &self,
        id: &str,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // 1. Get existing session and verify access
        let existing = self.get(id, auth).await?;

        // 2. Validate state - can only delete pending or active sessions
        match existing.status {
            LearningSessionStatus::Pending | LearningSessionStatus::Active => {
                // OK to delete
            }
            LearningSessionStatus::Completing | LearningSessionStatus::Completed => {
                return Err(InternalError::conflict(format!(
                    "Cannot delete session in '{}' state. Session is already completed.",
                    existing.status
                )));
            }
            LearningSessionStatus::Cancelled => {
                return Err(InternalError::conflict("Session is already cancelled".to_string()));
            }
            LearningSessionStatus::Failed => {
                return Err(InternalError::conflict("Cannot delete failed session".to_string()));
            }
        }

        // 3. Delete via repository
        let cluster_repo = self
            .xds_state
            .cluster_repository
            .as_ref()
            .ok_or_else(|| InternalError::service_unavailable("Repository not configured"))?;
        let repository = crate::storage::repositories::LearningSessionRepository::new(
            cluster_repo.pool().clone(),
        );

        repository.delete(id, &existing.team).await.map_err(InternalError::from)?;

        // 4. If session was active, unregister from access log service
        if existing.status == LearningSessionStatus::Active {
            if let Some(access_log_service) = &self.xds_state.access_log_service {
                access_log_service.remove_session(id).await;
                info!(
                    session_id = %id,
                    "Unregistered learning session from Access Log Service"
                );
            }

            // Unregister from ExtProc service
            if let Some(ext_proc_service) = &self.xds_state.ext_proc_service {
                ext_proc_service.remove_session(id).await;
                info!(
                    session_id = %id,
                    "Unregistered learning session from ExtProc Service"
                );
            }

            // Trigger LDS update to remove access log configuration
            if let Err(e) = self.xds_state.refresh_listeners_from_repository().await {
                tracing::error!(
                    session_id = %id,
                    error = %e,
                    "Failed to refresh listeners after session deletion"
                );
            } else {
                info!(
                    session_id = %id,
                    "Triggered LDS update to remove access log configuration"
                );
            }
        }

        info!(
            session_id = %id,
            team = %existing.team,
            "Learning session deleted via internal API"
        );

        Ok(OperationResult::with_message((), "Learning session deleted successfully."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimpleXdsConfig;
    use crate::storage::test_helpers::{TestDatabase, TEAM_A_ID, TEAM_B_ID, TEST_TEAM_ID};

    async fn setup_state() -> (TestDatabase, Arc<XdsState>) {
        let test_db = TestDatabase::new("internal_api_learning").await;
        let pool = test_db.pool.clone();
        let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool));
        (test_db, state)
    }

    #[tokio::test]
    async fn test_create_learning_session_admin() {
        let (_db, state) = setup_state().await;
        let ops = LearningSessionOperations::new(state);
        let auth = InternalAuthContext::admin();

        let req = CreateLearningSessionInternalRequest {
            team: Some(TEST_TEAM_ID.to_string()),
            route_pattern: "^/api/users.*".to_string(),
            cluster_name: Some("users-api".to_string()),
            http_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
            target_sample_count: 100,
            auto_start: Some(false),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());

        let op_result = result.unwrap();
        assert_eq!(op_result.data.team, TEST_TEAM_ID);
        assert_eq!(op_result.data.route_pattern, "^/api/users.*");
        assert_eq!(op_result.data.status, LearningSessionStatus::Pending);
        assert!(op_result.message.is_some());
    }

    #[tokio::test]
    async fn test_create_learning_session_team_user() {
        let (_db, state) = setup_state().await;
        let ops = LearningSessionOperations::new(state);
        let auth = InternalAuthContext::for_team(TEAM_A_ID);

        let req = CreateLearningSessionInternalRequest {
            team: Some(TEAM_A_ID.to_string()),
            route_pattern: "^/api/.*".to_string(),
            cluster_name: None,
            http_methods: None,
            target_sample_count: 50,
            auto_start: Some(false),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_learning_session_wrong_team() {
        let (_db, state) = setup_state().await;
        let ops = LearningSessionOperations::new(state);
        let auth = InternalAuthContext::for_team(TEAM_A_ID);

        let req = CreateLearningSessionInternalRequest {
            team: Some(TEAM_B_ID.to_string()), // Different team
            route_pattern: "^/api/.*".to_string(),
            cluster_name: None,
            http_methods: None,
            target_sample_count: 50,
            auto_start: Some(false),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::Forbidden { .. }));
    }

    #[tokio::test]
    async fn test_create_learning_session_invalid_sample_count() {
        let (_db, state) = setup_state().await;
        let ops = LearningSessionOperations::new(state);
        let auth = InternalAuthContext::admin();

        let req = CreateLearningSessionInternalRequest {
            team: Some(TEST_TEAM_ID.to_string()),
            route_pattern: "^/api/.*".to_string(),
            cluster_name: None,
            http_methods: None,
            target_sample_count: 0, // Invalid
            auto_start: Some(false),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::InvalidInput { .. }));
    }

    #[tokio::test]
    async fn test_get_learning_session_not_found() {
        let (_db, state) = setup_state().await;
        let ops = LearningSessionOperations::new(state);
        let auth = InternalAuthContext::admin();

        let result = ops.get("nonexistent", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_learning_session_cross_team_returns_not_found() {
        let (_db, state) = setup_state().await;
        let ops = LearningSessionOperations::new(state.clone());

        // Create session as admin for team-a
        let admin_auth = InternalAuthContext::admin();
        let req = CreateLearningSessionInternalRequest {
            team: Some(TEAM_A_ID.to_string()),
            route_pattern: "^/api/.*".to_string(),
            cluster_name: None,
            http_methods: None,
            target_sample_count: 100,
            auto_start: Some(false),
        };
        let created = ops.create(req, &admin_auth).await.expect("create session");

        // Try to access from team-b
        let team_b_auth = InternalAuthContext::for_team(TEAM_B_ID);
        let result = ops.get(&created.data.id, &team_b_auth).await;

        assert!(result.is_err());
        // Should return NotFound to hide existence
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_list_learning_sessions_team_filtering() {
        let (_db, state) = setup_state().await;
        let ops = LearningSessionOperations::new(state.clone());
        let admin_auth = InternalAuthContext::admin();

        // Create sessions for different teams
        for (team, count) in [(TEAM_A_ID, 2), (TEAM_B_ID, 1)] {
            for i in 0..count {
                let req = CreateLearningSessionInternalRequest {
                    team: Some(team.to_string()),
                    route_pattern: format!("^/api/v{}/.*", i),
                    cluster_name: None,
                    http_methods: None,
                    target_sample_count: 100,
                    auto_start: Some(false),
                };
                ops.create(req, &admin_auth).await.expect("create session");
            }
        }

        // List as team-a
        let team_a_auth = InternalAuthContext::for_team(TEAM_A_ID);
        let list_req = ListLearningSessionsRequest { status: None, limit: None, offset: None };
        let result = ops.list(list_req, &team_a_auth).await.expect("list sessions");

        // Should only see team-a sessions
        assert_eq!(result.len(), 2);
        for session in &result {
            assert_eq!(session.team, TEAM_A_ID);
        }
    }

    #[tokio::test]
    async fn test_delete_learning_session() {
        let (_db, state) = setup_state().await;
        let ops = LearningSessionOperations::new(state.clone());
        let auth = InternalAuthContext::admin();

        // Create a session
        let create_req = CreateLearningSessionInternalRequest {
            team: Some(TEST_TEAM_ID.to_string()),
            route_pattern: "^/api/.*".to_string(),
            cluster_name: None,
            http_methods: None,
            target_sample_count: 100,
            auto_start: Some(false),
        };
        let created = ops.create(create_req, &auth).await.expect("create session");

        // Delete it
        let result = ops.delete(&created.data.id, &auth).await;
        assert!(result.is_ok());

        // Verify it's gone
        let get_result = ops.get(&created.data.id, &auth).await;
        assert!(get_result.is_err());
    }
}
