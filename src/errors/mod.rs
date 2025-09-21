//! # Error Handling
//!
//! This module provides error handling for the Magaya control plane.
//! It defines custom error types using `thiserror` for the minimal XDS server.

/// Custom result type for Magaya operations
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for the Magaya control plane
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
}

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
}
