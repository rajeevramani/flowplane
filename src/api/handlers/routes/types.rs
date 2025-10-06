//! Route handler DTOs and XDS conversion implementations

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;
use validator::Validate;

use crate::{
    api::error::ApiError,
    errors::Error,
    xds::filters::http::HttpScopedConfig,
    xds::route::{
        HeaderMatchConfig as XdsHeaderMatchConfig, PathMatch as XdsPathMatch,
        QueryParameterMatchConfig as XdsQueryParameterMatchConfig,
        RouteActionConfig as XdsRouteActionConfig, RouteConfig as XdsRouteConfig,
        RouteMatchConfig as XdsRouteMatchConfig, RouteRule as XdsRouteRule,
        VirtualHostConfig as XdsVirtualHostConfig,
        WeightedClusterConfig as XdsWeightedClusterConfig,
    },
};

// === Request & Response Models ===

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
pub struct RouteDefinition {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(length(min = 1))]
    #[schema(min_items = 1, value_type = Vec<VirtualHostDefinition>)]
    pub virtual_hosts: Vec<VirtualHostDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VirtualHostDefinition {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(length(min = 1))]
    #[schema(min_items = 1)]
    pub domains: Vec<String>,

    #[validate(length(min = 1))]
    #[schema(min_items = 1, value_type = Vec<RouteRuleDefinition>)]
    pub routes: Vec<RouteRuleDefinition>,

    #[serde(default)]
    #[schema(value_type = Object)]
    pub typed_per_filter_config: HashMap<String, HttpScopedConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteRuleDefinition {
    #[validate(length(min = 1, max = 100))]
    pub name: Option<String>,

    #[validate(nested)]
    pub r#match: RouteMatchDefinition,

    pub action: RouteActionDefinition,

    #[serde(default)]
    #[schema(value_type = Object)]
    pub typed_per_filter_config: HashMap<String, HttpScopedConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteMatchDefinition {
    pub path: PathMatchDefinition,

    #[serde(default)]
    #[schema(value_type = Vec<HeaderMatchDefinition>)]
    pub headers: Vec<HeaderMatchDefinition>,

    #[serde(default)]
    #[schema(value_type = Vec<QueryParameterMatchDefinition>)]
    pub query_parameters: Vec<QueryParameterMatchDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PathMatchDefinition {
    #[schema(example = json!({"type": "exact", "value": "/health"}))]
    Exact { value: String },
    #[schema(example = json!({"type": "prefix", "value": "/api"}))]
    Prefix { value: String },
    #[schema(example = json!({"type": "regex", "value": "^/v[0-9]+/.*"}))]
    Regex { value: String },
    #[schema(example = json!({"type": "template", "template": "/api/v1/users/{user_id}"}))]
    Template { template: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeaderMatchDefinition {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub present: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryParameterMatchDefinition {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub present: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RouteActionDefinition {
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
        clusters: Vec<WeightedClusterDefinition>,
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

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeightedClusterDefinition {
    #[schema(example = "blue-canary")]
    pub name: String,
    #[schema(example = 80)]
    pub weight: u32,

    #[serde(default)]
    #[schema(value_type = Object)]
    pub typed_per_filter_config: HashMap<String, HttpScopedConfig>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteResponse {
    pub name: String,
    pub path_prefix: String,
    pub cluster_targets: String,
    pub config: RouteDefinition,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListRoutesQuery {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

// === Conversion Helpers ===

impl RouteDefinition {
    pub(super) fn to_xds_config(&self) -> Result<XdsRouteConfig, ApiError> {
        let virtual_hosts = self
            .virtual_hosts
            .iter()
            .map(VirtualHostDefinition::to_xds_config)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(XdsRouteConfig { name: self.name.clone(), virtual_hosts })
    }

    pub(super) fn from_xds_config(config: &XdsRouteConfig) -> Self {
        RouteDefinition {
            name: config.name.clone(),
            virtual_hosts: config
                .virtual_hosts
                .iter()
                .map(VirtualHostDefinition::from_xds_config)
                .collect(),
        }
    }
}

impl VirtualHostDefinition {
    fn to_xds_config(&self) -> Result<XdsVirtualHostConfig, ApiError> {
        let routes = self
            .routes
            .iter()
            .map(RouteRuleDefinition::to_xds_config)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(XdsVirtualHostConfig {
            name: self.name.clone(),
            domains: self.domains.clone(),
            routes,
            typed_per_filter_config: self.typed_per_filter_config.clone(),
        })
    }

    fn from_xds_config(config: &XdsVirtualHostConfig) -> Self {
        VirtualHostDefinition {
            name: config.name.clone(),
            domains: config.domains.clone(),
            routes: config.routes.iter().map(RouteRuleDefinition::from_xds_config).collect(),
            typed_per_filter_config: config.typed_per_filter_config.clone(),
        }
    }
}

impl RouteRuleDefinition {
    fn to_xds_config(&self) -> Result<XdsRouteRule, ApiError> {
        Ok(XdsRouteRule {
            name: self.name.clone(),
            r#match: self.r#match.to_xds_config()?,
            action: self.action.to_xds_config()?,
            typed_per_filter_config: self.typed_per_filter_config.clone(),
        })
    }

    fn from_xds_config(config: &XdsRouteRule) -> Self {
        RouteRuleDefinition {
            name: config.name.clone(),
            r#match: RouteMatchDefinition::from_xds_config(&config.r#match),
            action: RouteActionDefinition::from_xds_config(&config.action),
            typed_per_filter_config: config.typed_per_filter_config.clone(),
        }
    }
}

impl RouteMatchDefinition {
    fn to_xds_config(&self) -> Result<XdsRouteMatchConfig, ApiError> {
        let headers = if self.headers.is_empty() {
            None
        } else {
            Some(self.headers.iter().map(HeaderMatchDefinition::to_xds_config).collect())
        };

        let query_parameters = if self.query_parameters.is_empty() {
            None
        } else {
            Some(
                self.query_parameters
                    .iter()
                    .map(QueryParameterMatchDefinition::to_xds_config)
                    .collect(),
            )
        };

        Ok(XdsRouteMatchConfig { path: self.path.to_xds_config(), headers, query_parameters })
    }

    fn from_xds_config(config: &XdsRouteMatchConfig) -> Self {
        RouteMatchDefinition {
            path: PathMatchDefinition::from_xds_config(&config.path),
            headers: config
                .headers
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(HeaderMatchDefinition::from_xds_config)
                .collect(),
            query_parameters: config
                .query_parameters
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(QueryParameterMatchDefinition::from_xds_config)
                .collect(),
        }
    }
}

impl PathMatchDefinition {
    fn to_xds_config(&self) -> XdsPathMatch {
        match self {
            PathMatchDefinition::Exact { value } => XdsPathMatch::Exact(value.clone()),
            PathMatchDefinition::Prefix { value } => XdsPathMatch::Prefix(value.clone()),
            PathMatchDefinition::Regex { value } => XdsPathMatch::Regex(value.clone()),
            PathMatchDefinition::Template { template } => XdsPathMatch::Template(template.clone()),
        }
    }

    fn from_xds_config(path: &XdsPathMatch) -> Self {
        match path {
            XdsPathMatch::Exact(value) => PathMatchDefinition::Exact { value: value.clone() },
            XdsPathMatch::Prefix(value) => PathMatchDefinition::Prefix { value: value.clone() },
            XdsPathMatch::Regex(value) => PathMatchDefinition::Regex { value: value.clone() },
            XdsPathMatch::Template(value) => {
                PathMatchDefinition::Template { template: value.clone() }
            }
        }
    }
}

impl HeaderMatchDefinition {
    fn to_xds_config(&self) -> XdsHeaderMatchConfig {
        XdsHeaderMatchConfig {
            name: self.name.clone(),
            value: self.value.clone(),
            regex: self.regex.clone(),
            present: self.present,
        }
    }

    fn from_xds_config(config: XdsHeaderMatchConfig) -> Self {
        HeaderMatchDefinition {
            name: config.name,
            value: config.value,
            regex: config.regex,
            present: config.present,
        }
    }
}

impl QueryParameterMatchDefinition {
    fn to_xds_config(&self) -> XdsQueryParameterMatchConfig {
        XdsQueryParameterMatchConfig {
            name: self.name.clone(),
            value: self.value.clone(),
            regex: self.regex.clone(),
            present: self.present,
        }
    }

    fn from_xds_config(config: XdsQueryParameterMatchConfig) -> Self {
        QueryParameterMatchDefinition {
            name: config.name,
            value: config.value,
            regex: config.regex,
            present: config.present,
        }
    }
}

impl RouteActionDefinition {
    fn to_xds_config(&self) -> Result<XdsRouteActionConfig, ApiError> {
        match self {
            RouteActionDefinition::Forward {
                cluster,
                timeout_seconds,
                prefix_rewrite,
                template_rewrite,
            } => Ok(XdsRouteActionConfig::Cluster {
                name: cluster.clone(),
                timeout: *timeout_seconds,
                prefix_rewrite: prefix_rewrite.clone(),
                path_template_rewrite: template_rewrite.clone(),
            }),
            RouteActionDefinition::Weighted { clusters, total_weight } => {
                if clusters.is_empty() {
                    return Err(ApiError::from(Error::validation(
                        "Weighted route must include at least one cluster",
                    )));
                }

                let weights = clusters
                    .iter()
                    .map(|cluster| XdsWeightedClusterConfig {
                        name: cluster.name.clone(),
                        weight: cluster.weight,
                        typed_per_filter_config: cluster.typed_per_filter_config.clone(),
                    })
                    .collect();

                Ok(XdsRouteActionConfig::WeightedClusters {
                    clusters: weights,
                    total_weight: *total_weight,
                })
            }
            RouteActionDefinition::Redirect { host_redirect, path_redirect, response_code } => {
                Ok(XdsRouteActionConfig::Redirect {
                    host_redirect: host_redirect.clone(),
                    path_redirect: path_redirect.clone(),
                    response_code: *response_code,
                })
            }
        }
    }

    fn from_xds_config(config: &XdsRouteActionConfig) -> Self {
        match config {
            XdsRouteActionConfig::Cluster {
                name,
                timeout,
                prefix_rewrite,
                path_template_rewrite,
            } => RouteActionDefinition::Forward {
                cluster: name.clone(),
                timeout_seconds: *timeout,
                prefix_rewrite: prefix_rewrite.clone(),
                template_rewrite: path_template_rewrite.clone(),
            },
            XdsRouteActionConfig::WeightedClusters { clusters, total_weight } => {
                RouteActionDefinition::Weighted {
                    clusters: clusters
                        .iter()
                        .map(|cluster| WeightedClusterDefinition {
                            name: cluster.name.clone(),
                            weight: cluster.weight,
                            typed_per_filter_config: cluster.typed_per_filter_config.clone(),
                        })
                        .collect(),
                    total_weight: *total_weight,
                }
            }
            XdsRouteActionConfig::Redirect { host_redirect, path_redirect, response_code } => {
                RouteActionDefinition::Redirect {
                    host_redirect: host_redirect.clone(),
                    path_redirect: path_redirect.clone(),
                    response_code: *response_code,
                }
            }
        }
    }
}
