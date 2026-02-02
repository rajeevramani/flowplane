//! MCP Error Types

use crate::mcp::protocol::{error_codes, JsonRpcError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum McpError {
    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Method not found: {0}")]
    MethodNotFound(String),

    #[error("Invalid parameters: {0}")]
    InvalidParams(String),

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Resource not found: {0}")]
    ResourceNotFound(String),

    #[error("Gateway execution error: {0}")]
    GatewayError(String),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Not initialized")]
    NotInitialized,

    #[error("Connection limit ({limit}) exceeded for team: {team}")]
    ConnectionLimitExceeded { team: String, limit: usize },

    #[error("Prompt not found: {0}")]
    PromptNotFound(String),

    #[error("Unsupported protocol version '{client}'. Supported versions: {}", supported.join(", "))]
    UnsupportedProtocolVersion { client: String, supported: Vec<String> },

    /// Authorization failure - insufficient permissions
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// Validation failed on domain object
    #[error("Validation error: {0}")]
    ValidationError(String),

    /// Conflict with existing resource
    #[error("Conflict: {0}")]
    Conflict(String),

    /// Configuration error - missing or invalid configuration
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// Invalid origin header - not in allowlist (CSRF protection)
    #[error("Invalid origin: {0}")]
    InvalidOrigin(String),

    /// Missing required Origin header
    #[error("Missing required Origin header")]
    MissingOrigin,

    /// Malformed session ID format
    #[error("Malformed session ID: {0}")]
    MalformedSessionId(String),

    /// Invalid event ID format for SSE resumability
    #[error("Invalid event ID: {0}")]
    InvalidEventId(String),
}

impl McpError {
    /// Convert to JSON-RPC error code
    pub fn error_code(&self) -> i32 {
        match self {
            McpError::ParseError(_) => error_codes::PARSE_ERROR,
            McpError::InvalidRequest(_) | McpError::NotInitialized => error_codes::INVALID_REQUEST,
            McpError::MethodNotFound(_)
            | McpError::ToolNotFound(_)
            | McpError::ResourceNotFound(_) => error_codes::METHOD_NOT_FOUND,
            McpError::InvalidParams(_) => error_codes::INVALID_PARAMS,
            McpError::InternalError(_)
            | McpError::GatewayError(_)
            | McpError::DatabaseError(_)
            | McpError::SerializationError(_)
            | McpError::IoError(_)
            | McpError::ConnectionLimitExceeded { .. } => error_codes::INTERNAL_ERROR,
            McpError::PromptNotFound(_) => error_codes::METHOD_NOT_FOUND,
            McpError::UnsupportedProtocolVersion { .. } => error_codes::INVALID_REQUEST,
            McpError::Forbidden(_) => error_codes::INVALID_REQUEST, // No dedicated 403 in JSON-RPC
            McpError::ValidationError(_) => error_codes::INVALID_PARAMS,
            McpError::Conflict(_) => error_codes::INVALID_PARAMS,
            McpError::Configuration(_) => error_codes::INTERNAL_ERROR, // Configuration issues are internal
            McpError::InvalidOrigin(_) => error_codes::INVALID_REQUEST, // 403 Forbidden intent
            McpError::MissingOrigin => error_codes::INVALID_REQUEST,
            McpError::MalformedSessionId(_) => error_codes::INVALID_REQUEST, // 400 Bad Request intent
            McpError::InvalidEventId(_) => error_codes::INVALID_REQUEST, // 400 Bad Request intent
        }
    }

    /// Convert to JsonRpcError
    pub fn to_json_rpc_error(&self) -> JsonRpcError {
        let data = match self {
            McpError::UnsupportedProtocolVersion { supported, .. } => {
                Some(serde_json::json!({ "supportedVersions": supported }))
            }
            _ => None,
        };

        JsonRpcError { code: self.error_code(), message: self.to_string(), data }
    }
}

/// Implement Into<JsonRpcError> for McpError
impl From<McpError> for JsonRpcError {
    fn from(error: McpError) -> Self {
        error.to_json_rpc_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_mapping() {
        assert_eq!(McpError::ParseError("test".to_string()).error_code(), error_codes::PARSE_ERROR);
        assert_eq!(
            McpError::InvalidRequest("test".to_string()).error_code(),
            error_codes::INVALID_REQUEST
        );
        assert_eq!(
            McpError::MethodNotFound("test".to_string()).error_code(),
            error_codes::METHOD_NOT_FOUND
        );
        assert_eq!(
            McpError::ToolNotFound("test".to_string()).error_code(),
            error_codes::METHOD_NOT_FOUND
        );
        assert_eq!(
            McpError::ResourceNotFound("test".to_string()).error_code(),
            error_codes::METHOD_NOT_FOUND
        );
        assert_eq!(
            McpError::InvalidParams("test".to_string()).error_code(),
            error_codes::INVALID_PARAMS
        );
        assert_eq!(
            McpError::InternalError("test".to_string()).error_code(),
            error_codes::INTERNAL_ERROR
        );
        assert_eq!(
            McpError::GatewayError("test".to_string()).error_code(),
            error_codes::INTERNAL_ERROR
        );
    }

    #[test]
    fn test_to_json_rpc_error() {
        let error = McpError::ToolNotFound("test_tool".to_string());
        let json_rpc_error = error.to_json_rpc_error();

        assert_eq!(json_rpc_error.code, error_codes::METHOD_NOT_FOUND);
        assert_eq!(json_rpc_error.message, "Tool not found: test_tool");
        assert!(json_rpc_error.data.is_none());
    }

    #[test]
    fn test_into_json_rpc_error() {
        let error = McpError::InvalidParams("Missing required field".to_string());
        let json_rpc_error: JsonRpcError = error.into();

        assert_eq!(json_rpc_error.code, error_codes::INVALID_PARAMS);
        assert!(json_rpc_error.message.contains("Invalid parameters"));
    }

    #[test]
    fn test_forbidden_error() {
        let error = McpError::Forbidden("Cannot modify resources for other teams".to_string());
        assert_eq!(error.error_code(), error_codes::INVALID_REQUEST);
        assert!(error.to_string().contains("Forbidden"));
        assert!(error.to_string().contains("Cannot modify"));
    }

    #[test]
    fn test_validation_error() {
        let error = McpError::ValidationError("endpoints must not be empty".to_string());
        assert_eq!(error.error_code(), error_codes::INVALID_PARAMS);
        assert!(error.to_string().contains("Validation error"));
        assert!(error.to_string().contains("endpoints"));
    }

    #[test]
    fn test_conflict_error() {
        let error = McpError::Conflict("Cluster 'api-backend' already exists".to_string());
        assert_eq!(error.error_code(), error_codes::INVALID_PARAMS);
        assert!(error.to_string().contains("Conflict"));
        assert!(error.to_string().contains("already exists"));
    }

    #[test]
    fn test_invalid_origin_error() {
        let error = McpError::InvalidOrigin("http://evil.com not allowed".to_string());
        assert_eq!(error.error_code(), error_codes::INVALID_REQUEST);
        assert!(error.to_string().contains("Invalid origin"));
    }

    #[test]
    fn test_missing_origin_error() {
        let error = McpError::MissingOrigin;
        assert_eq!(error.error_code(), error_codes::INVALID_REQUEST);
        assert!(error.to_string().contains("Missing required Origin"));
    }

    #[test]
    fn test_malformed_session_id_error() {
        let error = McpError::MalformedSessionId("bad-id".to_string());
        assert_eq!(error.error_code(), error_codes::INVALID_REQUEST);
        assert!(error.to_string().contains("Malformed session ID"));
    }

    #[test]
    fn test_invalid_event_id_error() {
        let error = McpError::InvalidEventId("missing sequence".to_string());
        assert_eq!(error.error_code(), error_codes::INVALID_REQUEST);
        assert!(error.to_string().contains("Invalid event ID"));
    }
}
