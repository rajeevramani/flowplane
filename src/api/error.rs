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
//! - `RateLimit` → 429 Too Many Requests (with Retry-After header)
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

use axum::{
    extract::{rejection::JsonRejection, FromRequest, Request},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{de::DeserializeOwned, Serialize};

use crate::auth::models::AuthError;
use crate::domain::SecretValidationError;
use crate::errors::FlowplaneError;

/// Custom JSON extractor that converts deserialization errors to JSON 400 responses
/// instead of Axum's default text/plain rejection.
pub struct JsonBody<T>(pub T);

impl<T, S> FromRequest<S> for JsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(JsonBody(value)),
            Err(rejection) => Err(match rejection {
                JsonRejection::JsonDataError(e) => {
                    ApiError::BadRequest(format!("Invalid JSON data: {}", e))
                }
                JsonRejection::JsonSyntaxError(e) => {
                    ApiError::BadRequest(format!("Invalid JSON syntax: {}", e))
                }
                JsonRejection::MissingJsonContentType(e) => {
                    ApiError::BadRequest(format!("Missing JSON content type: {}", e))
                }
                _ => ApiError::BadRequest("Invalid request body".to_string()),
            }),
        }
    }
}

/// Validate that a resource name doesn't contain control characters (including null bytes)
/// that would cause database errors.  Returns `Ok(())` if the name is safe for DB lookup,
/// or an appropriate `ApiError::NotFound` if it contains invalid characters.
pub fn validate_path_name(name: &str, resource_type: &str) -> Result<(), ApiError> {
    if name.contains('\0') || name.chars().any(|c| c.is_control()) {
        return Err(ApiError::NotFound(format!(
            "{} '{}' not found",
            resource_type,
            name.replace('\0', "").replace(|c: char| c.is_control(), "")
        )));
    }
    Ok(())
}

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
/// - `TooManyRequests`: 429 - Rate limit exceeded (with Retry-After header)
/// - `ServiceUnavailable`: 503 - Service temporarily unavailable (overload, maintenance)
/// - `Internal`: 500 - Internal server error (database errors, unexpected failures)
#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Conflict(String),
    NotFound(String),
    Unauthorized(String),
    Forbidden(String),
    /// Rate limit exceeded - includes retry_after_seconds for Retry-After header
    TooManyRequests {
        message: String,
        retry_after_seconds: u32,
    },
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
            ApiError::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
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
        use axum::http::header::RETRY_AFTER;

        let status = self.status_code();
        let (error_kind, message, retry_after) = match self {
            ApiError::BadRequest(msg) => ("bad_request", msg, None),
            ApiError::Conflict(msg) => ("conflict", msg, None),
            ApiError::NotFound(msg) => ("not_found", msg, None),
            ApiError::Unauthorized(msg) => ("unauthorized", msg, None),
            ApiError::Forbidden(msg) => ("forbidden", msg, None),
            ApiError::TooManyRequests { message, retry_after_seconds } => {
                ("too_many_requests", message, Some(retry_after_seconds))
            }
            ApiError::ServiceUnavailable(msg) => ("service_unavailable", msg, None),
            ApiError::Internal(msg) => {
                tracing::error!(error.internal = %msg, "Internal server error");
                ("internal_error", "An internal error occurred".to_string(), None)
            }
        };

        let body = Json(ErrorBody { error: error_kind, message });

        if let Some(seconds) = retry_after {
            (status, [(RETRY_AFTER, seconds.to_string())], body).into_response()
        } else {
            (status, body).into_response()
        }
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
            FlowplaneError::ConstraintViolation { message, source } => {
                // FK violation (23503) means a referenced resource doesn't exist → 400
                // All other constraints (unique 23505, not-null, check) → 409
                if let Some(db_err) = source.as_database_error() {
                    if db_err.code().is_some_and(|c| c.as_ref() == "23503") {
                        return ApiError::BadRequest(format!(
                            "Referenced resource does not exist: {}",
                            message
                        ));
                    }
                }
                ApiError::Conflict(message)
            }
            FlowplaneError::Database { context, .. } => ApiError::Internal(context),
            FlowplaneError::Config { message, .. }
            | FlowplaneError::Transport(message)
            | FlowplaneError::Internal { message, .. } => ApiError::Internal(message),
            FlowplaneError::Io { context, .. } => ApiError::Internal(context),
            FlowplaneError::Serialization { context, .. } => ApiError::BadRequest(context),
            FlowplaneError::Xds { message, .. } => ApiError::Internal(message),
            FlowplaneError::Http { message, .. } => ApiError::Internal(message),
            FlowplaneError::RateLimit { message, retry_after } => ApiError::TooManyRequests {
                message,
                retry_after_seconds: retry_after.unwrap_or(60) as u32,
            },
            FlowplaneError::Timeout { operation, duration_ms } => ApiError::Internal(format!(
                "Operation '{}' timed out after {}ms",
                operation, duration_ms
            )),
            FlowplaneError::Parse { context, .. } => ApiError::BadRequest(context),
            FlowplaneError::Sync { context } => ApiError::Internal(context),
            FlowplaneError::Conversion { context, .. } => ApiError::BadRequest(context),
            FlowplaneError::CrossOrgViolation { .. } => ApiError::Forbidden(err.to_string()),
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

/// Converts [`validator::ValidationErrors`] to API-layer [`ApiError`].
///
/// Formats field-level validation errors into a human-readable message:
/// `"Validation failed: field1: message1; field2: message2"`
impl From<validator::ValidationErrors> for ApiError {
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

        ApiError::BadRequest(format!("Validation failed: {}", message))
    }
}

/// Converts [`SecretValidationError`] to API-layer [`ApiError`].
///
/// Maps secret-specific validation errors to a 400 Bad Request response.
impl From<SecretValidationError> for ApiError {
    fn from(err: SecretValidationError) -> Self {
        ApiError::BadRequest(format!("Validation failed: {}", err))
    }
}

/// Converts [`McpServiceError`] to API-layer [`ApiError`].
///
/// Maps MCP service errors to appropriate HTTP status codes.
impl From<crate::services::McpServiceError> for ApiError {
    fn from(err: crate::services::McpServiceError) -> Self {
        match err {
            crate::services::McpServiceError::NotFound(msg) => ApiError::NotFound(msg),
            crate::services::McpServiceError::Validation(msg) => ApiError::BadRequest(msg),
            crate::services::McpServiceError::Database(e) => {
                ApiError::Internal(format!("Database error: {}", e))
            }
            crate::services::McpServiceError::Internal(msg) => ApiError::Internal(msg),
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

    /// Creates a bad request error (400).
    ///
    /// Use for validation errors and malformed input.
    pub fn validation<S: Into<String>>(msg: S) -> Self {
        ApiError::BadRequest(msg.into())
    }

    /// Creates an internal server error (500).
    ///
    /// Use for unexpected internal errors.
    pub fn internal<S: Into<String>>(msg: S) -> Self {
        ApiError::Internal(msg.into())
    }

    /// Creates a rate limit error (429) with Retry-After header.
    ///
    /// Use when rate limits are exceeded.
    pub fn rate_limited<S: Into<String>>(msg: S, retry_after_seconds: u32) -> Self {
        ApiError::TooManyRequests { message: msg.into(), retry_after_seconds }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn internal_error_does_not_leak_details() {
        let err = ApiError::Internal("secret SQL error: users table column mismatch".to_string());
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = response.into_body().collect().await.map(|b| b.to_bytes());
        let body_str = match body {
            Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            Err(_) => panic!("failed to read response body"),
        };

        assert!(
            !body_str.contains("SQL"),
            "response body should not contain internal details, got: {body_str}"
        );
        assert!(
            !body_str.contains("users table"),
            "response body should not contain table names, got: {body_str}"
        );
        assert!(
            body_str.contains("An internal error occurred"),
            "response body should contain generic message, got: {body_str}"
        );
    }

    #[tokio::test]
    async fn non_internal_errors_preserve_message() {
        let err = ApiError::NotFound("Cluster 'foo' not found".to_string());
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = response.into_body().collect().await.map(|b| b.to_bytes());
        let body_str = match body {
            Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            Err(_) => panic!("failed to read response body"),
        };

        assert!(
            body_str.contains("Cluster 'foo' not found"),
            "non-internal errors should preserve their message, got: {body_str}"
        );
    }

    #[tokio::test]
    async fn database_error_converts_to_generic_internal() {
        let fp_err = FlowplaneError::Database {
            context: "Failed to query agent_grants table".to_string(),
            source: sqlx::Error::RowNotFound,
        };
        let api_err: ApiError = fp_err.into();
        let response = api_err.into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = response.into_body().collect().await.map(|b| b.to_bytes());
        let body_str = match body {
            Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            Err(_) => panic!("failed to read response body"),
        };

        assert!(
            !body_str.contains("agent_grants"),
            "database errors should not leak table names, got: {body_str}"
        );
    }
}
