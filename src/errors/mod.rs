//! # Error Handling
//!
//! This module provides comprehensive error handling for the Flowplane control plane.
//! It defines custom error types using `thiserror` for all operations.

use std::fmt;

pub mod tls;

pub use tls::TlsError;

/// Custom result type for Flowplane operations
pub type Result<T> = std::result::Result<T, FlowplaneError>;

/// Main error type for the Flowplane control plane
#[derive(thiserror::Error, Debug)]
pub enum FlowplaneError {
    /// Configuration errors
    #[error("Configuration error: {message}")]
    Config {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Database and storage errors
    #[error("Database error: {context}")]
    Database {
        #[source]
        source: sqlx::Error,
        context: String,
    },

    /// Database constraint violation
    #[error("Database constraint violation: {message}")]
    ConstraintViolation {
        message: String,
        #[source]
        source: sqlx::Error,
    },

    /// I/O errors with additional context
    #[error("I/O error: {context}")]
    Io {
        #[source]
        source: std::io::Error,
        context: String,
    },

    /// Serialization/deserialization errors
    #[error("Serialization error: {context}")]
    Serialization {
        #[source]
        source: serde_json::Error,
        context: String,
    },

    /// Validation errors
    #[error("Validation error: {message}")]
    Validation { message: String, field: Option<String> },

    /// Authentication and authorization errors
    #[error("Authentication error: {message}")]
    Auth { message: String, error_type: AuthErrorType },

    /// xDS protocol errors
    #[error("xDS protocol error: {message}")]
    Xds { message: String, node_id: Option<String> },

    /// HTTP/API errors
    #[error("HTTP error: {message} (status: {status})")]
    Http { message: String, status: u16 },

    /// Internal server errors
    #[error("Internal server error: {message}")]
    Internal {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Resource not found errors
    #[error("Resource not found: {resource_type} with ID '{id}'")]
    NotFound { resource_type: String, id: String },

    /// Resource conflict errors (e.g., already exists)
    #[error("Resource conflict: {message}")]
    Conflict { message: String, resource_type: String },

    /// Rate limiting errors
    #[error("Rate limit exceeded: {message}")]
    RateLimit { message: String, retry_after: Option<u64> },

    /// Timeout errors
    #[error("Operation timed out: {operation} after {duration_ms}ms")]
    Timeout { operation: String, duration_ms: u64 },

    /// Network transport errors (gRPC, HTTP) - retained for backward compatibility
    #[error("Transport error: {0}")]
    Transport(String),

    /// Parsing/decoding errors (replaces unwrap on parse operations)
    #[error("Parse error: {context}")]
    Parse {
        context: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Lock/concurrency errors (replaces expect on RwLock/Mutex)
    #[error("Synchronization error: {context}")]
    Sync { context: String },

    /// Type conversion errors (replaces unwrap on TryFrom/TryInto)
    #[error("Conversion error: {context}")]
    Conversion {
        context: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

/// Authentication error subtypes
#[derive(Debug, Clone, PartialEq)]
pub enum AuthErrorType {
    InvalidToken,
    ExpiredToken,
    MissingToken,
    InsufficientPermissions,
    InvalidCredentials,
}

impl fmt::Display for AuthErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthErrorType::InvalidToken => write!(f, "invalid_token"),
            AuthErrorType::ExpiredToken => write!(f, "expired_token"),
            AuthErrorType::MissingToken => write!(f, "missing_token"),
            AuthErrorType::InsufficientPermissions => write!(f, "insufficient_permissions"),
            AuthErrorType::InvalidCredentials => write!(f, "invalid_credentials"),
        }
    }
}

/// Alias for backward compatibility
pub type Error = FlowplaneError;

impl FlowplaneError {
    /// Create a new configuration error
    pub fn config<S: Into<String>>(message: S) -> Self {
        Self::Config { message: message.into(), source: None }
    }

    /// Create a configuration error with source
    pub fn config_with_source<S: Into<String>>(
        message: S,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self::Config { message: message.into(), source: Some(source) }
    }

    /// Create a validation error
    pub fn validation<S: Into<String>>(message: S) -> Self {
        Self::Validation { message: message.into(), field: None }
    }

    /// Create a validation error with field information
    pub fn validation_field<S: Into<String>, F: Into<String>>(message: S, field: F) -> Self {
        Self::Validation { message: message.into(), field: Some(field.into()) }
    }

    /// Create an authentication error
    pub fn auth<S: Into<String>>(message: S, error_type: AuthErrorType) -> Self {
        Self::Auth { message: message.into(), error_type }
    }

    /// Create an xDS protocol error
    pub fn xds<S: Into<String>>(message: S) -> Self {
        Self::Xds { message: message.into(), node_id: None }
    }

    /// Create an xDS protocol error with node ID
    pub fn xds_with_node<S: Into<String>, N: Into<String>>(message: S, node_id: N) -> Self {
        Self::Xds { message: message.into(), node_id: Some(node_id.into()) }
    }

    /// Create an HTTP error
    pub fn http<S: Into<String>>(message: S, status: u16) -> Self {
        Self::Http { message: message.into(), status }
    }

    /// Create an internal server error
    pub fn internal<S: Into<String>>(message: S) -> Self {
        Self::Internal { message: message.into(), source: None }
    }

    /// Create a not found error
    pub fn not_found<R: Into<String>, I: Into<String>>(resource_type: R, id: I) -> Self {
        Self::NotFound { resource_type: resource_type.into(), id: id.into() }
    }

    /// Create a not found error with a single message (backward compatibility)
    pub fn not_found_msg<S: Into<String>>(message: S) -> Self {
        Self::NotFound { resource_type: "Resource".to_string(), id: message.into() }
    }

    /// Create a conflict error
    pub fn conflict<M: Into<String>, R: Into<String>>(message: M, resource_type: R) -> Self {
        Self::Conflict { message: message.into(), resource_type: resource_type.into() }
    }

    /// Create a rate limit error
    pub fn rate_limit<S: Into<String>>(message: S) -> Self {
        Self::RateLimit { message: message.into(), retry_after: None }
    }

    /// Create a timeout error
    pub fn timeout<S: Into<String>>(operation: S, duration_ms: u64) -> Self {
        Self::Timeout { operation: operation.into(), duration_ms }
    }

    /// Create a new transport error (for backward compatibility)
    pub fn transport<S: Into<String>>(message: S) -> Self {
        Self::Transport(message.into())
    }

    /// Create a new database error
    pub fn database(source: sqlx::Error, context: String) -> Self {
        Self::Database { source, context }
    }

    /// Create a parse error
    pub fn parse<S: Into<String>>(context: S) -> Self {
        Self::Parse { context: context.into(), source: None }
    }

    /// Create a parse error with source
    pub fn parse_with_source<S: Into<String>>(
        context: S,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self::Parse { context: context.into(), source: Some(source) }
    }

    /// Create a sync/lock error
    pub fn sync<S: Into<String>>(context: S) -> Self {
        Self::Sync { context: context.into() }
    }

    /// Create a conversion error
    pub fn conversion<S: Into<String>>(context: S) -> Self {
        Self::Conversion { context: context.into(), source: None }
    }

    /// Create a conversion error with source
    pub fn conversion_with_source<S: Into<String>>(
        context: S,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self::Conversion { context: context.into(), source: Some(source) }
    }

    /// Create a serialization error with custom context
    pub fn serialization<S: Into<String>>(source: serde_json::Error, context: S) -> Self {
        Self::Serialization { source, context: context.into() }
    }

    /// Add context to an error (used by ErrorContext trait)
    #[allow(dead_code)]
    pub(crate) fn add_context(&mut self, context: String) {
        match self {
            FlowplaneError::Io { context: ref mut ctx, .. } => {
                *ctx = format!("{}: {}", context, ctx);
            }
            FlowplaneError::Database { context: ref mut ctx, .. } => {
                *ctx = format!("{}: {}", context, ctx);
            }
            FlowplaneError::Serialization { context: ref mut ctx, .. } => {
                *ctx = format!("{}: {}", context, ctx);
            }
            _ => {
                // For other error types, we could extend them to support context
                // For now, we'll leave them as-is
            }
        }
    }

    /// Get the HTTP status code that should be returned for this error
    pub fn status_code(&self) -> u16 {
        match self {
            FlowplaneError::Config { .. } => 500,
            FlowplaneError::Database { .. } => 500,
            FlowplaneError::Io { .. } => 500,
            FlowplaneError::Serialization { .. } => 400,
            FlowplaneError::Validation { .. } => 400,
            FlowplaneError::Auth { .. } => 401,
            FlowplaneError::Xds { .. } => 500,
            FlowplaneError::Http { status, .. } => *status,
            FlowplaneError::Internal { .. } => 500,
            FlowplaneError::NotFound { .. } => 404,
            FlowplaneError::Conflict { .. } => 409,
            FlowplaneError::RateLimit { .. } => 429,
            FlowplaneError::Timeout { .. } => 408,
            FlowplaneError::ConstraintViolation { .. } => 409,
            FlowplaneError::Transport(_) => 500,
            FlowplaneError::Parse { .. } => 400,
            FlowplaneError::Sync { .. } => 500,
            FlowplaneError::Conversion { .. } => 400,
        }
    }

    /// Check if this error should be retried
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            FlowplaneError::Database { .. }
                | FlowplaneError::Io { .. }
                | FlowplaneError::Timeout { .. }
                | FlowplaneError::RateLimit { .. }
        )
    }
}

// Error conversions for common external error types
impl From<sqlx::Error> for FlowplaneError {
    fn from(error: sqlx::Error) -> Self {
        // Check for constraint violations
        if let Some(db_err) = error.as_database_error() {
            if let Some(code) = db_err.code() {
                // PostgreSQL constraint violation error codes (Class 23)
                // 23505 = unique_violation
                // 23503 = foreign_key_violation
                // 23502 = not_null_violation
                // 23514 = check_violation
                if code.as_ref().starts_with("23") {
                    return Self::ConstraintViolation {
                        message: db_err.message().to_string(),
                        source: error,
                    };
                }
            }
        }

        Self::Database { source: error, context: "Database operation failed".to_string() }
    }
}

impl From<std::io::Error> for FlowplaneError {
    fn from(error: std::io::Error) -> Self {
        Self::Io { source: error, context: "I/O operation failed".to_string() }
    }
}

impl From<serde_json::Error> for FlowplaneError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization { source: error, context: "JSON serialization failed".to_string() }
    }
}

impl From<config::ConfigError> for FlowplaneError {
    fn from(error: config::ConfigError) -> Self {
        Self::config_with_source("Configuration loading failed", Box::new(error))
    }
}

impl From<validator::ValidationErrors> for FlowplaneError {
    fn from(errors: validator::ValidationErrors) -> Self {
        let message = errors
            .field_errors()
            .iter()
            .map(|(field, field_errors)| {
                let error_messages: Vec<String> = field_errors
                    .iter()
                    .map(|e| {
                        e.message.as_ref().map_or("Invalid value".to_string(), |m| m.to_string())
                    })
                    .collect();
                format!("{}: {}", field, error_messages.join(", "))
            })
            .collect::<Vec<_>>()
            .join("; ");

        Self::validation(format!("Validation failed: {}", message))
    }
}

impl From<TlsError> for FlowplaneError {
    fn from(error: TlsError) -> Self {
        Self::Config { message: error.to_string(), source: None }
    }
}

impl From<std::num::ParseIntError> for FlowplaneError {
    fn from(error: std::num::ParseIntError) -> Self {
        Self::parse_with_source("Integer parsing failed", Box::new(error))
    }
}

impl From<std::num::ParseFloatError> for FlowplaneError {
    fn from(error: std::num::ParseFloatError) -> Self {
        Self::parse_with_source("Float parsing failed", Box::new(error))
    }
}

impl From<url::ParseError> for FlowplaneError {
    fn from(error: url::ParseError) -> Self {
        Self::parse_with_source("URL parsing failed", Box::new(error))
    }
}

impl From<uuid::Error> for FlowplaneError {
    fn from(error: uuid::Error) -> Self {
        Self::parse_with_source("UUID parsing failed", Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_creation() {
        let error = FlowplaneError::config("Test configuration error");
        assert!(matches!(error, FlowplaneError::Config { .. }));
        assert_eq!(error.to_string(), "Configuration error: Test configuration error");
    }

    #[test]
    fn test_validation_error() {
        let error = FlowplaneError::validation_field("Invalid email format", "email");
        assert!(matches!(error, FlowplaneError::Validation { .. }));
        if let FlowplaneError::Validation { field, .. } = error {
            assert_eq!(field, Some("email".to_string()));
        }
    }

    #[test]
    fn test_auth_error() {
        let error = FlowplaneError::auth("Invalid token", AuthErrorType::InvalidToken);
        assert!(matches!(error, FlowplaneError::Auth { .. }));
        if let FlowplaneError::Auth { error_type, .. } = error {
            assert_eq!(error_type, AuthErrorType::InvalidToken);
        }
    }

    #[test]
    fn test_status_codes() {
        assert_eq!(FlowplaneError::validation("test").status_code(), 400);
        assert_eq!(FlowplaneError::auth("test", AuthErrorType::InvalidToken).status_code(), 401);
        assert_eq!(FlowplaneError::not_found("cluster", "test").status_code(), 404);
        assert_eq!(FlowplaneError::conflict("test", "cluster").status_code(), 409);
        assert_eq!(FlowplaneError::rate_limit("test").status_code(), 429);
        assert_eq!(FlowplaneError::internal("test").status_code(), 500);
    }

    #[test]
    fn test_retryable_errors() {
        assert!(FlowplaneError::timeout("test", 1000).is_retryable());
        assert!(FlowplaneError::rate_limit("test").is_retryable());
        assert!(!FlowplaneError::validation("test").is_retryable());
        assert!(!FlowplaneError::not_found("cluster", "test").is_retryable());
    }

    #[test]
    fn test_error_conversions() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let flowplane_error: FlowplaneError = io_error.into();
        assert!(matches!(flowplane_error, FlowplaneError::Io { .. }));

        let json_error = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let flowplane_error: FlowplaneError = json_error.into();
        assert!(matches!(flowplane_error, FlowplaneError::Serialization { .. }));
    }

    #[test]
    fn test_auth_error_type_display() {
        assert_eq!(AuthErrorType::InvalidToken.to_string(), "invalid_token");
        assert_eq!(AuthErrorType::ExpiredToken.to_string(), "expired_token");
        assert_eq!(AuthErrorType::MissingToken.to_string(), "missing_token");
        assert_eq!(AuthErrorType::InsufficientPermissions.to_string(), "insufficient_permissions");
        assert_eq!(AuthErrorType::InvalidCredentials.to_string(), "invalid_credentials");
    }

    #[test]
    fn test_backward_compatibility_simple_variants() {
        // Test that simple error constructors from old Error type still work
        let _config = FlowplaneError::config("test");
        let _transport = FlowplaneError::transport("test");
        let _internal = FlowplaneError::internal("test");
        let _validation = FlowplaneError::validation("test");
    }

    #[test]
    fn test_parse_error() {
        let error = FlowplaneError::parse("Invalid format");
        assert!(matches!(error, FlowplaneError::Parse { .. }));
        assert_eq!(error.status_code(), 400);
        assert_eq!(error.to_string(), "Parse error: Invalid format");
    }

    #[test]
    fn test_sync_error() {
        let error = FlowplaneError::sync("Lock poisoned");
        assert!(matches!(error, FlowplaneError::Sync { .. }));
        assert_eq!(error.status_code(), 500);
        assert_eq!(error.to_string(), "Synchronization error: Lock poisoned");
    }

    #[test]
    fn test_conversion_error() {
        let error = FlowplaneError::conversion("Value out of range");
        assert!(matches!(error, FlowplaneError::Conversion { .. }));
        assert_eq!(error.status_code(), 400);
        assert_eq!(error.to_string(), "Conversion error: Value out of range");
    }

    #[test]
    fn test_parse_error_conversions() {
        // Test ParseIntError conversion
        let int_error = "not_a_number".parse::<i64>().unwrap_err();
        let flowplane_error: FlowplaneError = int_error.into();
        assert!(matches!(flowplane_error, FlowplaneError::Parse { .. }));

        // Test UUID parse error conversion
        let uuid_error = uuid::Uuid::parse_str("not-a-uuid").unwrap_err();
        let flowplane_error: FlowplaneError = uuid_error.into();
        assert!(matches!(flowplane_error, FlowplaneError::Parse { .. }));

        // Test URL parse error conversion
        let url_error = url::Url::parse("not a valid url").unwrap_err();
        let flowplane_error: FlowplaneError = url_error.into();
        assert!(matches!(flowplane_error, FlowplaneError::Parse { .. }));
    }
}
