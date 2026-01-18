//! MCP Logging API
//!
//! Provides logging functionality compliant with MCP 2025-11-25 specification.
//! Implements notifications/message with RFC 5424 syslog severity levels.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::mcp::connection::ConnectionId;
use crate::mcp::notifications::{LogLevel, LogNotification};

/// Request to set minimum log level for a connection
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetLogLevelParams {
    pub level: LogLevel,
}

/// MCP logger that manages log levels and creates notifications
pub struct McpLogger {
    /// Per-connection minimum log level filters
    connection_filters: Arc<DashMap<String, LogLevel>>,
}

impl McpLogger {
    /// Create a new MCP logger
    pub fn new() -> Self {
        Self { connection_filters: Arc::new(DashMap::new()) }
    }

    /// Create a log notification
    ///
    /// This method creates a notification that should be sent to connected clients.
    /// Use `should_log()` to filter based on connection-specific log levels.
    pub fn log(
        &self,
        level: LogLevel,
        logger: Option<String>,
        data: serde_json::Value,
    ) -> LogNotification {
        LogNotification::new(level, logger, data)
    }

    /// Set minimum log level for a connection
    ///
    /// Only logs at or above this level will be sent to the connection.
    pub fn set_level(&self, connection_id: &ConnectionId, level: LogLevel) {
        self.connection_filters.insert(connection_id.as_str().to_string(), level);
    }

    /// Set minimum log level for a connection by string ID
    pub fn set_level_by_id(&self, connection_id: &str, level: LogLevel) {
        self.connection_filters.insert(connection_id.to_string(), level);
    }

    /// Check if a log should be sent to a connection
    ///
    /// Returns true if:
    /// - No filter is set for the connection (send all logs), OR
    /// - The log level is at or above the connection's minimum level
    pub fn should_log(&self, connection_id: &str, level: LogLevel) -> bool {
        match self.connection_filters.get(connection_id) {
            Some(min_level) => level >= *min_level,
            None => true, // No filter set, send all logs
        }
    }

    /// Remove connection filter
    ///
    /// Should be called when a connection is closed.
    pub fn remove_connection(&self, connection_id: &str) {
        self.connection_filters.remove(connection_id);
    }

    /// Get current log level for a connection
    pub fn get_level(&self, connection_id: &str) -> Option<LogLevel> {
        self.connection_filters.get(connection_id).map(|r| *r)
    }
}

impl Default for McpLogger {
    fn default() -> Self {
        Self::new()
    }
}

/// Arc wrapper for shared logger
pub type SharedMcpLogger = Arc<McpLogger>;

/// Create a new shared MCP logger
pub fn create_mcp_logger() -> SharedMcpLogger {
    Arc::new(McpLogger::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_notification_creation() {
        let logger = McpLogger::new();

        let notification = logger.log(
            LogLevel::Error,
            Some("database".to_string()),
            serde_json::json!({
                "error": "Connection failed",
                "details": {
                    "host": "localhost",
                    "port": 5432
                }
            }),
        );

        assert_eq!(notification.method, "notifications/message");
        assert_eq!(notification.params.level, LogLevel::Error);
        assert_eq!(notification.params.logger, Some("database".to_string()));
        assert!(notification.params.data.is_object());
    }

    #[test]
    fn test_log_notification_without_logger() {
        let logger = McpLogger::new();

        let notification =
            logger.log(LogLevel::Info, None, serde_json::json!({"message": "Server started"}));

        assert_eq!(notification.method, "notifications/message");
        assert_eq!(notification.params.logger, None);
    }

    #[test]
    fn test_set_level() {
        let logger = McpLogger::new();

        logger.set_level_by_id("conn-1", LogLevel::Warning);

        let level = logger.get_level("conn-1");
        assert_eq!(level, Some(LogLevel::Warning));
    }

    #[test]
    fn test_should_log_filtering() {
        let logger = McpLogger::new();

        // Set minimum level to Warning
        logger.set_level_by_id("conn-1", LogLevel::Warning);

        // Debug and Info should be filtered out
        assert!(!logger.should_log("conn-1", LogLevel::Debug));
        assert!(!logger.should_log("conn-1", LogLevel::Info));
        assert!(!logger.should_log("conn-1", LogLevel::Notice));

        // Warning and above should pass
        assert!(logger.should_log("conn-1", LogLevel::Warning));
        assert!(logger.should_log("conn-1", LogLevel::Error));
        assert!(logger.should_log("conn-1", LogLevel::Critical));
        assert!(logger.should_log("conn-1", LogLevel::Alert));
        assert!(logger.should_log("conn-1", LogLevel::Emergency));
    }

    #[test]
    fn test_should_log_no_filter() {
        let logger = McpLogger::new();

        // No filter set - all levels should pass
        assert!(logger.should_log("conn-unknown", LogLevel::Debug));
        assert!(logger.should_log("conn-unknown", LogLevel::Info));
        assert!(logger.should_log("conn-unknown", LogLevel::Emergency));
    }

    #[test]
    fn test_remove_connection() {
        let logger = McpLogger::new();

        logger.set_level_by_id("conn-1", LogLevel::Error);
        assert_eq!(logger.get_level("conn-1"), Some(LogLevel::Error));

        logger.remove_connection("conn-1");
        assert_eq!(logger.get_level("conn-1"), None);
    }

    #[test]
    fn test_set_log_level_params_serialization() {
        let params = SetLogLevelParams { level: LogLevel::Warning };
        let json = serde_json::to_value(&params).expect("Failed to serialize");
        assert_eq!(json["level"], "warning");

        let deserialized: SetLogLevelParams =
            serde_json::from_value(json).expect("Failed to deserialize");
        assert_eq!(deserialized.level, LogLevel::Warning);
    }
}
