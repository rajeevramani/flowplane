//! Route DTOs for API request/response handling

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;
use validator::Validate;

use crate::xds::filters::http::HttpScopedConfig;

/// Request body for creating/updating a route configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "name": "primary-routes",
    "virtualHosts": [
        {
            "name": "default",
            "domains": ["*"],
            "routes": [
                {
                    "name": "api",
                    "match": {"path": {"type": "prefix", "value": "/api"}},
                    "action": {"type": "forward", "cluster": "api-cluster", "timeoutSeconds": 5}
                }
            ]
        }
    ]
}))]
pub struct RouteDefinitionDto {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(length(min = 1))]
    #[schema(min_items = 1, value_type = Vec<VirtualHostDefinitionDto>)]
    pub virtual_hosts: Vec<VirtualHostDefinitionDto>,
}

/// Virtual host configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VirtualHostDefinitionDto {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(length(min = 1))]
    #[schema(min_items = 1)]
    pub domains: Vec<String>,

    #[validate(length(min = 1))]
    #[schema(min_items = 1, value_type = Vec<RouteRuleDefinitionDto>)]
    pub routes: Vec<RouteRuleDefinitionDto>,

    #[serde(default)]
    #[schema(value_type = Object)]
    pub typed_per_filter_config: HashMap<String, HttpScopedConfig>,
}

/// Route rule configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteRuleDefinitionDto {
    #[validate(length(min = 1, max = 100))]
    pub name: Option<String>,

    #[validate(nested)]
    pub r#match: RouteMatchDefinitionDto,

    pub action: RouteActionDefinitionDto,

    #[serde(default)]
    #[schema(value_type = Object)]
    pub typed_per_filter_config: HashMap<String, HttpScopedConfig>,
}

/// Route match criteria DTO
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteMatchDefinitionDto {
    pub path: PathMatchDefinitionDto,

    #[serde(default)]
    #[schema(value_type = Vec<HeaderMatchDefinitionDto>)]
    pub headers: Vec<HeaderMatchDefinitionDto>,

    #[serde(default)]
    #[schema(value_type = Vec<QueryParameterMatchDefinitionDto>)]
    pub query_parameters: Vec<QueryParameterMatchDefinitionDto>,
}

/// Path matching strategy DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PathMatchDefinitionDto {
    #[schema(example = json!({"type": "exact", "value": "/health"}))]
    Exact { value: String },
    #[schema(example = json!({"type": "prefix", "value": "/api"}))]
    Prefix { value: String },
    #[schema(example = json!({"type": "regex", "value": "^/v[0-9]+/.*"}))]
    Regex { value: String },
    #[schema(example = json!({"type": "template", "template": "/api/v1/users/{user_id}"}))]
    Template { template: String },
}

/// Header match configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeaderMatchDefinitionDto {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub present: Option<bool>,
}

/// Query parameter match configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryParameterMatchDefinitionDto {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub present: Option<bool>,
}

/// Route action definition DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RouteActionDefinitionDto {
    #[serde(rename_all = "camelCase")]
    Forward {
        #[schema(example = "demo_cluster_api_002")]
        cluster: String,
        #[serde(default)]
        #[schema(example = 5)]
        timeout_seconds: Option<u64>,
        #[serde(default)]
        #[schema(example = "/internal/api")]
        prefix_rewrite: Option<String>,
        #[serde(default)]
        #[schema(example = "/users/{user_id}")]
        template_rewrite: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Weighted {
        clusters: Vec<WeightedClusterDefinitionDto>,
        #[serde(default)]
        total_weight: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    Redirect {
        #[serde(default)]
        host_redirect: Option<String>,
        #[serde(default)]
        path_redirect: Option<String>,
        #[serde(default)]
        response_code: Option<u32>,
    },
}

/// Weighted cluster configuration DTO
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeightedClusterDefinitionDto {
    #[schema(example = "blue-canary")]
    pub name: String,
    #[schema(example = 80)]
    pub weight: u32,

    #[serde(default)]
    #[schema(value_type = Object)]
    pub typed_per_filter_config: HashMap<String, HttpScopedConfig>,
}

/// Response DTO for route details
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteResponseDto {
    pub name: String,
    pub path_prefix: String,
    pub cluster_targets: String,
    pub config: RouteDefinitionDto,
}
