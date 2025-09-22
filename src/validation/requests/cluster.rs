//! Cluster-related validated request structures and schema-level checks.

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use crate::validation::{
    validate_address, validate_cluster_name, validate_lb_policy,
    business_rules::{
        validate_circuit_breaker_config, validate_endpoint_weights, validate_health_check_config,
    },
};

/// Validated request for creating a cluster
#[derive(Debug, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_create_cluster_request"))]
pub struct ValidatedCreateClusterRequest {
    #[validate(length(min = 1, max = 50), custom(function = "validate_cluster_name"))]
    pub name: String,

    #[validate(length(min = 1, max = 20))]
    #[validate]
    pub endpoints: Vec<ValidatedEndpointRequest>,

    #[validate(custom(function = "validate_lb_policy"))]
    pub lb_policy: Option<String>,

    #[validate(range(min = 1, max = 300))]
    pub connect_timeout_seconds: Option<u64>,

    #[validate]
    pub health_check: Option<ValidatedHealthCheckRequest>,

    #[validate]
    pub circuit_breaker: Option<ValidatedCircuitBreakerRequest>,
}

/// Validated request for updating a cluster
#[derive(Debug, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_update_cluster_request"))]
pub struct ValidatedUpdateClusterRequest {
    #[validate(length(min = 1, max = 20))]
    #[validate]
    pub endpoints: Vec<ValidatedEndpointRequest>,

    #[validate(custom(function = "validate_lb_policy"))]
    pub lb_policy: Option<String>,

    #[validate(range(min = 1, max = 300))]
    pub connect_timeout_seconds: Option<u64>,

    #[validate]
    pub health_check: Option<ValidatedHealthCheckRequest>,

    #[validate]
    pub circuit_breaker: Option<ValidatedCircuitBreakerRequest>,
}

/// Validated endpoint configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedEndpointRequest {
    #[validate(length(min = 1, max = 255), custom(function = "validate_address"))]
    pub address: String,

    #[validate(range(min = 1, max = 65535))]
    pub port: u32,

    #[validate(range(min = 1, max = 1000))]
    pub weight: Option<u32>,
}

/// Validated health check configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_health_check_request"))]
pub struct ValidatedHealthCheckRequest {
    #[validate(range(min = 1, max = 60))]
    pub timeout_seconds: u64,

    #[validate(range(min = 1, max = 300))]
    pub interval_seconds: u64,

    #[validate(range(min = 1, max = 10))]
    pub healthy_threshold: u32,

    #[validate(range(min = 1, max = 10))]
    pub unhealthy_threshold: u32,

    #[validate(length(min = 1, max = 200))]
    pub path: Option<String>,
}

/// Validated circuit breaker configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_circuit_breaker_request"))]
pub struct ValidatedCircuitBreakerRequest {
    #[validate(range(min = 1, max = 10000))]
    pub max_connections: Option<u32>,

    #[validate(range(min = 1, max = 10000))]
    pub max_pending_requests: Option<u32>,

    #[validate(range(min = 1, max = 10000))]
    pub max_requests: Option<u32>,

    #[validate(range(max = 10))]
    pub max_retries: Option<u32>,
}

/// Validate create cluster request with business rules
pub fn validate_create_cluster_request(
    request: &ValidatedCreateClusterRequest,
) -> Result<(), ValidationError> {
    let weights: Vec<Option<u32>> = request.endpoints.iter().map(|e| e.weight).collect();
    validate_endpoint_weights(&weights)
        .map_err(|_| ValidationError::new("invalid_endpoint_weights"))?;
    Ok(())
}

/// Validate update cluster request with business rules
pub fn validate_update_cluster_request(
    request: &ValidatedUpdateClusterRequest,
) -> Result<(), ValidationError> {
    let weights: Vec<Option<u32>> = request.endpoints.iter().map(|e| e.weight).collect();
    validate_endpoint_weights(&weights)
        .map_err(|_| ValidationError::new("invalid_endpoint_weights"))?;
    Ok(())
}

/// Validate health check request with business rules
pub fn validate_health_check_request(
    request: &ValidatedHealthCheckRequest,
) -> Result<(), ValidationError> {
    validate_health_check_config(
        request.timeout_seconds,
        request.interval_seconds,
        request.healthy_threshold,
        request.unhealthy_threshold,
        &request.path,
    )
    .map_err(|_| ValidationError::new("invalid_health_check_config"))?;
    Ok(())
}

/// Validate circuit breaker request with business rules
pub fn validate_circuit_breaker_request(
    request: &ValidatedCircuitBreakerRequest,
) -> Result<(), ValidationError> {
    validate_circuit_breaker_config(
        request.max_connections,
        request.max_pending_requests,
        request.max_requests,
        request.max_retries,
    )
    .map_err(|_| ValidationError::new("invalid_circuit_breaker_config"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validated_create_cluster_request() {
        let request = ValidatedCreateClusterRequest {
            name: "test-cluster".to_string(),
            endpoints: vec![
                ValidatedEndpointRequest {
                    address: "127.0.0.1".to_string(),
                    port: 8080,
                    weight: Some(100),
                },
                ValidatedEndpointRequest {
                    address: "127.0.0.1".to_string(),
                    port: 8081,
                    weight: Some(100),
                },
            ],
            lb_policy: Some("ROUND_ROBIN".to_string()),
            connect_timeout_seconds: Some(5),
            health_check: None,
            circuit_breaker: None,
        };

        assert!(request.validate().is_ok());

        let invalid_request = ValidatedCreateClusterRequest {
            name: "".to_string(),
            endpoints: vec![],
            lb_policy: None,
            connect_timeout_seconds: None,
            health_check: None,
            circuit_breaker: None,
        };

        assert!(invalid_request.validate().is_err());
    }

    #[test]
    fn test_endpoint_weight_validation() {
        let request = ValidatedCreateClusterRequest {
            name: "test-cluster".to_string(),
            endpoints: vec![
                ValidatedEndpointRequest {
                    address: "127.0.0.1".to_string(),
                    port: 8080,
                    weight: Some(100),
                },
                ValidatedEndpointRequest {
                    address: "127.0.0.1".to_string(),
                    port: 8081,
                    weight: Some(200),
                },
            ],
            lb_policy: None,
            connect_timeout_seconds: None,
            health_check: None,
            circuit_breaker: None,
        };

        assert!(request.validate().is_ok());

        let invalid_request = ValidatedCreateClusterRequest {
            name: "test-cluster".to_string(),
            endpoints: vec![
                ValidatedEndpointRequest {
                    address: "127.0.0.1".to_string(),
                    port: 8080,
                    weight: Some(100),
                },
                ValidatedEndpointRequest {
                    address: "127.0.0.1".to_string(),
                    port: 8081,
                    weight: None,
                },
            ],
            lb_policy: None,
            connect_timeout_seconds: None,
            health_check: None,
            circuit_breaker: None,
        };

        assert!(invalid_request.validate().is_err());
    }

    #[test]
    fn test_health_check_validation() {
        let valid_health_check = ValidatedHealthCheckRequest {
            timeout_seconds: 5,
            interval_seconds: 10,
            healthy_threshold: 2,
            unhealthy_threshold: 3,
            path: Some("/health".to_string()),
        };

        assert!(valid_health_check.validate().is_ok());

        let invalid_health_check = ValidatedHealthCheckRequest {
            timeout_seconds: 10,
            interval_seconds: 10,
            healthy_threshold: 2,
            unhealthy_threshold: 3,
            path: None,
        };

        assert!(invalid_health_check.validate().is_err());
    }
}
