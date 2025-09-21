//! # Error Handling
//!
//! This module provides comprehensive error handling for the Magaya control plane.
//! It defines custom error types using `thiserror` and provides consistent error
//! handling patterns throughout the application.

pub mod types;

pub use types::{MagayaError, Result};

/// Error context for adding additional information to errors
pub trait ErrorContext<T> {
    /// Add context to an error
    fn with_context<F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> String;
}

impl<T, E> ErrorContext<T> for std::result::Result<T, E>
where
    E: Into<MagayaError>,
{
    fn with_context<F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> String,
    {
        self.map_err(|e| {
            let mut error = e.into();
            error.add_context(f());
            error
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::MagayaError;

    #[test]
    fn test_error_context() {
        let result: std::result::Result<(), std::io::Error> =
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));

        let error = result
            .with_context(|| "Failed to read configuration file".to_string())
            .unwrap_err();

        match error {
            MagayaError::Io { source: _, context } => {
                assert!(context.contains("Failed to read configuration file"));
            }
            _ => panic!("Expected IoError"),
        }
    }
}