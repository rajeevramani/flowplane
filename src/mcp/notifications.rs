//! MCP Notification Types
//!
//! Defines notification message types for SSE streaming including progress and log notifications.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::mcp::protocol::JsonRpcResponse;

/// Newtype for progress tokens
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct ProgressToken(String);

impl ProgressToken {
    /// Create a new progress token
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Get the token as a string slice
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ProgressToken {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ProgressToken {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Display for ProgressToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// RFC 5424 syslog severity levels for MCP logging
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
    Emergency,
}

/// Progress notification parameters
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProgressParams {
    pub progress_token: ProgressToken,
    pub progress: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Progress notification
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProgressNotification {
    pub method: String,
    pub params: ProgressParams,
}

impl ProgressNotification {
    /// Create a new progress notification
    pub fn new(
        token: ProgressToken,
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
    ) -> Self {
        Self {
            method: "notifications/progress".to_string(),
            params: ProgressParams { progress_token: token, progress, total, message },
        }
    }
}

/// Log notification parameters
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LogParams {
    pub level: LogLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,
    pub data: serde_json::Value,
}

/// Log notification
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LogNotification {
    pub method: String,
    pub params: LogParams,
}

impl LogNotification {
    /// Create a new log notification
    pub fn new(level: LogLevel, logger: Option<String>, data: serde_json::Value) -> Self {
        Self {
            method: "notifications/message".to_string(),
            params: LogParams { level, logger, data },
        }
    }
}

/// Server-sent event types for MCP streaming
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NotificationMessage {
    /// JSON-RPC response message
    Message { data: JsonRpcResponse },
    /// Progress update notification
    Progress { data: ProgressNotification },
    /// Log message notification
    Log { data: LogNotification },
    /// Heartbeat ping
    Ping { timestamp: i64 },
}

impl NotificationMessage {
    /// Get the event type name for SSE
    pub fn event_type(&self) -> &'static str {
        match self {
            NotificationMessage::Message { .. } => "message",
            NotificationMessage::Progress { .. } => "progress",
            NotificationMessage::Log { .. } => "log",
            NotificationMessage::Ping { .. } => "ping",
        }
    }

    /// Create a message notification
    pub fn message(response: JsonRpcResponse) -> Self {
        NotificationMessage::Message { data: response }
    }

    /// Create a progress notification
    pub fn progress(notification: ProgressNotification) -> Self {
        NotificationMessage::Progress { data: notification }
    }

    /// Create a log notification
    pub fn log(notification: LogNotification) -> Self {
        NotificationMessage::Log { data: notification }
    }

    /// Create a ping notification with current timestamp
    pub fn ping() -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        NotificationMessage::Ping { timestamp }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_token_creation() {
        let token = ProgressToken::new("test-token-123".to_string());
        assert_eq!(token.as_str(), "test-token-123");
        assert_eq!(format!("{}", token), "test-token-123");
    }

    #[test]
    fn test_progress_token_from() {
        let token1: ProgressToken = "test".into();
        assert_eq!(token1.as_str(), "test");

        let token2: ProgressToken = String::from("test2").into();
        assert_eq!(token2.as_str(), "test2");
    }

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Notice);
        assert!(LogLevel::Notice < LogLevel::Warning);
        assert!(LogLevel::Warning < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Critical);
        assert!(LogLevel::Critical < LogLevel::Alert);
        assert!(LogLevel::Alert < LogLevel::Emergency);
    }

    #[test]
    fn test_progress_notification_creation() {
        let token = ProgressToken::new("tok-1".to_string());
        let notification =
            ProgressNotification::new(token, 50.0, Some(100.0), Some("Processing".to_string()));

        assert_eq!(notification.method, "notifications/progress");
        assert_eq!(notification.params.progress, 50.0);
        assert_eq!(notification.params.total, Some(100.0));
        assert_eq!(notification.params.message, Some("Processing".to_string()));
    }

    #[test]
    fn test_log_notification_creation() {
        let notification = LogNotification::new(
            LogLevel::Error,
            Some("database".to_string()),
            serde_json::json!({"error": "Connection failed"}),
        );

        assert_eq!(notification.method, "notifications/message");
        assert_eq!(notification.params.level, LogLevel::Error);
        assert_eq!(notification.params.logger, Some("database".to_string()));
    }

    #[test]
    fn test_notification_message_event_types() {
        let msg = NotificationMessage::message(crate::mcp::protocol::JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
        });
        assert_eq!(msg.event_type(), "message");

        let progress = NotificationMessage::progress(ProgressNotification::new(
            ProgressToken::new("tok".to_string()),
            10.0,
            None,
            None,
        ));
        assert_eq!(progress.event_type(), "progress");

        let log = NotificationMessage::log(LogNotification::new(
            LogLevel::Info,
            None,
            serde_json::json!({"msg": "test"}),
        ));
        assert_eq!(log.event_type(), "log");

        let ping = NotificationMessage::ping();
        assert_eq!(ping.event_type(), "ping");
    }

    #[test]
    fn test_progress_notification_serialization() {
        let notification = ProgressNotification::new(
            ProgressToken::new("tok-1".to_string()),
            50.0,
            Some(100.0),
            None,
        );

        let json = serde_json::to_value(&notification).expect("Failed to serialize");
        assert_eq!(json["method"], "notifications/progress");
        assert_eq!(json["params"]["progressToken"], "tok-1");
        assert_eq!(json["params"]["progress"], 50.0);
        assert_eq!(json["params"]["total"], 100.0);
    }

    #[test]
    fn test_log_notification_serialization() {
        let notification = LogNotification::new(
            LogLevel::Warning,
            None,
            serde_json::json!({"status": "degraded"}),
        );

        let json = serde_json::to_value(&notification).expect("Failed to serialize");
        assert_eq!(json["method"], "notifications/message");
        assert_eq!(json["params"]["level"], "warning");
    }

    #[test]
    fn test_log_level_serialization() {
        assert_eq!(serde_json::to_string(&LogLevel::Debug).unwrap(), r#""debug""#);
        assert_eq!(serde_json::to_string(&LogLevel::Emergency).unwrap(), r#""emergency""#);
    }
}
