//! MCP Connection Registry
//!
//! Manages active SSE connections with per-team limits and message broadcasting.

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::mcp::error::McpError;
use crate::mcp::notifications::{LogLevel, NotificationMessage};

/// Maximum connections per team
const MAX_CONNECTIONS_PER_TEAM: usize = 10;

/// Channel capacity for SSE messages
const CHANNEL_CAPACITY: usize = 100;

/// Newtype for connection IDs
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectionId(String);

impl ConnectionId {
    /// Create a new connection ID
    pub fn new(id: String) -> Self {
        Self(id)
    }

    /// Get the ID as a string slice
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Individual connection state
pub struct Connection {
    /// Channel sender for this connection
    pub sender: mpsc::Sender<NotificationMessage>,
    /// Team this connection belongs to
    pub team: String,
    /// Minimum log level for this connection
    pub min_log_level: LogLevel,
}

/// Connection manager for SSE streaming
pub struct ConnectionManager {
    /// All active connections indexed by connection ID
    connections: DashMap<ConnectionId, Connection>,
    /// Connection count per team
    team_counts: DashMap<String, usize>,
    /// Counter for generating unique connection IDs
    id_counter: AtomicU64,
}

impl ConnectionManager {
    /// Create a new connection manager
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
            team_counts: DashMap::new(),
            id_counter: AtomicU64::new(0),
        }
    }

    /// Register a new connection
    ///
    /// Returns the connection ID and a receiver for messages.
    ///
    /// # Errors
    /// Returns `McpError::ConnectionLimitExceeded` if the team has reached the connection limit.
    pub fn register(
        &self,
        team: String,
    ) -> Result<(ConnectionId, mpsc::Receiver<NotificationMessage>), McpError> {
        // Check team connection limit
        let current_count = self.team_counts.get(&team).map(|r| *r).unwrap_or(0);
        if current_count >= MAX_CONNECTIONS_PER_TEAM {
            return Err(McpError::ConnectionLimitExceeded {
                team: team.clone(),
                limit: MAX_CONNECTIONS_PER_TEAM,
            });
        }

        // Generate unique connection ID
        let id_num = self.id_counter.fetch_add(1, Ordering::SeqCst);
        let connection_id = ConnectionId::new(format!("conn-{}-{}", team, id_num));

        // Create bounded channel
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);

        // Create connection
        let connection = Connection { sender, team: team.clone(), min_log_level: LogLevel::Info };

        // Insert connection
        self.connections.insert(connection_id.clone(), connection);

        // Update team count
        self.team_counts.entry(team.clone()).and_modify(|c| *c += 1).or_insert(1);

        debug!(
            connection_id = %connection_id,
            team = %team,
            "Registered new SSE connection"
        );

        Ok((connection_id, receiver))
    }

    /// Unregister a connection
    pub fn unregister(&self, connection_id: &ConnectionId) {
        if let Some((_, connection)) = self.connections.remove(connection_id) {
            // Update team count
            if let Some(mut count) = self.team_counts.get_mut(&connection.team) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    drop(count);
                    self.team_counts.remove(&connection.team);
                }
            }

            debug!(
                connection_id = %connection_id,
                team = %connection.team,
                "Unregistered SSE connection"
            );
        }
    }

    /// Set log level filter for a connection
    pub fn set_log_level(&self, connection_id: &ConnectionId, level: LogLevel) {
        if let Some(mut conn) = self.connections.get_mut(connection_id) {
            conn.min_log_level = level;
            debug!(
                connection_id = %connection_id,
                level = ?level,
                "Updated log level for connection"
            );
        }
    }

    /// Get log level for a connection
    pub fn get_log_level(&self, connection_id: &ConnectionId) -> Option<LogLevel> {
        self.connections.get(connection_id).map(|c| c.min_log_level)
    }

    /// Broadcast a message to all connections for a team
    ///
    /// Uses non-blocking sends; slow clients will miss messages.
    pub async fn broadcast_to_team(&self, team: &str, message: NotificationMessage) {
        let mut sent_count = 0;
        let mut failed_count = 0;

        for conn_ref in self.connections.iter() {
            if conn_ref.team == team {
                // For log messages, check level filter
                if let NotificationMessage::Log { ref data } = message {
                    if data.params.level < conn_ref.min_log_level {
                        continue;
                    }
                }

                match conn_ref.sender.try_send(message.clone()) {
                    Ok(_) => sent_count += 1,
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            connection_id = %conn_ref.key(),
                            "Connection channel full, dropping message"
                        );
                        failed_count += 1;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        // Connection closed, will be cleaned up
                        failed_count += 1;
                    }
                }
            }
        }

        debug!(
            team = %team,
            sent = sent_count,
            failed = failed_count,
            "Broadcast message to team"
        );
    }

    /// Broadcast a message to a specific connection
    pub async fn send_to_connection(
        &self,
        connection_id: &ConnectionId,
        message: NotificationMessage,
    ) -> Result<(), McpError> {
        let conn = self.connections.get(connection_id).ok_or_else(|| {
            McpError::InvalidParams(format!("Connection not found: {}", connection_id))
        })?;

        conn.sender
            .send(message)
            .await
            .map_err(|_| McpError::InternalError("Connection channel closed".to_string()))
    }

    /// Get connection count for a team
    pub fn team_connection_count(&self, team: &str) -> usize {
        self.team_counts.get(team).map(|r| *r).unwrap_or(0)
    }

    /// Get total connection count
    pub fn total_connections(&self) -> usize {
        self.connections.len()
    }

    /// Check if a connection exists
    pub fn exists(&self, connection_id: &ConnectionId) -> bool {
        self.connections.contains_key(connection_id)
    }

    /// Get team for a connection
    pub fn get_team(&self, connection_id: &ConnectionId) -> Option<String> {
        self.connections.get(connection_id).map(|c| c.team.clone())
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Arc wrapper for shared connection manager
pub type SharedConnectionManager = Arc<ConnectionManager>;

/// Create a new shared connection manager
pub fn create_connection_manager() -> SharedConnectionManager {
    Arc::new(ConnectionManager::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_id() {
        let id = ConnectionId::new("test-1".to_string());
        assert_eq!(id.as_str(), "test-1");
        assert_eq!(format!("{}", id), "test-1");
    }

    #[tokio::test]
    async fn test_register_connection() {
        let manager = ConnectionManager::new();

        let result = manager.register("test-team".to_string());
        assert!(result.is_ok());

        let (id, _receiver) = result.unwrap();
        assert!(manager.exists(&id));
        assert_eq!(manager.team_connection_count("test-team"), 1);
    }

    #[tokio::test]
    async fn test_unregister_connection() {
        let manager = ConnectionManager::new();

        let (id, _receiver) = manager.register("test-team".to_string()).unwrap();
        assert!(manager.exists(&id));

        manager.unregister(&id);
        assert!(!manager.exists(&id));
        assert_eq!(manager.team_connection_count("test-team"), 0);
    }

    #[tokio::test]
    async fn test_connection_limit() {
        let manager = ConnectionManager::new();
        let mut receivers = Vec::new();

        // Register up to the limit
        for _ in 0..MAX_CONNECTIONS_PER_TEAM {
            let result = manager.register("test-team".to_string());
            assert!(result.is_ok());
            let (_, receiver) = result.unwrap();
            receivers.push(receiver);
        }

        // Next registration should fail
        let result = manager.register("test-team".to_string());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::ConnectionLimitExceeded { .. }));

        // Different team should still work
        let result = manager.register("other-team".to_string());
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_set_log_level() {
        let manager = ConnectionManager::new();
        let (id, _receiver) = manager.register("test-team".to_string()).unwrap();

        // Default level is Info
        assert_eq!(manager.get_log_level(&id), Some(LogLevel::Info));

        // Set to Warning
        manager.set_log_level(&id, LogLevel::Warning);
        assert_eq!(manager.get_log_level(&id), Some(LogLevel::Warning));
    }

    #[tokio::test]
    async fn test_broadcast_to_team() {
        let manager = ConnectionManager::new();

        let (id1, mut receiver1) = manager.register("team-a".to_string()).unwrap();
        let (_id2, mut receiver2) = manager.register("team-a".to_string()).unwrap();
        let (_id3, mut receiver3) = manager.register("team-b".to_string()).unwrap();

        // Broadcast to team-a
        let message = NotificationMessage::ping();
        manager.broadcast_to_team("team-a", message).await;

        // team-a connections should receive
        assert!(receiver1.try_recv().is_ok());
        assert!(receiver2.try_recv().is_ok());

        // team-b should not receive
        assert!(receiver3.try_recv().is_err());

        // Cleanup
        manager.unregister(&id1);
    }

    #[tokio::test]
    async fn test_send_to_connection() {
        let manager = ConnectionManager::new();
        let (id, mut receiver) = manager.register("test-team".to_string()).unwrap();

        let message = NotificationMessage::ping();
        let result = manager.send_to_connection(&id, message).await;
        assert!(result.is_ok());

        let received = receiver.try_recv();
        assert!(received.is_ok());
        assert!(matches!(received.unwrap(), NotificationMessage::Ping { .. }));
    }

    #[tokio::test]
    async fn test_get_team() {
        let manager = ConnectionManager::new();
        let (id, _receiver) = manager.register("my-team".to_string()).unwrap();

        assert_eq!(manager.get_team(&id), Some("my-team".to_string()));

        let unknown = ConnectionId::new("unknown".to_string());
        assert_eq!(manager.get_team(&unknown), None);
    }

    #[test]
    fn test_total_connections() {
        let manager = ConnectionManager::new();
        assert_eq!(manager.total_connections(), 0);

        let (id1, _r1) = manager.register("team-a".to_string()).unwrap();
        assert_eq!(manager.total_connections(), 1);

        let (_id2, _r2) = manager.register("team-b".to_string()).unwrap();
        assert_eq!(manager.total_connections(), 2);

        manager.unregister(&id1);
        assert_eq!(manager.total_connections(), 1);
    }
}
