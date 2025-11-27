use crate::errors::Error;
use crate::storage::{CreateClusterRequest, CreateListenerRequest, CreateRouteRepositoryRequest};
use crate::xds::XdsState;
use crate::xds::{
    filters::http::{HttpFilterConfigEntry, HttpFilterKind},
    listener::{FilterChainConfig, FilterConfig, FilterType, ListenerConfig},
    route::{
        PathMatch, RouteActionConfig, RouteConfig as XdsRouteConfig, RouteMatchConfig, RouteRule,
        VirtualHostConfig,
    },
    ClusterSpec, EndpointSpec,
};
use serde_json::Value;

fn serialize_value<T: serde::Serialize>(value: &T, context: &str) -> Result<Value, Error> {
    serde_json::to_value(value)
        .map_err(|err| Error::internal(format!("Failed to serialize {}: {}", context, err)))
}
use tracing::info;

pub const DEFAULT_GATEWAY_CLUSTER: &str = "default-gateway-cluster";
pub const DEFAULT_GATEWAY_ROUTES: &str = "default-gateway-routes";
pub const DEFAULT_GATEWAY_LISTENER: &str = "default-gateway-listener";
pub const DEFAULT_GATEWAY_VHOST: &str = "default-gateway-vhost";
const DEFAULT_GATEWAY_ROUTE_RULE: &str = "default-gateway-route";
pub const DEFAULT_GATEWAY_ADDRESS: &str = "0.0.0.0";
pub const DEFAULT_GATEWAY_PORT: u16 = 10000;

pub async fn ensure_default_gateway_resources(state: &XdsState) -> Result<(), Error> {
    let cluster_repo = match &state.cluster_repository {
        Some(repo) => repo.clone(),
        None => return Ok(()),
    };
    let route_repo = match &state.route_repository {
        Some(repo) => repo.clone(),
        None => return Ok(()),
    };
    let listener_repo = match &state.listener_repository {
        Some(repo) => repo.clone(),
        None => return Ok(()),
    };

    // NOTE: Bootstrap token auto-generation is now handled in the startup module
    // (See src/startup.rs and main.rs for the new auto-generation flow)

    if !cluster_repo.exists_by_name(DEFAULT_GATEWAY_CLUSTER).await? {
        let cluster_spec = ClusterSpec {
            connect_timeout_seconds: Some(5),
            endpoints: vec![EndpointSpec::Address { host: "127.0.0.1".to_string(), port: 65535 }],
            use_tls: Some(false),
            tls_server_name: None,
            dns_lookup_family: None,
            lb_policy: None,
            least_request: None,
            ring_hash: None,
            maglev: None,
            circuit_breakers: None,
            health_checks: Vec::new(),
            outlier_detection: None,
            protocol_type: None,
        };

        let cluster_config = cluster_spec.to_value()?;

        let request = CreateClusterRequest {
            name: DEFAULT_GATEWAY_CLUSTER.to_string(),
            service_name: DEFAULT_GATEWAY_CLUSTER.to_string(),
            configuration: cluster_config,
            team: None, // Default gateway cluster is not team-scoped
            import_id: None,
        };

        cluster_repo.create(request).await?;
        info!("Created default gateway cluster");
    }

    if !route_repo.exists_by_name(DEFAULT_GATEWAY_ROUTES).await? {
        let route_rule = RouteRule {
            name: Some(DEFAULT_GATEWAY_ROUTE_RULE.to_string()),
            r#match: RouteMatchConfig {
                path: PathMatch::Prefix("/".to_string()),
                headers: None,
                query_parameters: None,
            },
            action: RouteActionConfig::Cluster {
                name: DEFAULT_GATEWAY_CLUSTER.to_string(),
                timeout: Some(15),
                prefix_rewrite: None,
                path_template_rewrite: None,
            },
            typed_per_filter_config: Default::default(),
        };

        let virtual_host = VirtualHostConfig {
            name: DEFAULT_GATEWAY_VHOST.to_string(),
            domains: vec!["*".to_string()],
            routes: vec![route_rule],
            typed_per_filter_config: Default::default(),
        };

        let route_config = XdsRouteConfig {
            name: DEFAULT_GATEWAY_ROUTES.to_string(),
            virtual_hosts: vec![virtual_host],
        };

        let route_configuration: Value = serialize_value(&route_config, "default route config")?;

        let request = CreateRouteRepositoryRequest {
            name: DEFAULT_GATEWAY_ROUTES.to_string(),
            path_prefix: "/".to_string(),
            cluster_name: DEFAULT_GATEWAY_CLUSTER.to_string(),
            configuration: route_configuration,
            team: None, // Default gateway routes are not team-scoped
            import_id: None,
            route_order: None,
            headers: None,
        };

        route_repo.create(request).await?;
        info!("Created default gateway route configuration");
    }

    if !listener_repo.exists_by_name(DEFAULT_GATEWAY_LISTENER).await? {
        let listener_config = ListenerConfig {
            name: DEFAULT_GATEWAY_LISTENER.to_string(),
            address: DEFAULT_GATEWAY_ADDRESS.to_string(),
            port: DEFAULT_GATEWAY_PORT as u32,
            filter_chains: vec![FilterChainConfig {
                name: Some("default-gateway-chain".to_string()),
                filters: vec![FilterConfig {
                    name: "envoy.filters.network.http_connection_manager".to_string(),
                    filter_type: FilterType::HttpConnectionManager {
                        route_config_name: Some(DEFAULT_GATEWAY_ROUTES.to_string()),
                        inline_route_config: None,
                        access_log: None,
                        tracing: None,
                        http_filters: vec![
                            // Apply a conservative global local rate limit. Per‑route overrides can disable or adjust.
                            HttpFilterConfigEntry {
                                name: None,
                                is_optional: true,
                                disabled: false,
                                filter: HttpFilterKind::LocalRateLimit(
                                    crate::xds::filters::http::local_rate_limit::LocalRateLimitConfig {
                                        stat_prefix: "gateway_rl".to_string(),
                                        token_bucket: Some(
                                            crate::xds::filters::http::local_rate_limit::TokenBucketConfig {
                                                max_tokens: 5,
                                                tokens_per_fill: Some(5),
                                                fill_interval_ms: 2000,
                                            },
                                        ),
                                        status_code: Some(429),
                                        filter_enabled: None,
                                        filter_enforced: None,
                                        per_downstream_connection: None,
                                        rate_limited_as_resource_exhausted: None,
                                        max_dynamic_descriptors: None,
                                        always_consume_default_token_bucket: None,
                                    },
                                ),
                            },
                            HttpFilterConfigEntry {
                                name: None,
                                is_optional: false,
                                disabled: false,
                                filter: HttpFilterKind::Router,
                            },
                        ],
                    },
                }],
                tls_context: None,
            }],
        };

        let listener_configuration: Value =
            serialize_value(&listener_config, "default listener config")?;

        let request = CreateListenerRequest {
            name: DEFAULT_GATEWAY_LISTENER.to_string(),
            address: DEFAULT_GATEWAY_ADDRESS.to_string(),
            port: Some(DEFAULT_GATEWAY_PORT as i64),
            protocol: Some("HTTP".to_string()),
            configuration: listener_configuration,
            team: None, // Default gateway listener is not team-scoped
            import_id: None,
        };

        listener_repo.create(request).await?;
        info!("Created default gateway listener");
    }

    state.refresh_clusters_from_repository().await?;
    state.refresh_routes_from_repository().await?;
    state.refresh_listeners_from_repository().await?;

    Ok(())
}

pub fn is_default_gateway_cluster(name: &str) -> bool {
    name == DEFAULT_GATEWAY_CLUSTER
}

pub fn is_default_gateway_route(name: &str) -> bool {
    name == DEFAULT_GATEWAY_ROUTES
}

pub fn is_default_gateway_listener(name: &str) -> bool {
    name == DEFAULT_GATEWAY_LISTENER
}
