//! Learning session business logic service
//!
//! This module contains the state machine logic for learning session lifecycle management,
//! including automatic completion and state transitions.

use std::sync::{Arc, Weak};
use tracing::{error, info, instrument, warn};

use crate::{
    errors::{Error, Result},
    services::{
        webhook_service::{LearningSessionWebhookEvent, WebhookService},
        SchemaAggregator,
    },
    storage::repositories::{
        LearningSessionData, LearningSessionRepository, LearningSessionStatus,
        UpdateLearningSessionRequest,
    },
    xds::services::access_log_service::{FlowplaneAccessLogService, LearningSession},
};

/// Service for managing learning session lifecycle and state machine
#[derive(Clone)]
pub struct LearningSessionService {
    repository: LearningSessionRepository,
    access_log_service: Option<Arc<FlowplaneAccessLogService>>,
    ext_proc_service: Option<Arc<crate::xds::services::ext_proc_service::FlowplaneExtProcService>>,
    webhook_service: Option<Arc<WebhookService>>,
    xds_state: Option<Weak<crate::xds::XdsState>>,
    schema_aggregator: Option<Arc<SchemaAggregator>>,
}

// Manual Debug implementation to avoid XdsState debug requirements
impl std::fmt::Debug for LearningSessionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearningSessionService")
            .field("repository", &self.repository)
            .field("has_access_log_service", &self.access_log_service.is_some())
            .field("has_webhook_service", &self.webhook_service.is_some())
            .field("has_xds_state", &self.xds_state.is_some())
            .field("has_schema_aggregator", &self.schema_aggregator.is_some())
            .finish()
    }
}

impl LearningSessionService {
    /// Create a new learning session service
    pub fn new(repository: LearningSessionRepository) -> Self {
        Self {
            repository,
            access_log_service: None,
            ext_proc_service: None,
            webhook_service: None,
            xds_state: None,
            schema_aggregator: None,
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

    /// Set the ExtProc service for body capture session registration
    pub fn with_ext_proc_service(
        mut self,
        service: Arc<crate::xds::services::ext_proc_service::FlowplaneExtProcService>,
    ) -> Self {
        self.ext_proc_service = Some(service);
        self
    }

    /// Set the XDS state for dynamic listener configuration
    pub fn with_xds_state(mut self, state: Arc<crate::xds::XdsState>) -> Self {
        self.xds_state = Some(Arc::downgrade(&state));
        self
    }

    /// Set the schema aggregator for session completion
    pub fn with_schema_aggregator(mut self, aggregator: Arc<SchemaAggregator>) -> Self {
        self.schema_aggregator = Some(aggregator);
        self
    }

    /// Activate a learning session (transition: pending → active)
    #[instrument(skip(self), fields(session_id = %session_id), name = "activate_learning_session")]
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

        // Register with ExtProc Service for body capture
        if let Some(ext_proc_service) = &self.ext_proc_service {
            if let Err(e) = ext_proc_service
                .add_session(updated.id.clone(), updated.route_pattern.clone())
                .await
            {
                warn!(
                    session_id = %session_id,
                    error = %e,
                    "Failed to register learning session with ExtProc Service"
                );
            } else {
                info!(
                    session_id = %session_id,
                    route_pattern = %updated.route_pattern,
                    "Registered learning session with ExtProc Service"
                );
            }
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

        // Trigger LDS update to inject access log configuration
        if let Some(weak_state) = &self.xds_state {
            if let Some(xds_state) = weak_state.upgrade() {
                if let Err(e) = xds_state.refresh_listeners_from_repository().await {
                    error!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to refresh listeners after session activation"
                    );
                } else {
                    info!(
                        session_id = %session_id,
                        "Triggered LDS update for access log configuration"
                    );
                }
            } else {
                warn!(
                    session_id = %session_id,
                    "XdsState dropped, cannot refresh listeners after session activation"
                );
            }
        }

        Ok(updated)
    }

    /// Check if a session should be completed (or snapshotted) and transition if needed
    #[instrument(skip(self), fields(session_id = %session_id), name = "check_learning_session_completion")]
    ///
    /// This method checks:
    /// 1. Has the target sample count been reached?
    /// 2. Has the session timed out (ends_at exceeded)?
    ///
    /// For auto_aggregate sessions: target reached → snapshot (stay active)
    /// For normal sessions: target reached → completing → completed
    /// Timeout always completes regardless of auto_aggregate.
    pub async fn check_completion(&self, session_id: &str) -> Result<Option<LearningSessionData>> {
        let session = self.repository.get_by_id(session_id).await?;

        // Only check completion for active sessions
        if session.status != LearningSessionStatus::Active {
            return Ok(None);
        }

        let target_reached = session.current_sample_count >= session.target_sample_count;
        let timed_out = session.ends_at.is_some_and(|ends_at| chrono::Utc::now() >= ends_at);

        if target_reached && session.auto_aggregate && !timed_out {
            // Auto-aggregate mode: trigger snapshot, stay active
            self.snapshot_session(session_id).await.map(Some)
        } else if target_reached || timed_out {
            // Normal mode or timeout: complete the session
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
            self.complete_session(session_id).await.map(Some)
        } else {
            Ok(None)
        }
    }

    /// Determine if a session should be completed (used by tests; does NOT account for auto_aggregate snapshot)
    #[cfg(test)]
    fn should_complete(&self, session: &LearningSessionData) -> bool {
        let now = chrono::Utc::now();
        let target_reached = session.current_sample_count >= session.target_sample_count;
        let timed_out = session.ends_at.is_some_and(|ends_at| now >= ends_at);
        target_reached || timed_out
    }

    /// Take a snapshot of the current session state (auto-aggregate mode)
    ///
    /// Triggers aggregation as a snapshot, resets current_sample_count, increments
    /// snapshot_count, and keeps the session Active for continued collection.
    #[instrument(skip(self), fields(session_id = %session_id), name = "snapshot_learning_session")]
    pub async fn snapshot_session(&self, session_id: &str) -> Result<LearningSessionData> {
        let session = self.repository.get_by_id(session_id).await?;

        if session.status != LearningSessionStatus::Active {
            return Err(Error::validation(format!(
                "Cannot snapshot session in '{}' state. Must be 'active'",
                session.status
            )));
        }

        if !session.auto_aggregate {
            return Err(Error::validation(
                "Cannot snapshot a session that is not in auto-aggregate mode".to_string(),
            ));
        }

        // Calculate the next snapshot number
        let next_snapshot = session.snapshot_count + 1;

        info!(
            session_id = %session_id,
            snapshot_number = next_snapshot,
            current_samples = session.current_sample_count,
            "Taking snapshot of learning session"
        );

        // Trigger schema aggregation with snapshot metadata
        if let Some(schema_aggregator) = &self.schema_aggregator {
            match schema_aggregator
                .aggregate_session_with_snapshot(
                    session_id,
                    Some(session_id.to_string()),
                    Some(next_snapshot),
                )
                .await
            {
                Ok(aggregated_ids) => {
                    info!(
                        session_id = %session_id,
                        snapshot_number = next_snapshot,
                        aggregated_count = aggregated_ids.len(),
                        "Successfully aggregated snapshot schemas"
                    );
                }
                Err(e) => {
                    error!(
                        session_id = %session_id,
                        snapshot_number = next_snapshot,
                        error = %e,
                        "Failed to aggregate snapshot schemas — continuing with snapshot"
                    );
                }
            }
        } else {
            warn!(
                session_id = %session_id,
                "Schema aggregator not configured — skipping snapshot aggregation"
            );
        }

        // Reset sample count and increment snapshot count atomically
        let new_snapshot_count =
            self.repository.reset_sample_count_and_increment_snapshot(session_id).await?;

        info!(
            session_id = %session_id,
            snapshot_count = new_snapshot_count,
            "Snapshot complete — session continues collecting"
        );

        // Publish webhook event
        if let Some(webhook_service) = &self.webhook_service {
            let event = LearningSessionWebhookEvent::snapshot_completed(
                session.id.clone(),
                session.team.clone(),
                session.route_pattern.clone(),
                session.target_sample_count,
                session.current_sample_count,
                new_snapshot_count,
            );
            webhook_service.publish_event(event).await;
        }

        self.repository.get_by_id(session_id).await
    }

    /// Stop an auto-aggregate session (trigger final aggregation + complete)
    ///
    /// This is called by cp_stop_learning to explicitly end a session that
    /// would otherwise continue collecting indefinitely.
    #[instrument(skip(self), fields(session_id = %session_id), name = "stop_learning_session")]
    pub async fn stop_session(&self, session_id: &str) -> Result<LearningSessionData> {
        let session = self.repository.get_by_id(session_id).await?;

        if session.status != LearningSessionStatus::Active {
            return Err(Error::validation(format!(
                "Cannot stop session in '{}' state. Must be 'active'",
                session.status
            )));
        }

        info!(
            session_id = %session_id,
            auto_aggregate = session.auto_aggregate,
            snapshot_count = session.snapshot_count,
            current_samples = session.current_sample_count,
            "Stopping learning session — triggering final aggregation"
        );

        // Complete the session (which triggers final aggregation)
        self.complete_session(session_id).await
    }

    /// Complete a learning session (active → completing → completed)
    ///
    /// Uses atomic state transition to prevent race conditions when multiple
    /// callers try to complete the same session concurrently. Only the first
    /// caller to successfully transition from Active → Completing will proceed;
    /// subsequent callers will return Ok(None) indicating the session was
    /// already being completed.
    #[instrument(skip(self), fields(session_id = %session_id), name = "complete_learning_session")]
    async fn complete_session(&self, session_id: &str) -> Result<LearningSessionData> {
        // Atomically transition to 'completing' state using conditional UPDATE
        // This prevents race conditions - only one caller can win this transition
        let transitioned = self
            .repository
            .transition_status(
                session_id,
                LearningSessionStatus::Active,
                LearningSessionStatus::Completing,
            )
            .await?;

        if !transitioned {
            // Another caller already started completing this session
            warn!(
                session_id = %session_id,
                "Session completion race detected - another process is already completing this session"
            );
            // Return the current session state
            return self.repository.get_by_id(session_id).await;
        }

        info!(session_id = %session_id, "Session transitioning: active → completing (won race)");

        // Unregister from Access Log Service
        if let Some(access_log_service) = &self.access_log_service {
            access_log_service.remove_session(session_id).await;
            info!(
                session_id = %session_id,
                "Unregistered learning session from Access Log Service"
            );
        }

        // Unregister from ExtProc Service
        if let Some(ext_proc_service) = &self.ext_proc_service {
            ext_proc_service.remove_session(session_id).await;
            info!(
                session_id = %session_id,
                "Unregistered learning session from ExtProc Service"
            );
        }

        // Trigger LDS update to remove access log configuration
        if let Some(weak_state) = &self.xds_state {
            if let Some(xds_state) = weak_state.upgrade() {
                if let Err(e) = xds_state.refresh_listeners_from_repository().await {
                    error!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to refresh listeners after session completion"
                    );
                } else {
                    info!(
                        session_id = %session_id,
                        "Triggered LDS update to remove access log configuration"
                    );
                }
            }
        }

        // Task 6.6: Trigger schema aggregation
        if let Some(schema_aggregator) = &self.schema_aggregator {
            info!(session_id = %session_id, "Starting schema aggregation for completed session");

            match schema_aggregator.aggregate_session(session_id).await {
                Ok(aggregated_ids) => {
                    info!(
                        session_id = %session_id,
                        aggregated_count = aggregated_ids.len(),
                        aggregated_ids = ?aggregated_ids,
                        "Successfully aggregated schemas for session"
                    );
                }
                Err(e) => {
                    error!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to aggregate schemas for session - continuing with session completion"
                    );
                    // Continue with session completion even if aggregation fails
                    // The session data is still valid and stored in inferred_schemas table
                }
            }
        } else {
            warn!(
                session_id = %session_id,
                "Schema aggregator not configured - skipping aggregation"
            );
        }

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
    #[instrument(skip(self), fields(session_id = %session_id), name = "fail_learning_session")]
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

        // Unregister from ExtProc Service
        if let Some(ext_proc_service) = &self.ext_proc_service {
            ext_proc_service.remove_session(session_id).await;
        }

        // Trigger LDS update to remove access log configuration
        if let Some(weak_state) = &self.xds_state {
            if let Some(xds_state) = weak_state.upgrade() {
                if let Err(e) = xds_state.refresh_listeners_from_repository().await {
                    error!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to refresh listeners after session failure"
                    );
                } else {
                    info!(
                        session_id = %session_id,
                        "Triggered LDS update to remove access log configuration after failure"
                    );
                }
            }
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

    /// Cancel a learning session (user-initiated cancellation)
    ///
    /// Similar to fail_session but sets status to Cancelled instead of Failed.
    pub async fn cancel_session(&self, session_id: &str) -> Result<LearningSessionData> {
        let update_request = UpdateLearningSessionRequest {
            status: Some(LearningSessionStatus::Cancelled),
            started_at: None,
            ends_at: None,
            completed_at: Some(chrono::Utc::now()),
            current_sample_count: None,
            error_message: Some("Cancelled by user".to_string()),
        };

        let cancelled = self.repository.update(session_id, update_request).await?;

        // Unregister from Access Log Service
        if let Some(access_log_service) = &self.access_log_service {
            access_log_service.remove_session(session_id).await;
        }

        // Unregister from ExtProc Service
        if let Some(ext_proc_service) = &self.ext_proc_service {
            ext_proc_service.remove_session(session_id).await;
        }

        // Trigger LDS update to remove access log configuration
        if let Some(weak_state) = &self.xds_state {
            if let Some(xds_state) = weak_state.upgrade() {
                if let Err(e) = xds_state.refresh_listeners_from_repository().await {
                    error!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to refresh listeners after session cancellation"
                    );
                } else {
                    info!(
                        session_id = %session_id,
                        "Triggered LDS update to remove access log configuration after cancellation"
                    );
                }
            }
        }

        info!(
            session_id = %session_id,
            "Session cancelled by user"
        );

        // Publish webhook event if webhook service is available
        if let Some(webhook_service) = &self.webhook_service {
            let event = LearningSessionWebhookEvent::failed(
                cancelled.id.clone(),
                cancelled.team.clone(),
                cancelled.route_pattern.clone(),
                "Cancelled by user".to_string(),
                cancelled.current_sample_count,
                cancelled.target_sample_count,
            );
            webhook_service.publish_event(event).await;
        }

        Ok(cancelled)
    }

    /// Get all active learning sessions
    ///
    /// This is used by XdsState to inject access log configuration
    pub async fn list_active_sessions(&self) -> Result<Vec<LearningSessionData>> {
        self.repository.list_active().await
    }

    /// Background worker that checks all active sessions for completion
    ///
    /// This should be called periodically (e.g., every 30 seconds)
    #[instrument(skip(self), name = "bg_check_active_sessions")]
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
    #[instrument(skip(self), name = "bg_sync_sessions_with_als")]
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

/// Generate a human-friendly session name from a route pattern.
///
/// Strips regex metacharacters, replaces `/` with `-`, collapses dashes,
/// and truncates to 48 chars. Returns e.g. `"v2-api"` from `"^/v2/api/.*"`.
pub fn generate_session_name(route_pattern: &str) -> String {
    let name: String = route_pattern
        .chars()
        .map(|c| match c {
            '^' | '$' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' => {
                ' '
            }
            '/' => '-',
            _ => c,
        })
        .collect();

    // Collapse whitespace and dashes, trim
    let mut result = String::with_capacity(name.len());
    let mut last_was_dash = true; // true to trim leading dash
    for c in name.chars() {
        if c == '-' || c == ' ' {
            if !last_was_dash {
                result.push('-');
                last_was_dash = true;
            }
        } else {
            result.push(c);
            last_was_dash = false;
        }
    }

    // Trim trailing dash
    let result = result.trim_end_matches('-');

    // Truncate to 48 chars
    if result.len() > 48 {
        result[..48].trim_end_matches('-').to_string()
    } else {
        result.to_string()
    }
}

/// Generate a unique session name, appending `-2`, `-3`, etc. on conflict.
///
/// Tries the base name first, then appends a numeric suffix until no UNIQUE
/// violation occurs (up to 100 attempts).
pub async fn generate_unique_session_name(
    repo: &crate::storage::repositories::LearningSessionRepository,
    team: &str,
    route_pattern: &str,
) -> Result<String> {
    let base = generate_session_name(route_pattern);
    if base.is_empty() {
        return Ok(format!("session-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    }

    // Try the base name first
    if repo.get_by_name(team, &base).await.is_err() {
        return Ok(base);
    }

    // Append suffix until unique
    for i in 2..=100 {
        let candidate = format!("{}-{}", base, i);
        if repo.get_by_name(team, &candidate).await.is_err() {
            return Ok(candidate);
        }
    }

    // Fallback to uuid suffix
    Ok(format!("{}-{}", base, &uuid::Uuid::new_v4().to_string()[..8]))
}

/// Convert a LearningSessionData to an Access Log Service LearningSession
fn convert_to_access_log_session(session: &LearningSessionData) -> Result<LearningSession> {
    let pattern = regex::Regex::new(&session.route_pattern).map_err(|e| {
        Error::validation(format!("Invalid route pattern regex '{}': {}", session.route_pattern, e))
    })?;

    Ok(LearningSession {
        id: session.id.clone(),
        team: session.team.clone(),
        route_patterns: vec![pattern],
        methods: session.http_methods.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    async fn create_test_service() -> (TestDatabase, LearningSessionService) {
        let test_db = TestDatabase::new("learning_session_service").await;
        let pool = test_db.pool.clone();
        let service = LearningSessionService::new(LearningSessionRepository::new(pool));
        (test_db, service)
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
            auto_aggregate: false,
            snapshot_count: 0,
            name: None,
        };

        let (_db, service) = create_test_service().await;
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
            auto_aggregate: false,
            snapshot_count: 0,
            name: None,
        };

        let (_db, service) = create_test_service().await;
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
            auto_aggregate: false,
            snapshot_count: 0,
            name: None,
        };

        let (_db, service) = create_test_service().await;
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
            auto_aggregate: false,
            snapshot_count: 0,
            name: None,
        };

        let result = convert_to_access_log_session(&session);
        assert!(result.is_ok());

        let learning_session = result.unwrap();
        assert_eq!(learning_session.id, "test-session");
        assert_eq!(learning_session.route_patterns.len(), 1);
        assert_eq!(learning_session.methods, Some(vec!["GET".to_string(), "POST".to_string()]));
    }

    #[tokio::test]
    async fn test_should_complete_auto_aggregate_target_reached() {
        // should_complete returns true even for auto_aggregate sessions
        // (the snapshot vs complete branching is in check_completion, not should_complete)
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
            auto_aggregate: true, // Auto-aggregate enabled
            snapshot_count: 2,    // Already had 2 snapshots
            name: None,
        };

        let (_db, service) = create_test_service().await;
        // should_complete is a basic check — always true when target reached
        assert!(service.should_complete(&session));
    }

    #[tokio::test]
    async fn test_should_not_complete_auto_aggregate_below_target() {
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
            target_sample_count: 500,
            current_sample_count: 250, // Below target
            triggered_by: None,
            deployment_version: None,
            configuration_snapshot: None,
            error_message: None,
            updated_at: chrono::Utc::now(),
            auto_aggregate: true,
            snapshot_count: 0,
            name: None,
        };

        let (_db, service) = create_test_service().await;
        assert!(!service.should_complete(&session));
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
            auto_aggregate: false,
            snapshot_count: 0,
            name: None,
        };

        let result = convert_to_access_log_session(&session);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_session_name_basic() {
        assert_eq!(generate_session_name("^/v2/api/.*"), "v2-api");
    }

    #[test]
    fn test_generate_session_name_users_endpoint() {
        assert_eq!(generate_session_name("^/api/v1/users/.*"), "api-v1-users");
    }

    #[test]
    fn test_generate_session_name_complex_regex() {
        assert_eq!(generate_session_name("^/api/v[0-9]+/orders/[0-9]+$"), "api-v-0-9-orders-0-9");
    }

    #[test]
    fn test_generate_session_name_simple_path() {
        assert_eq!(generate_session_name("/api/health"), "api-health");
    }

    #[test]
    fn test_generate_session_name_empty() {
        assert_eq!(generate_session_name(""), "");
    }

    #[test]
    fn test_generate_session_name_only_regex_chars() {
        assert_eq!(generate_session_name("^.*$"), "");
    }

    #[test]
    fn test_generate_session_name_truncation() {
        let long_pattern = "^/api/v1/this/is/a/very/long/path/that/exceeds/the/limit/of/forty/eight/characters/definitely/.*";
        let result = generate_session_name(long_pattern);
        assert!(result.len() <= 48);
        assert!(!result.ends_with('-'));
    }

    #[test]
    fn test_generate_session_name_no_leading_trailing_dashes() {
        assert_eq!(generate_session_name("/api/users/"), "api-users");
        assert_eq!(generate_session_name("^/api/"), "api");
    }

    #[test]
    fn test_generate_session_name_collapse_dashes() {
        // Multiple slashes should collapse to single dash
        assert_eq!(generate_session_name("/api///users"), "api-users");
    }
}
