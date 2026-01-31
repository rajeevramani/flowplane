//! Internal API Error Types
//!
//! This module defines the unified error type for the internal API layer.
//! It provides conversions to both REST API errors and MCP errors.

use crate::api::error::ApiError;
use crate::errors::FlowplaneError;
use crate::mcp::error::McpError;
use thiserror::Error;

/// Internal API error type
///
/// This error type abstracts away the specific error formats of REST and MCP,
/// providing a unified interface for error handling in the internal API layer.
#[derive(Error, Debug)]
pub enum InternalError {
    /// Validation error - invalid input data
    #[error("Invalid input: {message}")]
    InvalidInput { message: String, field: Option<String> },

    /// Resource not found
    #[error("{resource} '{id}' not found")]
    NotFound { resource: String, id: String },

    /// Access forbidden - user lacks permission
    #[error("Forbidden: {message}")]
    Forbidden { message: String },

    /// Resource already exists
    #[error("{resource} '{id}' already exists")]
    AlreadyExists { resource: String, id: String },

    /// Resource is in use by other resources
    #[error("{resource} '{id}' is in use by: {}", dependencies.join(", "))]
    InUse { resource: String, id: String, dependencies: Vec<String> },

    /// General conflict error
    #[error("Conflict: {message}")]
    Conflict { message: String },

    /// Database operation failed
    #[error("Database error: {message}")]
    DatabaseError { message: String },

    /// Internal server error
    #[error("Internal error: {message}")]
    InternalError { message: String },

    /// Service unavailable
    #[error("Service unavailable: {message}")]
    ServiceUnavailable { message: String },
}

impl InternalError {
    /// Create a validation error
    pub fn validation(message: impl Into<String>) -> Self {
        Self::InvalidInput { message: message.into(), field: None }
    }

    /// Create a validation error with field name
    pub fn validation_field(message: impl Into<String>, field: impl Into<String>) -> Self {
        Self::InvalidInput { message: message.into(), field: Some(field.into()) }
    }

    /// Create a not found error
    pub fn not_found(resource: impl Into<String>, id: impl Into<String>) -> Self {
        Self::NotFound { resource: resource.into(), id: id.into() }
    }

    /// Create a forbidden error
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden { message: message.into() }
    }

    /// Create an already exists error
    pub fn already_exists(resource: impl Into<String>, id: impl Into<String>) -> Self {
        Self::AlreadyExists { resource: resource.into(), id: id.into() }
    }

    /// Create an in-use error
    pub fn in_use(
        resource: impl Into<String>,
        id: impl Into<String>,
        dependencies: Vec<String>,
    ) -> Self {
        Self::InUse { resource: resource.into(), id: id.into(), dependencies }
    }

    /// Create a general conflict error
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict { message: message.into() }
    }

    /// Create a database error
    pub fn database(message: impl Into<String>) -> Self {
        Self::DatabaseError { message: message.into() }
    }

    /// Create an internal error
    pub fn internal(message: impl Into<String>) -> Self {
        Self::InternalError { message: message.into() }
    }

    /// Create a service unavailable error
    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::ServiceUnavailable { message: message.into() }
    }
}

/// Convert InternalError to ApiError for REST handlers
impl From<InternalError> for ApiError {
    fn from(err: InternalError) -> Self {
        match err {
            InternalError::InvalidInput { message, field } => {
                if let Some(f) = field {
                    ApiError::BadRequest(format!("{}: {}", f, message))
                } else {
                    ApiError::BadRequest(message)
                }
            }
            InternalError::NotFound { resource, id } => {
                ApiError::NotFound(format!("{} with name '{}' not found", resource, id))
            }
            InternalError::Forbidden { message } => ApiError::Forbidden(message),
            InternalError::AlreadyExists { resource, id } => {
                ApiError::Conflict(format!("{} '{}' already exists", resource, id))
            }
            InternalError::InUse { resource, id, dependencies } => ApiError::Conflict(format!(
                "{} '{}' is in use by {}. Remove references before deleting.",
                resource,
                id,
                dependencies.join(", ")
            )),
            InternalError::Conflict { message } => ApiError::Conflict(message),
            InternalError::DatabaseError { message } => ApiError::Internal(message),
            InternalError::InternalError { message } => ApiError::Internal(message),
            InternalError::ServiceUnavailable { message } => ApiError::ServiceUnavailable(message),
        }
    }
}

/// Convert InternalError to McpError for MCP tools
impl From<InternalError> for McpError {
    fn from(err: InternalError) -> Self {
        match err {
            InternalError::InvalidInput { message, field } => {
                if let Some(f) = field {
                    McpError::InvalidParams(format!("{}: {}", f, message))
                } else {
                    McpError::InvalidParams(message)
                }
            }
            InternalError::NotFound { resource, id } => {
                McpError::ResourceNotFound(format!("{} '{}' not found", resource, id))
            }
            InternalError::Forbidden { message } => McpError::Forbidden(message),
            InternalError::AlreadyExists { resource, id } => {
                McpError::Conflict(format!("{} '{}' already exists", resource, id))
            }
            InternalError::InUse { resource, id, dependencies } => McpError::Conflict(format!(
                "{} '{}' is in use by {}. Remove references before deleting.",
                resource,
                id,
                dependencies.join(", ")
            )),
            InternalError::Conflict { message } => McpError::Conflict(message),
            InternalError::DatabaseError { message } => McpError::InternalError(message),
            InternalError::InternalError { message } => McpError::InternalError(message),
            InternalError::ServiceUnavailable { message } => McpError::InternalError(message),
        }
    }
}

/// Convert FlowplaneError to InternalError
impl From<FlowplaneError> for InternalError {
    fn from(err: FlowplaneError) -> Self {
        match err {
            FlowplaneError::Validation { message, .. } => InternalError::validation(message),
            FlowplaneError::NotFound { resource_type, id } => {
                InternalError::not_found(resource_type, id)
            }
            FlowplaneError::Conflict { message, .. } => {
                // Try to parse conflict message to extract resource info
                InternalError::InternalError { message }
            }
            FlowplaneError::ConstraintViolation { message, .. } => {
                InternalError::InternalError { message }
            }
            FlowplaneError::Auth { message, .. } => InternalError::forbidden(message),
            FlowplaneError::Database { context, .. } => InternalError::database(context),
            FlowplaneError::Config { message, .. }
            | FlowplaneError::Transport(message)
            | FlowplaneError::Internal { message, .. } => InternalError::internal(message),
            FlowplaneError::Io { context, .. } => InternalError::internal(context),
            FlowplaneError::Serialization { context, .. } => InternalError::validation(context),
            FlowplaneError::Xds { message, .. } => InternalError::internal(message),
            FlowplaneError::Http { message, .. } => InternalError::internal(message),
            FlowplaneError::RateLimit { message, .. } => {
                InternalError::service_unavailable(message)
            }
            FlowplaneError::Timeout { operation, duration_ms } => InternalError::internal(format!(
                "Operation '{}' timed out after {}ms",
                operation, duration_ms
            )),
            FlowplaneError::Parse { context, .. } => InternalError::validation(context),
            FlowplaneError::Sync { context } => InternalError::internal(context),
            FlowplaneError::Conversion { context, .. } => InternalError::validation(context),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_error_to_api() {
        let err = InternalError::validation("invalid name");
        let api_err: ApiError = err.into();
        match api_err {
            ApiError::BadRequest(msg) => assert!(msg.contains("invalid name")),
            _ => panic!("Expected BadRequest"),
        }
    }

    #[test]
    fn test_validation_error_with_field_to_api() {
        let err = InternalError::validation_field("must be non-empty", "name");
        let api_err: ApiError = err.into();
        match api_err {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("name"));
                assert!(msg.contains("must be non-empty"));
            }
            _ => panic!("Expected BadRequest"),
        }
    }

    #[test]
    fn test_not_found_error_to_api() {
        let err = InternalError::not_found("Cluster", "api-backend");
        let api_err: ApiError = err.into();
        match api_err {
            ApiError::NotFound(msg) => {
                assert!(msg.contains("Cluster"));
                assert!(msg.contains("api-backend"));
            }
            _ => panic!("Expected NotFound"),
        }
    }

    #[test]
    fn test_not_found_error_to_mcp() {
        let err = InternalError::not_found("Cluster", "api-backend");
        let mcp_err: McpError = err.into();
        match mcp_err {
            McpError::ResourceNotFound(msg) => {
                assert!(msg.contains("Cluster"));
                assert!(msg.contains("api-backend"));
            }
            _ => panic!("Expected ResourceNotFound"),
        }
    }

    #[test]
    fn test_forbidden_error_to_api() {
        let err = InternalError::forbidden("Cannot access team resource");
        let api_err: ApiError = err.into();
        match api_err {
            ApiError::Forbidden(msg) => assert!(msg.contains("Cannot access")),
            _ => panic!("Expected Forbidden"),
        }
    }

    #[test]
    fn test_already_exists_error_to_api() {
        let err = InternalError::already_exists("Cluster", "api-backend");
        let api_err: ApiError = err.into();
        match api_err {
            ApiError::Conflict(msg) => {
                assert!(msg.contains("already exists"));
                assert!(msg.contains("api-backend"));
            }
            _ => panic!("Expected Conflict"),
        }
    }

    #[test]
    fn test_in_use_error_to_mcp() {
        let err = InternalError::in_use(
            "Cluster",
            "api-backend",
            vec!["route-config-1".to_string(), "filter-1".to_string()],
        );
        let mcp_err: McpError = err.into();
        match mcp_err {
            McpError::Conflict(msg) => {
                assert!(msg.contains("in use"));
                assert!(msg.contains("route-config-1"));
                assert!(msg.contains("filter-1"));
            }
            _ => panic!("Expected Conflict"),
        }
    }
}
