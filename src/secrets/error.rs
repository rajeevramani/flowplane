//! Error types for secrets management operations.

use thiserror::Error;

/// Result type for secrets operations.
pub type Result<T> = std::result::Result<T, SecretsError>;

/// Errors that can occur during secrets management operations.
#[derive(Error, Debug)]
pub enum SecretsError {
    /// Secret not found in the backend.
    #[error("Secret not found: {key}")]
    NotFound { key: String },

    /// Failed to connect to the secrets backend.
    #[error("Backend connection failed: {message}")]
    ConnectionFailed { message: String },

    /// Authentication with the secrets backend failed.
    #[error("Authentication failed: {message}")]
    AuthenticationFailed { message: String },

    /// Invalid secret key format.
    #[error("Invalid secret key: {key} - {reason}")]
    InvalidKey { key: String, reason: String },

    /// Secret value validation failed.
    #[error("Invalid secret value: {reason}")]
    InvalidValue { reason: String },

    /// Secret rotation failed.
    #[error("Rotation failed for secret '{key}': {reason}")]
    RotationFailed { key: String, reason: String },

    /// Backend-specific error.
    #[error("Backend error: {message}")]
    BackendError { message: String },

    /// Configuration error.
    #[error("Configuration error: {message}")]
    ConfigError { message: String },

    /// Serialization/deserialization error.
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// HTTP request error (for remote backends).
    #[error("HTTP request failed: {0}")]
    HttpError(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Generic internal error.
    #[error("Internal error: {message}")]
    Internal { message: String },
}

impl SecretsError {
    /// Create a not found error.
    pub fn not_found(key: impl Into<String>) -> Self {
        Self::NotFound { key: key.into() }
    }

    /// Create a connection failed error.
    pub fn connection_failed(message: impl Into<String>) -> Self {
        Self::ConnectionFailed { message: message.into() }
    }

    /// Create an authentication failed error.
    pub fn authentication_failed(message: impl Into<String>) -> Self {
        Self::AuthenticationFailed { message: message.into() }
    }

    /// Create an invalid key error.
    pub fn invalid_key(key: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidKey { key: key.into(), reason: reason.into() }
    }

    /// Create an invalid value error.
    pub fn invalid_value(reason: impl Into<String>) -> Self {
        Self::InvalidValue { reason: reason.into() }
    }

    /// Create a rotation failed error.
    pub fn rotation_failed(key: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::RotationFailed { key: key.into(), reason: reason.into() }
    }

    /// Create a backend error.
    pub fn backend_error(message: impl Into<String>) -> Self {
        Self::BackendError { message: message.into() }
    }

    /// Create a config error.
    pub fn config_error(message: impl Into<String>) -> Self {
        Self::ConfigError { message: message.into() }
    }

    /// Create an internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal { message: message.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_constructors() {
        let err = SecretsError::not_found("test_key");
        assert!(matches!(err, SecretsError::NotFound { .. }));
        assert_eq!(err.to_string(), "Secret not found: test_key");

        let err = SecretsError::connection_failed("timeout");
        assert!(matches!(err, SecretsError::ConnectionFailed { .. }));

        let err = SecretsError::invalid_key("key", "too short");
        assert!(matches!(err, SecretsError::InvalidKey { .. }));
    }

    #[test]
    fn test_error_display() {
        let err = SecretsError::rotation_failed("bootstrap_token", "generation failed");
        assert!(err.to_string().contains("Rotation failed"));
        assert!(err.to_string().contains("bootstrap_token"));
    }
}
