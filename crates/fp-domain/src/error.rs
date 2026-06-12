//! The single error taxonomy (spec/10 §8).
//!
//! Every failure that can cross an API boundary is a [`DomainError`] carrying a stable
//! machine-readable [`ErrorCode`], a human message, and — wherever we can say it — a `hint`
//! telling the caller what to do next. Surfaces add transport concerns (HTTP status, JSON-RPC
//! code, CLI exit code) by mapping `ErrorCode`; they never invent their own failure shapes.

use serde::{Deserialize, Serialize};

/// Closed set of machine-actionable error codes. Serialized as `snake_case` strings; the set
/// is part of the public API contract (documented in OpenAPI) and may only grow, never change
/// meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Request was syntactically or semantically invalid.
    ValidationFailed,
    /// Authentication missing or invalid.
    Unauthorized,
    /// Authenticated but not permitted; message names the missing (resource, action).
    Forbidden,
    /// Resource does not exist *within the caller's visibility* (cross-tenant existence is
    /// deliberately indistinguishable from absence — spec/08a).
    NotFound,
    /// Uniqueness or state conflict (duplicate name, illegal lifecycle transition).
    Conflict,
    /// Optimistic-concurrency failure: the resource changed since the revision the caller read.
    RevisionMismatch,
    /// Per-tenant quota exceeded.
    QuotaExceeded,
    /// Request rate limit exceeded; pairs with a retry-after.
    RateLimited,
    /// Payload exceeds a configured size limit.
    PayloadTooLarge,
    /// Server-side configuration problem detected at startup or reload.
    InvalidConfig,
    /// A dependency (database, IdP, provider) is unavailable; safe to retry.
    Unavailable,
    /// Unexpected internal failure. Details are logged, never returned.
    Internal,
}

impl ErrorCode {
    /// Stable wire string (matches the serde representation).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ValidationFailed => "validation_failed",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::RevisionMismatch => "revision_mismatch",
            Self::QuotaExceeded => "quota_exceeded",
            Self::RateLimited => "rate_limited",
            Self::PayloadTooLarge => "payload_too_large",
            Self::InvalidConfig => "invalid_config",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        }
    }

    /// Whether a client may retry the identical request without modification.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Unavailable)
    }
}

/// The error type carried through every layer.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct DomainError {
    pub code: ErrorCode,
    /// Human-readable statement of fact. Must not contain secrets or cross-tenant data.
    pub message: String,
    /// What the caller should do next (copy-pasteable command where possible).
    pub hint: Option<String>,
    /// Optional structured context safe for the caller to see.
    pub details: Option<serde_json::Value>,
    /// Seconds after which a retry may succeed (rate limiting / unavailability).
    pub retry_after_seconds: Option<u32>,
}

impl DomainError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            details: None,
            retry_after_seconds: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn with_retry_after(mut self, seconds: u32) -> Self {
        self.retry_after_seconds = Some(seconds);
        self
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ValidationFailed, message)
    }

    pub fn not_found(resource_kind: &str, handle: &str) -> Self {
        Self::new(
            ErrorCode::NotFound,
            format!("{resource_kind} \"{handle}\" not found"),
        )
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    pub fn invalid_config(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidConfig, message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, message)
    }

    /// Internal failure: `message` is for logs/operators; the API layer replaces it with a
    /// generic message so internals never leak (spec/01 baseline kept in v2).
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

pub type DomainResult<T> = Result<T, DomainError>;

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialize_snake_case_and_match_as_str() {
        for code in [
            ErrorCode::ValidationFailed,
            ErrorCode::Unauthorized,
            ErrorCode::Forbidden,
            ErrorCode::NotFound,
            ErrorCode::Conflict,
            ErrorCode::RevisionMismatch,
            ErrorCode::QuotaExceeded,
            ErrorCode::RateLimited,
            ErrorCode::PayloadTooLarge,
            ErrorCode::InvalidConfig,
            ErrorCode::Unavailable,
            ErrorCode::Internal,
        ] {
            let json = serde_json::to_value(code).unwrap_or_default();
            assert_eq!(json, serde_json::Value::String(code.as_str().to_string()));
        }
    }

    #[test]
    fn builder_attaches_hint_and_retry() {
        let err = DomainError::unavailable("database is unreachable")
            .with_hint("check DATABASE_URL and that PostgreSQL is running")
            .with_retry_after(5);
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert_eq!(err.retry_after_seconds, Some(5));
        assert!(err.code.is_retryable());
        assert!(err.hint.is_some());
    }

    #[test]
    fn not_found_message_names_resource_and_handle() {
        let err = DomainError::not_found("cluster", "payments-db");
        assert_eq!(err.message, "cluster \"payments-db\" not found");
        assert!(!err.code.is_retryable());
    }
}
