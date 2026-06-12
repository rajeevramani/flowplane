//! Domain → Envoy proto translation (the IR seam, spec/10 §5).

use envoy_types::pb::envoy::config::cluster::v3 as exc;
use envoy_types::pb::envoy::config::core::v3 as core;
use envoy_types::pb::envoy::config::endpoint::v3 as ep;
use envoy_types::pb::envoy::config::listener::v3 as lst;
use envoy_types::pb::envoy::config::route::v3 as rt;
use envoy_types::pb::envoy::extensions::filters::http::router::v3::Router;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3 as hcm;
use envoy_types::pb::envoy::extensions::transport_sockets::tls::v3 as tls;
use envoy_types::pb::google::protobuf as wkt;
use fp_domain::gateway::cluster::{ClusterSpec, LbPolicy};
use fp_domain::gateway::listener::ListenerSpec;
use fp_domain::gateway::route_config::{PathMatch, RouteConfigSpec};
use fp_domain::{DomainError, DomainResult};
use prost::Message;

fn any<M: Message>(type_url: &str, msg: &M) -> wkt::Any {
    wkt::Any {
        type_url: type_url.to_string(),
        value: msg.encode_to_vec(),
    }
}

fn duration(secs: u32) -> wkt::Duration {
    wkt::Duration {
        seconds: i64::from(secs),
        nanos: 0,
    }
}

fn u32_value(value: u32) -> wkt::UInt32Value {
    wkt::UInt32Value { value }
}

fn socket_address(host: &str, port: u16) -> core::Address {
    core::Address {
        address: Some(core::address::Address::SocketAddress(core::SocketAddress {
            address: host.to_string(),
            port_specifier: Some(core::socket_address::PortSpecifier::PortValue(u32::from(
                port,
            ))),
            ..Default::default()
        })),
    }
}

/// EDS only carries socket addresses — Envoy never DNS-resolves EDS endpoints. Clusters
/// whose endpoints are all IP literals go over EDS (endpoint churn never touches cluster
/// bytes, spec/10 §5); hostname endpoints stay STRICT_DNS with inline assignment.
pub fn cluster_uses_eds(spec: &ClusterSpec) -> bool {
    spec.endpoints
        .iter()
        .all(|e| e.host.parse::<std::net::IpAddr>().is_ok())
}

/// The ClusterLoadAssignment for an EDS cluster. Endpoints sorted (host, port).
pub fn endpoints_to_proto(name: &str, spec: &ClusterSpec) -> ep::ClusterLoadAssignment {
    ep::ClusterLoadAssignment {
        cluster_name: name.to_string(),
        endpoints: vec![ep::LocalityLbEndpoints {
            lb_endpoints: sorted_lb_endpoints(spec),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn sorted_lb_endpoints(spec: &ClusterSpec) -> Vec<ep::LbEndpoint> {
    let mut endpoints = spec.endpoints.clone();
    endpoints.sort_by(|a, b| (a.host.as_str(), a.port).cmp(&(b.host.as_str(), b.port)));
    endpoints
        .iter()
        .map(|endpoint| ep::LbEndpoint {
            host_identifier: Some(ep::lb_endpoint::HostIdentifier::Endpoint(ep::Endpoint {
                address: Some(socket_address(&endpoint.host, endpoint.port)),
                ..Default::default()
            })),
            load_balancing_weight: endpoint.weight.map(u32_value),
            ..Default::default()
        })
        .collect()
}

/// Translate a validated ClusterSpec. Endpoints are sorted (host, port) for determinism.
pub fn cluster_to_proto(name: &str, spec: &ClusterSpec) -> DomainResult<exc::Cluster> {
    let lb_policy = match spec.lb_policy {
        LbPolicy::RoundRobin => exc::cluster::LbPolicy::RoundRobin,
        LbPolicy::LeastRequest => exc::cluster::LbPolicy::LeastRequest,
        LbPolicy::Random => exc::cluster::LbPolicy::Random,
        LbPolicy::RingHash => exc::cluster::LbPolicy::RingHash,
    };

    let transport_socket = if spec.use_tls {
        Some(core::TransportSocket {
            name: "envoy.transport_sockets.tls".to_string(),
            config_type: Some(core::transport_socket::ConfigType::TypedConfig(any(
                "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext",
                &tls::UpstreamTlsContext::default(),
            ))),
        })
    } else {
        None
    };

    let health_checks = spec
        .health_check
        .as_ref()
        .map(|hc| {
            vec![core::HealthCheck {
                timeout: Some(duration(hc.timeout_seconds)),
                interval: Some(duration(hc.interval_seconds)),
                healthy_threshold: Some(u32_value(hc.healthy_threshold)),
                unhealthy_threshold: Some(u32_value(hc.unhealthy_threshold)),
                health_checker: Some(core::health_check::HealthChecker::HttpHealthCheck(
                    core::health_check::HttpHealthCheck {
                        path: hc.path.clone(),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }]
        })
        .unwrap_or_default();

    let circuit_breakers = spec
        .circuit_breaker
        .as_ref()
        .map(|cb| exc::CircuitBreakers {
            thresholds: vec![exc::circuit_breakers::Thresholds {
                max_connections: Some(u32_value(cb.max_connections)),
                max_pending_requests: Some(u32_value(cb.max_pending_requests)),
                max_requests: Some(u32_value(cb.max_requests)),
                max_retries: Some(u32_value(cb.max_retries)),
                ..Default::default()
            }],
            ..Default::default()
        });

    let outlier_detection = spec
        .outlier_detection
        .as_ref()
        .map(|od| exc::OutlierDetection {
            consecutive_5xx: Some(u32_value(od.consecutive_5xx)),
            interval: Some(duration(od.interval_seconds)),
            base_ejection_time: Some(duration(od.base_ejection_seconds)),
            max_ejection_percent: Some(u32_value(od.max_ejection_percent)),
            ..Default::default()
        });

    let (discovery_type, eds_cluster_config, load_assignment) = if cluster_uses_eds(spec) {
        (
            exc::cluster::DiscoveryType::Eds,
            Some(exc::cluster::EdsClusterConfig {
                eds_config: Some(core::ConfigSource {
                    resource_api_version: core::ApiVersion::V3 as i32,
                    config_source_specifier: Some(core::config_source::ConfigSourceSpecifier::Ads(
                        core::AggregatedConfigSource {},
                    )),
                    ..Default::default()
                }),
                service_name: String::new(), // EDS resource name = cluster name
            }),
            None,
        )
    } else {
        (
            exc::cluster::DiscoveryType::StrictDns,
            None,
            Some(endpoints_to_proto(name, spec)),
        )
    };

    Ok(exc::Cluster {
        name: name.to_string(),
        connect_timeout: Some(duration(spec.connect_timeout_secs)),
        cluster_discovery_type: Some(exc::cluster::ClusterDiscoveryType::Type(
            discovery_type as i32,
        )),
        eds_cluster_config,
        lb_policy: lb_policy as i32,
        load_assignment,
        transport_socket,
        health_checks,
        circuit_breakers,
        outlier_detection,
        ..Default::default()
    })
}

/// Translate a validated RouteConfigSpec. Vhosts and routes keep their declared order
/// (route order is semantic in Envoy — first match wins).
pub fn route_config_to_proto(
    name: &str,
    spec: &RouteConfigSpec,
) -> DomainResult<rt::RouteConfiguration> {
    let virtual_hosts = spec
        .virtual_hosts
        .iter()
        .map(|vhost| {
            let routes = vhost
                .routes
                .iter()
                .map(|rule| {
                    let path_specifier = match &rule.matcher {
                        PathMatch::Prefix { prefix } => {
                            rt::route_match::PathSpecifier::Prefix(prefix.clone())
                        }
                        PathMatch::Exact { path } => {
                            rt::route_match::PathSpecifier::Path(path.clone())
                        }
                        PathMatch::Template { template } => {
                            rt::route_match::PathSpecifier::PathMatchPolicy(
                                core::TypedExtensionConfig {
                                    name: "envoy.path.match.uri_template.uri_template_matcher"
                                        .to_string(),
                                    typed_config: Some(any(
                                        "type.googleapis.com/envoy.extensions.path.match.uri_template.v3.UriTemplateMatchConfig",
                                        &envoy_types::pb::envoy::extensions::path::r#match::uri_template::v3::UriTemplateMatchConfig {
                                            path_template: template.clone(),
                                        },
                                    )),
                                },
                            )
                        }
                    };
                    rt::Route {
                        name: rule.name.clone(),
                        r#match: Some(rt::RouteMatch {
                            path_specifier: Some(path_specifier),
                            ..Default::default()
                        }),
                        action: Some(rt::route::Action::Route(rt::RouteAction {
                            cluster_specifier: Some(rt::route_action::ClusterSpecifier::Cluster(
                                rule.action.cluster.clone(),
                            )),
                            prefix_rewrite: rule
                                .action
                                .prefix_rewrite
                                .clone()
                                .unwrap_or_default(),
                            timeout: Some(duration(rule.action.timeout_secs)),
                            ..Default::default()
                        })),
                        ..Default::default()
                    }
                })
                .collect();
            rt::VirtualHost {
                name: vhost.name.clone(),
                domains: vhost.domains.clone(),
                routes,
                ..Default::default()
            }
        })
        .collect();

    Ok(rt::RouteConfiguration {
        name: name.to_string(),
        virtual_hosts,
        ..Default::default()
    })
}

const HCM_TYPE_URL: &str = "type.googleapis.com/envoy.extensions.filters.network.\
                            http_connection_manager.v3.HttpConnectionManager";
const ROUTER_TYPE_URL: &str = "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router";

/// Translate a validated ListenerSpec. The HCM points at the bound route config via RDS
/// (delivered over the same ADS stream).
/// Translate one chain entry to an HCM HttpFilter (S5.8). Filters keep declared order;
/// the router is appended by the caller.
fn http_filter_to_proto(
    entry: &fp_domain::gateway::filters::HttpFilterEntry,
) -> DomainResult<hcm::HttpFilter> {
    use envoy_types::pb::envoy::extensions::filters::http::header_mutation::v3 as hm;
    use envoy_types::pb::envoy::extensions::filters::http::local_ratelimit::v3 as lrl;
    use fp_domain::gateway::filters::HttpFilterSpec;

    let (name, typed) = match &entry.filter {
        HttpFilterSpec::Cors(_) => {
            // The listener-level cors filter is an empty marker; the policy lives in
            // per-route config, which lands with the override plumbing (S5.8b). Until
            // then a cors chain entry would be a silent no-op — reject it loudly.
            return Err(DomainError::validation(
                "cors in the listener chain requires per-route policy support (not yet \
                 available); remove it for now",
            ));
        }
        HttpFilterSpec::LocalRateLimit(c) => {
            let proto = lrl::LocalRateLimit {
                stat_prefix: c.stat_prefix.clone(),
                token_bucket: Some(envoy_types::pb::envoy::r#type::v3::TokenBucket {
                    max_tokens: c.token_bucket.max_tokens,
                    tokens_per_fill: Some(u32_value(
                        c.token_bucket
                            .tokens_per_fill
                            .unwrap_or(c.token_bucket.max_tokens),
                    )),
                    fill_interval: Some(wkt::Duration {
                        seconds: (c.token_bucket.fill_interval_ms / 1000) as i64,
                        nanos: ((c.token_bucket.fill_interval_ms % 1000) * 1_000_000) as i32,
                    }),
                }),
                status: c
                    .status_code
                    .map(|code| envoy_types::pb::envoy::r#type::v3::HttpStatus {
                        code: i32::from(code),
                    }),
                // Enforce 100% by default (spec/04 §4.1: enabled/enforced default 100%).
                filter_enabled: Some(core::RuntimeFractionalPercent {
                    default_value: Some(envoy_types::pb::envoy::r#type::v3::FractionalPercent {
                        numerator: 100,
                        denominator: 0, // HUNDRED
                    }),
                    runtime_key: String::new(),
                }),
                filter_enforced: Some(core::RuntimeFractionalPercent {
                    default_value: Some(envoy_types::pb::envoy::r#type::v3::FractionalPercent {
                        numerator: 100,
                        denominator: 0,
                    }),
                    runtime_key: String::new(),
                }),
                ..Default::default()
            };
            (
                "envoy.filters.http.local_ratelimit",
                any(
                    "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit",
                    &proto,
                ),
            )
        }
        HttpFilterSpec::HeaderMutation(c) => {
            let proto = hm::HeaderMutation {
                mutations: Some(hm::Mutations {
                    request_mutations: c
                        .request_headers_to_add
                        .iter()
                        .map(|hv| header_mutation_entry(hv, false))
                        .chain(
                            c.request_headers_to_remove
                                .iter()
                                .map(|k| header_removal_entry(k)),
                        )
                        .collect(),
                    response_mutations: c
                        .response_headers_to_add
                        .iter()
                        .map(|hv| header_mutation_entry(hv, false))
                        .chain(
                            c.response_headers_to_remove
                                .iter()
                                .map(|k| header_removal_entry(k)),
                        )
                        .collect(),
                    ..Default::default()
                }),
                ..Default::default()
            };
            (
                "envoy.filters.http.header_mutation",
                any(
                    "type.googleapis.com/envoy.extensions.filters.http.header_mutation.v3.HeaderMutation",
                    &proto,
                ),
            )
        }
    };
    Ok(hcm::HttpFilter {
        name: name.to_string(),
        config_type: Some(hcm::http_filter::ConfigType::TypedConfig(typed)),
        disabled: entry.disabled,
        ..Default::default()
    })
}

fn header_mutation_entry(
    hv: &fp_domain::gateway::filters::HeaderValue,
    _removal: bool,
) -> envoy_types::pb::envoy::config::common::mutation_rules::v3::HeaderMutation {
    use envoy_types::pb::envoy::config::common::mutation_rules::v3 as mr;
    mr::HeaderMutation {
        action: Some(mr::header_mutation::Action::Append(
            core::HeaderValueOption {
                header: Some(core::HeaderValue {
                    key: hv.key.clone(),
                    value: hv.value.clone(),
                    ..Default::default()
                }),
                append_action: if hv.append {
                    core::header_value_option::HeaderAppendAction::AppendIfExistsOrAdd as i32
                } else {
                    core::header_value_option::HeaderAppendAction::OverwriteIfExistsOrAdd as i32
                },
                ..Default::default()
            },
        )),
    }
}

fn header_removal_entry(
    key: &str,
) -> envoy_types::pb::envoy::config::common::mutation_rules::v3::HeaderMutation {
    use envoy_types::pb::envoy::config::common::mutation_rules::v3 as mr;
    mr::HeaderMutation {
        action: Some(mr::header_mutation::Action::Remove(key.to_string())),
    }
}

pub fn listener_to_proto(name: &str, spec: &ListenerSpec) -> DomainResult<lst::Listener> {
    let route_config_name = spec.route_config.clone().ok_or_else(|| {
        DomainError::validation(format!(
            "listener \"{name}\" has no route_config bound; it cannot serve traffic yet"
        ))
    })?;

    // Chain: declared filters in order, router appended last (spec/04 §4.2).
    let mut http_filters = Vec::with_capacity(spec.http_filters.len() + 1);
    for entry in &spec.http_filters {
        http_filters.push(http_filter_to_proto(entry)?);
    }
    http_filters.push(hcm::HttpFilter {
        name: "envoy.filters.http.router".to_string(),
        config_type: Some(hcm::http_filter::ConfigType::TypedConfig(any(
            ROUTER_TYPE_URL,
            &Router::default(),
        ))),
        ..Default::default()
    });

    let manager = hcm::HttpConnectionManager {
        stat_prefix: name.to_string(),
        route_specifier: Some(hcm::http_connection_manager::RouteSpecifier::Rds(
            hcm::Rds {
                route_config_name,
                config_source: Some(core::ConfigSource {
                    resource_api_version: core::ApiVersion::V3 as i32,
                    config_source_specifier: Some(core::config_source::ConfigSourceSpecifier::Ads(
                        core::AggregatedConfigSource {},
                    )),
                    ..Default::default()
                }),
            },
        )),
        http_filters,
        ..Default::default()
    };

    Ok(lst::Listener {
        name: name.to_string(),
        address: Some(socket_address(&spec.address, spec.port)),
        filter_chains: vec![lst::FilterChain {
            filters: vec![lst::Filter {
                name: "envoy.filters.network.http_connection_manager".to_string(),
                config_type: Some(lst::filter::ConfigType::TypedConfig(any(
                    HCM_TYPE_URL,
                    &manager,
                ))),
            }],
            ..Default::default()
        }],
        ..Default::default()
    })
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use fp_domain::gateway::cluster::Endpoint;
    use fp_domain::gateway::route_config::{RouteAction, RouteRule, VirtualHost};

    fn cluster_spec() -> ClusterSpec {
        ClusterSpec {
            endpoints: vec![
                Endpoint {
                    host: "b.example".into(),
                    port: 9000,
                    weight: Some(2),
                },
                Endpoint {
                    host: "a.example".into(),
                    port: 8080,
                    weight: Some(1),
                },
            ],
            lb_policy: LbPolicy::LeastRequest,
            connect_timeout_secs: 7,
            use_tls: true,
            health_check: None,
            circuit_breaker: None,
            outlier_detection: None,
        }
    }

    #[test]
    fn cluster_translation_is_deterministic_and_sorted() {
        let a = cluster_to_proto("payments", &cluster_spec()).expect("translate");
        let b = cluster_to_proto("payments", &cluster_spec()).expect("translate");
        assert_eq!(
            a.encode_to_vec(),
            b.encode_to_vec(),
            "byte-identical across runs"
        );

        let assignment = a.load_assignment.expect("assignment");
        let hosts: Vec<String> = assignment.endpoints[0]
            .lb_endpoints
            .iter()
            .map(|e| match e.host_identifier.as_ref().expect("host") {
                ep::lb_endpoint::HostIdentifier::Endpoint(endpoint) => {
                    match endpoint
                        .address
                        .as_ref()
                        .expect("addr")
                        .address
                        .as_ref()
                        .expect("a")
                    {
                        core::address::Address::SocketAddress(s) => s.address.clone(),
                        _ => panic!("unexpected address kind"),
                    }
                }
                _ => panic!("unexpected host identifier"),
            })
            .collect();
        assert_eq!(hosts, vec!["a.example", "b.example"], "endpoints sorted");
        assert!(
            a.transport_socket.is_some(),
            "explicit TLS produced a transport socket"
        );
        assert_eq!(a.connect_timeout.expect("timeout").seconds, 7);
    }

    #[test]
    fn route_config_translates_all_match_kinds() {
        let spec = RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![
                    RouteRule {
                        name: "exact".into(),
                        matcher: PathMatch::Exact {
                            path: "/health".into(),
                        },
                        action: RouteAction {
                            cluster: "c1".into(),
                            prefix_rewrite: None,
                            template_rewrite: None,
                            timeout_secs: 15,
                        },
                    },
                    RouteRule {
                        name: "prefixed".into(),
                        matcher: PathMatch::Prefix {
                            prefix: "/api".into(),
                        },
                        action: RouteAction {
                            cluster: "c2".into(),
                            prefix_rewrite: Some("/v2".into()),
                            template_rewrite: None,
                            timeout_secs: 30,
                        },
                    },
                    RouteRule {
                        name: "templated".into(),
                        matcher: PathMatch::Template {
                            template: "/users/{id}".into(),
                        },
                        action: RouteAction {
                            cluster: "c3".into(),
                            prefix_rewrite: None,
                            template_rewrite: None,
                            timeout_secs: 15,
                        },
                    },
                ],
            }],
        };
        let proto = route_config_to_proto("orders", &spec).expect("translate");
        let routes = &proto.virtual_hosts[0].routes;
        assert_eq!(routes.len(), 3);
        assert!(matches!(
            routes[0].r#match.as_ref().expect("m").path_specifier,
            Some(rt::route_match::PathSpecifier::Path(_))
        ));
        assert!(matches!(
            routes[2].r#match.as_ref().expect("m").path_specifier,
            Some(rt::route_match::PathSpecifier::PathMatchPolicy(_))
        ));
        // Route ORDER is preserved exactly (first-match-wins semantics).
        let names: Vec<&str> = routes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["exact", "prefixed", "templated"]);
    }

    #[test]
    fn listener_requires_a_bound_route_config() {
        let unbound = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10001,
            route_config: None,
            http_filters: Vec::new(),
        };
        assert!(listener_to_proto("edge", &unbound).is_err());

        let bound = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10001,
            route_config: Some("orders".into()),
            http_filters: Vec::new(),
        };
        let proto = listener_to_proto("edge", &bound).expect("translate");
        assert_eq!(proto.filter_chains.len(), 1);

        let a = cluster_to_proto("x", &cluster_spec()).expect("t");
        let b = listener_to_proto("edge", &bound).expect("t");
        assert_eq!(
            b.encode_to_vec(),
            listener_to_proto("edge", &bound)
                .expect("t")
                .encode_to_vec()
        );
        drop(a);
    }

    #[test]
    fn filter_chain_keeps_order_router_last_and_cors_rejected() {
        {
            use fp_domain::gateway::filters::*;
            let chain = vec![
                HttpFilterEntry {
                    filter: HttpFilterSpec::LocalRateLimit(LocalRateLimitConfig {
                        stat_prefix: "edge".into(),
                        token_bucket: TokenBucket {
                            max_tokens: 10,
                            tokens_per_fill: None,
                            fill_interval_ms: 1000,
                        },
                        status_code: Some(429),
                    }),
                    disabled: false,
                },
                HttpFilterEntry {
                    filter: HttpFilterSpec::HeaderMutation(HeaderMutationConfig {
                        request_headers_to_add: vec![HeaderValue {
                            key: "x-edge".into(),
                            value: "1".into(),
                            append: false,
                        }],
                        request_headers_to_remove: vec!["x-internal".into()],
                        response_headers_to_add: vec![],
                        response_headers_to_remove: vec![],
                    }),
                    disabled: true,
                },
            ];
            let spec = ListenerSpec {
                address: "0.0.0.0".into(),
                port: 10001,
                route_config: Some("orders".into()),
                http_filters: chain,
            };
            let proto = listener_to_proto("edge", &spec).expect("translate");
            let manager = match &proto.filter_chains[0].filters[0].config_type {
                Some(lst::filter::ConfigType::TypedConfig(a)) => {
                    hcm::HttpConnectionManager::decode(a.value.as_slice()).expect("hcm")
                }
                _ => panic!("expected typed HCM"),
            };
            let names: Vec<_> = manager
                .http_filters
                .iter()
                .map(|f| f.name.as_str())
                .collect();
            assert_eq!(
                names,
                vec![
                    "envoy.filters.http.local_ratelimit",
                    "envoy.filters.http.header_mutation",
                    "envoy.filters.http.router"
                ],
                "declared order, router appended last"
            );
            assert!(manager.http_filters[1].disabled, "disabled flag carried");

            // cors without per-route policy plumbing must fail loudly, not no-op.
            let cors_spec = ListenerSpec {
                address: "0.0.0.0".into(),
                port: 10002,
                route_config: Some("orders".into()),
                http_filters: vec![HttpFilterEntry {
                    filter: HttpFilterSpec::Cors(CorsConfig {
                        allow_origin: vec![OriginMatcher::Exact {
                            value: "https://a.example".into(),
                        }],
                        allow_methods: vec![],
                        allow_headers: vec![],
                        expose_headers: vec![],
                        max_age_seconds: None,
                        allow_credentials: false,
                    }),
                    disabled: false,
                }],
            };
            assert!(listener_to_proto("edge2", &cors_spec).is_err());
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod type_url_tests {
    use super::*;
    use fp_domain::gateway::route_config::{RouteAction, RouteRule, VirtualHost};

    /// Rust raw-identifier syntax (`r#match`) must never leak into protobuf type URLs —
    /// Envoy would NACK the resource (caught during S5.1 review).
    #[test]
    fn emitted_type_urls_are_valid_proto_paths() {
        let spec = RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "templated".into(),
                    matcher: PathMatch::Template {
                        template: "/users/{id}".into(),
                    },
                    action: RouteAction {
                        cluster: "c".into(),
                        prefix_rewrite: None,
                        template_rewrite: None,
                        timeout_secs: 15,
                    },
                }],
            }],
        };
        let proto = route_config_to_proto("rc", &spec).expect("translate");
        let matcher = proto.virtual_hosts[0].routes[0]
            .r#match
            .as_ref()
            .expect("match");
        let Some(rt::route_match::PathSpecifier::PathMatchPolicy(policy)) = &matcher.path_specifier
        else {
            panic!("expected a PathMatchPolicy");
        };
        let url = &policy.typed_config.as_ref().expect("typed config").type_url;
        assert!(
            !url.contains("r#"),
            "raw identifier leaked into the type URL: {url}"
        );
        assert_eq!(
            url,
            "type.googleapis.com/envoy.extensions.path.match.uri_template.v3.UriTemplateMatchConfig"
        );
    }
}
