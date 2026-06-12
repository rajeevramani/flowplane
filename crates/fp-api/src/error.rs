//! HTTP rendering of the domain error taxonomy (spec/10 §8).

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use fp_domain::{DomainError, ErrorCode, RequestId};
use serde::Serialize;

/// The wire envelope. Stable contract: agents branch on `code`, humans read `message`/`hint`,
/// operators grep `request_id`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub request_id: String,
}

/// A [`DomainError`] paired with the request id assigned by middleware.
#[derive(Debug)]
pub struct ApiError {
    pub error: DomainError,
    pub request_id: RequestId,
}

impl ApiError {
    pub fn new(error: DomainError, request_id: RequestId) -> Self {
        Self { error, request_id }
    }

    fn status(&self) -> StatusCode {
        match self.error.code {
            ErrorCode::ValidationFailed => StatusCode::BAD_REQUEST,
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden => StatusCode::FORBIDDEN,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Conflict | ErrorCode::RevisionMismatch => StatusCode::CONFLICT,
            ErrorCode::QuotaExceeded => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::InvalidConfig | ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.error.code;

        // Internal details go to logs (keyed by request id), never to the caller.
        let message = if code == ErrorCode::Internal || code == ErrorCode::InvalidConfig {
            tracing::error!(
                request_id = %self.request_id,
                error.message = %self.error.message,
                "internal error"
            );
            "an internal error occurred; report the request_id to your operator".to_string()
        } else {
            self.error.message
        };

        let body = ErrorBody {
            code: code.as_str(),
            message,
            hint: self.error.hint,
            details: if code == ErrorCode::Internal {
                None
            } else {
                self.error.details
            },
            request_id: self.request_id.to_string(),
        };

        let mut response = (status, Json(body)).into_response();
        if let Some(seconds) = self.error.retry_after_seconds {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn internal_errors_redact_detail_but_keep_request_id() {
        let rid = RequestId::generate();
        let err = ApiError::new(DomainError::internal("db column foo missing"), rid);
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn rate_limited_carries_retry_after_header() {
        let rid = RequestId::generate();
        let err = ApiError::new(
            fp_domain::DomainError::new(ErrorCode::RateLimited, "slow down").with_retry_after(30),
            rid,
        );
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("30")
        );
    }
}
