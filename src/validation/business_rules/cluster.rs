use crate::errors::types::{MagayaError, Result};

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
                "max_connections",
            ));
        }
    }

    if let Some(max_pending) = max_pending_requests {
        if max_pending == 0 || max_pending > 10000 {
            return Err(MagayaError::validation_field(
                "Max pending requests must be between 1 and 10000",
                "max_pending_requests",
            ));
        }
    }

    if let Some(max_req) = max_requests {
        if max_req == 0 || max_req > 10000 {
            return Err(MagayaError::validation_field(
                "Max requests must be between 1 and 10000",
                "max_requests",
            ));
        }
    }

    if let Some(max_ret) = max_retries {
        if max_ret > 10 {
            return Err(MagayaError::validation_field(
                "Max retries must be 10 or less",
                "max_retries",
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
                        "Endpoint weight must be greater than 0 when specified",
                    ));
                }
                if *weight > 1000 {
                    return Err(MagayaError::validation(
                        "Endpoint weight must be 1000 or less",
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

    if has_weighted && has_unweighted {
        return Err(MagayaError::validation(
            "Cannot mix weighted and unweighted endpoints in the same cluster",
        ));
    }

    if has_weighted && total_weight > 10000 {
        return Err(MagayaError::validation(
            "Total endpoint weights exceed maximum of 10000",
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
    if timeout_seconds >= interval_seconds {
        return Err(MagayaError::validation(
            "Health check timeout must be less than interval",
        ));
    }

    if timeout_seconds == 0 || timeout_seconds > 60 {
        return Err(MagayaError::validation_field(
            "Health check timeout must be between 1 and 60 seconds",
            "timeout",
        ));
    }

    if interval_seconds == 0 || interval_seconds > 300 {
        return Err(MagayaError::validation_field(
            "Health check interval must be between 1 and 300 seconds",
            "interval",
        ));
    }

    if healthy_threshold == 0 || healthy_threshold > 10 {
        return Err(MagayaError::validation_field(
            "Healthy threshold must be between 1 and 10",
            "healthy_threshold",
        ));
    }

    if unhealthy_threshold == 0 || unhealthy_threshold > 10 {
        return Err(MagayaError::validation_field(
            "Unhealthy threshold must be between 1 and 10",
            "unhealthy_threshold",
        ));
    }

    if let Some(hc_path) = path {
        if !hc_path.starts_with('/') {
            return Err(MagayaError::validation_field(
                "Health check path must start with '/'",
                "path",
            ));
        }
        if hc_path.contains("..") {
            return Err(MagayaError::validation_field(
                "Health check path cannot contain '..' (path traversal)",
                "path",
            ));
        }
        if hc_path.len() > 200 {
            return Err(MagayaError::validation_field(
                "Health check path cannot exceed 200 characters",
                "path",
            ));
        }
    }

    Ok(())
}

/// Validate cluster naming conventions and constraints
pub fn validate_cluster_naming_rules(name: &str, existing_names: &[String]) -> Result<()> {
    let reserved_prefixes = ["envoy.", "xds.", "internal.", "system."];
    for prefix in &reserved_prefixes {
        if name.starts_with(prefix) {
            return Err(MagayaError::validation_field(
                format!("Cluster name cannot start with reserved prefix '{}'", prefix),
                "name",
            ));
        }
    }

    if existing_names
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(name))
    {
        return Err(MagayaError::validation_field(
            "Cluster name conflicts with existing cluster (case-insensitive)",
            "name",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circuit_breaker_config_validation() {
        assert!(validate_circuit_breaker_config(Some(100), Some(50), Some(200), Some(3)).is_ok());
        assert!(validate_circuit_breaker_config(Some(0), None, None, None).is_err());
        assert!(validate_circuit_breaker_config(Some(20000), None, None, None).is_err());
        assert!(validate_circuit_breaker_config(None, None, None, Some(15)).is_err());
    }

    #[test]
    fn endpoint_weight_validation() {
        assert!(validate_endpoint_weights(&[Some(100), Some(200), Some(50)]).is_ok());
        assert!(validate_endpoint_weights(&[None, None, None]).is_ok());
        assert!(validate_endpoint_weights(&[Some(100), None, Some(200)]).is_err());
        assert!(validate_endpoint_weights(&[Some(0), Some(100)]).is_err());
        assert!(validate_endpoint_weights(&[Some(2000)]).is_err());
    }

    #[test]
    fn health_check_validation() {
        assert!(validate_health_check_config(5, 10, 2, 3, &Some("/health".to_string())).is_ok());
        assert!(validate_health_check_config(10, 10, 2, 3, &None).is_err());
        assert!(validate_health_check_config(15, 10, 2, 3, &None).is_err());
        assert!(validate_health_check_config(5, 10, 0, 3, &None).is_err());
        assert!(validate_health_check_config(5, 10, 2, 15, &None).is_err());
        assert!(validate_health_check_config(5, 10, 2, 3, &Some("health".to_string())).is_err());
    }

    #[test]
    fn cluster_naming_rules() {
        let existing = vec!["existing-cluster".to_string(), "another-cluster".to_string()];
        assert!(validate_cluster_naming_rules("new-cluster", &existing).is_ok());
        assert!(validate_cluster_naming_rules("envoy.test", &existing).is_err());
        assert!(validate_cluster_naming_rules("Existing-Cluster", &existing).is_err());
    }
}
