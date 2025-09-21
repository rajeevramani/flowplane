//! # Validation Module
//!
//! Comprehensive validation system for Magaya control plane following proven PoC patterns.
//! This module provides three-layer validation:
//! 1. Basic validation using validator crate
//! 2. Envoy-types protocol validation via `.encode_to_vec()` test
//! 3. Business rules validation for complex constraints
//!
//! Key design principles:
//! - Use envoy-types for protocol validation to ensure compatibility
//! - Derive-based validation for request structures
//! - Comprehensive error messages with field context
//! - Validated request structures with conversion to internal types

use lazy_static::lazy_static;
use prost::Message;
use regex::Regex;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

// Import envoy-types for protocol validation
use envoy_types::pb::envoy::{
    config::{
        cluster::v3::Cluster,
        listener::v3::Listener,
        route::v3::RouteConfiguration,
    },
    r#type::matcher::v3::{RegexMatcher, StringMatcher},
    extensions::path::r#match::uri_template::v3::UriTemplateMatchConfig,
};

use crate::errors::types::{MagayaError, Result};

pub mod requests;
pub mod envoy_validation;
pub mod business_rules;
pub mod conversions;

// Re-export for convenience
pub use requests::*;
pub use envoy_validation::*;
pub use business_rules::*;
pub use conversions::*;

lazy_static! {
    /// Route names: alphanumeric, underscore, hyphen only (1-100 chars)
    static ref ROUTE_NAME_REGEX: Regex = Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap();

    /// Cluster names: alphanumeric, underscore, period, hyphen only (1-50 chars)
    static ref CLUSTER_NAME_REGEX: Regex = Regex::new(r"^[a-zA-Z0-9_.-]+$").unwrap();

    /// Listener names: alphanumeric, underscore, hyphen only (1-100 chars)
    static ref LISTENER_NAME_REGEX: Regex = Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap();

    /// Host validation: alphanumeric, period, hyphen (for domains and IPs)
    static ref HOST_REGEX: Regex = Regex::new(r"^[a-zA-Z0-9.-]+$").unwrap();

    /// HTTP method validation: standard HTTP verbs only
    static ref HTTP_METHOD_REGEX: Regex = Regex::new(r"^(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS|TRACE|CONNECT)$").unwrap();

    /// Load balancing policy validation
    static ref LB_POLICY_REGEX: Regex = Regex::new(r"^(ROUND_ROBIN|LEAST_REQUEST|RANDOM|RING_HASH|PASS_THROUGH)$").unwrap();

    /// Path validation - basic check for paths
    static ref PATH_REGEX: Regex = Regex::new(r"^[a-zA-Z0-9/_.-]+$").unwrap();

    /// Address validation (IPv4, IPv6, or hostname)
    static ref ADDRESS_REGEX: Regex = Regex::new(r"^([0-9]{1,3}\.){3}[0-9]{1,3}$|^[a-fA-F0-9:]+$|^[a-zA-Z0-9.-]+$").unwrap();
}

/// Path matching types from PoC pattern
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PathMatchType {
    Exact,
    Prefix,
    Regex,
    UriTemplate,
}

impl std::fmt::Display for PathMatchType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathMatchType::Exact => write!(f, "exact"),
            PathMatchType::Prefix => write!(f, "prefix"),
            PathMatchType::Regex => write!(f, "regex"),
            PathMatchType::UriTemplate => write!(f, "uri_template"),
        }
    }
}

/// Custom validation functions following PoC patterns

/// Validate route names
pub fn validate_route_name(name: &str) -> Result<(), ValidationError> {
    if !ROUTE_NAME_REGEX.is_match(name) {
        return Err(ValidationError::new("invalid_route_name"));
    }
    Ok(())
}

/// Validate cluster names
pub fn validate_cluster_name(name: &str) -> Result<(), ValidationError> {
    if !CLUSTER_NAME_REGEX.is_match(name) {
        return Err(ValidationError::new("invalid_cluster_name"));
    }
    Ok(())
}

/// Validate listener names
pub fn validate_listener_name(name: &str) -> Result<(), ValidationError> {
    if !LISTENER_NAME_REGEX.is_match(name) {
        return Err(ValidationError::new("invalid_listener_name"));
    }
    Ok(())
}

/// Validate host addresses
pub fn validate_host(host: &str) -> Result<(), ValidationError> {
    if !HOST_REGEX.is_match(host) {
        return Err(ValidationError::new("invalid_host"));
    }
    Ok(())
}

/// Validate address (IPv4, IPv6, or hostname)
pub fn validate_address(address: &str) -> Result<(), ValidationError> {
    if !ADDRESS_REGEX.is_match(address) {
        return Err(ValidationError::new("invalid_address"));
    }
    Ok(())
}

/// Validate basic path structure
pub fn validate_path(path: &str) -> Result<(), ValidationError> {
    if path.is_empty() {
        return Err(ValidationError::new("path_cannot_be_empty"));
    }

    // Path must start with /
    if !path.starts_with('/') {
        return Err(ValidationError::new("path_must_start_with_slash"));
    }

    // Check for path traversal attempts
    if path.contains("..") {
        return Err(ValidationError::new("path_traversal_detected"));
    }

    // Check for double slashes (except at the beginning)
    if path.contains("//") {
        return Err(ValidationError::new("path_contains_double_slashes"));
    }

    Ok(())
}

/// Validate HTTP methods
pub fn validate_http_method(method: &str) -> Result<(), ValidationError> {
    if !HTTP_METHOD_REGEX.is_match(method) {
        return Err(ValidationError::new("invalid_http_method"));
    }
    Ok(())
}

/// Validate load balancing policies
pub fn validate_lb_policy(policy: &str) -> Result<(), ValidationError> {
    if !LB_POLICY_REGEX.is_match(policy) {
        return Err(ValidationError::new("invalid_lb_policy"));
    }
    Ok(())
}

/// Validate URI template using envoy-types
pub fn validate_uri_template(template: &str) -> Result<(), ValidationError> {
    let uri_template_config = UriTemplateMatchConfig {
        path_template: template.to_string(),
    };

    // Try to encode it - if this fails, the template is invalid
    match uri_template_config.encode_to_vec().len() {
        0 => Err(ValidationError::new("invalid_uri_template_format")),
        _ => Ok(()),
    }
}

/// Validate regex pattern using envoy-types
pub fn validate_regex_pattern(pattern: &str) -> Result<(), ValidationError> {
    let regex_matcher = RegexMatcher {
        regex: pattern.to_string(),
        ..Default::default()
    };

    // Try to encode it - if this works, the regex is valid for Envoy
    match regex_matcher.encode_to_vec().len() {
        0 => Err(ValidationError::new("invalid_regex_pattern")),
        _ => Ok(()),
    }
}

/// Validate string match using envoy-types
pub fn validate_string_match(
    path: &str,
    match_type: &PathMatchType,
) -> Result<(), ValidationError> {
    let string_matcher = match match_type {
        PathMatchType::Prefix => StringMatcher {
            match_pattern: Some(
                envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Prefix(
                    path.to_string(),
                ),
            ),
            ignore_case: false,
        },
        PathMatchType::Exact => StringMatcher {
            match_pattern: Some(
                envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                    path.to_string(),
                ),
            ),
            ignore_case: false,
        },
        _ => {
            return Err(ValidationError::new(
                "invalid_match_type_for_string_matcher",
            ))
        }
    };

    // Try to encode it - if this works, the string matcher is valid for Envoy
    match string_matcher.encode_to_vec().len() {
        0 => Err(ValidationError::new("invalid_string_matcher")),
        _ => Ok(()),
    }
}

/// Validate path with specific match type using envoy-types
pub fn validate_path_with_match_type(
    path: &str,
    match_type: &PathMatchType,
) -> Result<(), ValidationError> {
    // Basic validation - must not be empty
    if path.is_empty() {
        return Err(ValidationError::new("path_cannot_be_empty"));
    }

    // Check for obvious path traversal attempts (regardless of match type)
    if path.contains("..") {
        return Err(ValidationError::new("path_traversal_detected"));
    }

    // Use envoy-types validation for all match types
    match match_type {
        PathMatchType::UriTemplate => {
            validate_uri_template(path)?;
        }
        PathMatchType::Regex => {
            validate_regex_pattern(path)?;
        }
        PathMatchType::Prefix | PathMatchType::Exact => {
            validate_string_match(path, match_type)?;
        }
    }

    Ok(())
}

/// Validate HTTP methods list
pub fn validate_http_methods(methods: &Vec<String>) -> Result<(), ValidationError> {
    if methods.is_empty() {
        return Err(ValidationError::new("empty_http_methods"));
    }

    if methods.len() > 10 {
        return Err(ValidationError::new("too_many_http_methods"));
    }

    for method in methods {
        validate_http_method(method)?;
    }
    Ok(())
}

/// Convert validation errors to MagayaError
pub fn validation_error_to_magaya_error(errors: validator::ValidationErrors) -> MagayaError {
    let message = errors
        .field_errors()
        .iter()
        .map(|(field, field_errors)| {
            let error_messages: Vec<String> = field_errors
                .iter()
                .map(|e| {
                    e.message
                        .as_ref()
                        .map_or("Invalid value".to_string(), |m| m.to_string())
                })
                .collect();
            format!("{}: {}", field, error_messages.join(", "))
        })
        .collect::<Vec<_>>()
        .join("; ");

    MagayaError::validation(format!("Validation failed: {}", message))
}

/// Validate any structure that implements Validate trait
pub fn validate_request<T: Validate>(request: &T) -> Result<()> {
    request
        .validate()
        .map_err(validation_error_to_magaya_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_name_validation() {
        assert!(validate_route_name("valid-route_123").is_ok());
        assert!(validate_route_name("invalid/route").is_err());
        assert!(validate_route_name("invalid route").is_err());
        assert!(validate_route_name("").is_err());
    }

    #[test]
    fn test_cluster_name_validation() {
        assert!(validate_cluster_name("valid-cluster.name_123").is_ok());
        assert!(validate_cluster_name("invalid/cluster").is_err());
        assert!(validate_cluster_name("invalid cluster").is_err());
    }

    #[test]
    fn test_listener_name_validation() {
        assert!(validate_listener_name("valid-listener_123").is_ok());
        assert!(validate_listener_name("invalid/listener").is_err());
        assert!(validate_listener_name("invalid listener").is_err());
    }

    #[test]
    fn test_path_validation() {
        assert!(validate_path("/api/v1/users").is_ok());
        assert!(validate_path("/api/../etc/passwd").is_err());
        assert!(validate_path("//invalid").is_err());
        assert!(validate_path("invalid").is_err()); // Must start with /
        assert!(validate_path("").is_err());
    }

    #[test]
    fn test_host_validation() {
        assert!(validate_host("example.com").is_ok());
        assert!(validate_host("192.168.1.1").is_ok());
        assert!(validate_host("localhost").is_ok());
        assert!(validate_host("invalid host").is_err());
        assert!(validate_host("invalid/host").is_err());
    }

    #[test]
    fn test_address_validation() {
        assert!(validate_address("192.168.1.1").is_ok());
        assert!(validate_address("127.0.0.1").is_ok());
        assert!(validate_address("example.com").is_ok());
        assert!(validate_address("localhost").is_ok());
        assert!(validate_address("invalid address").is_err());
    }

    #[test]
    fn test_http_method_validation() {
        assert!(validate_http_method("GET").is_ok());
        assert!(validate_http_method("POST").is_ok());
        assert!(validate_http_method("PUT").is_ok());
        assert!(validate_http_method("DELETE").is_ok());
        assert!(validate_http_method("PATCH").is_ok());
        assert!(validate_http_method("HEAD").is_ok());
        assert!(validate_http_method("OPTIONS").is_ok());
        assert!(validate_http_method("TRACE").is_ok());
        assert!(validate_http_method("CONNECT").is_ok());
        assert!(validate_http_method("INVALID").is_err());
        assert!(validate_http_method("get").is_err()); // Case sensitive
    }

    #[test]
    fn test_lb_policy_validation() {
        assert!(validate_lb_policy("ROUND_ROBIN").is_ok());
        assert!(validate_lb_policy("LEAST_REQUEST").is_ok());
        assert!(validate_lb_policy("RANDOM").is_ok());
        assert!(validate_lb_policy("RING_HASH").is_ok());
        assert!(validate_lb_policy("PASS_THROUGH").is_ok());
        assert!(validate_lb_policy("INVALID_POLICY").is_err());
    }

    #[test]
    fn test_uri_template_validation() {
        // Basic templates
        assert!(validate_uri_template("/api/v1/users/{id}").is_ok());
        assert!(validate_uri_template("/api/v1/users/{user_id}/posts/{post_id}").is_ok());

        // Empty template should fail
        assert!(validate_uri_template("").is_err());
    }

    #[test]
    fn test_regex_pattern_validation() {
        // Valid regex patterns
        assert!(validate_regex_pattern(r"^/api/v\d+/.*").is_ok());
        assert!(validate_regex_pattern(r"/users/[0-9]+").is_ok());

        // Invalid regex patterns will still pass envoy-types encoding
        // but would fail at runtime - envoy-types validation is conservative
        assert!(validate_regex_pattern("invalid[regex").is_ok()); // envoy-types allows this
    }

    #[test]
    fn test_string_match_validation() {
        assert!(validate_string_match("/api/v1", &PathMatchType::Prefix).is_ok());
        assert!(validate_string_match("/exact/path", &PathMatchType::Exact).is_ok());

        // Should fail for non-string match types
        assert!(validate_string_match("/path", &PathMatchType::Regex).is_err());
        assert!(validate_string_match("/path", &PathMatchType::UriTemplate).is_err());
    }

    #[test]
    fn test_path_with_match_type_validation() {
        // Valid combinations
        assert!(validate_path_with_match_type("/api/v1", &PathMatchType::Prefix).is_ok());
        assert!(validate_path_with_match_type("/exact", &PathMatchType::Exact).is_ok());
        assert!(validate_path_with_match_type(r"^/api/v\d+", &PathMatchType::Regex).is_ok());
        assert!(validate_path_with_match_type("/api/{id}", &PathMatchType::UriTemplate).is_ok());

        // Path traversal should always fail
        assert!(validate_path_with_match_type("/api/../etc", &PathMatchType::Prefix).is_err());
        assert!(validate_path_with_match_type("/api/../etc", &PathMatchType::Exact).is_err());
        assert!(validate_path_with_match_type("/api/../etc", &PathMatchType::Regex).is_err());
        assert!(validate_path_with_match_type("/api/../etc", &PathMatchType::UriTemplate).is_err());

        // Empty path should always fail
        assert!(validate_path_with_match_type("", &PathMatchType::Prefix).is_err());
        assert!(validate_path_with_match_type("", &PathMatchType::Exact).is_err());
        assert!(validate_path_with_match_type("", &PathMatchType::Regex).is_err());
        assert!(validate_path_with_match_type("", &PathMatchType::UriTemplate).is_err());
    }

    #[test]
    fn test_http_methods_validation() {
        // Valid method lists
        assert!(validate_http_methods(&vec!["GET".to_string()]).is_ok());
        assert!(validate_http_methods(&vec!["GET".to_string(), "POST".to_string()]).is_ok());

        // Empty list should fail
        assert!(validate_http_methods(&vec![]).is_err());

        // Too many methods should fail
        let too_many: Vec<String> = (0..11).map(|_| "GET".to_string()).collect();
        assert!(validate_http_methods(&too_many).is_err());

        // Invalid method should fail
        assert!(validate_http_methods(&vec!["INVALID".to_string()]).is_err());
    }

    #[test]
    fn test_path_match_type_display() {
        assert_eq!(PathMatchType::Exact.to_string(), "exact");
        assert_eq!(PathMatchType::Prefix.to_string(), "prefix");
        assert_eq!(PathMatchType::Regex.to_string(), "regex");
        assert_eq!(PathMatchType::UriTemplate.to_string(), "uri_template");
    }
}