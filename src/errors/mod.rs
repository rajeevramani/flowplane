//! # Error Handling
//!
//! This module provides error handling for the Flowplane control plane.
//! It defines custom error types using `thiserror` for the minimal XDS server.

pub mod tls;

pub use tls::TlsError;

/// Custom result type for Flowplane operations
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for the Flowplane control plane
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    /// Network transport errors (gRPC, HTTP)
    #[error("Transport error: {0}")]
    Transport(String),

    /// I/O errors
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Internal server errors
    #[error("Internal error: {0}")]
    Internal(String),

    /// Database errors
    #[error("Database error: {context}")]
    Database {
        #[source]
        source: sqlx::Error,
        context: String,
    },

    /// Validation errors
    #[error("Validation error: {0}")]
    Validation(String),

    /// Resource not found errors
    #[error("Not found: {0}")]
    NotFound(String),
}

/// Alias for compatibility with storage layer
pub type FlowplaneError = Error;

impl Error {
    /// Create a new configuration error
    pub fn config<S: Into<String>>(message: S) -> Self {
        Self::Config(message.into())
    }

    /// Create a new transport error
    pub fn transport<S: Into<String>>(message: S) -> Self {
        Self::Transport(message.into())
    }

    /// Create a new internal error
    pub fn internal<S: Into<String>>(message: S) -> Self {
        Self::Internal(message.into())
    }

    /// Create a new validation error
    pub fn validation<S: Into<String>>(message: S) -> Self {
        Self::Validation(message.into())
    }

    /// Create a new not found error
    pub fn not_found<S: Into<String>>(message: S) -> Self {
        Self::NotFound(message.into())
    }

    /// Create a new database error
    pub fn database(source: sqlx::Error, context: String) -> Self {
        Self::Database { source, context }
    }
}

// Conversion from validator errors
impl From<validator::ValidationErrors> for Error {
    fn from(err: validator::ValidationErrors) -> Self {
        Self::Validation(format!("Validation failed: {}", err))
    }
}

impl From<TlsError> for Error {
    fn from(error: TlsError) -> Self {
        Self::Config(error.to_string())
    }
}
