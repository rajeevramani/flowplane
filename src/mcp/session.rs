//! MCP Session State Management
//!
//! Tracks per-session MCP state across HTTP requests for strict protocol compliance.
//! Sessions are keyed by authentication token and expire after a configurable TTL.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::debug;

use crate::mcp::notifications::LogLevel;
use crate::mcp::protocol::ClientInfo;

/// Default session TTL (1 hour)
const DEFAULT_SESSION_TTL_SECS: u64 = 3600;

/// Session identifier derived from authentication token
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Create a session ID from a token ID
    pub fn from_token(token_id: &str) -> Self {
        Self(format!("token:{}", token_id))
    }

    /// Create a session ID from an explicit session header
    pub fn from_header(session_id: &str) -> Self {
        Self(format!("session:{}", session_id))
    }

    /// Get the ID as a string slice
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Per-session MCP state
#[derive(Debug, Clone)]
pub struct McpSession {
    /// Whether the session has been initialized
    pub initialized: bool,
    /// Negotiated protocol version
    pub protocol_version: Option<String>,
    /// Client information from initialize request
    pub client_info: Option<ClientInfo>,
    /// Minimum log level for notifications
    pub log_level: LogLevel,
    /// Team this session belongs to
    pub team: Option<String>,
    /// When the session was created
    pub created_at: Instant,
    /// When the session was last accessed
    pub last_activity: Instant,
}

impl Default for McpSession {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            initialized: false,
            protocol_version: None,
            client_info: None,
            log_level: LogLevel::Info,
            team: None,
            created_at: now,
            last_activity: now,
        }
    }
}

impl McpSession {
    /// Create a new session for a specific team
    pub fn for_team(team: String) -> Self {
        let now = Instant::now();
        Self {
            initialized: false,
            protocol_version: None,
            client_info: None,
            log_level: LogLevel::Info,
            team: Some(team),
            created_at: now,
            last_activity: now,
        }
    }

    /// Update the last activity timestamp
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Check if the session has expired
    pub fn is_expired(&self, ttl: Duration) -> bool {
        self.last_activity.elapsed() > ttl
    }
}

/// Session manager with TTL-based expiration
pub struct SessionManager {
    /// All active sessions indexed by session ID
    sessions: DashMap<SessionId, McpSession>,
    /// Session time-to-live
    ttl: Duration,
}

impl SessionManager {
    /// Create a new session manager with the specified TTL
    pub fn new(ttl: Duration) -> Self {
        Self { sessions: DashMap::new(), ttl }
    }

    /// Get an existing session or create a new one
    ///
    /// This method automatically updates the last activity timestamp.
    pub fn get_or_create(&self, id: &SessionId) -> McpSession {
        let mut session = self
            .sessions
            .entry(id.clone())
            .or_insert_with(|| {
                debug!(session_id = %id, "Created new MCP session");
                McpSession::default()
            })
            .clone();

        // Touch the session to update last activity
        session.touch();
        if let Some(mut entry) = self.sessions.get_mut(id) {
            entry.touch();
        }

        session
    }

    /// Get an existing session or create a new one for a specific team
    ///
    /// This method automatically updates the last activity timestamp.
    pub fn get_or_create_for_team(&self, id: &SessionId, team: &str) -> McpSession {
        let team_string = team.to_string();
        let mut session = self
            .sessions
            .entry(id.clone())
            .or_insert_with(|| {
                debug!(session_id = %id, team = %team, "Created new MCP session for team");
                McpSession::for_team(team_string.clone())
            })
            .clone();

        // Touch the session to update last activity
        session.touch();
        if let Some(mut entry) = self.sessions.get_mut(id) {
            entry.touch();
            // Update team if not set
            if entry.team.is_none() {
                entry.team = Some(team_string);
            }
        }

        session
    }

    /// Mark a session as initialized
    pub fn mark_initialized(
        &self,
        id: &SessionId,
        protocol_version: String,
        client_info: ClientInfo,
    ) {
        self.mark_initialized_with_team(id, protocol_version, client_info, None);
    }

    /// Mark a session as initialized with team info
    pub fn mark_initialized_with_team(
        &self,
        id: &SessionId,
        protocol_version: String,
        client_info: ClientInfo,
        team: Option<String>,
    ) {
        if let Some(mut session) = self.sessions.get_mut(id) {
            session.initialized = true;
            session.protocol_version = Some(protocol_version.clone());
            session.client_info = Some(client_info.clone());
            if let Some(t) = &team {
                session.team = Some(t.clone());
            }
            session.touch();

            debug!(
                session_id = %id,
                protocol_version = %protocol_version,
                client_name = %client_info.name,
                team = ?team,
                "Marked session as initialized"
            );
        } else {
            // Create a new session if it doesn't exist
            let now = Instant::now();
            let session = McpSession {
                initialized: true,
                protocol_version: Some(protocol_version.clone()),
                client_info: Some(client_info.clone()),
                log_level: LogLevel::Info,
                team,
                created_at: now,
                last_activity: now,
            };
            self.sessions.insert(id.clone(), session);

            debug!(
                session_id = %id,
                protocol_version = %protocol_version,
                "Created and initialized new session"
            );
        }
    }

    /// List all sessions for a specific team
    pub fn list_sessions_by_team(&self, team: &str) -> Vec<(String, McpSession)> {
        self.sessions
            .iter()
            .filter(|entry| entry.team.as_deref() == Some(team))
            .map(|entry| (entry.key().as_str().to_string(), entry.value().clone()))
            .collect()
    }

    /// Check if a session is initialized
    pub fn is_initialized(&self, id: &SessionId) -> bool {
        self.sessions.get(id).map(|s| s.initialized).unwrap_or(false)
    }

    /// Set the log level for a session
    pub fn set_log_level(&self, id: &SessionId, level: LogLevel) {
        if let Some(mut session) = self.sessions.get_mut(id) {
            session.log_level = level;
            session.touch();
            debug!(session_id = %id, level = ?level, "Updated session log level");
        }
    }

    /// Get the log level for a session
    pub fn get_log_level(&self, id: &SessionId) -> Option<LogLevel> {
        self.sessions.get(id).map(|s| s.log_level)
    }

    /// Get the protocol version for a session
    pub fn get_protocol_version(&self, id: &SessionId) -> Option<String> {
        self.sessions.get(id).and_then(|s| s.protocol_version.clone())
    }

    /// Get the client info for a session
    pub fn get_client_info(&self, id: &SessionId) -> Option<ClientInfo> {
        self.sessions.get(id).and_then(|s| s.client_info.clone())
    }

    /// Remove expired sessions
    ///
    /// Returns the number of sessions removed.
    pub fn cleanup_expired(&self) -> usize {
        let before = self.sessions.len();

        self.sessions.retain(|id, session| {
            let keep = !session.is_expired(self.ttl);
            if !keep {
                debug!(session_id = %id, "Removed expired session");
            }
            keep
        });

        let removed = before - self.sessions.len();
        if removed > 0 {
            debug!(
                removed = removed,
                remaining = self.sessions.len(),
                "Cleaned up expired sessions"
            );
        }
        removed
    }

    /// Get the total number of active sessions
    pub fn total_sessions(&self) -> usize {
        self.sessions.len()
    }

    /// Check if a session exists
    pub fn exists(&self, id: &SessionId) -> bool {
        self.sessions.contains_key(id)
    }

    /// Remove a session
    ///
    /// Returns true if the session was removed.
    pub fn remove(&self, id: &SessionId) -> bool {
        let removed = self.sessions.remove(id).is_some();
        if removed {
            debug!(session_id = %id, "Removed session");
        }
        removed
    }

    /// Get the TTL for this session manager
    pub fn ttl(&self) -> Duration {
        self.ttl
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(Duration::from_secs(DEFAULT_SESSION_TTL_SECS))
    }
}

/// Arc wrapper for shared session manager
pub type SharedSessionManager = Arc<SessionManager>;

/// Create a new shared session manager with default TTL
pub fn create_session_manager() -> SharedSessionManager {
    Arc::new(SessionManager::default())
}

/// Create a new shared session manager with custom TTL
pub fn create_session_manager_with_ttl(ttl: Duration) -> SharedSessionManager {
    Arc::new(SessionManager::new(ttl))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_id_from_token() {
        let id = SessionId::from_token("abc123");
        assert_eq!(id.as_str(), "token:abc123");
        assert_eq!(format!("{}", id), "token:abc123");
    }

    #[test]
    fn test_session_id_from_header() {
        let id = SessionId::from_header("session-xyz");
        assert_eq!(id.as_str(), "session:session-xyz");
    }

    #[test]
    fn test_session_id_equality() {
        let id1 = SessionId::from_token("abc");
        let id2 = SessionId::from_token("abc");
        let id3 = SessionId::from_token("xyz");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_mcp_session_default() {
        let session = McpSession::default();

        assert!(!session.initialized);
        assert!(session.protocol_version.is_none());
        assert!(session.client_info.is_none());
        assert_eq!(session.log_level, LogLevel::Info);
    }

    #[test]
    fn test_mcp_session_touch() {
        let mut session = McpSession::default();
        let initial_activity = session.last_activity;

        std::thread::sleep(std::time::Duration::from_millis(10));
        session.touch();

        assert!(session.last_activity > initial_activity);
    }

    #[test]
    fn test_mcp_session_is_expired() {
        let session = McpSession::default();

        // Should not be expired with long TTL
        assert!(!session.is_expired(Duration::from_secs(3600)));

        // Should be expired with zero TTL
        assert!(session.is_expired(Duration::from_secs(0)));
    }

    #[test]
    fn test_session_manager_get_or_create() {
        let manager = SessionManager::default();
        let id = SessionId::from_token("test-token");

        // First call creates a new session
        let session1 = manager.get_or_create(&id);
        assert!(!session1.initialized);
        assert_eq!(manager.total_sessions(), 1);

        // Second call returns existing session
        let session2 = manager.get_or_create(&id);
        assert!(!session2.initialized);
        assert_eq!(manager.total_sessions(), 1);
    }

    #[test]
    fn test_session_manager_mark_initialized() {
        let manager = SessionManager::default();
        let id = SessionId::from_token("test-token");

        // Create session
        let _ = manager.get_or_create(&id);
        assert!(!manager.is_initialized(&id));

        // Mark as initialized
        let client_info =
            ClientInfo { name: "test-client".to_string(), version: "1.0.0".to_string() };
        manager.mark_initialized(&id, "2025-11-25".to_string(), client_info.clone());

        assert!(manager.is_initialized(&id));
        assert_eq!(manager.get_protocol_version(&id), Some("2025-11-25".to_string()));
        assert_eq!(manager.get_client_info(&id).map(|c| c.name), Some("test-client".to_string()));
    }

    #[test]
    fn test_session_manager_mark_initialized_creates_session() {
        let manager = SessionManager::default();
        let id = SessionId::from_token("new-token");

        // Mark as initialized without creating first
        let client_info =
            ClientInfo { name: "test-client".to_string(), version: "1.0.0".to_string() };
        manager.mark_initialized(&id, "2025-11-25".to_string(), client_info);

        assert!(manager.is_initialized(&id));
        assert!(manager.exists(&id));
    }

    #[test]
    fn test_session_manager_is_initialized_nonexistent() {
        let manager = SessionManager::default();
        let id = SessionId::from_token("nonexistent");

        assert!(!manager.is_initialized(&id));
    }

    #[test]
    fn test_session_manager_set_log_level() {
        let manager = SessionManager::default();
        let id = SessionId::from_token("test-token");

        // Create session
        let _ = manager.get_or_create(&id);

        // Default is Info
        assert_eq!(manager.get_log_level(&id), Some(LogLevel::Info));

        // Set to Warning
        manager.set_log_level(&id, LogLevel::Warning);
        assert_eq!(manager.get_log_level(&id), Some(LogLevel::Warning));

        // Set to Debug
        manager.set_log_level(&id, LogLevel::Debug);
        assert_eq!(manager.get_log_level(&id), Some(LogLevel::Debug));
    }

    #[test]
    fn test_session_manager_cleanup_expired() {
        let manager = SessionManager::new(Duration::from_millis(10));
        let id1 = SessionId::from_token("token-1");
        let id2 = SessionId::from_token("token-2");

        // Create sessions
        let _ = manager.get_or_create(&id1);
        let _ = manager.get_or_create(&id2);
        assert_eq!(manager.total_sessions(), 2);

        // Wait for expiration
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Cleanup
        let removed = manager.cleanup_expired();
        assert_eq!(removed, 2);
        assert_eq!(manager.total_sessions(), 0);
    }

    #[test]
    fn test_session_manager_cleanup_partial() {
        let manager = SessionManager::new(Duration::from_millis(50));
        let id1 = SessionId::from_token("token-1");
        let id2 = SessionId::from_token("token-2");

        // Create first session
        let _ = manager.get_or_create(&id1);

        // Wait a bit
        std::thread::sleep(std::time::Duration::from_millis(30));

        // Create second session (newer)
        let _ = manager.get_or_create(&id2);

        // Wait for first to expire but not second
        std::thread::sleep(std::time::Duration::from_millis(30));

        // Cleanup
        let removed = manager.cleanup_expired();
        assert_eq!(removed, 1);
        assert!(!manager.exists(&id1));
        assert!(manager.exists(&id2));
    }

    #[test]
    fn test_session_manager_remove() {
        let manager = SessionManager::default();
        let id = SessionId::from_token("test-token");

        // Create session
        let _ = manager.get_or_create(&id);
        assert!(manager.exists(&id));

        // Remove
        let removed = manager.remove(&id);
        assert!(removed);
        assert!(!manager.exists(&id));

        // Remove again (should return false)
        let removed_again = manager.remove(&id);
        assert!(!removed_again);
    }

    #[test]
    fn test_session_manager_ttl() {
        let manager = SessionManager::new(Duration::from_secs(300));
        assert_eq!(manager.ttl(), Duration::from_secs(300));

        let default_manager = SessionManager::default();
        assert_eq!(default_manager.ttl(), Duration::from_secs(DEFAULT_SESSION_TTL_SECS));
    }

    #[test]
    fn test_create_session_manager() {
        let manager = create_session_manager();
        assert_eq!(manager.ttl(), Duration::from_secs(DEFAULT_SESSION_TTL_SECS));
    }

    #[test]
    fn test_create_session_manager_with_ttl() {
        let manager = create_session_manager_with_ttl(Duration::from_secs(600));
        assert_eq!(manager.ttl(), Duration::from_secs(600));
    }

    #[test]
    fn test_concurrent_session_access() {
        use std::sync::Arc;
        use std::thread;

        let manager = Arc::new(SessionManager::default());
        let id = SessionId::from_token("concurrent-token");

        // Create session first
        let _ = manager.get_or_create(&id);

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let manager = Arc::clone(&manager);
                let id = id.clone();
                thread::spawn(move || {
                    for _ in 0..100 {
                        let _ = manager.get_or_create(&id);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // Should still have exactly one session
        assert_eq!(manager.total_sessions(), 1);
        assert!(manager.exists(&id));
    }
}
