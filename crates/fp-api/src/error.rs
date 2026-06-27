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
            ErrorCode::ValidationFailed | ErrorCode::OrgSelectorRequired => StatusCode::BAD_REQUEST,
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

        // Internal/invalid-config details go to logs (keyed by request id), never to the caller.
        // The hint is logged too: for a misconfiguration (e.g. an unconfigured cert issuer) it
        // carries the actionable prerequisite the operator needs, which the redacted client
        // message omits (fpv2-86m.4 / #193 Obs-1).
        let message = if code == ErrorCode::Internal || code == ErrorCode::InvalidConfig {
            tracing::error!(
                request_id = %self.request_id,
                error.message = %self.error.message,
                error.hint = self.error.hint.as_deref().unwrap_or(""),
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
            details: if matches!(code, ErrorCode::Internal | ErrorCode::InvalidConfig) {
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
    use http_body_util::BodyExt;

    #[test]
    fn internal_errors_redact_detail_but_keep_request_id() {
        let rid = RequestId::generate();
        let err = ApiError::new(DomainError::internal("db column foo missing"), rid);
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn invalid_config_errors_redact_message_and_detail() {
        let rid = RequestId::generate();
        let err = ApiError::new(
            DomainError::invalid_config("tls private key missing")
                .with_details(serde_json::json!({"path": "/run/secrets/server.key"})),
            rid,
        );
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json["code"], "invalid_config");
        assert_eq!(json["request_id"], rid.to_string());
        assert_eq!(
            json["message"],
            "an internal error occurred; report the request_id to your operator"
        );
        assert!(json.get("details").is_none());
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
