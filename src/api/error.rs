use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;

use crate::auth::models::AuthError;
use crate::errors::Error;

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

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
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

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        match err {
            Error::Validation(msg) => ApiError::BadRequest(msg),
            Error::NotFound(msg) => ApiError::NotFound(msg),
            Error::Database { source, context } => {
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
            Error::Config(msg) | Error::Transport(msg) | Error::Internal(msg) => {
                ApiError::Internal(msg)
            }
            Error::Io(err) => ApiError::Internal(err.to_string()),
        }
    }
}

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
    pub fn service_unavailable<S: Into<String>>(msg: S) -> Self {
        ApiError::ServiceUnavailable(msg.into())
    }

    pub fn unauthorized<S: Into<String>>(msg: S) -> Self {
        ApiError::Unauthorized(msg.into())
    }

    pub fn forbidden<S: Into<String>>(msg: S) -> Self {
        ApiError::Forbidden(msg.into())
    }
}
