//! DRY Resource Setup Builder Pattern
//!
//! Provides a builder pattern for common test resource setup to eliminate duplication
//! across filter tests (Phases 5-9).
//!
//! Example usage:
//! ```rust,ignore
//! let resources = ResourceSetup::new(&api, &token, &team)
//!     .with_cluster("test-cluster", "127.0.0.1", mock_port)
//!     .with_route("test-route", "/testing/path")
//!     .with_listener("test-listener", 10020)
//!     .with_filter("test-filter", "local_rate_limit", rate_limit_config)
//!     .build()
//!     .await?;
//! ```

use anyhow::Result;
use serde_json::Value;

use super::api_client::{
    ApiClient, ClusterEndpoint, ClusterResponse, CreateClusterRequest, CreateListenerRequest,
    CreateRouteRequest, FilterInstallationResponse, FilterResponse, ListenerFilterChainInput,
    ListenerFilterInput, ListenerFilterTypeInput, ListenerResponse, PathMatch, Route, RouteAction,
    RouteConfigResponse, RouteMatch, VirtualHost,
};
use super::timeout::{with_timeout, TestTimeout};

/// Configuration for a cluster to be created
#[derive(Clone)]
pub struct ClusterConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub circuit_breakers: Option<Value>,
    pub outlier_detection: Option<Value>,
}

impl ClusterConfig {
    pub fn new(name: &str, host: &str, port: u16) -> Self {
        Self {
            name: name.to_string(),
            host: host.to_string(),
            port,
            circuit_breakers: None,
            outlier_detection: None,
        }
    }

    pub fn with_circuit_breakers(mut self, config: Value) -> Self {
        self.circuit_breakers = Some(config);
        self
    }

    pub fn with_outlier_detection(mut self, config: Value) -> Self {
        self.outlier_detection = Some(config);
        self
    }
}

/// Configuration for a route to be created
#[derive(Clone)]
pub struct RouteConfig {
    pub name: String,
    pub domain: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub retry_policy: Option<Value>,
    pub prefix_rewrite: Option<String>,
}

impl RouteConfig {
    pub fn new(name: &str, path_prefix: &str, cluster_name: &str) -> Self {
        Self {
            name: name.to_string(),
            domain: format!("{}.e2e.local", name),
            path_prefix: path_prefix.to_string(),
            cluster_name: cluster_name.to_string(),
            retry_policy: None,
            prefix_rewrite: None,
        }
    }

    pub fn with_domain(mut self, domain: &str) -> Self {
        self.domain = domain.to_string();
        self
    }

    pub fn with_retry_policy(mut self, config: Value) -> Self {
        self.retry_policy = Some(config);
        self
    }

    pub fn with_prefix_rewrite(mut self, rewrite: &str) -> Self {
        self.prefix_rewrite = Some(rewrite.to_string());
        self
    }
}

/// Configuration for a listener to be created
#[derive(Clone)]
pub struct ListenerConfig {
    pub name: String,
    pub port: u16,
    pub route_config_name: String,
    pub dataplane_id: String,
}

impl ListenerConfig {
    /// Create new listener config. dataplane_id will be set by ResourceSetup builder.
    pub fn new(name: &str, port: u16, route_config_name: &str) -> Self {
        Self {
            name: name.to_string(),
            port,
            route_config_name: route_config_name.to_string(),
            dataplane_id: String::new(), // Will be overridden by ResourceSetup
        }
    }

    /// Create new listener config with explicit dataplane_id
    pub fn new_with_dataplane(
        name: &str,
        port: u16,
        route_config_name: &str,
        dataplane_id: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            port,
            route_config_name: route_config_name.to_string(),
            dataplane_id: dataplane_id.to_string(),
        }
    }
}

/// Configuration for a filter to be created
#[derive(Clone)]
pub struct FilterConfig {
    pub name: String,
    pub filter_type: String,
    pub config: Value,
    pub priority: Option<i32>,
}

impl FilterConfig {
    pub fn new(name: &str, filter_type: &str, config: Value) -> Self {
        Self {
            name: name.to_string(),
            filter_type: filter_type.to_string(),
            config,
            priority: None,
        }
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = Some(priority);
        self
    }
}

/// Result of building test resources
pub struct TestResources {
    pub cluster: Option<ClusterResponse>,
    pub route: Option<RouteConfigResponse>,
    pub listener: Option<ListenerResponse>,
    pub filter: Option<FilterResponse>,
    pub filter_installation: Option<FilterInstallationResponse>,
}

impl TestResources {
    /// Get the cluster or panic
    pub fn cluster(&self) -> &ClusterResponse {
        self.cluster.as_ref().expect("Cluster was not created")
    }

    /// Get the route or panic
    pub fn route(&self) -> &RouteConfigResponse {
        self.route.as_ref().expect("Route was not created")
    }

    /// Get the listener or panic
    pub fn listener(&self) -> &ListenerResponse {
        self.listener.as_ref().expect("Listener was not created")
    }

    /// Get the filter or panic
    pub fn filter(&self) -> &FilterResponse {
        self.filter.as_ref().expect("Filter was not created")
    }
}

/// Builder for common test resource setup
pub struct ResourceSetup<'a> {
    api: &'a ApiClient,
    token: &'a str,
    team: &'a str,
    dataplane_id: &'a str,
    cluster: Option<ClusterConfig>,
    route: Option<RouteConfig>,
    listener: Option<ListenerConfig>,
    filter: Option<FilterConfig>,
}

impl<'a> ResourceSetup<'a> {
    /// Create a new resource setup builder
    pub fn new(api: &'a ApiClient, token: &'a str, team: &'a str, dataplane_id: &'a str) -> Self {
        Self {
            api,
            token,
            team,
            dataplane_id,
            cluster: None,
            route: None,
            listener: None,
            filter: None,
        }
    }

    /// Add a cluster configuration
    pub fn with_cluster(mut self, name: &str, host: &str, port: u16) -> Self {
        self.cluster = Some(ClusterConfig::new(name, host, port));
        self
    }

    /// Add a cluster with full configuration
    pub fn with_cluster_config(mut self, config: ClusterConfig) -> Self {
        self.cluster = Some(config);
        self
    }

    /// Add a route configuration
    pub fn with_route(mut self, name: &str, path_prefix: &str) -> Self {
        // Cluster name will be set in build() based on actual cluster
        self.route = Some(RouteConfig::new(name, path_prefix, ""));
        self
    }

    /// Add a route with full configuration
    pub fn with_route_config(mut self, config: RouteConfig) -> Self {
        self.route = Some(config);
        self
    }

    /// Add a listener configuration
    pub fn with_listener(mut self, name: &str, port: u16) -> Self {
        // Route config name will be set in build() based on actual route
        // dataplane_id comes from the ResourceSetup
        self.listener = Some(ListenerConfig::new_with_dataplane(name, port, "", self.dataplane_id));
        self
    }

    /// Add a listener with full configuration
    /// Note: dataplane_id is overridden from the ResourceSetup's dataplane_id
    pub fn with_listener_config(mut self, mut config: ListenerConfig) -> Self {
        config.dataplane_id = self.dataplane_id.to_string();
        self.listener = Some(config);
        self
    }

    /// Add a filter configuration
    pub fn with_filter(mut self, name: &str, filter_type: &str, config: Value) -> Self {
        self.filter = Some(FilterConfig::new(name, filter_type, config));
        self
    }

    /// Add a filter with full configuration
    pub fn with_filter_config(mut self, config: FilterConfig) -> Self {
        self.filter = Some(config);
        self
    }

    /// Build all resources in the correct order
    pub async fn build(self) -> Result<TestResources> {
        let mut resources = TestResources {
            cluster: None,
            route: None,
            listener: None,
            filter: None,
            filter_installation: None,
        };

        // Create cluster first
        if let Some(cluster_config) = self.cluster {
            let cluster_req = CreateClusterRequest {
                team: self.team.to_string(),
                name: cluster_config.name.clone(),
                service_name: None,
                endpoints: vec![ClusterEndpoint {
                    host: cluster_config.host,
                    port: cluster_config.port,
                }],
                connect_timeout_seconds: None,
                use_tls: None,
                tls_server_name: None,
                dns_lookup_family: None,
                lb_policy: None,
                health_checks: vec![],
                circuit_breakers: cluster_config.circuit_breakers,
                outlier_detection: cluster_config.outlier_detection,
                protocol_type: None,
            };

            let cluster = with_timeout(
                TestTimeout::default_with_label(format!("Create cluster {}", cluster_req.name)),
                async { self.api.create_cluster(self.token, &cluster_req).await },
            )
            .await?;

            resources.cluster = Some(cluster);
        }

        // Create route (needs cluster name)
        if let Some(mut route_config) = self.route {
            // If cluster name wasn't set, use the created cluster's name
            if route_config.cluster_name.is_empty() {
                if let Some(ref cluster) = resources.cluster {
                    route_config.cluster_name = cluster.name.clone();
                }
            }

            let route_req = build_route_request(self.team, &route_config);

            let route = with_timeout(
                TestTimeout::default_with_label(format!("Create route {}", route_req.name)),
                async { self.api.create_route(self.token, &route_req).await },
            )
            .await?;

            resources.route = Some(route);
        }

        // Create listener (needs route name)
        if let Some(mut listener_config) = self.listener {
            // If route config name wasn't set, use the created route's name
            if listener_config.route_config_name.is_empty() {
                if let Some(ref route) = resources.route {
                    listener_config.route_config_name = route.name.clone();
                }
            }

            let listener_req = CreateListenerRequest {
                team: self.team.to_string(),
                name: listener_config.name,
                address: "0.0.0.0".to_string(),
                port: listener_config.port,
                filter_chains: vec![ListenerFilterChainInput {
                    name: Some("default".to_string()),
                    filters: vec![ListenerFilterInput {
                        name: "envoy.filters.network.http_connection_manager".to_string(),
                        filter_type: ListenerFilterTypeInput::HttpConnectionManager {
                            route_config_name: Some(listener_config.route_config_name),
                            inline_route_config: None,
                            access_log: None,
                            tracing: None,
                            http_filters: vec![],
                        },
                    }],
                    tls_context: None,
                }],
                protocol: None,
                dataplane_id: listener_config.dataplane_id,
            };

            let listener = with_timeout(
                TestTimeout::default_with_label(format!("Create listener {}", listener_req.name)),
                async { self.api.create_listener(self.token, &listener_req).await },
            )
            .await?;

            resources.listener = Some(listener);
        }

        // Create filter and install on listener
        if let Some(filter_config) = self.filter {
            let filter = with_timeout(
                TestTimeout::default_with_label(format!("Create filter {}", filter_config.name)),
                async {
                    self.api
                        .create_filter(
                            self.token,
                            self.team,
                            &filter_config.name,
                            &filter_config.filter_type,
                            filter_config.config.clone(),
                        )
                        .await
                },
            )
            .await?;

            // Install filter on listener if listener exists
            if let Some(ref listener) = resources.listener {
                let installation = with_timeout(
                    TestTimeout::default_with_label(format!(
                        "Install filter {} on {}",
                        filter.name, listener.name
                    )),
                    async {
                        self.api
                            .install_filter(
                                self.token,
                                &filter.id,
                                &listener.name,
                                filter_config.priority.map(|p| p as i64),
                            )
                            .await
                    },
                )
                .await?;

                resources.filter_installation = Some(installation);
            }

            resources.filter = Some(filter);
        }

        Ok(resources)
    }
}

/// Build a route request with optional retry policy and prefix rewrite
fn build_route_request(team: &str, config: &RouteConfig) -> CreateRouteRequest {
    CreateRouteRequest {
        team: team.to_string(),
        name: config.name.clone(),
        virtual_hosts: vec![VirtualHost {
            name: format!("{}-vh", config.name),
            domains: vec![config.domain.clone()],
            routes: vec![Route {
                name: format!("{}-route", config.name),
                route_match: RouteMatch {
                    path: PathMatch {
                        match_type: "prefix".to_string(),
                        value: config.path_prefix.clone(),
                    },
                },
                action: RouteAction {
                    action_type: "forward".to_string(),
                    cluster: config.cluster_name.clone(),
                    timeout_seconds: Some(30),
                    prefix_rewrite: config.prefix_rewrite.clone(),
                    retry_policy: config.retry_policy.clone(),
                },
            }],
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_config_builder() {
        let config = ClusterConfig::new("test", "127.0.0.1", 8080)
            .with_circuit_breakers(serde_json::json!({"maxConnections": 100}));

        assert_eq!(config.name, "test");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert!(config.circuit_breakers.is_some());
    }

    #[test]
    fn test_route_config_builder() {
        let config = RouteConfig::new("test-route", "/api", "test-cluster")
            .with_domain("api.test.local")
            .with_retry_policy(serde_json::json!({"maxRetries": 3}));

        assert_eq!(config.name, "test-route");
        assert_eq!(config.domain, "api.test.local");
        assert!(config.retry_policy.is_some());
    }

    #[test]
    fn test_filter_config_builder() {
        let config = FilterConfig::new("test-filter", "local_rate_limit", serde_json::json!({}))
            .with_priority(50);

        assert_eq!(config.name, "test-filter");
        assert_eq!(config.filter_type, "local_rate_limit");
        assert_eq!(config.priority, Some(50));
    }
}
