//! # Error Types
//!
//! Comprehensive error types for the Magaya control plane using `thiserror`.

use std::fmt;

/// Custom result type for Magaya operations
pub type Result<T> = std::result::Result<T, MagayaError>;

/// Main error type for the Magaya control plane
#[derive(thiserror::Error, Debug)]
pub enum MagayaError {
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
    Validation {
        message: String,
        field: Option<String>,
    },

    /// Authentication and authorization errors
    #[error("Authentication error: {message}")]
    Auth {
        message: String,
        error_type: AuthErrorType,
    },

    /// xDS protocol errors
    #[error("xDS protocol error: {message}")]
    Xds {
        message: String,
        node_id: Option<String>,
    },

    /// HTTP/API errors
    #[error("HTTP error: {message} (status: {status})")]
    Http {
        message: String,
        status: u16,
    },

    /// Internal server errors
    #[error("Internal server error: {message}")]
    Internal {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Resource not found errors
    #[error("Resource not found: {resource_type} with ID '{id}'")]
    NotFound {
        resource_type: String,
        id: String,
    },

    /// Resource conflict errors (e.g., already exists)
    #[error("Resource conflict: {message}")]
    Conflict {
        message: String,
        resource_type: String,
    },

    /// Rate limiting errors
    #[error("Rate limit exceeded: {message}")]
    RateLimit {
        message: String,
        retry_after: Option<u64>,
    },

    /// Timeout errors
    #[error("Operation timed out: {operation} after {duration_ms}ms")]
    Timeout {
        operation: String,
        duration_ms: u64,
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

impl MagayaError {
    /// Create a new configuration error
    pub fn config<S: Into<String>>(message: S) -> Self {
        Self::Config {
            message: message.into(),
            source: None,
        }
    }

    /// Create a configuration error with source
    pub fn config_with_source<S: Into<String>>(
        message: S,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        Self::Config {
            message: message.into(),
            source: Some(source),
        }
    }

    /// Create a validation error
    pub fn validation<S: Into<String>>(message: S) -> Self {
        Self::Validation {
            message: message.into(),
            field: None,
        }
    }

    /// Create a validation error with field information
    pub fn validation_field<S: Into<String>, F: Into<String>>(message: S, field: F) -> Self {
        Self::Validation {
            message: message.into(),
            field: Some(field.into()),
        }
    }

    /// Create an authentication error
    pub fn auth<S: Into<String>>(message: S, error_type: AuthErrorType) -> Self {
        Self::Auth {
            message: message.into(),
            error_type,
        }
    }

    /// Create an xDS protocol error
    pub fn xds<S: Into<String>>(message: S) -> Self {
        Self::Xds {
            message: message.into(),
            node_id: None,
        }
    }

    /// Create an xDS protocol error with node ID
    pub fn xds_with_node<S: Into<String>, N: Into<String>>(message: S, node_id: N) -> Self {
        Self::Xds {
            message: message.into(),
            node_id: Some(node_id.into()),
        }
    }

    /// Create an HTTP error
    pub fn http<S: Into<String>>(message: S, status: u16) -> Self {
        Self::Http {
            message: message.into(),
            status,
        }
    }

    /// Create an internal server error
    pub fn internal<S: Into<String>>(message: S) -> Self {
        Self::Internal {
            message: message.into(),
            source: None,
        }
    }

    /// Create a not found error
    pub fn not_found<R: Into<String>, I: Into<String>>(resource_type: R, id: I) -> Self {
        Self::NotFound {
            resource_type: resource_type.into(),
            id: id.into(),
        }
    }

    /// Create a conflict error
    pub fn conflict<M: Into<String>, R: Into<String>>(message: M, resource_type: R) -> Self {
        Self::Conflict {
            message: message.into(),
            resource_type: resource_type.into(),
        }
    }

    /// Create a rate limit error
    pub fn rate_limit<S: Into<String>>(message: S) -> Self {
        Self::RateLimit {
            message: message.into(),
            retry_after: None,
        }
    }

    /// Create a timeout error
    pub fn timeout<S: Into<String>>(operation: S, duration_ms: u64) -> Self {
        Self::Timeout {
            operation: operation.into(),
            duration_ms,
        }
    }

    /// Add context to an error (used by ErrorContext trait)
    pub(crate) fn add_context(&mut self, context: String) {
        match self {
            MagayaError::Io { context: ref mut ctx, .. } => {
                *ctx = format!("{}: {}", context, ctx);
            }
            MagayaError::Database { context: ref mut ctx, .. } => {
                *ctx = format!("{}: {}", context, ctx);
            }
            MagayaError::Serialization { context: ref mut ctx, .. } => {
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
            MagayaError::Config { .. } => 500,
            MagayaError::Database { .. } => 500,
            MagayaError::Io { .. } => 500,
            MagayaError::Serialization { .. } => 400,
            MagayaError::Validation { .. } => 400,
            MagayaError::Auth { .. } => 401,
            MagayaError::Xds { .. } => 500,
            MagayaError::Http { status, .. } => *status,
            MagayaError::Internal { .. } => 500,
            MagayaError::NotFound { .. } => 404,
            MagayaError::Conflict { .. } => 409,
            MagayaError::RateLimit { .. } => 429,
            MagayaError::Timeout { .. } => 408,
        }
    }

    /// Check if this error should be retried
    pub fn is_retryable(&self) -> bool {
        match self {
            MagayaError::Database { .. } => true,
            MagayaError::Io { .. } => true,
            MagayaError::Timeout { .. } => true,
            MagayaError::RateLimit { .. } => true,
            _ => false,
        }
    }
}

// Error conversions for common external error types
impl From<sqlx::Error> for MagayaError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database {
            source: error,
            context: "Database operation failed".to_string(),
        }
    }
}

impl From<std::io::Error> for MagayaError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            source: error,
            context: "I/O operation failed".to_string(),
        }
    }
}

impl From<serde_json::Error> for MagayaError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization {
            source: error,
            context: "JSON serialization failed".to_string(),
        }
    }
}

impl From<config::ConfigError> for MagayaError {
    fn from(error: config::ConfigError) -> Self {
        Self::config_with_source("Configuration loading failed", Box::new(error))
    }
}

impl From<validator::ValidationErrors> for MagayaError {
    fn from(errors: validator::ValidationErrors) -> Self {
        let message = errors
            .field_errors()
            .iter()
            .map(|(field, field_errors)| {
                let error_messages: Vec<String> = field_errors
                    .iter()
                    .map(|e| e.message.as_ref().map_or("Invalid value".to_string(), |m| m.to_string()))
                    .collect();
                format!("{}: {}", field, error_messages.join(", "))
            })
            .collect::<Vec<_>>()
            .join("; ");

        Self::validation(format!("Validation failed: {}", message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_creation() {
        let error = MagayaError::config("Test configuration error");
        assert!(matches!(error, MagayaError::Config { .. }));
        assert_eq!(error.to_string(), "Configuration error: Test configuration error");
    }

    #[test]
    fn test_validation_error() {
        let error = MagayaError::validation_field("Invalid email format", "email");
        assert!(matches!(error, MagayaError::Validation { .. }));
        if let MagayaError::Validation { field, .. } = error {
            assert_eq!(field, Some("email".to_string()));
        }
    }

    #[test]
    fn test_auth_error() {
        let error = MagayaError::auth("Invalid token", AuthErrorType::InvalidToken);
        assert!(matches!(error, MagayaError::Auth { .. }));
        if let MagayaError::Auth { error_type, .. } = error {
            assert_eq!(error_type, AuthErrorType::InvalidToken);
        }
    }

    #[test]
    fn test_status_codes() {
        assert_eq!(MagayaError::validation("test").status_code(), 400);
        assert_eq!(MagayaError::auth("test", AuthErrorType::InvalidToken).status_code(), 401);
        assert_eq!(MagayaError::not_found("cluster", "test").status_code(), 404);
        assert_eq!(MagayaError::conflict("test", "cluster").status_code(), 409);
        assert_eq!(MagayaError::rate_limit("test").status_code(), 429);
        assert_eq!(MagayaError::internal("test").status_code(), 500);
    }

    #[test]
    fn test_retryable_errors() {
        assert!(MagayaError::timeout("test", 1000).is_retryable());
        assert!(MagayaError::rate_limit("test").is_retryable());
        assert!(!MagayaError::validation("test").is_retryable());
        assert!(!MagayaError::not_found("cluster", "test").is_retryable());
    }

    #[test]
    fn test_error_conversions() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let magaya_error: MagayaError = io_error.into();
        assert!(matches!(magaya_error, MagayaError::Io { .. }));

        let json_error = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let magaya_error: MagayaError = json_error.into();
        assert!(matches!(magaya_error, MagayaError::Serialization { .. }));
    }

    #[test]
    fn test_auth_error_type_display() {
        assert_eq!(AuthErrorType::InvalidToken.to_string(), "invalid_token");
        assert_eq!(AuthErrorType::ExpiredToken.to_string(), "expired_token");
        assert_eq!(AuthErrorType::MissingToken.to_string(), "missing_token");
        assert_eq!(AuthErrorType::InsufficientPermissions.to_string(), "insufficient_permissions");
        assert_eq!(AuthErrorType::InvalidCredentials.to_string(), "invalid_credentials");
    }
}