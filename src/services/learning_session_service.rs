//! Learning session business logic service
//!
//! This module contains the state machine logic for learning session lifecycle management,
//! including automatic completion and state transitions.

use std::sync::Arc;
use tracing::{error, info, warn};

use crate::{
    errors::{Error, Result},
    services::webhook_service::{LearningSessionWebhookEvent, WebhookService},
    storage::repositories::{
        LearningSessionData, LearningSessionRepository, LearningSessionStatus,
        UpdateLearningSessionRequest,
    },
    xds::services::access_log_service::{FlowplaneAccessLogService, LearningSession},
};

/// Service for managing learning session lifecycle and state machine
#[derive(Clone, Debug)]
pub struct LearningSessionService {
    repository: LearningSessionRepository,
    access_log_service: Option<Arc<FlowplaneAccessLogService>>,
    webhook_service: Option<Arc<WebhookService>>,
}

impl LearningSessionService {
    /// Create a new learning session service
    pub fn new(repository: LearningSessionRepository) -> Self {
        Self {
            repository,
            access_log_service: None,
            webhook_service: None,
        }
    }

    /// Set the access log service for integration
    pub fn with_access_log_service(mut self, service: Arc<FlowplaneAccessLogService>) -> Self {
        self.access_log_service = Some(service);
        self
    }

    /// Set the webhook service for event notifications
    pub fn with_webhook_service(mut self, service: Arc<WebhookService>) -> Self {
        self.webhook_service = Some(service);
        self
    }

    /// Activate a learning session (transition: pending → active)
    ///
    /// This method:
    /// 1. Updates session status to 'active'
    /// 2. Sets started_at timestamp
    /// 3. Registers with Access Log Service for filtering
    pub async fn activate_session(&self, session_id: &str) -> Result<LearningSessionData> {
        let session = self.repository.get_by_id(session_id).await?;

        // Validate state transition
        if session.status != LearningSessionStatus::Pending {
            return Err(Error::validation(format!(
                "Cannot activate session in '{}' state. Must be 'pending'",
                session.status
            )));
        }

        // Update to active
        let now = chrono::Utc::now();
        let update_request = UpdateLearningSessionRequest {
            status: Some(LearningSessionStatus::Active),
            started_at: Some(now),
            ends_at: None,
            completed_at: None,
            current_sample_count: None,
            error_message: None,
        };

        let updated = self.repository.update(session_id, update_request).await?;

        // Register with Access Log Service if available
        if let Some(access_log_service) = &self.access_log_service {
            let learning_session = convert_to_access_log_session(&updated)?;
            access_log_service.add_session(learning_session).await;
            info!(
                session_id = %session_id,
                route_pattern = %updated.route_pattern,
                "Registered learning session with Access Log Service"
            );
        }

        info!(
            session_id = %session_id,
            "Activated learning session: pending → active"
        );

        // Publish webhook event if webhook service is available
        if let Some(webhook_service) = &self.webhook_service {
            let event = LearningSessionWebhookEvent::activated(
                updated.id.clone(),
                updated.team.clone(),
                updated.route_pattern.clone(),
                updated.target_sample_count,
            );
            webhook_service.publish_event(event).await;
        }

        Ok(updated)
    }

    /// Check if a session should be completed and transition if needed
    ///
    /// This method checks:
    /// 1. Has the target sample count been reached?
    /// 2. Has the session timed out (ends_at exceeded)?
    ///
    /// If either condition is true: active → completing → completed
    pub async fn check_completion(&self, session_id: &str) -> Result<Option<LearningSessionData>> {
        let session = self.repository.get_by_id(session_id).await?;

        // Only check completion for active sessions
        if session.status != LearningSessionStatus::Active {
            return Ok(None);
        }

        let should_complete = self.should_complete(&session);

        if should_complete {
            self.complete_session(session_id).await.map(Some)
        } else {
            Ok(None)
        }
    }

    /// Determine if a session should be completed
    fn should_complete(&self, session: &LearningSessionData) -> bool {
        let now = chrono::Utc::now();

        // Check if target sample count reached
        let target_reached = session.current_sample_count >= session.target_sample_count;

        // Check if session timed out
        let timed_out = session.ends_at.is_some_and(|ends_at| now >= ends_at);

        if target_reached {
            info!(
                session_id = %session.id,
                current = session.current_sample_count,
                target = session.target_sample_count,
                "Session reached target sample count"
            );
        }

        if timed_out {
            warn!(
                session_id = %session.id,
                ends_at = %session.ends_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                "Session timed out"
            );
        }

        target_reached || timed_out
    }

    /// Complete a learning session (active → completing → completed)
    async fn complete_session(&self, session_id: &str) -> Result<LearningSessionData> {
        // First, transition to 'completing' state
        let update_completing = UpdateLearningSessionRequest {
            status: Some(LearningSessionStatus::Completing),
            started_at: None,
            ends_at: None,
            completed_at: None,
            current_sample_count: None,
            error_message: None,
        };

        self.repository.update(session_id, update_completing).await?;

        info!(session_id = %session_id, "Session transitioning: active → completing");

        // Unregister from Access Log Service
        if let Some(access_log_service) = &self.access_log_service {
            access_log_service.remove_session(session_id).await;
            info!(
                session_id = %session_id,
                "Unregistered learning session from Access Log Service"
            );
        }

        // TODO: Trigger schema aggregation here (Task 6)
        // For now, just complete immediately

        // Transition to 'completed' state
        let now = chrono::Utc::now();
        let update_completed = UpdateLearningSessionRequest {
            status: Some(LearningSessionStatus::Completed),
            started_at: None,
            ends_at: None,
            completed_at: Some(now),
            current_sample_count: None,
            error_message: None,
        };

        let completed = self.repository.update(session_id, update_completed).await?;

        info!(
            session_id = %session_id,
            sample_count = completed.current_sample_count,
            "Session completed: completing → completed"
        );

        // Publish webhook event if webhook service is available
        if let Some(webhook_service) = &self.webhook_service {
            let event = LearningSessionWebhookEvent::completed(
                completed.id.clone(),
                completed.team.clone(),
                completed.route_pattern.clone(),
                completed.target_sample_count,
                completed.current_sample_count,
            );
            webhook_service.publish_event(event).await;
        }

        Ok(completed)
    }

    /// Mark a session as failed with an error message
    pub async fn fail_session(
        &self,
        session_id: &str,
        error_message: String,
    ) -> Result<LearningSessionData> {
        let update_request = UpdateLearningSessionRequest {
            status: Some(LearningSessionStatus::Failed),
            started_at: None,
            ends_at: None,
            completed_at: Some(chrono::Utc::now()),
            current_sample_count: None,
            error_message: Some(error_message.clone()),
        };

        let failed = self.repository.update(session_id, update_request).await?;

        // Unregister from Access Log Service
        if let Some(access_log_service) = &self.access_log_service {
            access_log_service.remove_session(session_id).await;
        }

        error!(
            session_id = %session_id,
            error = %error_message,
            "Session failed"
        );

        // Publish webhook event if webhook service is available
        if let Some(webhook_service) = &self.webhook_service {
            let event = LearningSessionWebhookEvent::failed(
                failed.id.clone(),
                failed.team.clone(),
                failed.route_pattern.clone(),
                error_message,
                failed.current_sample_count,
                failed.target_sample_count,
            );
            webhook_service.publish_event(event).await;
        }

        Ok(failed)
    }

    /// Background worker that checks all active sessions for completion
    ///
    /// This should be called periodically (e.g., every 30 seconds)
    pub async fn check_all_active_sessions(&self) -> Result<Vec<String>> {
        let active_sessions = self.repository.list_active().await?;

        let mut completed_sessions = Vec::new();

        for session in active_sessions {
            match self.check_completion(&session.id).await {
                Ok(Some(_)) => {
                    completed_sessions.push(session.id.clone());
                }
                Ok(None) => {
                    // Session not ready for completion yet
                }
                Err(e) => {
                    error!(
                        session_id = %session.id,
                        error = %e,
                        "Failed to check completion for session"
                    );
                }
            }
        }

        if !completed_sessions.is_empty() {
            info!(
                count = completed_sessions.len(),
                sessions = ?completed_sessions,
                "Auto-completed sessions"
            );
        }

        Ok(completed_sessions)
    }

    /// Sync all active sessions with the Access Log Service
    ///
    /// This is useful for recovery after restarts
    pub async fn sync_active_sessions_with_access_log_service(&self) -> Result<usize> {
        let Some(access_log_service) = &self.access_log_service else {
            warn!("Access Log Service not configured, skipping sync");
            return Ok(0);
        };

        let active_sessions = self.repository.list_active().await?;

        let mut synced_count = 0;
        for session in active_sessions {
            match convert_to_access_log_session(&session) {
                Ok(learning_session) => {
                    access_log_service.add_session(learning_session).await;
                    synced_count += 1;
                }
                Err(e) => {
                    error!(
                        session_id = %session.id,
                        error = %e,
                        "Failed to convert session for Access Log Service"
                    );
                }
            }
        }

        info!(count = synced_count, "Synced active learning sessions with Access Log Service");

        Ok(synced_count)
    }
}

/// Convert a LearningSessionData to an Access Log Service LearningSession
fn convert_to_access_log_session(session: &LearningSessionData) -> Result<LearningSession> {
    let pattern = regex::Regex::new(&session.route_pattern).map_err(|e| {
        Error::validation(format!("Invalid route pattern regex '{}': {}", session.route_pattern, e))
    })?;

    Ok(LearningSession {
        id: session.id.clone(),
        route_patterns: vec![pattern],
        methods: session.http_methods.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create a test service for should_complete tests
    // We use a minimal pool config to avoid tokio runtime issues
    fn create_test_service() -> LearningSessionService {
        // For unit tests of should_complete, we don't actually use the repository
        // So we can use a minimal pool configuration
        let pool = sqlx::Pool::connect_lazy("sqlite::memory:").expect("create test pool");
        LearningSessionService::new(LearningSessionRepository::new(pool))
    }

    #[tokio::test]
    async fn test_should_complete_target_reached() {
        let session = LearningSessionData {
            id: "test-session".to_string(),
            team: "test-team".to_string(),
            route_pattern: "^/api/.*".to_string(),
            cluster_name: None,
            http_methods: None,
            status: LearningSessionStatus::Active,
            created_at: chrono::Utc::now(),
            started_at: Some(chrono::Utc::now()),
            ends_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            completed_at: None,
            target_sample_count: 100,
            current_sample_count: 100, // Target reached
            triggered_by: None,
            deployment_version: None,
            configuration_snapshot: None,
            error_message: None,
            updated_at: chrono::Utc::now(),
        };

        let service = create_test_service();
        assert!(service.should_complete(&session));
    }

    #[tokio::test]
    async fn test_should_complete_timeout() {
        let session = LearningSessionData {
            id: "test-session".to_string(),
            team: "test-team".to_string(),
            route_pattern: "^/api/.*".to_string(),
            cluster_name: None,
            http_methods: None,
            status: LearningSessionStatus::Active,
            created_at: chrono::Utc::now(),
            started_at: Some(chrono::Utc::now()),
            ends_at: Some(chrono::Utc::now() - chrono::Duration::seconds(1)), // Timed out
            completed_at: None,
            target_sample_count: 100,
            current_sample_count: 50, // Not reached target
            triggered_by: None,
            deployment_version: None,
            configuration_snapshot: None,
            error_message: None,
            updated_at: chrono::Utc::now(),
        };

        let service = create_test_service();
        assert!(service.should_complete(&session));
    }

    #[tokio::test]
    async fn test_should_not_complete() {
        let session = LearningSessionData {
            id: "test-session".to_string(),
            team: "test-team".to_string(),
            route_pattern: "^/api/.*".to_string(),
            cluster_name: None,
            http_methods: None,
            status: LearningSessionStatus::Active,
            created_at: chrono::Utc::now(),
            started_at: Some(chrono::Utc::now()),
            ends_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            completed_at: None,
            target_sample_count: 100,
            current_sample_count: 50, // Not reached yet
            triggered_by: None,
            deployment_version: None,
            configuration_snapshot: None,
            error_message: None,
            updated_at: chrono::Utc::now(),
        };

        let service = create_test_service();
        assert!(!service.should_complete(&session));
    }

    #[test]
    fn test_convert_to_access_log_session() {
        let session = LearningSessionData {
            id: "test-session".to_string(),
            team: "test-team".to_string(),
            route_pattern: "^/api/v1/users/.*".to_string(),
            cluster_name: Some("users-api".to_string()),
            http_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
            status: LearningSessionStatus::Active,
            created_at: chrono::Utc::now(),
            started_at: Some(chrono::Utc::now()),
            ends_at: None,
            completed_at: None,
            target_sample_count: 1000,
            current_sample_count: 0,
            triggered_by: None,
            deployment_version: None,
            configuration_snapshot: None,
            error_message: None,
            updated_at: chrono::Utc::now(),
        };

        let result = convert_to_access_log_session(&session);
        assert!(result.is_ok());

        let learning_session = result.unwrap();
        assert_eq!(learning_session.id, "test-session");
        assert_eq!(learning_session.route_patterns.len(), 1);
        assert_eq!(learning_session.methods, Some(vec!["GET".to_string(), "POST".to_string()]));
    }

    #[test]
    fn test_convert_invalid_regex() {
        let session = LearningSessionData {
            id: "test-session".to_string(),
            team: "test-team".to_string(),
            route_pattern: "[invalid(regex".to_string(), // Invalid regex
            cluster_name: None,
            http_methods: None,
            status: LearningSessionStatus::Active,
            created_at: chrono::Utc::now(),
            started_at: Some(chrono::Utc::now()),
            ends_at: None,
            completed_at: None,
            target_sample_count: 1000,
            current_sample_count: 0,
            triggered_by: None,
            deployment_version: None,
            configuration_snapshot: None,
            error_message: None,
            updated_at: chrono::Utc::now(),
        };

        let result = convert_to_access_log_session(&session);
        assert!(result.is_err());
    }
}
