//! MCP Progress Tracking
//!
//! Provides progress notifications for long-running operations compliant with
//! MCP 2025-11-25 specification.

use crate::mcp::error::McpError;
use crate::mcp::notifications::{ProgressNotification, ProgressToken};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::SystemTime;

/// Internal progress session state
#[derive(Debug, Clone)]
struct ProgressSession {
    current_progress: f64,
    total: Option<f64>,
    #[allow(dead_code)] // Reserved for future session timeout functionality
    created_at: SystemTime,
}

/// Progress tracker for managing long-running operations
pub struct ProgressTracker {
    sessions: Arc<DashMap<ProgressToken, ProgressSession>>,
}

impl ProgressTracker {
    /// Create a new progress tracker
    pub fn new() -> Self {
        Self { sessions: Arc::new(DashMap::new()) }
    }

    /// Start a new progress session
    ///
    /// # Errors
    /// - Returns `McpError::InvalidParams` if the token already exists
    /// - Returns `McpError::InvalidParams` if total is invalid (negative or non-finite)
    pub fn start(&self, token: ProgressToken, total: Option<f64>) -> Result<(), McpError> {
        // Check for duplicate token
        if self.sessions.contains_key(&token) {
            return Err(McpError::InvalidParams(format!(
                "Progress token already exists: {}",
                token.as_str()
            )));
        }

        // Validate total if provided
        if let Some(t) = total {
            if !t.is_finite() || t < 0.0 {
                return Err(McpError::InvalidParams(format!(
                    "Total must be a finite positive number, got: {}",
                    t
                )));
            }
        }

        let session =
            ProgressSession { current_progress: 0.0, total, created_at: SystemTime::now() };

        self.sessions.insert(token, session);
        Ok(())
    }

    /// Update progress and create notification
    ///
    /// # Errors
    /// - Returns `McpError::InvalidParams` if:
    ///   - Token does not exist
    ///   - Progress is not increasing (monotonicity violation)
    ///   - Progress value is invalid (NaN, infinite, negative)
    pub fn update(
        &self,
        token: &ProgressToken,
        progress: f64,
        message: Option<String>,
    ) -> Result<ProgressNotification, McpError> {
        // Validate progress value
        if !progress.is_finite() || progress < 0.0 {
            return Err(McpError::InvalidParams(format!(
                "Progress must be a finite positive number, got: {}",
                progress
            )));
        }

        // Get mutable reference to session
        let mut session_ref = self.sessions.get_mut(token).ok_or_else(|| {
            McpError::InvalidParams(format!("Unknown progress token: {}", token.as_str()))
        })?;

        let session = session_ref.value_mut();

        // Validate monotonicity - progress must increase
        if progress <= session.current_progress {
            return Err(McpError::InvalidParams(format!(
                "Progress must increase (current: {}, new: {})",
                session.current_progress, progress
            )));
        }

        // Update session
        session.current_progress = progress;

        // Create notification
        Ok(ProgressNotification::new(token.clone(), progress, session.total, message))
    }

    /// Complete and remove a progress session
    ///
    /// # Errors
    /// - Returns `McpError::InvalidParams` if token does not exist
    pub fn complete(&self, token: &ProgressToken) -> Result<(), McpError> {
        self.sessions
            .remove(token)
            .ok_or_else(|| {
                McpError::InvalidParams(format!("Unknown progress token: {}", token.as_str()))
            })
            .map(|_| ())
    }

    /// Check if a progress token exists
    pub fn exists(&self, token: &ProgressToken) -> bool {
        self.sessions.contains_key(token)
    }

    /// Get current progress value for a token
    pub fn get_progress(&self, token: &ProgressToken) -> Option<f64> {
        self.sessions.get(token).map(|r| r.current_progress)
    }

    /// Get total for a token
    pub fn get_total(&self, token: &ProgressToken) -> Option<f64> {
        self.sessions.get(token).and_then(|r| r.total)
    }
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Arc wrapper for shared progress tracker
pub type SharedProgressTracker = Arc<ProgressTracker>;

/// Create a new shared progress tracker
pub fn create_progress_tracker() -> SharedProgressTracker {
    Arc::new(ProgressTracker::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_session() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("test-1".to_string());

        let result = tracker.start(token.clone(), Some(100.0));
        assert!(result.is_ok());
        assert!(tracker.exists(&token));
    }

    #[test]
    fn test_start_session_duplicate_token() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("test-1".to_string());

        tracker.start(token.clone(), Some(100.0)).unwrap();

        // Second start with same token should fail
        let result = tracker.start(token, None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[test]
    fn test_start_session_invalid_total() {
        let tracker = ProgressTracker::new();

        // Negative total should fail
        let result = tracker.start(ProgressToken::new("test-1".to_string()), Some(-10.0));
        assert!(result.is_err());

        // Infinite total should fail
        let result = tracker.start(ProgressToken::new("test-2".to_string()), Some(f64::INFINITY));
        assert!(result.is_err());
    }

    #[test]
    fn test_update_progress() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("test-1".to_string());

        tracker.start(token.clone(), Some(100.0)).unwrap();

        let notification = tracker.update(&token, 50.0, Some("Processing...".to_string()));
        assert!(notification.is_ok());

        let notif = notification.unwrap();
        assert_eq!(notif.method, "notifications/progress");
        assert_eq!(notif.params.progress, 50.0);
        assert_eq!(notif.params.total, Some(100.0));
        assert_eq!(notif.params.message, Some("Processing...".to_string()));
    }

    #[test]
    fn test_progress_monotonicity() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("test-1".to_string());

        tracker.start(token.clone(), Some(100.0)).unwrap();
        tracker.update(&token, 50.0, None).unwrap();

        // Progress must increase
        let result = tracker.update(&token, 40.0, None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));

        // Equal progress also fails
        let result = tracker.update(&token, 50.0, None);
        assert!(result.is_err());

        // Increasing progress succeeds
        let result = tracker.update(&token, 75.0, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_unknown_token() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("unknown".to_string());

        let result = tracker.update(&token, 50.0, None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[test]
    fn test_update_invalid_progress() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("test-1".to_string());

        tracker.start(token.clone(), None).unwrap();

        // Negative progress should fail
        let result = tracker.update(&token, -10.0, None);
        assert!(result.is_err());

        // NaN should fail
        let result = tracker.update(&token, f64::NAN, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_complete_session() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("test-1".to_string());

        tracker.start(token.clone(), Some(100.0)).unwrap();
        assert!(tracker.exists(&token));

        let result = tracker.complete(&token);
        assert!(result.is_ok());
        assert!(!tracker.exists(&token));
    }

    #[test]
    fn test_complete_unknown_token() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("unknown".to_string());

        let result = tracker.complete(&token);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[test]
    fn test_get_progress() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("test-1".to_string());

        tracker.start(token.clone(), Some(100.0)).unwrap();
        tracker.update(&token, 50.0, None).unwrap();

        let progress = tracker.get_progress(&token);
        assert_eq!(progress, Some(50.0));

        let total = tracker.get_total(&token);
        assert_eq!(total, Some(100.0));

        tracker.complete(&token).unwrap();
        let progress = tracker.get_progress(&token);
        assert_eq!(progress, None);
    }

    #[test]
    fn test_start_session_no_total() {
        let tracker = ProgressTracker::new();
        let token = ProgressToken::new("test-1".to_string());

        let result = tracker.start(token.clone(), None);
        assert!(result.is_ok());

        let total = tracker.get_total(&token);
        assert_eq!(total, None);
    }
}
