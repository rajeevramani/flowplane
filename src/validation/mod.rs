//! Minimal validation helpers for the Platform API abstraction.

use lazy_static::lazy_static;
use regex::Regex;
use validator::ValidationError;

pub mod business_rules;
pub mod requests;

lazy_static! {
    static ref HOST_REGEX: Regex = Regex::new(r"^[a-zA-Z0-9.-]+$").expect("valid host regex");
    /// Resource names: alphanumeric, hyphens, underscores, dots. Must start with alphanumeric.
    static ref RESOURCE_NAME_REGEX: Regex =
        Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._-]*$").expect("valid resource name regex");
}

/// Validate that the provided host/domain name matches Flowplane's basic expectations.
pub fn validate_host(host: &str) -> Result<(), ValidationError> {
    if !HOST_REGEX.is_match(host) {
        return Err(ValidationError::new("invalid_host"));
    }
    Ok(())
}

/// Validate a resource name (cluster, listener, route config, service, filter, etc.).
///
/// Names must:
/// - Be 1–100 characters long
/// - Start with an alphanumeric character
/// - Contain only alphanumeric characters, hyphens (`-`), underscores (`_`), and dots (`.`)
///
/// Returns a descriptive error message on failure.
pub fn validate_resource_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("Name cannot be empty".to_string());
    }
    if name.len() > 100 {
        return Err(format!("Name '{}' is too long ({} chars, max 100)", &name[..40], name.len()));
    }
    if !RESOURCE_NAME_REGEX.is_match(name) {
        return Err(format!(
            "Name '{}' contains invalid characters; \
             names must start with a letter or digit and contain only \
             letters, digits, hyphens, underscores, and dots",
            name
        ));
    }
    Ok(())
}

/// Validate that a port number is non-zero (within valid TCP range 1–65535).
///
/// Port 0 is technically valid in some OS contexts (ephemeral port) but not valid
/// as a target for upstream services or listeners.
pub fn validate_port_nonzero(port: u16) -> std::result::Result<(), String> {
    if port == 0 {
        return Err("Port 0 is not valid; port must be between 1 and 65535".to_string());
    }
    Ok(())
}

/// Validate an upstream string does not contain whitespace.
///
/// Upstreams must be in the form `[http[s]://]host:port[/path]` with no spaces.
pub fn validate_upstream(upstream: &str) -> std::result::Result<(), String> {
    if upstream.contains(char::is_whitespace) {
        return Err(format!(
            "Upstream '{}' contains whitespace; expected format: [http://]host:port[/path]",
            upstream
        ));
    }
    Ok(())
}

/// Convenience alias so callers can work directly with `validator::ValidationError`.
pub type Result<T, E = ValidationError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_name_valid() {
        assert!(validate_resource_name("my-cluster").is_ok());
        assert!(validate_resource_name("my_cluster").is_ok());
        assert!(validate_resource_name("my.cluster").is_ok());
        assert!(validate_resource_name("a").is_ok());
        assert!(validate_resource_name("Cluster1").is_ok());
        assert!(validate_resource_name("0-init").is_ok());
    }

    #[test]
    fn resource_name_special_chars_rejected() {
        assert!(validate_resource_name("test@!#").is_err());
        assert!(validate_resource_name("my cluster").is_err());
        assert!(validate_resource_name("name/slash").is_err());
        assert!(validate_resource_name("name:colon").is_err());
        assert!(validate_resource_name("name;semi").is_err());
        assert!(validate_resource_name("$dollar").is_err());
        assert!(validate_resource_name("name\ttab").is_err());
    }

    #[test]
    fn resource_name_must_start_alphanumeric() {
        assert!(validate_resource_name("-leading-hyphen").is_err());
        assert!(validate_resource_name("_leading-underscore").is_err());
        assert!(validate_resource_name(".leading-dot").is_err());
    }

    #[test]
    fn resource_name_empty_rejected() {
        assert!(validate_resource_name("").is_err());
    }

    #[test]
    fn resource_name_too_long_rejected() {
        let long_name = "a".repeat(101);
        assert!(validate_resource_name(&long_name).is_err());
        let ok_name = "a".repeat(100);
        assert!(validate_resource_name(&ok_name).is_ok());
    }

    #[test]
    fn port_zero_rejected() {
        assert!(validate_port_nonzero(0).is_err());
        assert!(validate_port_nonzero(1).is_ok());
        assert!(validate_port_nonzero(65535).is_ok());
    }

    #[test]
    fn upstream_with_spaces_rejected() {
        assert!(validate_upstream("localhost :8080").is_err());
        assert!(validate_upstream("localhost: 8080").is_err());
        assert!(validate_upstream("local host:8080").is_err());
        assert!(validate_upstream("http://host:8080 /path").is_err());
    }

    #[test]
    fn upstream_valid() {
        assert!(validate_upstream("localhost:8080").is_ok());
        assert!(validate_upstream("http://localhost:8080").is_ok());
        assert!(validate_upstream("https://api.example.com:443/v1").is_ok());
    }
}
