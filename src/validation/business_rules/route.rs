use crate::errors::types::{FlowplaneError, Result};
use crate::validation::{validate_path_with_match_type, PathMatchType};

use super::helpers::is_valid_domain_format;

/// Validate route path and rewrite combinations
pub fn validate_route_path_rewrite_compatibility(
    path: &str,
    path_match_type: &PathMatchType,
    prefix_rewrite: &Option<String>,
    uri_template_rewrite: &Option<String>,
) -> Result<()> {
    validate_path_with_match_type(path, path_match_type).map_err(|e| {
        FlowplaneError::validation_field(
            format!("Path validation failed: {}", e.message.unwrap_or_default()),
            "path",
        )
    })?;

    if uri_template_rewrite.is_some() && *path_match_type != PathMatchType::UriTemplate {
        return Err(FlowplaneError::validation(
            "URI template rewrite can only be used with URI template path matching",
        ));
    }

    if prefix_rewrite.is_some() && *path_match_type == PathMatchType::UriTemplate {
        return Err(FlowplaneError::validation(
            "Prefix rewrite cannot be used with URI template path matching",
        ));
    }

    if prefix_rewrite.is_some() && uri_template_rewrite.is_some() {
        return Err(FlowplaneError::validation(
            "Cannot specify both prefix rewrite and URI template rewrite",
        ));
    }

    Ok(())
}

/// Validate virtual host domain constraints
pub fn validate_virtual_host_domains(domains: &[String]) -> Result<()> {
    if domains.is_empty() {
        return Err(FlowplaneError::validation(
            "Virtual host must have at least one domain",
        ));
    }

    if domains.len() > 50 {
        return Err(FlowplaneError::validation(
            "Virtual host cannot have more than 50 domains",
        ));
    }

    for (index, domain) in domains.iter().enumerate() {
        if domain.is_empty() {
            return Err(FlowplaneError::validation_field(
                format!("Domain {} cannot be empty", index),
                "domains",
            ));
        }

        if domain.len() > 253 {
            return Err(FlowplaneError::validation_field(
                format!("Domain {} exceeds maximum length of 253 characters", index),
                "domains",
            ));
        }

        if domain != "*" && !is_valid_domain_format(domain) {
            return Err(FlowplaneError::validation_field(
                format!("Domain {} has invalid format", index),
                "domains",
            ));
        }
    }

    let mut unique = std::collections::HashSet::new();
    for domain in domains {
        if !unique.insert(domain.to_lowercase()) {
            return Err(FlowplaneError::validation(
                format!("Duplicate domain found: {}", domain),
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_path_rewrite_compatibility() {
        assert!(validate_route_path_rewrite_compatibility(
            "/api/v1",
            &PathMatchType::Prefix,
            &Some("/v2".to_string()),
            &None,
        )
        .is_ok());

        assert!(validate_route_path_rewrite_compatibility(
            "/api/{id}",
            &PathMatchType::UriTemplate,
            &None,
            &Some("/v2/{id}".to_string()),
        )
        .is_ok());

        assert!(validate_route_path_rewrite_compatibility(
            "/api/v1",
            &PathMatchType::Prefix,
            &None,
            &Some("/v2/{id}".to_string()),
        )
        .is_err());

        assert!(validate_route_path_rewrite_compatibility(
            "/api/{id}",
            &PathMatchType::UriTemplate,
            &Some("/v2".to_string()),
            &None,
        )
        .is_err());

        assert!(validate_route_path_rewrite_compatibility(
            "/api/v1",
            &PathMatchType::Prefix,
            &Some("/v2".to_string()),
            &Some("/v3/{id}".to_string()),
        )
        .is_err());
    }

    #[test]
    fn virtual_host_domain_validation() {
        assert!(validate_virtual_host_domains(&[
            "example.com".to_string(),
            "*.example.com".to_string(),
            "api.example.com".to_string(),
        ])
        .is_ok());

        assert!(validate_virtual_host_domains(&[]).is_err());

        assert!(validate_virtual_host_domains(&[
            "example.com".to_string(),
            "Example.Com".to_string(),
        ])
        .is_err());

        assert!(validate_virtual_host_domains(&[
            "example.com".to_string(),
            "".to_string(),
        ])
        .is_err());

        let long_domain = "a".repeat(254);
        assert!(validate_virtual_host_domains(&[long_domain]).is_err());
    }
}
