//! Route-related validated request structures and schema-level checks.

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use crate::validation::{
    PathMatchType,
    validate_cluster_name, validate_route_name, validate_http_methods,
    business_rules::{
        validate_route_path_rewrite_compatibility, validate_virtual_host_domains,
    },
};

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

/// Validated inline route configuration
#[derive(Debug, Serialize, Deserialize, Validate)]
pub struct ValidatedInlineRouteConfigRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate]
    pub virtual_hosts: Vec<ValidatedVirtualHostRequest>,
}

/// Validate create route request with business rules
pub fn validate_create_route_request(
    request: &ValidatedCreateRouteRequest,
) -> Result<(), ValidationError> {
    validate_route_path_rewrite_compatibility(
        &request.path,
        &request.path_match_type,
        &request.prefix_rewrite,
        &request.uri_template_rewrite,
    )
    .map_err(|_| ValidationError::new("invalid_path_rewrite_combination"))?;
    Ok(())
}

/// Validate update route request with business rules
pub fn validate_update_route_request(
    request: &ValidatedUpdateRouteRequest,
) -> Result<(), ValidationError> {
    validate_route_path_rewrite_compatibility(
        &request.path,
        &request.path_match_type,
        &request.prefix_rewrite,
        &request.uri_template_rewrite,
    )
    .map_err(|_| ValidationError::new("invalid_path_rewrite_combination"))?;
    Ok(())
}

/// Validate virtual host request with business rules
pub fn validate_virtual_host_request(
    request: &ValidatedVirtualHostRequest,
) -> Result<(), ValidationError> {
    validate_virtual_host_domains(&request.domains)
        .map_err(|_| ValidationError::new("invalid_virtual_host_domains"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let invalid_request = ValidatedCreateRouteRequest {
            name: "test-route".to_string(),
            path: "/api/{id}".to_string(),
            path_match_type: PathMatchType::UriTemplate,
            cluster_name: "test-cluster".to_string(),
            prefix_rewrite: Some("/v2".to_string()),
            uri_template_rewrite: None,
            http_methods: None,
            timeout_seconds: None,
            retry_attempts: None,
        };

        assert!(invalid_request.validate().is_err());
    }
}
