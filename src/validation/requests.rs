//! # Validated Request Structures
//!
//! This module defines validated request structures that use derive-based validation
//! and custom validation functions to ensure all incoming requests are valid
//! before processing. These structures follow the PoC patterns and integrate
//! with Magaya's error handling system.

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use crate::validation::{
    PathMatchType,
    validate_route_name, validate_cluster_name, validate_listener_name,
    validate_host, validate_address, validate_http_method, validate_lb_policy,
    validate_path_with_match_type, validate_http_methods,
    business_rules::{
        validate_route_path_rewrite_compatibility,
        validate_endpoint_weights,
        validate_health_check_config,
        validate_virtual_host_domains,
        validate_listener_address_port,
        validate_circuit_breaker_config,
    }
};

// ============================================================================
// CLUSTER REQUEST STRUCTURES
// ============================================================================

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

// ============================================================================
// ROUTE REQUEST STRUCTURES
// ============================================================================

/// Validated request for creating a route
#[derive(Debug, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_create_route_request"))]
pub struct ValidatedCreateRouteRequest {
    #[validate(length(min = 1, max = 100), custom(function = "validate_route_name"))]
    pub name: String,

    #[validate(length(min = 1, max = 200))]
    pub path: String,

    pub path_match_type: PathMatchType,

    #[validate(length(min = 1, max = 50), custom(function = "validate_cluster_name"))]
    pub cluster_name: String,

    #[validate(length(max = 100))]
    pub prefix_rewrite: Option<String>,

    #[validate(length(max = 200))]
    pub uri_template_rewrite: Option<String>,

    #[validate(custom(function = "validate_http_methods"))]
    pub http_methods: Option<Vec<String>>,

    #[validate(range(min = 1, max = 300))]
    pub timeout_seconds: Option<u64>,

    #[validate(range(min = 1, max = 10))]
    pub retry_attempts: Option<u32>,
}

/// Validated request for updating a route
#[derive(Debug, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_update_route_request"))]
pub struct ValidatedUpdateRouteRequest {
    #[validate(length(min = 1, max = 200))]
    pub path: String,

    pub path_match_type: PathMatchType,

    #[validate(length(min = 1, max = 50), custom(function = "validate_cluster_name"))]
    pub cluster_name: String,

    #[validate(length(max = 100))]
    pub prefix_rewrite: Option<String>,

    #[validate(length(max = 200))]
    pub uri_template_rewrite: Option<String>,

    #[validate(custom(function = "validate_http_methods"))]
    pub http_methods: Option<Vec<String>>,

    #[validate(range(min = 1, max = 300))]
    pub timeout_seconds: Option<u64>,

    #[validate(range(min = 1, max = 10))]
    pub retry_attempts: Option<u32>,
}

/// Validated virtual host configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_virtual_host_request"))]
pub struct ValidatedVirtualHostRequest {
    #[validate(length(min = 1, max = 100), custom(function = "validate_route_name"))]
    pub name: String,

    #[validate(length(min = 1, max = 50))]
    pub domains: Vec<String>,

    #[validate]
    pub routes: Vec<ValidatedRouteRuleRequest>,
}

/// Validated route rule configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedRouteRuleRequest {
    #[validate(length(max = 100))]
    pub name: Option<String>,

    #[validate]
    pub r#match: ValidatedRouteMatchRequest,

    #[validate]
    pub action: ValidatedRouteActionRequest,
}

/// Validated route match configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedRouteMatchRequest {
    #[validate(length(min = 1, max = 200))]
    pub path: String,

    pub path_match_type: PathMatchType,

    #[validate]
    pub headers: Option<Vec<ValidatedHeaderMatchRequest>>,

    #[validate]
    pub query_parameters: Option<Vec<ValidatedQueryParameterMatchRequest>>,
}

/// Validated header match configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedHeaderMatchRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(length(max = 500))]
    pub value: Option<String>,

    #[validate(length(max = 200))]
    pub regex: Option<String>,

    pub present: Option<bool>,
}

/// Validated query parameter match configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedQueryParameterMatchRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(length(max = 500))]
    pub value: Option<String>,

    #[validate(length(max = 200))]
    pub regex: Option<String>,

    pub present: Option<bool>,
}

/// Validated route action configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedRouteActionRequest {
    #[serde(flatten)]
    pub action_type: ValidatedRouteActionType,
}

/// Validated route action types
#[derive(Debug, Serialize, Deserialize, Validate)]
#[serde(tag = "type")]
pub enum ValidatedRouteActionType {
    #[serde(rename = "cluster")]
    Cluster {
        #[validate(length(min = 1, max = 50), custom(function = "validate_cluster_name"))]
        cluster_name: String,

        #[validate(range(min = 1, max = 300))]
        timeout_seconds: Option<u64>,
    },
    #[serde(rename = "weighted_clusters")]
    WeightedClusters {
        #[validate]
        clusters: Vec<ValidatedWeightedClusterRequest>,

        #[validate(range(min = 1, max = 10000))]
        total_weight: Option<u32>,
    },
    #[serde(rename = "redirect")]
    Redirect {
        #[validate(length(max = 255))]
        host_redirect: Option<String>,

        #[validate(length(max = 200))]
        path_redirect: Option<String>,

        #[validate(range(min = 300, max = 399))]
        response_code: Option<u32>,
    },
}

/// Validated weighted cluster configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedWeightedClusterRequest {
    #[validate(length(min = 1, max = 50), custom(function = "validate_cluster_name"))]
    pub name: String,

    #[validate(range(min = 1, max = 1000))]
    pub weight: u32,
}

// ============================================================================
// LISTENER REQUEST STRUCTURES
// ============================================================================

/// Validated request for creating a listener
#[derive(Debug, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_create_listener_request"))]
pub struct ValidatedCreateListenerRequest {
    #[validate(length(min = 1, max = 100), custom(function = "validate_listener_name"))]
    pub name: String,

    #[validate(length(min = 1, max = 255), custom(function = "validate_address"))]
    pub address: String,

    #[validate(range(min = 1024, max = 65535))]
    pub port: u32,

    #[validate]
    pub filter_chains: Vec<ValidatedFilterChainRequest>,
}

/// Validated request for updating a listener
#[derive(Debug, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_update_listener_request"))]
pub struct ValidatedUpdateListenerRequest {
    #[validate(length(min = 1, max = 255), custom(function = "validate_address"))]
    pub address: String,

    #[validate(range(min = 1024, max = 65535))]
    pub port: u32,

    #[validate]
    pub filter_chains: Vec<ValidatedFilterChainRequest>,
}

/// Validated filter chain configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedFilterChainRequest {
    #[validate(length(max = 100))]
    pub name: Option<String>,

    #[validate(length(min = 1, max = 10))]
    #[validate]
    pub filters: Vec<ValidatedFilterRequest>,

    #[validate]
    pub tls_context: Option<ValidatedTlsContextRequest>,
}

/// Validated filter configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedFilterRequest {
    #[validate(length(min = 1, max = 200))]
    pub name: String,

    #[validate]
    pub filter_type: ValidatedFilterType,
}

/// Validated filter types
#[derive(Debug, Serialize, Deserialize, Validate)]
#[serde(tag = "type")]
pub enum ValidatedFilterType {
    #[serde(rename = "http_connection_manager")]
    HttpConnectionManager {
        #[validate(length(max = 100))]
        route_config_name: Option<String>,

        #[validate]
        inline_route_config: Option<ValidatedInlineRouteConfigRequest>,

        #[validate]
        access_log: Option<ValidatedAccessLogRequest>,

        #[validate]
        tracing: Option<ValidatedTracingRequest>,
    },
    #[serde(rename = "tcp_proxy")]
    TcpProxy {
        #[validate(length(min = 1, max = 50), custom(function = "validate_cluster_name"))]
        cluster: String,

        #[validate]
        access_log: Option<ValidatedAccessLogRequest>,
    },
}

/// Validated inline route configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedInlineRouteConfigRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate]
    pub virtual_hosts: Vec<ValidatedVirtualHostRequest>,
}

/// Validated TLS context configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedTlsContextRequest {
    #[validate(length(max = 500))]
    pub cert_chain_file: Option<String>,

    #[validate(length(max = 500))]
    pub private_key_file: Option<String>,

    #[validate(length(max = 500))]
    pub ca_cert_file: Option<String>,

    pub require_client_certificate: Option<bool>,
}

/// Validated access log configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedAccessLogRequest {
    #[validate(length(max = 500))]
    pub path: Option<String>,

    #[validate(length(max = 1000))]
    pub format: Option<String>,
}

/// Validated tracing configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedTracingRequest {
    #[validate(length(min = 1, max = 100))]
    pub provider: String,

    pub config: std::collections::HashMap<String, String>,
}

// ============================================================================
// SCHEMA-LEVEL VALIDATION FUNCTIONS
// ============================================================================

/// Validate create cluster request with business rules
pub fn validate_create_cluster_request(request: &ValidatedCreateClusterRequest) -> Result<(), ValidationError> {
    // Validate endpoint weights
    let weights: Vec<Option<u32>> = request.endpoints.iter().map(|e| e.weight).collect();
    validate_endpoint_weights(&weights)
        .map_err(|_| ValidationError::new("invalid_endpoint_weights"))?;

    Ok(())
}

/// Validate update cluster request with business rules
pub fn validate_update_cluster_request(request: &ValidatedUpdateClusterRequest) -> Result<(), ValidationError> {
    // Validate endpoint weights
    let weights: Vec<Option<u32>> = request.endpoints.iter().map(|e| e.weight).collect();
    validate_endpoint_weights(&weights)
        .map_err(|_| ValidationError::new("invalid_endpoint_weights"))?;

    Ok(())
}

/// Validate health check request with business rules
pub fn validate_health_check_request(request: &ValidatedHealthCheckRequest) -> Result<(), ValidationError> {
    validate_health_check_config(
        request.timeout_seconds,
        request.interval_seconds,
        request.healthy_threshold,
        request.unhealthy_threshold,
        &request.path,
    ).map_err(|_| ValidationError::new("invalid_health_check_config"))?;

    Ok(())
}

/// Validate circuit breaker request with business rules
pub fn validate_circuit_breaker_request(request: &ValidatedCircuitBreakerRequest) -> Result<(), ValidationError> {
    validate_circuit_breaker_config(
        request.max_connections,
        request.max_pending_requests,
        request.max_requests,
        request.max_retries,
    ).map_err(|_| ValidationError::new("invalid_circuit_breaker_config"))?;

    Ok(())
}

/// Validate create route request with business rules
pub fn validate_create_route_request(request: &ValidatedCreateRouteRequest) -> Result<(), ValidationError> {
    validate_route_path_rewrite_compatibility(
        &request.path,
        &request.path_match_type,
        &request.prefix_rewrite,
        &request.uri_template_rewrite,
    ).map_err(|_| ValidationError::new("invalid_path_rewrite_combination"))?;

    Ok(())
}

/// Validate update route request with business rules
pub fn validate_update_route_request(request: &ValidatedUpdateRouteRequest) -> Result<(), ValidationError> {
    validate_route_path_rewrite_compatibility(
        &request.path,
        &request.path_match_type,
        &request.prefix_rewrite,
        &request.uri_template_rewrite,
    ).map_err(|_| ValidationError::new("invalid_path_rewrite_combination"))?;

    Ok(())
}

/// Validate virtual host request with business rules
pub fn validate_virtual_host_request(request: &ValidatedVirtualHostRequest) -> Result<(), ValidationError> {
    validate_virtual_host_domains(&request.domains)
        .map_err(|_| ValidationError::new("invalid_virtual_host_domains"))?;

    Ok(())
}

/// Validate create listener request with business rules
pub fn validate_create_listener_request(request: &ValidatedCreateListenerRequest) -> Result<(), ValidationError> {
    validate_listener_address_port(&request.address, request.port)
        .map_err(|_| ValidationError::new("invalid_listener_address_port"))?;

    Ok(())
}

/// Validate update listener request with business rules
pub fn validate_update_listener_request(request: &ValidatedUpdateListenerRequest) -> Result<(), ValidationError> {
    validate_listener_address_port(&request.address, request.port)
        .map_err(|_| ValidationError::new("invalid_listener_address_port"))?;

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

        // Test validation failure
        let invalid_request = ValidatedCreateClusterRequest {
            name: "".to_string(), // Invalid empty name
            endpoints: vec![],     // Invalid empty endpoints
            lb_policy: None,
            connect_timeout_seconds: None,
            health_check: None,
            circuit_breaker: None,
        };

        assert!(invalid_request.validate().is_err());
    }

    #[test]
    fn test_validated_create_route_request() {
        let request = ValidatedCreateRouteRequest {
            name: "test-route".to_string(),
            path: "/api/v1".to_string(),
            path_match_type: PathMatchType::Prefix,
            cluster_name: "test-cluster".to_string(),
            prefix_rewrite: Some("/v2".to_string()),
            uri_template_rewrite: None,
            http_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
            timeout_seconds: Some(30),
            retry_attempts: Some(3),
        };

        assert!(request.validate().is_ok());

        // Test invalid combination
        let invalid_request = ValidatedCreateRouteRequest {
            name: "test-route".to_string(),
            path: "/api/{id}".to_string(),
            path_match_type: PathMatchType::UriTemplate,
            cluster_name: "test-cluster".to_string(),
            prefix_rewrite: Some("/v2".to_string()), // Invalid with URI template
            uri_template_rewrite: None,
            http_methods: None,
            timeout_seconds: None,
            retry_attempts: None,
        };

        assert!(invalid_request.validate().is_err());
    }

    #[test]
    fn test_validated_create_listener_request() {
        let request = ValidatedCreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            filter_chains: vec![
                ValidatedFilterChainRequest {
                    name: Some("default".to_string()),
                    filters: vec![
                        ValidatedFilterRequest {
                            name: "envoy.filters.network.http_connection_manager".to_string(),
                            filter_type: ValidatedFilterType::HttpConnectionManager {
                                route_config_name: Some("default-route".to_string()),
                                inline_route_config: None,
                                access_log: None,
                                tracing: None,
                            },
                        },
                    ],
                    tls_context: None,
                },
            ],
        };

        assert!(request.validate().is_ok());

        // Test invalid port (reserved range)
        let invalid_request = ValidatedCreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 80, // Reserved port
            filter_chains: vec![],
        };

        assert!(invalid_request.validate().is_err());
    }

    #[test]
    fn test_endpoint_weight_validation() {
        // Valid: All weighted
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

        // Invalid: Mixed weighted and unweighted
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
                    weight: None, // Mixed with weighted endpoint
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

        // Invalid: timeout >= interval
        let invalid_health_check = ValidatedHealthCheckRequest {
            timeout_seconds: 10,
            interval_seconds: 10, // Should be > timeout
            healthy_threshold: 2,
            unhealthy_threshold: 3,
            path: None,
        };

        assert!(invalid_health_check.validate().is_err());
    }
}