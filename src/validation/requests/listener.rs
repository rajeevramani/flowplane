//! Listener-related validated request structures and schema-level checks.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use crate::validation::{
    validate_address, validate_cluster_name, validate_listener_name,
    business_rules::validate_listener_address_port,
};

use super::route::ValidatedInlineRouteConfigRequest;

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

    pub config: HashMap<String, String>,
}

/// Validate create listener request with business rules
pub fn validate_create_listener_request(
    request: &ValidatedCreateListenerRequest,
) -> Result<(), ValidationError> {
    validate_listener_address_port(&request.address, request.port)
        .map_err(|_| ValidationError::new("invalid_listener_address_port"))?;
    Ok(())
}

/// Validate update listener request with business rules
pub fn validate_update_listener_request(
    request: &ValidatedUpdateListenerRequest,
) -> Result<(), ValidationError> {
    validate_listener_address_port(&request.address, request.port)
        .map_err(|_| ValidationError::new("invalid_listener_address_port"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_validated_create_listener_request() {
        let request = ValidatedCreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 8080,
            filter_chains: vec![ValidatedFilterChainRequest {
                name: Some("default".to_string()),
                filters: vec![ValidatedFilterRequest {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: ValidatedFilterType::HttpConnectionManager {
                        route_config_name: Some("default-route".to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                    },
                }],
                tls_context: None,
            }],
        };

        assert!(request.validate().is_ok());

        let invalid_request = ValidatedCreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: 80,
            filter_chains: vec![],
        };

        assert!(invalid_request.validate().is_err());
    }
}
