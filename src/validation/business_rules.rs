//! # Business Rules Validation
//!
//! This module contains business-specific validation rules that go beyond
//! basic field validation and envoy-types protocol validation. These rules
//! enforce Magaya control plane specific constraints and policies.

use crate::errors::types::{MagayaError, Result};
use crate::validation::{PathMatchType, validate_path_with_match_type};
use validator::ValidationError;

/// Circuit breaker configuration validation
pub fn validate_circuit_breaker_config(
    max_connections: Option<u32>,
    max_pending_requests: Option<u32>,
    max_requests: Option<u32>,
    max_retries: Option<u32>,
) -> Result<()> {
    if let Some(max_conn) = max_connections {
        if max_conn == 0 || max_conn > 10000 {
            return Err(MagayaError::validation_field(
                "Max connections must be between 1 and 10000",
                "max_connections"
            ));
        }
    }

    if let Some(max_pending) = max_pending_requests {
        if max_pending == 0 || max_pending > 10000 {
            return Err(MagayaError::validation_field(
                "Max pending requests must be between 1 and 10000",
                "max_pending_requests"
            ));
        }
    }

    if let Some(max_req) = max_requests {
        if max_req == 0 || max_req > 10000 {
            return Err(MagayaError::validation_field(
                "Max requests must be between 1 and 10000",
                "max_requests"
            ));
        }
    }

    if let Some(max_ret) = max_retries {
        if max_ret > 10 {
            return Err(MagayaError::validation_field(
                "Max retries must be 10 or less",
                "max_retries"
            ));
        }
    }

    Ok(())
}

/// Validate route path and rewrite combinations
pub fn validate_route_path_rewrite_compatibility(
    path: &str,
    path_match_type: &PathMatchType,
    prefix_rewrite: &Option<String>,
    uri_template_rewrite: &Option<String>,
) -> Result<()> {
    // Validate path with match type first
    validate_path_with_match_type(path, path_match_type)
        .map_err(|e| MagayaError::validation_field(
            format!("Path validation failed: {}", e.message.unwrap_or_default()),
            "path"
        ))?;

    // Business rule: URI template rewrite only with URI template match type
    if uri_template_rewrite.is_some() && *path_match_type != PathMatchType::UriTemplate {
        return Err(MagayaError::validation(
            "URI template rewrite can only be used with URI template path matching"
        ));
    }

    // Business rule: Prefix rewrite not with URI template match type
    if prefix_rewrite.is_some() && *path_match_type == PathMatchType::UriTemplate {
        return Err(MagayaError::validation(
            "Prefix rewrite cannot be used with URI template path matching"
        ));
    }

    // Business rule: Cannot have both prefix and URI template rewrite
    if prefix_rewrite.is_some() && uri_template_rewrite.is_some() {
        return Err(MagayaError::validation(
            "Cannot specify both prefix rewrite and URI template rewrite"
        ));
    }

    // Validate prefix rewrite format
    if let Some(prefix) = prefix_rewrite {
        if !prefix.starts_with('/') {
            return Err(MagayaError::validation_field(
                "Prefix rewrite must start with '/'",
                "prefix_rewrite"
            ));
        }
        if prefix.contains("..") {
            return Err(MagayaError::validation_field(
                "Prefix rewrite cannot contain '..' (path traversal)",
                "prefix_rewrite"
            ));
        }
    }

    // Validate URI template rewrite format
    if let Some(template) = uri_template_rewrite {
        if template.is_empty() {
            return Err(MagayaError::validation_field(
                "URI template rewrite cannot be empty",
                "uri_template_rewrite"
            ));
        }
        // Basic template validation - should contain at least one variable
        if !template.contains('{') || !template.contains('}') {
            return Err(MagayaError::validation_field(
                "URI template rewrite must contain at least one variable (e.g., {id})",
                "uri_template_rewrite"
            ));
        }
    }

    Ok(())
}

/// Validate cluster endpoint weights
pub fn validate_endpoint_weights(weights: &[Option<u32>]) -> Result<()> {
    let mut total_weight = 0u32;
    let mut has_weighted = false;
    let mut has_unweighted = false;

    for weight_opt in weights {
        match weight_opt {
            Some(weight) => {
                if *weight == 0 {
                    return Err(MagayaError::validation(
                        "Endpoint weight must be greater than 0 when specified"
                    ));
                }
                if *weight > 1000 {
                    return Err(MagayaError::validation(
                        "Endpoint weight must be 1000 or less"
                    ));
                }
                total_weight = total_weight.saturating_add(*weight);
                has_weighted = true;
            }
            None => {
                has_unweighted = true;
            }
        }
    }

    // Business rule: Cannot mix weighted and unweighted endpoints
    if has_weighted && has_unweighted {
        return Err(MagayaError::validation(
            "Cannot mix weighted and unweighted endpoints in the same cluster"
        ));
    }

    // Business rule: Total weight should be reasonable
    if has_weighted && total_weight > 10000 {
        return Err(MagayaError::validation(
            "Total endpoint weights exceed maximum of 10000"
        ));
    }

    Ok(())
}

/// Validate health check configuration
pub fn validate_health_check_config(
    timeout_seconds: u64,
    interval_seconds: u64,
    healthy_threshold: u32,
    unhealthy_threshold: u32,
    path: &Option<String>,
) -> Result<()> {
    // Timeout must be less than interval
    if timeout_seconds >= interval_seconds {
        return Err(MagayaError::validation(
            "Health check timeout must be less than interval"
        ));
    }

    // Reasonable timeout bounds
    if timeout_seconds == 0 || timeout_seconds > 60 {
        return Err(MagayaError::validation_field(
            "Health check timeout must be between 1 and 60 seconds",
            "timeout"
        ));
    }

    // Reasonable interval bounds
    if interval_seconds == 0 || interval_seconds > 300 {
        return Err(MagayaError::validation_field(
            "Health check interval must be between 1 and 300 seconds",
            "interval"
        ));
    }

    // Threshold validation
    if healthy_threshold == 0 || healthy_threshold > 10 {
        return Err(MagayaError::validation_field(
            "Healthy threshold must be between 1 and 10",
            "healthy_threshold"
        ));
    }

    if unhealthy_threshold == 0 || unhealthy_threshold > 10 {
        return Err(MagayaError::validation_field(
            "Unhealthy threshold must be between 1 and 10",
            "unhealthy_threshold"
        ));
    }

    // Validate health check path if provided
    if let Some(hc_path) = path {
        if !hc_path.starts_with('/') {
            return Err(MagayaError::validation_field(
                "Health check path must start with '/'",
                "path"
            ));
        }
        if hc_path.contains("..") {
            return Err(MagayaError::validation_field(
                "Health check path cannot contain '..' (path traversal)",
                "path"
            ));
        }
        if hc_path.len() > 200 {
            return Err(MagayaError::validation_field(
                "Health check path cannot exceed 200 characters",
                "path"
            ));
        }
    }

    Ok(())
}

/// Validate virtual host domain constraints
pub fn validate_virtual_host_domains(domains: &[String]) -> Result<()> {
    if domains.is_empty() {
        return Err(MagayaError::validation(
            "Virtual host must have at least one domain"
        ));
    }

    if domains.len() > 50 {
        return Err(MagayaError::validation(
            "Virtual host cannot have more than 50 domains"
        ));
    }

    for (index, domain) in domains.iter().enumerate() {
        if domain.is_empty() {
            return Err(MagayaError::validation_field(
                format!("Domain {} cannot be empty", index),
                "domains"
            ));
        }

        if domain.len() > 253 {
            return Err(MagayaError::validation_field(
                format!("Domain {} exceeds maximum length of 253 characters", index),
                "domains"
            ));
        }

        // Basic domain format validation
        if domain != "*" && !is_valid_domain_format(domain) {
            return Err(MagayaError::validation_field(
                format!("Domain {} has invalid format", index),
                "domains"
            ));
        }
    }

    // Check for duplicate domains
    let mut unique_domains = std::collections::HashSet::new();
    for domain in domains {
        if !unique_domains.insert(domain.to_lowercase()) {
            return Err(MagayaError::validation(
                format!("Duplicate domain found: {}", domain)
            ));
        }
    }

    Ok(())
}

/// Validate listener port and address constraints
pub fn validate_listener_address_port(address: &str, port: u32) -> Result<()> {
    if address.is_empty() {
        return Err(MagayaError::validation_field(
            "Listener address cannot be empty",
            "address"
        ));
    }

    if port == 0 || port > 65535 {
        return Err(MagayaError::validation_field(
            "Listener port must be between 1 and 65535",
            "port"
        ));
    }

    // Business rule: Reserved port ranges
    if port < 1024 {
        return Err(MagayaError::validation_field(
            "Ports below 1024 are reserved and cannot be used",
            "port"
        ));
    }

    // Validate address format
    if !is_valid_address_format(address) {
        return Err(MagayaError::validation_field(
            "Invalid address format",
            "address"
        ));
    }

    Ok(())
}

/// Validate cluster naming conventions and constraints
pub fn validate_cluster_naming_rules(name: &str, existing_names: &[String]) -> Result<()> {
    // Check for reserved prefixes
    let reserved_prefixes = ["envoy.", "xds.", "internal.", "system."];
    for prefix in &reserved_prefixes {
        if name.starts_with(prefix) {
            return Err(MagayaError::validation_field(
                format!("Cluster name cannot start with reserved prefix '{}'", prefix),
                "name"
            ));
        }
    }

    // Check for conflicts with existing names
    if existing_names.iter().any(|existing| existing.eq_ignore_ascii_case(name)) {
        return Err(MagayaError::validation_field(
            "Cluster name conflicts with existing cluster (case-insensitive)",
            "name"
        ));
    }

    Ok(())
}

/// Basic domain format validation
fn is_valid_domain_format(domain: &str) -> bool {
    // Allow wildcards at the beginning
    let domain_to_check = if domain.starts_with("*.") {
        &domain[2..]
    } else {
        domain
    };

    // Basic checks
    if domain_to_check.is_empty() ||
       domain_to_check.starts_with('.') ||
       domain_to_check.ends_with('.') ||
       domain_to_check.contains("..") {
        return false;
    }

    // Check each label
    for label in domain_to_check.split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }

        // Labels must start and end with alphanumeric
        let chars: Vec<char> = label.chars().collect();
        if !chars[0].is_alphanumeric() || !chars[chars.len() - 1].is_alphanumeric() {
            return false;
        }

        // Only alphanumeric and hyphens allowed
        if !label.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return false;
        }
    }

    true
}

/// Basic address format validation (IPv4, IPv6, or hostname)
fn is_valid_address_format(address: &str) -> bool {
    // IPv4 check
    if let Ok(_) = address.parse::<std::net::Ipv4Addr>() {
        return true;
    }

    // IPv6 check
    if let Ok(_) = address.parse::<std::net::Ipv6Addr>() {
        return true;
    }

    // Hostname check (basic)
    if address == "localhost" || address == "0.0.0.0" || address == "::" {
        return true;
    }

    // Basic hostname validation
    is_valid_domain_format(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_circuit_breaker_config() {
        // Valid configuration
        assert!(validate_circuit_breaker_config(
            Some(100), Some(50), Some(200), Some(3)
        ).is_ok());

        // Invalid max connections
        assert!(validate_circuit_breaker_config(
            Some(0), None, None, None
        ).is_err());

        assert!(validate_circuit_breaker_config(
            Some(20000), None, None, None
        ).is_err());

        // Invalid max retries
        assert!(validate_circuit_breaker_config(
            None, None, None, Some(15)
        ).is_err());
    }

    #[test]
    fn test_validate_route_path_rewrite_compatibility() {
        // Valid combinations
        assert!(validate_route_path_rewrite_compatibility(
            "/api/v1",
            &PathMatchType::Prefix,
            &Some("/v2".to_string()),
            &None
        ).is_ok());

        assert!(validate_route_path_rewrite_compatibility(
            "/api/{id}",
            &PathMatchType::UriTemplate,
            &None,
            &Some("/v2/{id}".to_string())
        ).is_ok());

        // Invalid: URI template rewrite with non-URI template match
        assert!(validate_route_path_rewrite_compatibility(
            "/api/v1",
            &PathMatchType::Prefix,
            &None,
            &Some("/v2/{id}".to_string())
        ).is_err());

        // Invalid: Prefix rewrite with URI template match
        assert!(validate_route_path_rewrite_compatibility(
            "/api/{id}",
            &PathMatchType::UriTemplate,
            &Some("/v2".to_string()),
            &None
        ).is_err());

        // Invalid: Both rewrites specified
        assert!(validate_route_path_rewrite_compatibility(
            "/api/v1",
            &PathMatchType::Prefix,
            &Some("/v2".to_string()),
            &Some("/v3/{id}".to_string())
        ).is_err());
    }

    #[test]
    fn test_validate_endpoint_weights() {
        // Valid: All weighted
        assert!(validate_endpoint_weights(&[Some(100), Some(200), Some(50)]).is_ok());

        // Valid: All unweighted
        assert!(validate_endpoint_weights(&[None, None, None]).is_ok());

        // Invalid: Mixed weighted and unweighted
        assert!(validate_endpoint_weights(&[Some(100), None, Some(200)]).is_err());

        // Invalid: Zero weight
        assert!(validate_endpoint_weights(&[Some(0), Some(100)]).is_err());

        // Invalid: Weight too high
        assert!(validate_endpoint_weights(&[Some(2000)]).is_err());
    }

    #[test]
    fn test_validate_health_check_config() {
        // Valid configuration
        assert!(validate_health_check_config(
            5, 10, 2, 3, &Some("/health".to_string())
        ).is_ok());

        // Invalid: timeout >= interval
        assert!(validate_health_check_config(
            10, 10, 2, 3, &None
        ).is_err());

        assert!(validate_health_check_config(
            15, 10, 2, 3, &None
        ).is_err());

        // Invalid: threshold out of range
        assert!(validate_health_check_config(
            5, 10, 0, 3, &None
        ).is_err());

        assert!(validate_health_check_config(
            5, 10, 2, 15, &None
        ).is_err());

        // Invalid: path format
        assert!(validate_health_check_config(
            5, 10, 2, 3, &Some("health".to_string())
        ).is_err());

        assert!(validate_health_check_config(
            5, 10, 2, 3, &Some("/health/../admin".to_string())
        ).is_err());
    }

    #[test]
    fn test_validate_virtual_host_domains() {
        // Valid domains
        assert!(validate_virtual_host_domains(&[
            "example.com".to_string(),
            "*.example.com".to_string(),
            "api.example.com".to_string()
        ]).is_ok());

        // Invalid: empty list
        assert!(validate_virtual_host_domains(&[]).is_err());

        // Invalid: duplicate domains
        assert!(validate_virtual_host_domains(&[
            "example.com".to_string(),
            "Example.Com".to_string() // Case-insensitive duplicate
        ]).is_err());

        // Invalid: empty domain
        assert!(validate_virtual_host_domains(&[
            "example.com".to_string(),
            "".to_string()
        ]).is_err());

        // Invalid: domain too long
        let long_domain = "a".repeat(254);
        assert!(validate_virtual_host_domains(&[long_domain]).is_err());
    }

    #[test]
    fn test_validate_listener_address_port() {
        // Valid combinations
        assert!(validate_listener_address_port("0.0.0.0", 8080).is_ok());
        assert!(validate_listener_address_port("127.0.0.1", 3000).is_ok());
        assert!(validate_listener_address_port("localhost", 8080).is_ok());

        // Invalid: empty address
        assert!(validate_listener_address_port("", 8080).is_err());

        // Invalid: port 0
        assert!(validate_listener_address_port("0.0.0.0", 0).is_err());

        // Invalid: port too high
        assert!(validate_listener_address_port("0.0.0.0", 70000).is_err());

        // Invalid: reserved port
        assert!(validate_listener_address_port("0.0.0.0", 80).is_err());
    }

    #[test]
    fn test_validate_cluster_naming_rules() {
        let existing_names = vec!["existing-cluster".to_string(), "another-cluster".to_string()];

        // Valid name
        assert!(validate_cluster_naming_rules("new-cluster", &existing_names).is_ok());

        // Invalid: reserved prefix
        assert!(validate_cluster_naming_rules("envoy.test", &existing_names).is_err());
        assert!(validate_cluster_naming_rules("xds.test", &existing_names).is_err());
        assert!(validate_cluster_naming_rules("internal.test", &existing_names).is_err());
        assert!(validate_cluster_naming_rules("system.test", &existing_names).is_err());

        // Invalid: conflicts with existing (case-insensitive)
        assert!(validate_cluster_naming_rules("Existing-Cluster", &existing_names).is_err());
        assert!(validate_cluster_naming_rules("ANOTHER-CLUSTER", &existing_names).is_err());
    }

    #[test]
    fn test_is_valid_domain_format() {
        // Valid domains
        assert!(is_valid_domain_format("example.com"));
        assert!(is_valid_domain_format("sub.example.com"));
        assert!(is_valid_domain_format("api-v1.example.com"));
        assert!(is_valid_domain_format("localhost"));

        // Valid wildcards
        assert!(is_valid_domain_format("*.example.com"));

        // Invalid domains
        assert!(!is_valid_domain_format(""));
        assert!(!is_valid_domain_format(".example.com"));
        assert!(!is_valid_domain_format("example.com."));
        assert!(!is_valid_domain_format("example..com"));
        assert!(!is_valid_domain_format("-example.com"));
        assert!(!is_valid_domain_format("example-.com"));

        // Long labels (over 63 chars)
        let long_label = "a".repeat(64);
        assert!(!is_valid_domain_format(&format!("{}.com", long_label)));
    }

    #[test]
    fn test_is_valid_address_format() {
        // Valid IPv4
        assert!(is_valid_address_format("192.168.1.1"));
        assert!(is_valid_address_format("127.0.0.1"));
        assert!(is_valid_address_format("0.0.0.0"));

        // Valid IPv6
        assert!(is_valid_address_format("::1"));
        assert!(is_valid_address_format("2001:db8::1"));

        // Valid hostnames
        assert!(is_valid_address_format("localhost"));
        assert!(is_valid_address_format("example.com"));

        // Invalid addresses
        assert!(!is_valid_address_format(""));
        assert!(!is_valid_address_format("256.256.256.256"));
        assert!(!is_valid_address_format("invalid..address"));
    }
}