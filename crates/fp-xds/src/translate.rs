//! Domain → Envoy proto translation (the IR seam, spec/10 §5).

use base64::Engine as _;
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
use fp_domain::{DomainError, DomainResult, SecretSpec};
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

fn millis_duration(ms: u64) -> wkt::Duration {
    wkt::Duration {
        seconds: (ms / 1000) as i64,
        nanos: ((ms % 1000) * 1_000_000) as i32,
    }
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

fn inline_string(value: String) -> core::DataSource {
    core::DataSource {
        specifier: Some(core::data_source::Specifier::InlineString(value)),
        ..Default::default()
    }
}

fn inline_bytes(value: Vec<u8>) -> core::DataSource {
    core::DataSource {
        specifier: Some(core::data_source::Specifier::InlineBytes(value)),
        ..Default::default()
    }
}

fn decode_base64(label: &str, value: &str) -> DomainResult<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| DomainError::validation(format!("{label} must be base64")))
}

pub fn secret_to_proto(name: &str, spec: &SecretSpec) -> DomainResult<tls::Secret> {
    let r#type = match spec {
        SecretSpec::GenericSecret { secret } => {
            tls::secret::Type::GenericSecret(tls::GenericSecret {
                secret: Some(inline_bytes(decode_base64("generic secret", secret)?)),
                ..Default::default()
            })
        }
        SecretSpec::TlsCertificate {
            certificate_chain,
            private_key,
            password,
            ocsp_staple,
        } => tls::secret::Type::TlsCertificate(tls::TlsCertificate {
            certificate_chain: Some(inline_string(certificate_chain.clone())),
            private_key: Some(inline_string(private_key.clone())),
            password: password.clone().map(inline_string),
            ocsp_staple: ocsp_staple
                .as_ref()
                .map(|staple| decode_base64("ocsp_staple", staple).map(inline_bytes))
                .transpose()?,
            ..Default::default()
        }),
        SecretSpec::CertificateValidationContext {
            trusted_ca,
            match_subject_alt_names,
            crl,
            only_verify_leaf_cert_crl,
        } => tls::secret::Type::ValidationContext(tls::CertificateValidationContext {
            trusted_ca: Some(inline_string(trusted_ca.clone())),
            match_typed_subject_alt_names: match_subject_alt_names
                .iter()
                .map(|value| tls::SubjectAltNameMatcher {
                    san_type: tls::subject_alt_name_matcher::SanType::Dns as i32,
                    matcher: Some(envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
                        match_pattern: Some(
                            envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                value.clone(),
                            ),
                        ),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .collect(),
            crl: crl.clone().map(inline_string),
            only_verify_leaf_cert_crl: *only_verify_leaf_cert_crl,
            ..Default::default()
        }),
        SecretSpec::SessionTicketKeys { keys } => {
            tls::secret::Type::SessionTicketKeys(tls::TlsSessionTicketKeys {
                keys: keys
                    .iter()
                    .map(|key| decode_base64("session ticket key", &key.key).map(inline_bytes))
                    .collect::<DomainResult<Vec<_>>>()?,
            })
        }
    };
    Ok(tls::Secret {
        name: name.to_string(),
        r#type: Some(r#type),
    })
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
    let mut virtual_hosts = Vec::with_capacity(spec.virtual_hosts.len());
    for vhost in &spec.virtual_hosts {
        let mut routes = Vec::with_capacity(vhost.routes.len());
        for rule in &vhost.routes {
            routes.push(rt::Route {
                name: rule.name.clone(),
                r#match: Some(route_match_proto(&rule.matcher)),
                action: Some(rt::route::Action::Route(rt::RouteAction {
                    cluster_specifier: Some(rt::route_action::ClusterSpecifier::Cluster(
                        rule.action.cluster.clone(),
                    )),
                    prefix_rewrite: rule.action.prefix_rewrite.clone().unwrap_or_default(),
                    timeout: Some(duration(rule.action.timeout_secs)),
                    ..Default::default()
                })),
                typed_per_filter_config: overrides_to_typed_config(&rule.filter_overrides)?,
                ..Default::default()
            });
        }
        virtual_hosts.push(rt::VirtualHost {
            name: vhost.name.clone(),
            domains: vhost.domains.clone(),
            routes,
            typed_per_filter_config: overrides_to_typed_config(&vhost.filter_overrides)?,
            ..Default::default()
        });
    }

    Ok(rt::RouteConfiguration {
        name: name.to_string(),
        virtual_hosts,
        ..Default::default()
    })
}

/// Per-scope `typed_per_filter_config` map from filter overrides (spec/04 §4.3). Keys are
/// Envoy filter names; values are the per-route proto for each filter type.
fn overrides_to_typed_config(
    overrides: &[fp_domain::gateway::filters::FilterOverride],
) -> DomainResult<std::collections::HashMap<String, wkt::Any>> {
    use fp_domain::gateway::filters::FilterOverride;
    let mut map = std::collections::HashMap::new();
    for ov in overrides {
        match ov {
            FilterOverride::Disable { filter_type } => {
                let envoy_name = envoy_filter_name(filter_type)?;
                map.insert(
                    envoy_name.to_string(),
                    any(
                        "type.googleapis.com/envoy.config.route.v3.FilterConfig",
                        &rt::FilterConfig {
                            disabled: true,
                            ..Default::default()
                        },
                    ),
                );
            }
            FilterOverride::Cors(c) => {
                map.insert(
                    "envoy.filters.http.cors".to_string(),
                    any(
                        "type.googleapis.com/envoy.extensions.filters.http.cors.v3.CorsPolicy",
                        &cors_policy_to_proto(c),
                    ),
                );
            }
            FilterOverride::LocalRateLimit(c) => {
                map.insert(
                    "envoy.filters.http.local_ratelimit".to_string(),
                    local_rate_limit_to_any(c),
                );
            }
            FilterOverride::JwtAuth { requirement_name } => {
                // Reference-only per-route config (spec/04 §4.1): name a requirement from
                // the chain filter's requirement_map.
                use envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3 as jwt;
                map.insert(
                    "envoy.filters.http.jwt_authn".to_string(),
                    any(
                        "type.googleapis.com/envoy.extensions.filters.http.jwt_authn.v3.PerRouteConfig",
                        &jwt::PerRouteConfig {
                            requirement_specifier: Some(
                                jwt::per_route_config::RequirementSpecifier::RequirementName(
                                    requirement_name.clone(),
                                ),
                            ),
                        },
                    ),
                );
            }
        }
    }
    Ok(map)
}

fn envoy_filter_name(kind: &str) -> DomainResult<&'static str> {
    match kind {
        "cors" => Ok("envoy.filters.http.cors"),
        "local_rate_limit" => Ok("envoy.filters.http.local_ratelimit"),
        "header_mutation" => Ok("envoy.filters.http.header_mutation"),
        "compressor" => Ok("envoy.filters.http.compressor"),
        "health_check" => Ok("envoy.filters.http.health_check"),
        "jwt_auth" => Ok("envoy.filters.http.jwt_authn"),
        "ext_authz" => Ok("envoy.filters.http.ext_authz"),
        "rbac" => Ok("envoy.filters.http.rbac"),
        other => Err(DomainError::validation(format!(
            "unknown filter type \"{other}\""
        ))),
    }
}

/// LocalRateLimit proto, used both in the listener chain and as per-route override (same
/// type URL in both positions, spec/04 §4.1).
fn local_rate_limit_to_any(c: &fp_domain::gateway::filters::LocalRateLimitConfig) -> wkt::Any {
    use envoy_types::pb::envoy::extensions::filters::http::local_ratelimit::v3 as lrl;
    let percent_100 = || core::RuntimeFractionalPercent {
        default_value: Some(envoy_types::pb::envoy::r#type::v3::FractionalPercent {
            numerator: 100,
            denominator: 0, // HUNDRED
        }),
        runtime_key: String::new(),
    };
    let proto = lrl::LocalRateLimit {
        stat_prefix: c.stat_prefix.clone(),
        token_bucket: Some(envoy_types::pb::envoy::r#type::v3::TokenBucket {
            max_tokens: c.token_bucket.max_tokens,
            tokens_per_fill: Some(u32_value(
                c.token_bucket
                    .tokens_per_fill
                    .unwrap_or(c.token_bucket.max_tokens),
            )),
            fill_interval: Some(millis_duration(c.token_bucket.fill_interval_ms)),
        }),
        status: c
            .status_code
            .map(|code| envoy_types::pb::envoy::r#type::v3::HttpStatus {
                code: i32::from(code),
            }),
        // Enforce 100% by default (spec/04 §4.1: enabled/enforced default 100%).
        filter_enabled: Some(percent_100()),
        filter_enforced: Some(percent_100()),
        ..Default::default()
    };
    any(
        "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit",
        &proto,
    )
}

fn cors_policy_to_proto(
    c: &fp_domain::gateway::filters::CorsConfig,
) -> envoy_types::pb::envoy::extensions::filters::http::cors::v3::CorsPolicy {
    use envoy_types::pb::envoy::r#type::matcher::v3 as sm;
    use fp_domain::gateway::filters::OriginMatcher;
    let allow_origin_string_match = c
        .allow_origin
        .iter()
        .map(|m| {
            let pattern = match m {
                OriginMatcher::Exact { value } => {
                    sm::string_matcher::MatchPattern::Exact(value.clone())
                }
                OriginMatcher::Prefix { value } => {
                    sm::string_matcher::MatchPattern::Prefix(value.clone())
                }
                OriginMatcher::Suffix { value } => {
                    sm::string_matcher::MatchPattern::Suffix(value.clone())
                }
                OriginMatcher::Contains { value } => {
                    sm::string_matcher::MatchPattern::Contains(value.clone())
                }
            };
            sm::StringMatcher {
                match_pattern: Some(pattern),
                ..Default::default()
            }
        })
        .collect();
    envoy_types::pb::envoy::extensions::filters::http::cors::v3::CorsPolicy {
        allow_origin_string_match,
        allow_methods: c.allow_methods.join(","),
        allow_headers: c.allow_headers.join(","),
        expose_headers: c.expose_headers.join(","),
        max_age: c.max_age_seconds.map(|v| v.to_string()).unwrap_or_default(),
        allow_credentials: c
            .allow_credentials
            .then_some(wkt::BoolValue { value: true }),
        ..Default::default()
    }
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
    use fp_domain::gateway::filters::HttpFilterSpec;

    let (name, typed) = match &entry.filter {
        HttpFilterSpec::Cors(_) => {
            // The chain entry is an empty marker (spec/04 §4.1); the policy is read from
            // per-scope typed_per_filter_config emitted by route_config_to_proto. The
            // chain-level CorsConfig is validated but documents the default policy only.
            (
                "envoy.filters.http.cors",
                any(
                    "type.googleapis.com/envoy.extensions.filters.http.cors.v3.Cors",
                    &envoy_types::pb::envoy::extensions::filters::http::cors::v3::Cors::default(),
                ),
            )
        }
        HttpFilterSpec::HealthCheck(c) => {
            let proto =
                envoy_types::pb::envoy::extensions::filters::http::health_check::v3::HealthCheck {
                    pass_through_mode: Some(wkt::BoolValue {
                        value: c.pass_through_mode,
                    }),
                    cache_time: c.cache_time_ms.map(millis_duration),
                    headers: vec![rt::HeaderMatcher {
                        name: ":path".to_string(),
                        header_match_specifier: Some(
                            rt::header_matcher::HeaderMatchSpecifier::StringMatch(
                                envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
                                    match_pattern: Some(
                                        envoy_types::pb::envoy::r#type::matcher::v3::string_matcher::MatchPattern::Exact(
                                            c.endpoint_path.clone(),
                                        ),
                                    ),
                                    ..Default::default()
                                },
                            ),
                        ),
                        ..Default::default()
                    }],
                    ..Default::default()
                };
            (
                "envoy.filters.http.health_check",
                any(
                    "type.googleapis.com/envoy.extensions.filters.http.health_check.v3.HealthCheck",
                    &proto,
                ),
            )
        }
        HttpFilterSpec::Compressor(c) => {
            use envoy_types::pb::envoy::extensions::compression::gzip::compressor::v3 as gz;
            use envoy_types::pb::envoy::extensions::filters::http::compressor::v3 as comp;
            let gzip = gz::Gzip {
                memory_level: c.memory_level.map(u32_value),
                window_bits: c.window_bits.map(u32_value),
                compression_level: c
                    .compression_level
                    .map(|level| match level {
                        fp_domain::gateway::filters::CompressionLevel::BestSpeed => {
                            gz::gzip::CompressionLevel::BestSpeed as i32
                        }
                        fp_domain::gateway::filters::CompressionLevel::DefaultCompression => {
                            gz::gzip::CompressionLevel::DefaultCompression as i32
                        }
                        fp_domain::gateway::filters::CompressionLevel::BestCompression => {
                            gz::gzip::CompressionLevel::CompressionLevel9 as i32
                        }
                    })
                    .unwrap_or_default(),
                ..Default::default()
            };
            let proto = comp::Compressor {
                compressor_library: Some(core::TypedExtensionConfig {
                    name: "gzip".to_string(),
                    typed_config: Some(any(
                        "type.googleapis.com/envoy.extensions.compression.gzip.compressor.v3.Gzip",
                        &gzip,
                    )),
                }),
                ..Default::default()
            };
            (
                "envoy.filters.http.compressor",
                any(
                    "type.googleapis.com/envoy.extensions.filters.http.compressor.v3.Compressor",
                    &proto,
                ),
            )
        }
        HttpFilterSpec::LocalRateLimit(c) => (
            "envoy.filters.http.local_ratelimit",
            local_rate_limit_to_any(c),
        ),
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
        HttpFilterSpec::JwtAuth(c) => (
            "envoy.filters.http.jwt_authn",
            any(
                "type.googleapis.com/envoy.extensions.filters.http.jwt_authn.v3.JwtAuthentication",
                &jwt_auth_to_proto(c),
            ),
        ),
        HttpFilterSpec::ExtAuthz(c) => (
            "envoy.filters.http.ext_authz",
            any(
                "type.googleapis.com/envoy.extensions.filters.http.ext_authz.v3.ExtAuthz",
                &ext_authz_to_proto(c),
            ),
        ),
        HttpFilterSpec::Rbac(c) => (
            "envoy.filters.http.rbac",
            // The proto message is `RBAC` (all-caps); prost names the Rust type `Rbac` but
            // the wire type URL must match the proto name or Envoy NACKs (caught by the
            // live E2E against a real proxy).
            any(
                "type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBAC",
                &rbac_to_proto(c),
            ),
        ),
    };
    Ok(hcm::HttpFilter {
        name: name.to_string(),
        config_type: Some(hcm::http_filter::ConfigType::TypedConfig(typed)),
        disabled: entry.disabled,
        ..Default::default()
    })
}

/// JwtAuthentication proto (spec/04 §4.1). One filter per chain (v2 invariant), so v1's
/// provider-merge machinery is unnecessary. With no rules, every path requires any
/// provider (v1 default rule).
fn jwt_auth_to_proto(
    c: &fp_domain::gateway::filters::JwtAuthConfig,
) -> envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::JwtAuthentication {
    use envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3 as jwt;
    use fp_domain::gateway::filters::{JwksSource, JwtRequirement};

    let providers = c
        .providers
        .iter()
        .map(|(name, p)| {
            let jwks = match &p.jwks {
                JwksSource::Remote {
                    uri,
                    cluster,
                    timeout_ms,
                    cache_duration_secs,
                } => jwt::jwt_provider::JwksSourceSpecifier::RemoteJwks(jwt::RemoteJwks {
                    http_uri: Some(core::HttpUri {
                        uri: uri.clone(),
                        timeout: Some(millis_duration(*timeout_ms)),
                        http_upstream_type: Some(core::http_uri::HttpUpstreamType::Cluster(
                            cluster.clone(),
                        )),
                    }),
                    cache_duration: cache_duration_secs.map(|s| duration(s as u32)),
                    ..Default::default()
                }),
                JwksSource::Inline { jwks } => {
                    jwt::jwt_provider::JwksSourceSpecifier::LocalJwks(core::DataSource {
                        specifier: Some(core::data_source::Specifier::InlineString(jwks.clone())),
                        ..Default::default()
                    })
                }
            };
            let provider = jwt::JwtProvider {
                issuer: p.issuer.clone().unwrap_or_default(),
                audiences: p.audiences.clone(),
                forward: p.forward,
                clock_skew_seconds: p.clock_skew_seconds,
                jwks_source_specifier: Some(jwks),
                ..Default::default()
            };
            (name.clone(), provider)
        })
        .collect();

    let requirement = |req: &JwtRequirement| -> jwt::JwtRequirement {
        let kind = match req {
            JwtRequirement::Provider { provider_name } => {
                jwt::jwt_requirement::RequiresType::ProviderName(provider_name.clone())
            }
            JwtRequirement::AnyOf { provider_names } => {
                jwt::jwt_requirement::RequiresType::RequiresAny(jwt::JwtRequirementOrList {
                    requirements: provider_names
                        .iter()
                        .map(|n| jwt::JwtRequirement {
                            requires_type: Some(jwt::jwt_requirement::RequiresType::ProviderName(
                                n.clone(),
                            )),
                        })
                        .collect(),
                })
            }
            JwtRequirement::AllowMissing => {
                jwt::jwt_requirement::RequiresType::AllowMissing(wkt::Empty {})
            }
            JwtRequirement::AllowMissingOrFailed => {
                jwt::jwt_requirement::RequiresType::AllowMissingOrFailed(wkt::Empty {})
            }
        };
        jwt::JwtRequirement {
            requires_type: Some(kind),
        }
    };

    let requirement_map = c
        .requirement_map
        .iter()
        .map(|(name, req)| (name.clone(), requirement(req)))
        .collect();

    let rules: Vec<jwt::RequirementRule> = c
        .rules
        .iter()
        .map(|rule| jwt::RequirementRule {
            r#match: Some(route_match_proto(&rule.matcher)),
            requirement_type: Some(jwt::requirement_rule::RequirementType::RequirementName(
                rule.requirement_name.clone(),
            )),
        })
        .collect();

    jwt::JwtAuthentication {
        providers,
        requirement_map,
        rules,
        bypass_cors_preflight: c.bypass_cors_preflight,
        ..Default::default()
    }
}

fn ext_authz_to_proto(
    c: &fp_domain::gateway::filters::ExtAuthzConfig,
) -> envoy_types::pb::envoy::extensions::filters::http::ext_authz::v3::ExtAuthz {
    use envoy_types::pb::envoy::extensions::filters::http::ext_authz::v3 as ea;
    ea::ExtAuthz {
        services: Some(ea::ext_authz::Services::GrpcService(core::GrpcService {
            target_specifier: Some(core::grpc_service::TargetSpecifier::EnvoyGrpc(
                core::grpc_service::EnvoyGrpc {
                    cluster_name: c.cluster.clone(),
                    ..Default::default()
                },
            )),
            timeout: Some(millis_duration(c.timeout_ms)),
            ..Default::default()
        })),
        failure_mode_allow: c.failure_mode_allow,
        include_peer_certificate: c.include_peer_certificate,
        ..Default::default()
    }
}

fn rbac_to_proto(
    c: &fp_domain::gateway::filters::RbacConfig,
) -> envoy_types::pb::envoy::extensions::filters::http::rbac::v3::Rbac {
    use envoy_types::pb::envoy::config::rbac::v3 as rbacpb;
    use envoy_types::pb::envoy::extensions::filters::http::rbac::v3 as httprbac;
    use fp_domain::gateway::filters::{RbacAction, RbacPermission, RbacPrincipal};

    let action = match c.action {
        RbacAction::Allow => rbacpb::rbac::Action::Allow,
        RbacAction::Deny => rbacpb::rbac::Action::Deny,
    } as i32;

    let policies = c
        .policies
        .iter()
        .map(|(name, p)| {
            let permissions = p
                .permissions
                .iter()
                .map(|perm| match perm {
                    RbacPermission::Any => rbacpb::Permission {
                        rule: Some(rbacpb::permission::Rule::Any(true)),
                    },
                    RbacPermission::Header { name, exact } => rbacpb::Permission {
                        rule: Some(rbacpb::permission::Rule::Header(rt::HeaderMatcher {
                            name: name.clone(),
                            header_match_specifier: exact.as_ref().map(|v| {
                                rt::header_matcher::HeaderMatchSpecifier::StringMatch(
                                    string_exact(v),
                                )
                            }),
                            ..Default::default()
                        })),
                    },
                    RbacPermission::UrlPath { prefix } => rbacpb::Permission {
                        rule: Some(rbacpb::permission::Rule::UrlPath(
                            envoy_types::pb::envoy::r#type::matcher::v3::PathMatcher {
                                rule: Some(
                                    envoy_types::pb::envoy::r#type::matcher::v3::path_matcher::Rule::Path(
                                        string_prefix(prefix),
                                    ),
                                ),
                            },
                        )),
                    },
                    RbacPermission::DestinationPort { port } => rbacpb::Permission {
                        rule: Some(rbacpb::permission::Rule::DestinationPort(u32::from(*port))),
                    },
                })
                .collect();
            let principals = p
                .principals
                .iter()
                .map(|pr| match pr {
                    RbacPrincipal::Any => rbacpb::Principal {
                        identifier: Some(rbacpb::principal::Identifier::Any(true)),
                    },
                    RbacPrincipal::SourceCidr { cidr } => rbacpb::Principal {
                        identifier: Some(rbacpb::principal::Identifier::DirectRemoteIp(
                            cidr_range(cidr),
                        )),
                    },
                    RbacPrincipal::Header { name, exact } => rbacpb::Principal {
                        identifier: Some(rbacpb::principal::Identifier::Header(
                            rt::HeaderMatcher {
                                name: name.clone(),
                                header_match_specifier: Some(
                                    rt::header_matcher::HeaderMatchSpecifier::StringMatch(
                                        string_exact(exact),
                                    ),
                                ),
                                ..Default::default()
                            },
                        )),
                    },
                })
                .collect();
            (
                name.clone(),
                rbacpb::Policy {
                    permissions,
                    principals,
                    ..Default::default()
                },
            )
        })
        .collect();

    httprbac::Rbac {
        rules: Some(rbacpb::Rbac {
            action,
            policies,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn string_exact(value: &str) -> envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
    use envoy_types::pb::envoy::r#type::matcher::v3 as sm;
    sm::StringMatcher {
        match_pattern: Some(sm::string_matcher::MatchPattern::Exact(value.to_string())),
        ..Default::default()
    }
}

fn string_prefix(value: &str) -> envoy_types::pb::envoy::r#type::matcher::v3::StringMatcher {
    use envoy_types::pb::envoy::r#type::matcher::v3 as sm;
    sm::StringMatcher {
        match_pattern: Some(sm::string_matcher::MatchPattern::Prefix(value.to_string())),
        ..Default::default()
    }
}

/// Parse `ip/len` (validated in the domain layer) into an Envoy CidrRange.
fn cidr_range(cidr: &str) -> core::CidrRange {
    let (addr, len) = cidr.split_once('/').unwrap_or((cidr, "32"));
    core::CidrRange {
        address_prefix: addr.to_string(),
        prefix_len: Some(u32_value(len.parse::<u32>().unwrap_or(32))),
    }
}

/// RouteMatch from a domain PathMatch — shared by jwt rules and route translation.
fn route_match_proto(matcher: &PathMatch) -> rt::RouteMatch {
    let path_specifier = match matcher {
        PathMatch::Prefix { prefix } => rt::route_match::PathSpecifier::Prefix(prefix.clone()),
        PathMatch::Exact { path } => rt::route_match::PathSpecifier::Path(path.clone()),
        PathMatch::Template { template } => {
            rt::route_match::PathSpecifier::PathMatchPolicy(core::TypedExtensionConfig {
                name: "envoy.path.match.uri_template.uri_template_matcher".to_string(),
                typed_config: Some(any(
                    "type.googleapis.com/envoy.extensions.path.match.uri_template.v3.UriTemplateMatchConfig",
                    &envoy_types::pb::envoy::extensions::path::r#match::uri_template::v3::UriTemplateMatchConfig {
                        path_template: template.clone(),
                    },
                )),
            })
        }
    };
    rt::RouteMatch {
        path_specifier: Some(path_specifier),
        ..Default::default()
    }
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
                        filter_overrides: Vec::new(),
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
                        filter_overrides: Vec::new(),
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
                        filter_overrides: Vec::new(),
                    },
                ],
                filter_overrides: Vec::new(),
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

            // cors in the chain is the empty marker; the policy travels per-scope.
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
            let proto = listener_to_proto("edge2", &cors_spec).expect("cors chain marker");
            let manager = match &proto.filter_chains[0].filters[0].config_type {
                Some(lst::filter::ConfigType::TypedConfig(a)) => {
                    hcm::HttpConnectionManager::decode(a.value.as_slice()).expect("hcm")
                }
                _ => panic!("expected typed HCM"),
            };
            assert_eq!(manager.http_filters[0].name, "envoy.filters.http.cors");
        }
    }

    #[test]
    fn filter_overrides_become_typed_per_filter_config() {
        use fp_domain::gateway::filters::*;
        use fp_domain::gateway::route_config::{RouteAction, RouteRule, VirtualHost};
        let spec = RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "quiet".into(),
                    matcher: PathMatch::Prefix {
                        prefix: "/quiet".into(),
                    },
                    action: RouteAction {
                        cluster: "c".into(),
                        prefix_rewrite: None,
                        template_rewrite: None,
                        timeout_secs: 15,
                    },
                    filter_overrides: vec![FilterOverride::Disable {
                        filter_type: "local_rate_limit".into(),
                    }],
                }],
                filter_overrides: vec![FilterOverride::Cors(CorsConfig {
                    allow_origin: vec![OriginMatcher::Suffix {
                        value: ".example".into(),
                    }],
                    allow_methods: vec!["GET".into(), "POST".into()],
                    allow_headers: vec![],
                    expose_headers: vec![],
                    max_age_seconds: Some(600),
                    allow_credentials: true,
                })],
            }],
        };
        let proto = route_config_to_proto("orders", &spec).expect("translate");
        let vhost = &proto.virtual_hosts[0];
        let cors = vhost
            .typed_per_filter_config
            .get("envoy.filters.http.cors")
            .expect("vhost cors policy");
        assert!(cors.type_url.ends_with("cors.v3.CorsPolicy"));
        let policy =
            envoy_types::pb::envoy::extensions::filters::http::cors::v3::CorsPolicy::decode(
                cors.value.as_slice(),
            )
            .expect("decode policy");
        assert_eq!(policy.allow_methods, "GET,POST");
        assert_eq!(policy.max_age, "600");
        assert_eq!(policy.allow_origin_string_match.len(), 1);

        let disable = vhost.routes[0]
            .typed_per_filter_config
            .get("envoy.filters.http.local_ratelimit")
            .expect("route disable override");
        assert!(disable.type_url.ends_with("route.v3.FilterConfig"));
        let cfg = rt::FilterConfig::decode(disable.value.as_slice()).expect("decode");
        assert!(cfg.disabled);
    }

    #[test]
    fn secret_specs_translate_to_envoy_sds_secrets() {
        let generic = secret_to_proto(
            "api-token",
            &SecretSpec::GenericSecret {
                secret: "aGVsbG8=".into(),
            },
        )
        .expect("generic secret");
        assert_eq!(generic.name, "api-token");
        match generic.r#type.expect("type") {
            tls::secret::Type::GenericSecret(secret) => {
                let data = secret.secret.expect("inline secret");
                assert_eq!(
                    data.specifier,
                    Some(core::data_source::Specifier::InlineBytes(b"hello".to_vec()))
                );
            }
            _ => panic!("expected generic secret"),
        }

        let tls_secret = secret_to_proto(
            "edge-cert",
            &SecretSpec::TlsCertificate {
                certificate_chain: "CERT".into(),
                private_key: "KEY".into(),
                password: None,
                ocsp_staple: None,
            },
        )
        .expect("tls secret");
        assert!(matches!(
            tls_secret.r#type,
            Some(tls::secret::Type::TlsCertificate(_))
        ));
    }

    fn hcm_of(spec: &ListenerSpec) -> hcm::HttpConnectionManager {
        let proto = listener_to_proto("edge", spec).expect("translate");
        match &proto.filter_chains[0].filters[0].config_type {
            Some(lst::filter::ConfigType::TypedConfig(a)) => {
                hcm::HttpConnectionManager::decode(a.value.as_slice()).expect("hcm")
            }
            _ => panic!("expected typed HCM"),
        }
    }

    #[test]
    fn auth_filters_translate_into_the_chain() {
        use fp_domain::gateway::filters::*;
        use std::collections::BTreeMap;

        let mut providers = BTreeMap::new();
        providers.insert(
            "auth0".to_string(),
            JwtProvider {
                issuer: Some("https://issuer.example".into()),
                audiences: vec!["api".into()],
                jwks: JwksSource::Remote {
                    uri: "https://issuer.example/jwks".into(),
                    cluster: "jwks-cluster".into(),
                    timeout_ms: 5000,
                    cache_duration_secs: Some(600),
                },
                clock_skew_seconds: 30,
                forward: true,
            },
        );
        let mut requirement_map = BTreeMap::new();
        requirement_map.insert(
            "default".to_string(),
            JwtRequirement::Provider {
                provider_name: "auth0".into(),
            },
        );
        let mut policies = BTreeMap::new();
        policies.insert(
            "internal".to_string(),
            RbacPolicy {
                permissions: vec![RbacPermission::UrlPath {
                    prefix: "/admin".into(),
                }],
                principals: vec![RbacPrincipal::SourceCidr {
                    cidr: "10.0.0.0/8".into(),
                }],
            },
        );
        let spec = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10001,
            route_config: Some("orders".into()),
            http_filters: vec![
                HttpFilterEntry {
                    filter: HttpFilterSpec::JwtAuth(JwtAuthConfig {
                        providers,
                        requirement_map,
                        rules: vec![JwtRule {
                            matcher: PathMatch::Prefix { prefix: "/".into() },
                            requirement_name: "default".into(),
                        }],
                        bypass_cors_preflight: true,
                    }),
                    disabled: false,
                },
                HttpFilterEntry {
                    filter: HttpFilterSpec::ExtAuthz(ExtAuthzConfig {
                        cluster: "authz".into(),
                        timeout_ms: 200,
                        failure_mode_allow: false,
                        include_peer_certificate: true,
                    }),
                    disabled: false,
                },
                HttpFilterEntry {
                    filter: HttpFilterSpec::Rbac(RbacConfig {
                        action: RbacAction::Allow,
                        policies,
                    }),
                    disabled: false,
                },
            ],
        };
        let manager = hcm_of(&spec);
        let names: Vec<_> = manager
            .http_filters
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "envoy.filters.http.jwt_authn",
                "envoy.filters.http.ext_authz",
                "envoy.filters.http.rbac",
                "envoy.filters.http.router",
            ],
            "auth filters in declared order, router last"
        );

        // The RBAC proto message is `RBAC` (all-caps); a `Rbac` type URL makes Envoy NACK.
        let rbac_any = match &manager.http_filters[2].config_type {
            Some(hcm::http_filter::ConfigType::TypedConfig(a)) => a,
            _ => panic!("typed rbac config"),
        };
        assert!(
            rbac_any.type_url.ends_with(".rbac.v3.RBAC"),
            "rbac type URL must be all-caps RBAC, got {}",
            rbac_any.type_url
        );

        // The jwt filter decodes back with its remote provider intact.
        let jwt_any = match &manager.http_filters[0].config_type {
            Some(hcm::http_filter::ConfigType::TypedConfig(a)) => a,
            _ => panic!("typed jwt config"),
        };
        let jwt = envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::JwtAuthentication::decode(
            jwt_any.value.as_slice(),
        )
        .expect("decode jwt");
        assert!(jwt.providers.contains_key("auth0"));
        assert_eq!(jwt.rules.len(), 1);

        // Determinism: identical input → identical bytes (BTreeMap ordering).
        assert_eq!(
            listener_to_proto("edge", &spec).expect("a").encode_to_vec(),
            listener_to_proto("edge", &spec).expect("b").encode_to_vec(),
        );
    }

    #[test]
    fn jwt_per_route_override_emits_reference_only_config() {
        use fp_domain::gateway::filters::*;
        use fp_domain::gateway::route_config::{RouteAction, RouteRule, VirtualHost};
        let spec = RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "admin".into(),
                    matcher: PathMatch::Prefix {
                        prefix: "/admin".into(),
                    },
                    action: RouteAction {
                        cluster: "c".into(),
                        prefix_rewrite: None,
                        template_rewrite: None,
                        timeout_secs: 15,
                    },
                    filter_overrides: vec![FilterOverride::JwtAuth {
                        requirement_name: "admins-only".into(),
                    }],
                }],
                filter_overrides: vec![FilterOverride::Disable {
                    filter_type: "rbac".into(),
                }],
            }],
        };
        let proto = route_config_to_proto("orders", &spec).expect("translate");
        let vhost = &proto.virtual_hosts[0];
        // vhost disables rbac.
        assert!(vhost
            .typed_per_filter_config
            .contains_key("envoy.filters.http.rbac"));
        // route references a jwt requirement by name.
        let jwt = vhost.routes[0]
            .typed_per_filter_config
            .get("envoy.filters.http.jwt_authn")
            .expect("jwt per-route");
        assert!(jwt.type_url.ends_with("jwt_authn.v3.PerRouteConfig"));
        let cfg =
            envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::PerRouteConfig::decode(
                jwt.value.as_slice(),
            )
            .expect("decode");
        assert!(matches!(
            cfg.requirement_specifier,
            Some(envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::per_route_config::RequirementSpecifier::RequirementName(n)) if n == "admins-only"
        ));
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
                    filter_overrides: Vec::new(),
                }],
                filter_overrides: Vec::new(),
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

    #[test]
    fn generic_secret_translates_to_envoy_sds_secret() {
        let secret = secret_to_proto(
            "api-token",
            &SecretSpec::GenericSecret {
                secret: "aGVsbG8=".into(),
            },
        )
        .expect("secret");
        assert_eq!(secret.name, "api-token");
        let tls::secret::Type::GenericSecret(generic) = secret.r#type.expect("type") else {
            panic!("expected generic secret");
        };
        let source = generic.secret.expect("source");
        assert!(matches!(
            source.specifier,
            Some(core::data_source::Specifier::InlineBytes(bytes)) if bytes == b"hello"
        ));
    }
}
