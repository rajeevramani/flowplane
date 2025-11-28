use std::collections::HashMap;

use crate::validation::{
    requests::{
        ValidatedCreateRouteRequest, ValidatedHeaderMatchRequest, ValidatedInlineRouteConfigRequest,
        ValidatedQueryParameterMatchRequest, ValidatedRouteActionRequest, ValidatedRouteActionType,
        ValidatedRouteMatchRequest, ValidatedRouteRuleRequest, ValidatedVirtualHostRequest,
        ValidatedWeightedClusterRequest,
    },
    PathMatchType,
};
use crate::xds::route;

impl From<ValidatedCreateRouteRequest> for route::RouteConfig {
    fn from(validated: ValidatedCreateRouteRequest) -> Self {
        let virtual_host = route::VirtualHostConfig {
            name: format!("{}-vhost", validated.name),
            domains: vec!["*".to_string()],
            routes: vec![route::RouteRule {
                name: Some(validated.name.clone()),
                r#match: route::RouteMatchConfig {
                    path: convert_path_match(&validated.path, &validated.path_match_type),
                    headers: None,
                    query_parameters: None,
                },
                action: route::RouteActionConfig::Cluster {
                    name: validated.cluster_name,
                    timeout: validated.timeout_seconds,
                    prefix_rewrite: validated.prefix_rewrite,
                    path_template_rewrite: validated.uri_template_rewrite,
                    retry_policy: None,
                },
                typed_per_filter_config: HashMap::new(),
            }],
            typed_per_filter_config: HashMap::new(),
        };

        route::RouteConfig {
            name: validated.name,
            virtual_hosts: vec![virtual_host],
        }
    }
}

impl From<ValidatedVirtualHostRequest> for route::VirtualHostConfig {
    fn from(validated: ValidatedVirtualHostRequest) -> Self {
        Self {
            name: validated.name,
            domains: validated.domains,
            routes: validated.routes.into_iter().map(Into::into).collect(),
            typed_per_filter_config: HashMap::new(),
        }
    }
}

impl From<ValidatedRouteRuleRequest> for route::RouteRule {
    fn from(validated: ValidatedRouteRuleRequest) -> Self {
        Self {
            name: validated.name,
            r#match: validated.r#match.into(),
            action: validated.action.into(),
            typed_per_filter_config: HashMap::new(),
        }
    }
}

impl From<ValidatedRouteMatchRequest> for route::RouteMatchConfig {
    fn from(validated: ValidatedRouteMatchRequest) -> Self {
        Self {
            path: convert_path_match(&validated.path, &validated.path_match_type),
            headers: validated.headers.map(|headers| headers.into_iter().map(Into::into).collect()),
            query_parameters: validated
                .query_parameters
                .map(|params| params.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<ValidatedHeaderMatchRequest> for route::HeaderMatchConfig {
    fn from(validated: ValidatedHeaderMatchRequest) -> Self {
        Self {
            name: validated.name,
            value: validated.value,
            regex: validated.regex,
            present: validated.present,
        }
    }
}

impl From<ValidatedQueryParameterMatchRequest> for route::QueryParameterMatchConfig {
    fn from(validated: ValidatedQueryParameterMatchRequest) -> Self {
        Self {
            name: validated.name,
            value: validated.value,
            regex: validated.regex,
            present: validated.present,
        }
    }
}

impl From<ValidatedRouteActionRequest> for route::RouteActionConfig {
    fn from(validated: ValidatedRouteActionRequest) -> Self {
        match validated.action_type {
            ValidatedRouteActionType::Cluster {
                cluster_name,
                timeout_seconds,
            } => route::RouteActionConfig::Cluster {
                name: cluster_name,
                timeout: timeout_seconds,
                prefix_rewrite: None,
                path_template_rewrite: None,
                retry_policy: None,
            },
            ValidatedRouteActionType::WeightedClusters {
                clusters,
                total_weight,
            } => route::RouteActionConfig::WeightedClusters {
                clusters: clusters.into_iter().map(Into::into).collect(),
                total_weight,
            },
            ValidatedRouteActionType::Redirect {
                host_redirect,
                path_redirect,
                response_code,
            } => route::RouteActionConfig::Redirect {
                host_redirect,
                path_redirect,
                response_code,
            },
        }
    }
}

impl From<ValidatedWeightedClusterRequest> for route::WeightedClusterConfig {
    fn from(validated: ValidatedWeightedClusterRequest) -> Self {
        Self {
            name: validated.name,
            weight: validated.weight,
            typed_per_filter_config: HashMap::new(),
        }
    }
}

impl From<ValidatedInlineRouteConfigRequest> for route::RouteConfig {
    fn from(validated: ValidatedInlineRouteConfigRequest) -> Self {
        Self {
            name: validated.name,
            virtual_hosts: validated.virtual_hosts.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<route::RouteConfig> for ValidatedInlineRouteConfigRequest {
    fn from(route_config: route::RouteConfig) -> Self {
        Self {
            name: route_config.name,
            virtual_hosts: route_config.virtual_hosts.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<route::VirtualHostConfig> for ValidatedVirtualHostRequest {
    fn from(virtual_host_config: route::VirtualHostConfig) -> Self {
        Self {
            name: virtual_host_config.name,
            domains: virtual_host_config.domains,
            routes: virtual_host_config.routes.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<route::RouteRule> for ValidatedRouteRuleRequest {
    fn from(route_rule: route::RouteRule) -> Self {
        Self {
            name: route_rule.name,
            r#match: route_rule.r#match.into(),
            action: route_rule.action.into(),
        }
    }
}

impl From<route::RouteMatchConfig> for ValidatedRouteMatchRequest {
    fn from(route_match_config: route::RouteMatchConfig) -> Self {
        let (path, path_match_type) = convert_path_match_reverse(&route_match_config.path);

        Self {
            path,
            path_match_type,
            headers: route_match_config
                .headers
                .map(|headers| headers.into_iter().map(Into::into).collect()),
            query_parameters: route_match_config
                .query_parameters
                .map(|params| params.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<route::HeaderMatchConfig> for ValidatedHeaderMatchRequest {
    fn from(header_match_config: route::HeaderMatchConfig) -> Self {
        Self {
            name: header_match_config.name,
            value: header_match_config.value,
            regex: header_match_config.regex,
            present: header_match_config.present,
        }
    }
}

impl From<route::QueryParameterMatchConfig> for ValidatedQueryParameterMatchRequest {
    fn from(query_param_config: route::QueryParameterMatchConfig) -> Self {
        Self {
            name: query_param_config.name,
            value: query_param_config.value,
            regex: query_param_config.regex,
            present: query_param_config.present,
        }
    }
}

impl From<route::RouteActionConfig> for ValidatedRouteActionRequest {
    fn from(route_action_config: route::RouteActionConfig) -> Self {
        let action_type = match route_action_config {
            route::RouteActionConfig::Cluster {
                name,
                timeout,
                ..
            } => ValidatedRouteActionType::Cluster {
                cluster_name: name,
                timeout_seconds: timeout,
            },
            route::RouteActionConfig::WeightedClusters {
                clusters,
                total_weight,
            } => ValidatedRouteActionType::WeightedClusters {
                clusters: clusters.into_iter().map(Into::into).collect(),
                total_weight,
            },
            route::RouteActionConfig::Redirect {
                host_redirect,
                path_redirect,
                response_code,
            } => ValidatedRouteActionType::Redirect {
                host_redirect,
                path_redirect,
                response_code,
            },
        };

        Self { action_type }
    }
}

impl From<route::WeightedClusterConfig> for ValidatedWeightedClusterRequest {
    fn from(weighted_cluster_config: route::WeightedClusterConfig) -> Self {
        Self {
            name: weighted_cluster_config.name,
            weight: weighted_cluster_config.weight,
        }
    }
}

fn convert_path_match(path: &str, match_type: &PathMatchType) -> route::PathMatch {
    match match_type {
        PathMatchType::Exact => route::PathMatch::Exact(path.to_string()),
        PathMatchType::Prefix => route::PathMatch::Prefix(path.to_string()),
        PathMatchType::Regex => route::PathMatch::Regex(path.to_string()),
        PathMatchType::UriTemplate => route::PathMatch::Template(path.to_string()),
    }
}

fn convert_path_match_reverse(path_match: &route::PathMatch) -> (String, PathMatchType) {
    match path_match {
        route::PathMatch::Exact(path) => (path.clone(), PathMatchType::Exact),
        route::PathMatch::Prefix(path) => (path.clone(), PathMatchType::Prefix),
        route::PathMatch::Regex(path) => (path.clone(), PathMatchType::Regex),
        route::PathMatch::Template(path) => (path.clone(), PathMatchType::UriTemplate),
    }
}
