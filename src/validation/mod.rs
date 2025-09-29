//! Minimal validation helpers for the Platform API abstraction.

use lazy_static::lazy_static;
use regex::Regex;
use validator::ValidationError;

pub mod business_rules;
pub mod requests;

lazy_static! {
    static ref HOST_REGEX: Regex = Regex::new(r"^[a-zA-Z0-9.-]+$").expect("valid host regex");
}

/// Validate that the provided host/domain name matches Flowplane's basic expectations.
pub fn validate_host(host: &str) -> Result<(), ValidationError> {
    if !HOST_REGEX.is_match(host) {
        return Err(ValidationError::new("invalid_host"));
    }
    Ok(())
}

/// Convenience alias so callers can work directly with `validator::ValidationError`.
pub type Result<T, E = ValidationError> = std::result::Result<T, E>;
