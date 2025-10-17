//! API error types and conversions for HTTP responses.
//!
//! This module defines the API-layer error type [`ApiError`] which converts internal
//! [`FlowplaneError`] and [`AuthError`] types into appropriate HTTP responses with
//! standardized JSON error bodies.
//!
//! # Error Mapping
//!
//! Internal errors are mapped to HTTP status codes:
//! - `Validation` → 400 Bad Request
//! - `NotFound` → 404 Not Found
//! - `Conflict` → 409 Conflict
//! - `Auth` → 401 Unauthorized
//! - `Database` → 409 Conflict (constraint violations) or 500 Internal Server Error
//! - `Config`, `Internal`, `Xds`, `Http` → 500 Internal Server Error
//! - `RateLimit` → 503 Service Unavailable
//! - `Timeout` → 500 Internal Server Error
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::api::error::ApiError;
//! use flowplane::errors::FlowplaneError;
//!
//! // Internal error gets converted to API error automatically
//! let internal_err = FlowplaneError::not_found("Listener", "123");
//! let api_err: ApiError = internal_err.into();
//!
//! // Returns: 404 Not Found with JSON body: {"error": "not_found", "message": "Listener with ID '123' not found"}
//! ```

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;

use crate::auth::models::AuthError;
use crate::errors::FlowplaneError;

/// API-layer error type for HTTP responses.
///
/// Represents errors that can be returned from HTTP handlers, with appropriate
/// status codes and JSON error bodies. Automatically converts from internal
/// [`FlowplaneError`] and [`AuthError`] types.
///
/// # Variants
///
/// - `BadRequest`: 400 - Invalid request (validation failures, malformed input)
/// - `Conflict`: 409 - Resource conflict (duplicate names, constraint violations)
/// - `NotFound`: 404 - Resource not found
/// - `Unauthorized`: 401 - Authentication required or failed
/// - `Forbidden`: 403 - Authenticated but insufficient permissions
/// - `ServiceUnavailable`: 503 - Service temporarily unavailable (rate limits, overload)
/// - `Internal`: 500 - Internal server error (database errors, unexpected failures)
#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Conflict(String),
    NotFound(String),
    Unauthorized(String),
    Forbidden(String),
    ServiceUnavailable(String),
    Internal(String),
}

impl ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// JSON error response body.
///
/// Standardized error format returned by all API endpoints:
/// ```json
/// {
///   "error": "not_found",
///   "message": "Listener with ID '123' not found"
/// }
/// ```
#[derive(Serialize)]
struct ErrorBody {
    /// Error type identifier (e.g., "not_found", "bad_request")
    error: &'static str,
    /// Human-readable error message
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = self.status_code();
        let error_kind = match self {
            ApiError::BadRequest(_) => "bad_request",
            ApiError::Conflict(_) => "conflict",
            ApiError::NotFound(_) => "not_found",
            ApiError::Unauthorized(_) => "unauthorized",
            ApiError::Forbidden(_) => "forbidden",
            ApiError::ServiceUnavailable(_) => "service_unavailable",
            ApiError::Internal(_) => "internal_error",
        };

        let message = match self {
            ApiError::BadRequest(msg)
            | ApiError::Conflict(msg)
            | ApiError::NotFound(msg)
            | ApiError::Unauthorized(msg)
            | ApiError::Forbidden(msg)
            | ApiError::ServiceUnavailable(msg)
            | ApiError::Internal(msg) => msg,
        };

        (status, Json(ErrorBody { error: error_kind, message })).into_response()
    }
}

/// Converts internal [`FlowplaneError`] to API-layer [`ApiError`].
///
/// Maps internal error types to appropriate HTTP status codes and error messages.
/// Database constraint violations are detected and converted to `Conflict` errors.
impl From<FlowplaneError> for ApiError {
    fn from(err: FlowplaneError) -> Self {
        match err {
            FlowplaneError::Validation { message, .. } => ApiError::BadRequest(message),
            FlowplaneError::NotFound { resource_type, id } => {
                ApiError::NotFound(format!("{} with ID '{}' not found", resource_type, id))
            }
            FlowplaneError::Conflict { message, .. } => ApiError::Conflict(message),
            FlowplaneError::Auth { message, .. } => ApiError::Unauthorized(message),
            FlowplaneError::Database { source, context } => {
                if let Some(db_err) = source.as_database_error() {
                    if let Some(code) = db_err.code() {
                        if code.as_ref() == "2067" || code.as_ref().starts_with("SQLITE_CONSTRAINT")
                        {
                            return ApiError::Conflict(context);
                        }
                    }
                }
                ApiError::Internal(context)
            }
            FlowplaneError::Config { message, .. }
            | FlowplaneError::Transport(message)
            | FlowplaneError::Internal { message, .. } => ApiError::Internal(message),
            FlowplaneError::Io { context, .. } => ApiError::Internal(context),
            FlowplaneError::Serialization { context, .. } => ApiError::BadRequest(context),
            FlowplaneError::Xds { message, .. } => ApiError::Internal(message),
            FlowplaneError::Http { message, .. } => ApiError::Internal(message),
            FlowplaneError::RateLimit { message, .. } => ApiError::ServiceUnavailable(message),
            FlowplaneError::Timeout { operation, duration_ms } => ApiError::Internal(format!(
                "Operation '{}' timed out after {}ms",
                operation, duration_ms
            )),
        }
    }
}

/// Converts authentication [`AuthError`] to API-layer [`ApiError`].
///
/// Maps authentication failures to appropriate HTTP 401/403 responses.
impl From<AuthError> for ApiError {
    fn from(err: AuthError) -> Self {
        match err {
            AuthError::MissingBearer => {
                ApiError::Unauthorized("Unauthorized: missing bearer token".to_string())
            }
            AuthError::MalformedBearer => {
                ApiError::Unauthorized("Unauthorized: malformed bearer token".to_string())
            }
            AuthError::TokenNotFound => {
                ApiError::Unauthorized("Unauthorized: token not found".to_string())
            }
            AuthError::InactiveToken => {
                ApiError::Unauthorized("Unauthorized: token is inactive".to_string())
            }
            AuthError::ExpiredToken => {
                ApiError::Unauthorized("Unauthorized: token has expired".to_string())
            }
            AuthError::Forbidden => {
                ApiError::Forbidden("Forbidden: insufficient permissions".to_string())
            }
            AuthError::Persistence(err) => {
                ApiError::Internal(format!("Authentication error: {}", err))
            }
        }
    }
}

impl ApiError {
    /// Creates a service unavailable error (503).
    ///
    /// Use for rate limiting, circuit breakers, or temporary overload conditions.
    pub fn service_unavailable<S: Into<String>>(msg: S) -> Self {
        ApiError::ServiceUnavailable(msg.into())
    }

    /// Creates an unauthorized error (401).
    ///
    /// Use for missing or invalid authentication credentials.
    pub fn unauthorized<S: Into<String>>(msg: S) -> Self {
        ApiError::Unauthorized(msg.into())
    }

    /// Creates a forbidden error (403).
    ///
    /// Use when authenticated user lacks required permissions.
    pub fn forbidden<S: Into<String>>(msg: S) -> Self {
        ApiError::Forbidden(msg.into())
    }
}
