//! # Type Conversions
//!
//! This module provides conversion functions between validated request types
//! and internal types used by the Magaya control plane. These conversions
//! ensure that validated data is properly transformed to internal representations.

use crate::validation::requests::*;
use crate::xds::{cluster, route, listener};

// ============================================================================
// CLUSTER CONVERSIONS
// ============================================================================

impl From<ValidatedCreateClusterRequest> for cluster::ClusterConfig {
    fn from(validated: ValidatedCreateClusterRequest) -> Self {
        Self {
            name: validated.name,
            endpoints: validated.endpoints.into_iter().map(Into::into).collect(),
            load_balancing_policy: validated.lb_policy
                .map(|policy| match policy.as_str() {
                    "ROUND_ROBIN" => cluster::LoadBalancingPolicy::RoundRobin,
                    "LEAST_REQUEST" => cluster::LoadBalancingPolicy::LeastRequest,
                    "RANDOM" => cluster::LoadBalancingPolicy::Random,
                    "PASS_THROUGH" => cluster::LoadBalancingPolicy::PassThrough,
                    _ => cluster::LoadBalancingPolicy::RoundRobin, // Default fallback
                })
                .unwrap_or(cluster::LoadBalancingPolicy::RoundRobin),
            connect_timeout: validated.connect_timeout_seconds,
            health_checks: validated.health_check.map(|hc| vec![hc.into()]),
        }
    }
}

impl From<ValidatedUpdateClusterRequest> for cluster::ClusterConfig {
    fn from(validated: ValidatedUpdateClusterRequest) -> Self {
        Self {
            name: String::new(), // Will be set by the update handler
            endpoints: validated.endpoints.into_iter().map(Into::into).collect(),
            load_balancing_policy: validated.lb_policy
                .map(|policy| match policy.as_str() {
                    "ROUND_ROBIN" => cluster::LoadBalancingPolicy::RoundRobin,
                    "LEAST_REQUEST" => cluster::LoadBalancingPolicy::LeastRequest,
                    "RANDOM" => cluster::LoadBalancingPolicy::Random,
                    "PASS_THROUGH" => cluster::LoadBalancingPolicy::PassThrough,
                    _ => cluster::LoadBalancingPolicy::RoundRobin,
                })
                .unwrap_or(cluster::LoadBalancingPolicy::RoundRobin),
            connect_timeout: validated.connect_timeout_seconds,
            health_checks: validated.health_check.map(|hc| vec![hc.into()]),
        }
    }
}

impl From<ValidatedEndpointRequest> for cluster::EndpointConfig {
    fn from(validated: ValidatedEndpointRequest) -> Self {
        Self {
            address: validated.address,
            port: validated.port,
            weight: validated.weight,
        }
    }
}

impl From<ValidatedHealthCheckRequest> for cluster::HealthCheckConfig {
    fn from(validated: ValidatedHealthCheckRequest) -> Self {
        Self {
            timeout: validated.timeout_seconds,
            interval: validated.interval_seconds,
            healthy_threshold: validated.healthy_threshold,
            unhealthy_threshold: validated.unhealthy_threshold,
            path: validated.path,
        }
    }
}

// ============================================================================
// ROUTE CONVERSIONS
// ============================================================================

impl From<ValidatedCreateRouteRequest> for route::RouteConfig {
    fn from(validated: ValidatedCreateRouteRequest) -> Self {
        // Create a simple route configuration with one virtual host
        let virtual_host = route::VirtualHostConfig {
            name: format!("{}-vhost", validated.name),
            domains: vec!["*".to_string()], // Default catch-all domain
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
                },
            }],
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
        }
    }
}

impl From<ValidatedRouteRuleRequest> for route::RouteRule {
    fn from(validated: ValidatedRouteRuleRequest) -> Self {
        Self {
            name: validated.name,
            r#match: validated.r#match.into(),
            action: validated.action.into(),
        }
    }
}

impl From<ValidatedRouteMatchRequest> for route::RouteMatchConfig {
    fn from(validated: ValidatedRouteMatchRequest) -> Self {
        Self {
            path: convert_path_match(&validated.path, &validated.path_match_type),
            headers: validated.headers.map(|headers| {
                headers.into_iter().map(Into::into).collect()
            }),
            query_parameters: validated.query_parameters.map(|params| {
                params.into_iter().map(Into::into).collect()
            }),
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
            ValidatedRouteActionType::Cluster { cluster_name, timeout_seconds } => {
                route::RouteActionConfig::Cluster {
                    name: cluster_name,
                    timeout: timeout_seconds,
                }
            }
            ValidatedRouteActionType::WeightedClusters { clusters, total_weight } => {
                route::RouteActionConfig::WeightedClusters {
                    clusters: clusters.into_iter().map(Into::into).collect(),
                    total_weight,
                }
            }
            ValidatedRouteActionType::Redirect { host_redirect, path_redirect, response_code } => {
                route::RouteActionConfig::Redirect {
                    host_redirect,
                    path_redirect,
                    response_code,
                }
            }
        }
    }
}

impl From<ValidatedWeightedClusterRequest> for route::WeightedClusterConfig {
    fn from(validated: ValidatedWeightedClusterRequest) -> Self {
        Self {
            name: validated.name,
            weight: validated.weight,
        }
    }
}

// ============================================================================
// LISTENER CONVERSIONS
// ============================================================================

impl From<ValidatedCreateListenerRequest> for listener::ListenerConfig {
    fn from(validated: ValidatedCreateListenerRequest) -> Self {
        Self {
            name: validated.name,
            address: validated.address,
            port: validated.port,
            filter_chains: validated.filter_chains.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ValidatedUpdateListenerRequest> for listener::ListenerConfig {
    fn from(validated: ValidatedUpdateListenerRequest) -> Self {
        Self {
            name: String::new(), // Will be set by the update handler
            address: validated.address,
            port: validated.port,
            filter_chains: validated.filter_chains.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ValidatedFilterChainRequest> for listener::FilterChainConfig {
    fn from(validated: ValidatedFilterChainRequest) -> Self {
        Self {
            name: validated.name,
            filters: validated.filters.into_iter().map(Into::into).collect(),
            tls_context: validated.tls_context.map(Into::into),
        }
    }
}

impl From<ValidatedFilterRequest> for listener::FilterConfig {
    fn from(validated: ValidatedFilterRequest) -> Self {
        Self {
            name: validated.name,
            filter_type: validated.filter_type.into(),
        }
    }
}

impl From<ValidatedFilterType> for listener::FilterType {
    fn from(validated: ValidatedFilterType) -> Self {
        match validated {
            ValidatedFilterType::HttpConnectionManager {
                route_config_name,
                inline_route_config,
                access_log,
                tracing,
            } => {
                listener::FilterType::HttpConnectionManager {
                    route_config_name,
                    inline_route_config: inline_route_config.map(Into::into),
                    access_log: access_log.map(Into::into),
                    tracing: tracing.map(Into::into),
                }
            }
            ValidatedFilterType::TcpProxy { cluster, access_log } => {
                listener::FilterType::TcpProxy {
                    cluster,
                    access_log: access_log.map(Into::into),
                }
            }
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

impl From<ValidatedTlsContextRequest> for listener::TlsContextConfig {
    fn from(validated: ValidatedTlsContextRequest) -> Self {
        Self {
            cert_chain_file: validated.cert_chain_file,
            private_key_file: validated.private_key_file,
            ca_cert_file: validated.ca_cert_file,
            require_client_certificate: validated.require_client_certificate,
        }
    }
}

impl From<ValidatedAccessLogRequest> for listener::AccessLogConfig {
    fn from(validated: ValidatedAccessLogRequest) -> Self {
        Self {
            path: validated.path,
            format: validated.format,
        }
    }
}

impl From<ValidatedTracingRequest> for listener::TracingConfig {
    fn from(validated: ValidatedTracingRequest) -> Self {
        Self {
            provider: validated.provider,
            config: validated.config,
        }
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Convert path and match type to route PathMatch
fn convert_path_match(path: &str, match_type: &crate::validation::PathMatchType) -> route::PathMatch {
    match match_type {
        crate::validation::PathMatchType::Exact => route::PathMatch::Exact(path.to_string()),
        crate::validation::PathMatchType::Prefix => route::PathMatch::Prefix(path.to_string()),
        crate::validation::PathMatchType::Regex => route::PathMatch::Regex(path.to_string()),
        crate::validation::PathMatchType::UriTemplate => {
            // For URI template, we'll use regex as the closest match since
            // route::PathMatch doesn't have a UriTemplate variant
            route::PathMatch::Regex(path.to_string())
        }
    }
}

// ============================================================================
// REVERSE CONVERSIONS (for API responses)
// ============================================================================

impl From<cluster::ClusterConfig> for ValidatedCreateClusterRequest {
    fn from(cluster_config: cluster::ClusterConfig) -> Self {
        Self {
            name: cluster_config.name,
            endpoints: cluster_config.endpoints.into_iter().map(Into::into).collect(),
            lb_policy: Some(match cluster_config.load_balancing_policy {
                cluster::LoadBalancingPolicy::RoundRobin => "ROUND_ROBIN".to_string(),
                cluster::LoadBalancingPolicy::LeastRequest => "LEAST_REQUEST".to_string(),
                cluster::LoadBalancingPolicy::Random => "RANDOM".to_string(),
                cluster::LoadBalancingPolicy::PassThrough => "PASS_THROUGH".to_string(),
            }),
            connect_timeout_seconds: cluster_config.connect_timeout,
            health_check: cluster_config.health_checks
                .and_then(|checks| checks.into_iter().next())
                .map(Into::into),
            circuit_breaker: None, // Not implemented in current cluster config
        }
    }
}

impl From<cluster::EndpointConfig> for ValidatedEndpointRequest {
    fn from(endpoint_config: cluster::EndpointConfig) -> Self {
        Self {
            address: endpoint_config.address,
            port: endpoint_config.port,
            weight: endpoint_config.weight,
        }
    }
}

impl From<cluster::HealthCheckConfig> for ValidatedHealthCheckRequest {
    fn from(health_check_config: cluster::HealthCheckConfig) -> Self {
        Self {
            timeout_seconds: health_check_config.timeout,
            interval_seconds: health_check_config.interval,
            healthy_threshold: health_check_config.healthy_threshold,
            unhealthy_threshold: health_check_config.unhealthy_threshold,
            path: health_check_config.path,
        }
    }
}

impl From<listener::ListenerConfig> for ValidatedCreateListenerRequest {
    fn from(listener_config: listener::ListenerConfig) -> Self {
        Self {
            name: listener_config.name,
            address: listener_config.address,
            port: listener_config.port,
            filter_chains: listener_config.filter_chains.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<listener::FilterChainConfig> for ValidatedFilterChainRequest {
    fn from(filter_chain_config: listener::FilterChainConfig) -> Self {
        Self {
            name: filter_chain_config.name,
            filters: filter_chain_config.filters.into_iter().map(Into::into).collect(),
            tls_context: filter_chain_config.tls_context.map(Into::into),
        }
    }
}

impl From<listener::FilterConfig> for ValidatedFilterRequest {
    fn from(filter_config: listener::FilterConfig) -> Self {
        Self {
            name: filter_config.name,
            filter_type: filter_config.filter_type.into(),
        }
    }
}

impl From<listener::FilterType> for ValidatedFilterType {
    fn from(filter_type: listener::FilterType) -> Self {
        match filter_type {
            listener::FilterType::HttpConnectionManager {
                route_config_name,
                inline_route_config,
                access_log,
                tracing,
            } => {
                ValidatedFilterType::HttpConnectionManager {
                    route_config_name,
                    inline_route_config: inline_route_config.map(Into::into),
                    access_log: access_log.map(Into::into),
                    tracing: tracing.map(Into::into),
                }
            }
            listener::FilterType::TcpProxy { cluster, access_log } => {
                ValidatedFilterType::TcpProxy {
                    cluster,
                    access_log: access_log.map(Into::into),
                }
            }
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
            headers: route_match_config.headers.map(|headers| {
                headers.into_iter().map(Into::into).collect()
            }),
            query_parameters: route_match_config.query_parameters.map(|params| {
                params.into_iter().map(Into::into).collect()
            }),
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
            route::RouteActionConfig::Cluster { name, timeout } => {
                ValidatedRouteActionType::Cluster {
                    cluster_name: name,
                    timeout_seconds: timeout,
                }
            }
            route::RouteActionConfig::WeightedClusters { clusters, total_weight } => {
                ValidatedRouteActionType::WeightedClusters {
                    clusters: clusters.into_iter().map(Into::into).collect(),
                    total_weight,
                }
            }
            route::RouteActionConfig::Redirect { host_redirect, path_redirect, response_code } => {
                ValidatedRouteActionType::Redirect {
                    host_redirect,
                    path_redirect,
                    response_code,
                }
            }
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

impl From<listener::TlsContextConfig> for ValidatedTlsContextRequest {
    fn from(tls_context_config: listener::TlsContextConfig) -> Self {
        Self {
            cert_chain_file: tls_context_config.cert_chain_file,
            private_key_file: tls_context_config.private_key_file,
            ca_cert_file: tls_context_config.ca_cert_file,
            require_client_certificate: tls_context_config.require_client_certificate,
        }
    }
}

impl From<listener::AccessLogConfig> for ValidatedAccessLogRequest {
    fn from(access_log_config: listener::AccessLogConfig) -> Self {
        Self {
            path: access_log_config.path,
            format: access_log_config.format,
        }
    }
}

impl From<listener::TracingConfig> for ValidatedTracingRequest {
    fn from(tracing_config: listener::TracingConfig) -> Self {
        Self {
            provider: tracing_config.provider,
            config: tracing_config.config,
        }
    }
}

/// Convert route PathMatch back to path string and match type
fn convert_path_match_reverse(path_match: &route::PathMatch) -> (String, crate::validation::PathMatchType) {
    match path_match {
        route::PathMatch::Exact(path) => (path.clone(), crate::validation::PathMatchType::Exact),
        route::PathMatch::Prefix(path) => (path.clone(), crate::validation::PathMatchType::Prefix),
        route::PathMatch::Regex(path) => (path.clone(), crate::validation::PathMatchType::Regex),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::PathMatchType;

    #[test]
    fn test_cluster_conversion() {
        let validated_request = ValidatedCreateClusterRequest {
            name: "test-cluster".to_string(),
            endpoints: vec![
                ValidatedEndpointRequest {
                    address: "127.0.0.1".to_string(),
                    port: 8080,
                    weight: Some(100),
                },
            ],
            lb_policy: Some("ROUND_ROBIN".to_string()),
            connect_timeout_seconds: Some(5),
            health_check: Some(ValidatedHealthCheckRequest {
                timeout_seconds: 5,
                interval_seconds: 10,
                healthy_threshold: 2,
                unhealthy_threshold: 3,
                path: Some("/health".to_string()),
            }),
            circuit_breaker: None,
        };

        let cluster_config: cluster::ClusterConfig = validated_request.into();

        assert_eq!(cluster_config.name, "test-cluster");
        assert_eq!(cluster_config.endpoints.len(), 1);
        assert_eq!(cluster_config.endpoints[0].address, "127.0.0.1");
        assert_eq!(cluster_config.endpoints[0].port, 8080);
        assert_eq!(cluster_config.endpoints[0].weight, Some(100));
        assert!(matches!(cluster_config.load_balancing_policy, cluster::LoadBalancingPolicy::RoundRobin));
        assert_eq!(cluster_config.connect_timeout, Some(5));
        assert!(cluster_config.health_checks.is_some());
    }

    #[test]
    fn test_listener_conversion() {
        let validated_request = ValidatedCreateListenerRequest {
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

        let listener_config: listener::ListenerConfig = validated_request.into();

        assert_eq!(listener_config.name, "test-listener");
        assert_eq!(listener_config.address, "0.0.0.0");
        assert_eq!(listener_config.port, 8080);
        assert_eq!(listener_config.filter_chains.len(), 1);
        assert_eq!(listener_config.filter_chains[0].filters.len(), 1);
    }

    #[test]
    fn test_route_conversion() {
        let validated_request = ValidatedCreateRouteRequest {
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

        let route_config: route::RouteConfig = validated_request.into();

        assert_eq!(route_config.name, "test-route");
        assert_eq!(route_config.virtual_hosts.len(), 1);
        assert_eq!(route_config.virtual_hosts[0].routes.len(), 1);

        let route_rule = &route_config.virtual_hosts[0].routes[0];
        assert_eq!(route_rule.name, Some("test-route".to_string()));
        assert!(matches!(route_rule.r#match.path, route::PathMatch::Prefix(_)));
        assert!(matches!(route_rule.action, route::RouteActionConfig::Cluster { .. }));
    }

    #[test]
    fn test_convert_path_match() {
        assert!(matches!(
            convert_path_match("/api", &PathMatchType::Exact),
            route::PathMatch::Exact(_)
        ));

        assert!(matches!(
            convert_path_match("/api", &PathMatchType::Prefix),
            route::PathMatch::Prefix(_)
        ));

        assert!(matches!(
            convert_path_match(r"^/api/v\d+", &PathMatchType::Regex),
            route::PathMatch::Regex(_)
        ));

        // URI template is converted to regex
        assert!(matches!(
            convert_path_match("/api/{id}", &PathMatchType::UriTemplate),
            route::PathMatch::Regex(_)
        ));
    }

    #[test]
    fn test_reverse_conversion() {
        let cluster_config = cluster::ClusterConfig {
            name: "test-cluster".to_string(),
            endpoints: vec![
                cluster::EndpointConfig {
                    address: "127.0.0.1".to_string(),
                    port: 8080,
                    weight: Some(100),
                },
            ],
            load_balancing_policy: cluster::LoadBalancingPolicy::RoundRobin,
            connect_timeout: Some(5),
            health_checks: Some(vec![
                cluster::HealthCheckConfig {
                    timeout: 5,
                    interval: 10,
                    healthy_threshold: 2,
                    unhealthy_threshold: 3,
                    path: Some("/health".to_string()),
                },
            ]),
        };

        let validated_request: ValidatedCreateClusterRequest = cluster_config.into();

        assert_eq!(validated_request.name, "test-cluster");
        assert_eq!(validated_request.endpoints.len(), 1);
        assert_eq!(validated_request.lb_policy, Some("ROUND_ROBIN".to_string()));
        assert_eq!(validated_request.connect_timeout_seconds, Some(5));
        assert!(validated_request.health_check.is_some());
    }
}