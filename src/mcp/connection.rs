//! MCP Connection Registry
//!
//! Manages active SSE connections with per-team limits and message broadcasting.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, warn};

use crate::mcp::error::McpError;
use crate::mcp::message_buffer::MessageBuffer;
use crate::mcp::notifications::{LogLevel, NotificationMessage};
use crate::mcp::protocol::{ClientInfo, ConnectionInfo, ConnectionType};

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
    /// Timestamp when connection was established
    pub created_at: DateTime<Utc>,
    /// Last activity timestamp (updated on message send)
    pub last_activity: Arc<RwLock<DateTime<Utc>>>,
    /// Client information (name, version) captured during initialize
    pub client_info: Arc<RwLock<Option<ClientInfo>>>,
    /// Negotiated protocol version
    pub protocol_version: Arc<RwLock<Option<String>>>,
    /// Whether the connection has completed initialization
    pub initialized: Arc<RwLock<bool>>,
    /// Message buffer for SSE resumability (MCP 2025-11-25)
    pub message_buffer: Arc<MessageBuffer>,
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

        // Create connection with timestamp tracking
        let now = Utc::now();
        let connection = Connection {
            sender,
            team: team.clone(),
            min_log_level: LogLevel::Info,
            created_at: now,
            last_activity: Arc::new(RwLock::new(now)),
            client_info: Arc::new(RwLock::new(None)),
            protocol_version: Arc::new(RwLock::new(None)),
            initialized: Arc::new(RwLock::new(false)),
            message_buffer: Arc::new(MessageBuffer::new()),
        };

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
    /// Messages are buffered for SSE resumability (MCP 2025-11-25).
    pub async fn broadcast_to_team(&self, team: &str, message: NotificationMessage) {
        let mut sent_count = 0;
        let mut failed_count = 0;
        let mut buffered_count = 0;

        for conn_ref in self.connections.iter() {
            if conn_ref.team == team {
                // For log messages, check level filter
                if let NotificationMessage::Log { ref data } = message {
                    if data.params.level < conn_ref.min_log_level {
                        continue;
                    }
                }

                // Buffer the message for resumability (regardless of send success)
                conn_ref.message_buffer.push(message.clone()).await;
                buffered_count += 1;

                match conn_ref.sender.try_send(message.clone()) {
                    Ok(_) => sent_count += 1,
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            connection_id = %conn_ref.key(),
                            "Connection channel full, message buffered only"
                        );
                        failed_count += 1;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        // Connection closed, message still buffered for potential reconnect
                        failed_count += 1;
                    }
                }
            }
        }

        debug!(
            team = %team,
            sent = sent_count,
            failed = failed_count,
            buffered = buffered_count,
            "Broadcast message to team"
        );
    }

    /// Send a message to a specific connection
    ///
    /// Messages are buffered for SSE resumability (MCP 2025-11-25).
    pub async fn send_to_connection(
        &self,
        connection_id: &ConnectionId,
        message: NotificationMessage,
    ) -> Result<(), McpError> {
        let conn = self.connections.get(connection_id).ok_or_else(|| {
            McpError::InvalidParams(format!("Connection not found: {}", connection_id))
        })?;

        // Buffer the message for resumability
        conn.message_buffer.push(message.clone()).await;

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

    /// Get message buffer for a connection (for resumability)
    ///
    /// Used to retrieve the message buffer for replaying messages
    /// when a client reconnects with `Last-Event-ID` header.
    pub fn get_message_buffer(&self, connection_id: &ConnectionId) -> Option<Arc<MessageBuffer>> {
        self.connections.get(connection_id).map(|c| c.message_buffer.clone())
    }

    /// List all connections for a specific team
    ///
    /// Returns connection information including timestamps, log levels, and client metadata.
    pub fn list_team_connections(&self, team: &str) -> Vec<ConnectionInfo> {
        self.connections
            .iter()
            .filter(|entry| entry.team == team)
            .map(|entry| {
                let last_activity = entry
                    .last_activity
                    .try_read()
                    .map(|guard| guard.to_rfc3339())
                    .unwrap_or_else(|_| entry.created_at.to_rfc3339());

                let client_info = entry.client_info.try_read().ok().and_then(|guard| guard.clone());

                let protocol_version =
                    entry.protocol_version.try_read().ok().and_then(|guard| guard.clone());

                let initialized = entry.initialized.try_read().map(|guard| *guard).unwrap_or(false);

                ConnectionInfo {
                    connection_id: entry.key().as_str().to_string(),
                    team: entry.team.clone(),
                    created_at: entry.created_at.to_rfc3339(),
                    last_activity,
                    log_level: format!("{:?}", entry.min_log_level).to_lowercase(),
                    client_info,
                    protocol_version,
                    initialized,
                    connection_type: ConnectionType::Sse,
                }
            })
            .collect()
    }

    /// List all connections (admin only)
    ///
    /// Returns all connections across all teams including client metadata.
    pub fn list_all_connections(&self) -> Vec<ConnectionInfo> {
        self.connections
            .iter()
            .map(|entry| {
                let last_activity = entry
                    .last_activity
                    .try_read()
                    .map(|guard| guard.to_rfc3339())
                    .unwrap_or_else(|_| entry.created_at.to_rfc3339());

                let client_info = entry.client_info.try_read().ok().and_then(|guard| guard.clone());

                let protocol_version =
                    entry.protocol_version.try_read().ok().and_then(|guard| guard.clone());

                let initialized = entry.initialized.try_read().map(|guard| *guard).unwrap_or(false);

                ConnectionInfo {
                    connection_id: entry.key().as_str().to_string(),
                    team: entry.team.clone(),
                    created_at: entry.created_at.to_rfc3339(),
                    last_activity,
                    log_level: format!("{:?}", entry.min_log_level).to_lowercase(),
                    client_info,
                    protocol_version,
                    initialized,
                    connection_type: ConnectionType::Sse,
                }
            })
            .collect()
    }

    /// Update the last activity timestamp for a connection
    ///
    /// Call this when a message is sent to update activity tracking.
    pub async fn update_last_activity(&self, connection_id: &ConnectionId) {
        if let Some(conn) = self.connections.get(connection_id) {
            let mut guard = conn.last_activity.write().await;
            *guard = Utc::now();
        }
    }

    /// Set client metadata for a connection after initialize handshake
    ///
    /// Called when the client sends an initialize request to store client info
    /// and negotiated protocol version.
    pub async fn set_client_metadata(
        &self,
        connection_id: &ConnectionId,
        client_info: ClientInfo,
        protocol_version: String,
    ) {
        if let Some(conn) = self.connections.get(connection_id) {
            let mut client_guard = conn.client_info.write().await;
            *client_guard = Some(client_info.clone());

            let mut version_guard = conn.protocol_version.write().await;
            *version_guard = Some(protocol_version.clone());

            let mut init_guard = conn.initialized.write().await;
            *init_guard = true;

            debug!(
                connection_id = %connection_id,
                client_name = %client_info.name,
                client_version = %client_info.version,
                protocol_version = %protocol_version,
                "Set client metadata for connection"
            );
        }
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

    // Message buffer integration tests
    mod buffer_integration_tests {
        use super::*;

        #[tokio::test]
        async fn test_connection_has_message_buffer() {
            let manager = ConnectionManager::new();
            let (id, _rx) = manager.register("test-team".to_string()).unwrap();

            let buffer = manager.get_message_buffer(&id);
            assert!(buffer.is_some());

            let buffer = buffer.unwrap();
            assert!(buffer.is_empty().await);
        }

        #[tokio::test]
        async fn test_get_message_buffer_nonexistent() {
            let manager = ConnectionManager::new();
            let unknown_id = ConnectionId::new("unknown-id".to_string());

            let buffer = manager.get_message_buffer(&unknown_id);
            assert!(buffer.is_none());
        }

        #[tokio::test]
        async fn test_buffer_push_and_replay() {
            let manager = ConnectionManager::new();
            let (id, _rx) = manager.register("test-team".to_string()).unwrap();

            let buffer = manager.get_message_buffer(&id).unwrap();

            // Push messages
            let msg1 = NotificationMessage::ping();
            let msg2 = NotificationMessage::ping();
            let msg3 = NotificationMessage::ping();

            let seq1 = buffer.push(msg1).await;
            let seq2 = buffer.push(msg2).await;
            let seq3 = buffer.push(msg3).await;

            assert_eq!(seq1, 0);
            assert_eq!(seq2, 1);
            assert_eq!(seq3, 2);

            // Replay from seq 1
            let replayed = buffer.replay_from(seq1).await;
            assert_eq!(replayed.len(), 2);
            assert_eq!(replayed[0].0, seq2);
            assert_eq!(replayed[1].0, seq3);
        }

        #[tokio::test]
        async fn test_buffer_isolation_between_connections() {
            let manager = ConnectionManager::new();
            let (id1, _rx1) = manager.register("team-a".to_string()).unwrap();
            let (id2, _rx2) = manager.register("team-b".to_string()).unwrap();

            let buffer1 = manager.get_message_buffer(&id1).unwrap();
            let buffer2 = manager.get_message_buffer(&id2).unwrap();

            // Push to buffer1
            buffer1.push(NotificationMessage::ping()).await;
            buffer1.push(NotificationMessage::ping()).await;

            // Push to buffer2
            buffer2.push(NotificationMessage::ping()).await;

            // Verify isolation
            assert_eq!(buffer1.len().await, 2);
            assert_eq!(buffer2.len().await, 1);
        }

        #[tokio::test]
        async fn test_buffer_persists_during_connection_lifetime() {
            let manager = ConnectionManager::new();
            let (id, _rx) = manager.register("test-team".to_string()).unwrap();

            // Get buffer and push
            {
                let buffer = manager.get_message_buffer(&id).unwrap();
                buffer.push(NotificationMessage::ping()).await;
            }

            // Get buffer again and verify message persists
            let buffer = manager.get_message_buffer(&id).unwrap();
            assert_eq!(buffer.len().await, 1);

            // Replay works
            let replayed = buffer.replay_from(0).await;
            assert_eq!(replayed.len(), 0); // seq 0 was pushed, replay from 0 returns > 0
        }

        #[tokio::test]
        async fn test_buffer_cleared_on_unregister() {
            let manager = ConnectionManager::new();
            let (id, _rx) = manager.register("test-team".to_string()).unwrap();

            let buffer = manager.get_message_buffer(&id).unwrap();
            buffer.push(NotificationMessage::ping()).await;
            assert_eq!(buffer.len().await, 1);

            // Unregister removes connection and its buffer
            manager.unregister(&id);
            assert!(manager.get_message_buffer(&id).is_none());
        }

        #[tokio::test]
        async fn test_send_to_connection_buffers_message() {
            let manager = ConnectionManager::new();
            let (id, mut rx) = manager.register("test-team".to_string()).unwrap();

            // Send a message
            let msg = NotificationMessage::ping();
            manager.send_to_connection(&id, msg).await.unwrap();

            // Verify message was received
            let received = rx.try_recv();
            assert!(received.is_ok());

            // Verify message was also buffered
            let buffer = manager.get_message_buffer(&id).unwrap();
            assert_eq!(buffer.len().await, 1);
        }

        #[tokio::test]
        async fn test_broadcast_to_team_buffers_messages() {
            let manager = ConnectionManager::new();
            let (id1, mut rx1) = manager.register("team-a".to_string()).unwrap();
            let (id2, mut rx2) = manager.register("team-a".to_string()).unwrap();
            let (_id3, mut rx3) = manager.register("team-b".to_string()).unwrap();

            // Broadcast to team-a
            let msg = NotificationMessage::ping();
            manager.broadcast_to_team("team-a", msg).await;

            // Verify team-a connections received
            assert!(rx1.try_recv().is_ok());
            assert!(rx2.try_recv().is_ok());
            // team-b should not receive
            assert!(rx3.try_recv().is_err());

            // Verify messages were buffered for team-a connections
            let buffer1 = manager.get_message_buffer(&id1).unwrap();
            let buffer2 = manager.get_message_buffer(&id2).unwrap();
            assert_eq!(buffer1.len().await, 1);
            assert_eq!(buffer2.len().await, 1);
        }

        #[tokio::test]
        async fn test_buffer_replay_after_send() {
            let manager = ConnectionManager::new();
            let (id, _rx) = manager.register("test-team".to_string()).unwrap();

            // Send multiple messages
            for _ in 0..5 {
                manager.send_to_connection(&id, NotificationMessage::ping()).await.unwrap();
            }

            // Replay from sequence 2 should return sequences 3, 4
            let buffer = manager.get_message_buffer(&id).unwrap();
            let replayed = buffer.replay_from(2).await;
            assert_eq!(replayed.len(), 2);
            assert_eq!(replayed[0].0, 3);
            assert_eq!(replayed[1].0, 4);
        }
    }
}
