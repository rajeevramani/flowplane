//! Route management using envoy-types
//!
//! This module provides functionality for creating and managing Envoy route configurations
//! using the proper envoy-types protobuf definitions.

use envoy_types::pb::envoy::config::core::v3::TypedExtensionConfig;
use envoy_types::pb::envoy::config::route::v3::{
    route_action::ClusterSpecifier, route_match::PathSpecifier, Route, RouteAction,
    RouteConfiguration, RouteMatch, VirtualHost,
};
use envoy_types::pb::envoy::extensions::path::r#match::uri_template::v3::UriTemplateMatchConfig;
use envoy_types::pb::envoy::extensions::path::rewrite::uri_template::v3::UriTemplateRewriteConfig;
use envoy_types::pb::google::protobuf::Any;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::xds::filters::http::HttpScopedConfig;

/// REST API representation of a route configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub name: String,
    pub virtual_hosts: Vec<VirtualHostConfig>,
}

/// REST API representation of a virtual host
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualHostConfig {
    pub name: String,
    pub domains: Vec<String>,
    pub routes: Vec<RouteRule>,
    #[serde(default)]
    pub typed_per_filter_config: HashMap<String, HttpScopedConfig>,
}

/// REST API representation of a route rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRule {
    pub name: Option<String>,
    pub r#match: RouteMatchConfig,
    pub action: RouteActionConfig,
    #[serde(default)]
    pub typed_per_filter_config: HashMap<String, HttpScopedConfig>,
}

/// REST API representation of route matching criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteMatchConfig {
    pub path: PathMatch,
    pub headers: Option<Vec<HeaderMatchConfig>>,
    pub query_parameters: Option<Vec<QueryParameterMatchConfig>>,
}

/// REST API representation of path matching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PathMatch {
    Exact(String),
    Prefix(String),
    Regex(String),
    Template(String),
}

/// REST API representation of header matching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderMatchConfig {
    pub name: String,
    pub value: Option<String>,
    pub regex: Option<String>,
    pub present: Option<bool>,
}

/// REST API representation of query parameter matching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParameterMatchConfig {
    pub name: String,
    pub value: Option<String>,
    pub regex: Option<String>,
    pub present: Option<bool>,
}

/// REST API representation of route actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RouteActionConfig {
    Cluster {
        name: String,
        timeout: Option<u64>, // seconds
        prefix_rewrite: Option<String>,
        path_template_rewrite: Option<String>,
    },
    WeightedClusters {
        clusters: Vec<WeightedClusterConfig>,
        total_weight: Option<u32>,
    },
    Redirect {
        host_redirect: Option<String>,
        path_redirect: Option<String>,
        response_code: Option<u32>,
    },
}

/// REST API representation of weighted cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedClusterConfig {
    pub name: String,
    pub weight: u32,
    #[serde(default)]
    pub typed_per_filter_config: HashMap<String, HttpScopedConfig>,
}

impl RouteConfig {
    /// Convert REST API RouteConfig to envoy-types RouteConfiguration
    pub fn to_envoy_route_configuration(&self) -> Result<RouteConfiguration, crate::Error> {
        let virtual_hosts: Result<Vec<VirtualHost>, crate::Error> = self
            .virtual_hosts
            .iter()
            .map(|vh| vh.to_envoy_virtual_host())
            .collect();

        let route_config = RouteConfiguration {
            name: self.name.clone(),
            virtual_hosts: virtual_hosts?,
            ..Default::default()
        };

        Ok(route_config)
    }
}

impl VirtualHostConfig {
    /// Convert REST API VirtualHostConfig to envoy-types VirtualHost
    fn to_envoy_virtual_host(&self) -> Result<VirtualHost, crate::Error> {
        let routes: Result<Vec<Route>, crate::Error> =
            self.routes.iter().map(|r| r.to_envoy_route()).collect();

        let mut virtual_host = VirtualHost {
            name: self.name.clone(),
            domains: self.domains.clone(),
            routes: routes?,
            ..Default::default()
        };

        if !self.typed_per_filter_config.is_empty() {
            virtual_host.typed_per_filter_config = self
                .typed_per_filter_config
                .iter()
                .map(|(name, config)| config.to_any().map(|any| (name.clone(), any)))
                .collect::<Result<_, crate::Error>>()?;
        }

        Ok(virtual_host)
    }
}

impl RouteRule {
    /// Convert REST API RouteRule to envoy-types Route
    fn to_envoy_route(&self) -> Result<Route, crate::Error> {
        let mut route = Route {
            name: self.name.clone().unwrap_or_default(),
            r#match: Some(self.r#match.to_envoy_route_match()?),
            action: Some(self.action.to_envoy_route_action()?),
            ..Default::default()
        };

        if !self.typed_per_filter_config.is_empty() {
            route.typed_per_filter_config = self
                .typed_per_filter_config
                .iter()
                .map(|(name, config)| config.to_any().map(|any| (name.clone(), any)))
                .collect::<Result<_, crate::Error>>()?;
        }

        Ok(route)
    }
}

impl RouteMatchConfig {
    /// Convert REST API RouteMatchConfig to envoy-types RouteMatch
    fn to_envoy_route_match(&self) -> Result<RouteMatch, crate::Error> {
        let path_specifier = match &self.path {
            PathMatch::Exact(path) => PathSpecifier::Path(path.clone()),
            PathMatch::Prefix(prefix) => PathSpecifier::Prefix(prefix.clone()),
            PathMatch::Regex(regex) => PathSpecifier::SafeRegex(
                envoy_types::pb::envoy::r#type::matcher::v3::RegexMatcher {
                    regex: regex.clone(),
                    ..Default::default()
                },
            ),
            PathMatch::Template(template) => {
                let config = UriTemplateMatchConfig {
                    path_template: template.clone(),
                };

                let typed_config = TypedExtensionConfig {
                    name: "envoy.path.match.uri_template".to_string(),
                    typed_config: Some(Any {
                        type_url:
                            "type.googleapis.com/envoy.extensions.path.match.uri_template.v3.UriTemplateMatchConfig"
                                .to_string(),
                        value: config.encode_to_vec(),
                    }),
                };

                PathSpecifier::PathMatchPolicy(typed_config)
            }
        };

        let route_match = RouteMatch {
            path_specifier: Some(path_specifier),
            // TODO: Add header and query parameter matching
            ..Default::default()
        };

        Ok(route_match)
    }
}

impl RouteActionConfig {
    /// Convert REST API RouteActionConfig to envoy-types route action
    fn to_envoy_route_action(
        &self,
    ) -> Result<envoy_types::pb::envoy::config::route::v3::route::Action, crate::Error> {
        let action = match self {
            RouteActionConfig::Cluster {
                name,
                timeout,
                prefix_rewrite,
                path_template_rewrite,
            } => {
                #[allow(deprecated)]
                let mut route_action = RouteAction {
                    cluster_specifier: Some(ClusterSpecifier::Cluster(name.clone())),
                    timeout: timeout.map(|t| envoy_types::pb::google::protobuf::Duration {
                        seconds: t as i64,
                        nanos: 0,
                    }),
                    ..Default::default()
                };

                if let Some(prefix) = prefix_rewrite {
                    route_action.prefix_rewrite = prefix.clone();
                }

                if let Some(template) = path_template_rewrite {
                    let rewrite_config = UriTemplateRewriteConfig {
                        path_template_rewrite: template.clone(),
                    };

                    let typed_config = TypedExtensionConfig {
                        name: "envoy.path.rewrite.uri_template".to_string(),
                        typed_config: Some(Any {
                            type_url:
                                "type.googleapis.com/envoy.extensions.path.rewrite.uri_template.v3.UriTemplateRewriteConfig"
                                    .to_string(),
                            value: rewrite_config.encode_to_vec(),
                        }),
                    };

                    route_action.path_rewrite_policy = Some(typed_config);
                }

                envoy_types::pb::envoy::config::route::v3::route::Action::Route(route_action)
            }
            RouteActionConfig::WeightedClusters {
                clusters,
                total_weight,
            } => {
                let weighted_clusters: Vec<
                    envoy_types::pb::envoy::config::route::v3::weighted_cluster::ClusterWeight,
                > = clusters
                    .iter()
                    .map(|wc| -> Result<_, crate::Error> {
                        let mut cluster_weight =
                            envoy_types::pb::envoy::config::route::v3::weighted_cluster::ClusterWeight {
                                name: wc.name.clone(),
                                weight: Some(envoy_types::pb::google::protobuf::UInt32Value {
                                    value: wc.weight,
                                }),
                                ..Default::default()
                            };

                        if !wc.typed_per_filter_config.is_empty() {
                            cluster_weight.typed_per_filter_config = wc
                                .typed_per_filter_config
                                .iter()
                                .map(|(name, config)| config.to_any().map(|any| (name.clone(), any)))
                                .collect::<Result<_, crate::Error>>()?;
                        }

                        Ok(cluster_weight)
                    })
                    .collect::<Result<_, crate::Error>>()?;

                let route_action = {
                    #[allow(deprecated)]
                    RouteAction {
                        cluster_specifier: Some(ClusterSpecifier::WeightedClusters(
                            envoy_types::pb::envoy::config::route::v3::WeightedCluster {
                                clusters: weighted_clusters,
                                total_weight: total_weight.map(|w| {
                                    envoy_types::pb::google::protobuf::UInt32Value { value: w }
                                }),
                                ..Default::default()
                            },
                        )),
                        ..Default::default()
                    }
                };

                envoy_types::pb::envoy::config::route::v3::route::Action::Route(route_action)
            }
            RouteActionConfig::Redirect {
                host_redirect,
                path_redirect,
                response_code,
            } => {
                let redirect_code = response_code
                    .and_then(|c| {
                        envoy_types::pb::envoy::config::route::v3::redirect_action::RedirectResponseCode::try_from(c as i32)
                            .ok()
                    })
                    .unwrap_or(
                        envoy_types::pb::envoy::config::route::v3::redirect_action::RedirectResponseCode::MovedPermanently,
                    );

                let redirect_action = envoy_types::pb::envoy::config::route::v3::RedirectAction {
                    host_redirect: host_redirect.clone().unwrap_or_default(),
                    path_rewrite_specifier: path_redirect
                        .clone()
                        .map(envoy_types::pb::envoy::config::route::v3::redirect_action::PathRewriteSpecifier::PathRedirect),
                    response_code: redirect_code as i32,
                    ..Default::default()
                };

                envoy_types::pb::envoy::config::route::v3::route::Action::Redirect(redirect_action)
            }
        };

        Ok(action)
    }
}

/// Route manager for handling route operations
#[derive(Debug)]
pub struct RouteManager {
    routes: HashMap<String, RouteConfiguration>,
}

impl RouteManager {
    /// Create a new route manager
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
        }
    }

    /// Add or update a route configuration
    pub fn upsert_route(&mut self, config: RouteConfig) -> Result<(), crate::Error> {
        let route_config = config.to_envoy_route_configuration()?;
        self.routes.insert(route_config.name.clone(), route_config);
        Ok(())
    }

    /// Remove a route configuration
    pub fn remove_route(&mut self, name: &str) -> Option<RouteConfiguration> {
        self.routes.remove(name)
    }

    /// Get a route configuration by name
    pub fn get_route(&self, name: &str) -> Option<&RouteConfiguration> {
        self.routes.get(name)
    }

    /// Get all route configurations
    pub fn get_all_routes(&self) -> Vec<RouteConfiguration> {
        self.routes.values().cloned().collect()
    }

    /// List route configuration names
    pub fn list_route_names(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
    }
}

impl Default for RouteManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xds::filters::http::{
        local_rate_limit::{
            FractionalPercentDenominator, LocalRateLimitConfig, RuntimeFractionalPercentConfig,
            TokenBucketConfig,
        },
        HttpScopedConfig,
    };
    use crate::xds::filters::TypedConfig;
    use envoy_types::pb::envoy::extensions::filters::http::local_ratelimit::v3::LocalRateLimit;
    use envoy_types::pb::envoy::r#type::v3::TokenBucket;
    use envoy_types::pb::google::protobuf::{Duration, UInt32Value};
    use std::collections::HashMap;

    #[test]
    fn test_route_config_conversion() {
        let config = RouteConfig {
            name: "test-route".to_string(),
            virtual_hosts: vec![VirtualHostConfig {
                name: "test-vhost".to_string(),
                domains: vec!["example.com".to_string(), "*.example.com".to_string()],
                routes: vec![RouteRule {
                    name: Some("api-route".to_string()),
                    r#match: RouteMatchConfig {
                        path: PathMatch::Prefix("/api".to_string()),
                        headers: None,
                        query_parameters: None,
                    },
                    action: RouteActionConfig::Cluster {
                        name: "api-cluster".to_string(),
                        timeout: Some(30),
                        prefix_rewrite: None,
                        path_template_rewrite: None,
                    },
                    typed_per_filter_config: HashMap::new(),
                }],
                typed_per_filter_config: HashMap::new(),
            }],
        };

        let route_config = config
            .to_envoy_route_configuration()
            .expect("Failed to convert route config");

        assert_eq!(route_config.name, "test-route");
        assert_eq!(route_config.virtual_hosts.len(), 1);

        let vhost = &route_config.virtual_hosts[0];
        assert_eq!(vhost.name, "test-vhost");
        assert_eq!(vhost.domains.len(), 2);
        assert_eq!(vhost.routes.len(), 1);

        let route = &vhost.routes[0];
        assert_eq!(route.name, "api-route");
        assert!(route.r#match.is_some());
        assert!(route.action.is_some());
    }

    #[test]
    fn test_route_manager() {
        let mut manager = RouteManager::new();

        let config = RouteConfig {
            name: "test-route".to_string(),
            virtual_hosts: vec![VirtualHostConfig {
                name: "test-vhost".to_string(),
                domains: vec!["test.com".to_string()],
                routes: vec![RouteRule {
                    name: None,
                    r#match: RouteMatchConfig {
                        path: PathMatch::Exact("/health".to_string()),
                        headers: None,
                        query_parameters: None,
                    },
                    action: RouteActionConfig::Cluster {
                        name: "health-cluster".to_string(),
                        timeout: None,
                        prefix_rewrite: None,
                        path_template_rewrite: None,
                    },
                    typed_per_filter_config: HashMap::new(),
                }],
                typed_per_filter_config: HashMap::new(),
            }],
        };

        manager.upsert_route(config).expect("Failed to add route");

        assert!(manager.get_route("test-route").is_some());
        assert_eq!(manager.list_route_names().len(), 1);

        let removed = manager.remove_route("test-route");
        assert!(removed.is_some());
        assert_eq!(manager.list_route_names().len(), 0);
    }

    #[test]
    fn test_path_matching() {
        let exact_match = RouteMatchConfig {
            path: PathMatch::Exact("/exact".to_string()),
            headers: None,
            query_parameters: None,
        };

        let prefix_match = RouteMatchConfig {
            path: PathMatch::Prefix("/prefix".to_string()),
            headers: None,
            query_parameters: None,
        };

        let regex_match = RouteMatchConfig {
            path: PathMatch::Regex(r"^/api/v\d+/.*".to_string()),
            headers: None,
            query_parameters: None,
        };

        let exact_envoy = exact_match
            .to_envoy_route_match()
            .expect("Failed to convert exact match");
        let prefix_envoy = prefix_match
            .to_envoy_route_match()
            .expect("Failed to convert prefix match");
        let regex_envoy = regex_match
            .to_envoy_route_match()
            .expect("Failed to convert regex match");

        assert!(matches!(
            exact_envoy.path_specifier,
            Some(PathSpecifier::Path(_))
        ));
        assert!(matches!(
            prefix_envoy.path_specifier,
            Some(PathSpecifier::Prefix(_))
        ));
        assert!(matches!(
            regex_envoy.path_specifier,
            Some(PathSpecifier::SafeRegex(_))
        ));
    }

    #[test]
    fn test_typed_per_filter_config_conversion() {
        let rate_limit_proto = LocalRateLimit {
            stat_prefix: "vh".into(),
            token_bucket: Some(TokenBucket {
                max_tokens: 5,
                tokens_per_fill: Some(UInt32Value { value: 5 }),
                fill_interval: Some(Duration {
                    seconds: 1,
                    nanos: 0,
                }),
            }),
            ..Default::default()
        };

        let typed_config = TypedConfig::from_message(
            "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit",
            &rate_limit_proto,
        );

        let structured_override = HttpScopedConfig::LocalRateLimit(LocalRateLimitConfig {
            stat_prefix: "route".into(),
            token_bucket: Some(TokenBucketConfig {
                max_tokens: 10,
                tokens_per_fill: Some(10),
                fill_interval_ms: 60_000,
            }),
            status_code: Some(429),
            filter_enabled: Some(RuntimeFractionalPercentConfig {
                runtime_key: None,
                numerator: 100,
                denominator: FractionalPercentDenominator::Hundred,
            }),
            filter_enforced: Some(RuntimeFractionalPercentConfig {
                runtime_key: None,
                numerator: 100,
                denominator: FractionalPercentDenominator::Hundred,
            }),
            per_downstream_connection: Some(false),
            rate_limited_as_resource_exhausted: None,
            max_dynamic_descriptors: None,
            always_consume_default_token_bucket: Some(false),
        });

        let route_config = RouteConfig {
            name: "rate-limited".into(),
            virtual_hosts: vec![VirtualHostConfig {
                name: "vh".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: None,
                    r#match: RouteMatchConfig {
                        path: PathMatch::Prefix("/".into()),
                        headers: None,
                        query_parameters: None,
                    },
                    action: RouteActionConfig::Cluster {
                        name: "backend".into(),
                        timeout: None,
                        prefix_rewrite: None,
                        path_template_rewrite: None,
                    },
                    typed_per_filter_config: HashMap::from([(
                        "envoy.filters.http.local_ratelimit".into(),
                        structured_override,
                    )]),
                }],
                typed_per_filter_config: HashMap::from([(
                    "envoy.filters.http.local_ratelimit".into(),
                    HttpScopedConfig::Typed(typed_config.clone()),
                )]),
            }],
        };

        let envoy_route = route_config
            .to_envoy_route_configuration()
            .expect("route to envoy");

        let vhost = &envoy_route.virtual_hosts[0];
        assert!(vhost
            .typed_per_filter_config
            .contains_key("envoy.filters.http.local_ratelimit"));

        let route = &vhost.routes[0];
        assert!(route
            .typed_per_filter_config
            .contains_key("envoy.filters.http.local_ratelimit"));
    }
}
