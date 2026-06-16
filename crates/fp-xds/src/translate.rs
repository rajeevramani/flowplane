//! Domain → Envoy proto translation (the IR seam, spec/10 §5).

use base64::Engine as _;
use envoy_types::pb::envoy::config::accesslog::v3 as accesslog;
use envoy_types::pb::envoy::config::cluster::v3 as exc;
use envoy_types::pb::envoy::config::common::mutation_rules::v3 as mutation_rules;
use envoy_types::pb::envoy::config::core::v3 as core;
use envoy_types::pb::envoy::config::endpoint::v3 as ep;
use envoy_types::pb::envoy::config::listener::v3 as lst;
use envoy_types::pb::envoy::config::ratelimit::v3 as ratelimit_cfg;
use envoy_types::pb::envoy::config::route::v3 as rt;
use envoy_types::pb::envoy::extensions::access_loggers::file::v3 as file_accesslog;
use envoy_types::pb::envoy::extensions::access_loggers::grpc::v3 as grpc_accesslog;
use envoy_types::pb::envoy::extensions::filters::http::ext_proc::v3 as ext_proc;
use envoy_types::pb::envoy::extensions::filters::http::ratelimit::v3 as rate_limit_filter;
use envoy_types::pb::envoy::extensions::filters::http::router::v3::Router;
use envoy_types::pb::envoy::extensions::filters::http::upstream_codec::v3 as upstream_codec;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3 as hcm;
use envoy_types::pb::envoy::extensions::path::rewrite::uri_template::v3 as uri_template_rewrite;
use envoy_types::pb::envoy::extensions::transport_sockets::tls::v3 as tls;
use envoy_types::pb::envoy::extensions::upstreams::http::v3 as upstream_http;
use envoy_types::pb::envoy::r#type::matcher::v3 as matcher_type;
use envoy_types::pb::envoy::r#type::v3 as envoy_type;
use envoy_types::pb::google::protobuf as wkt;
use fp_domain::gateway::cluster::{
    CircuitBreakerThresholds, ClusterSpec, DnsLookupFamily, HealthCheck, HttpHealthCheckMethod,
    LbPolicy, RingHashFunction, UpstreamProtocol,
};
use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec, ListenerTlsConfig};
use fp_domain::gateway::route_config::{PathMatch, RouteConfigSpec};
use fp_domain::{DomainError, DomainResult, SecretSpec};
use prost::Message;
use std::collections::BTreeMap;

pub const LEARNING_ALS_CLUSTER: &str = "xds_cluster";
pub const LEARNING_EXT_PROC_CLUSTER: &str = "xds_cluster";
pub const AI_EXT_PROC_CLUSTER: &str = "xds_cluster";
const LEARNING_ALS_NAME: &str = "envoy.access_loggers.http_grpc";
pub(crate) const LEARNING_EXT_PROC_FILTER_PREFIX: &str =
    "envoy.filters.http.ext_proc.flowplane_learning.";
const AI_EXT_PROC_FILTER_NAME: &str = "envoy.filters.http.ext_proc.flowplane_ai";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningCaptureInjection {
    pub session_id: uuid::Uuid,
    pub team_id: uuid::Uuid,
    pub api_definition_id: Option<uuid::Uuid>,
    pub route_config_id: uuid::Uuid,
    pub listener_id: Option<uuid::Uuid>,
    pub virtual_host: Option<String>,
    pub route: Option<String>,
    pub discovery: Option<DiscoveryCaptureMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryCaptureMetadata {
    pub forwarded_upstream_host: String,
    pub forwarded_upstream_port: i32,
    pub forwarded_upstream_ip: String,
    pub forwarded_upstream_tls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProcessorMetadata {
    pub team_id: uuid::Uuid,
    pub listener_id: uuid::Uuid,
    pub route_config_id: uuid::Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiUpstreamProcessorMetadata {
    pub team_id: uuid::Uuid,
    pub route_config_id: uuid::Uuid,
    pub provider_id: uuid::Uuid,
    pub backend_position: i32,
}

fn any<M: Message>(type_url: &str, msg: &M) -> wkt::Any {
    wkt::Any {
        type_url: type_url.to_string(),
        value: msg.encode_to_vec(),
    }
}

fn any_with_value(type_url: &str, value: Vec<u8>) -> wkt::Any {
    wkt::Any {
        type_url: type_url.to_string(),
        value,
    }
}

// envoy-types emits prost map fields as HashMap, whose randomized iteration order can change
// encoded bytes across control-plane restarts. These wrappers cover the subset Flowplane emits
// and use BTreeMap for maps; every stable encode is decoded back to the generated type below.
#[derive(Clone, PartialEq, Message)]
struct StableRouteConfiguration {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(message, repeated, tag = "2")]
    virtual_hosts: Vec<StableVirtualHost>,
    #[prost(btree_map = "string, message", tag = "16")]
    typed_per_filter_config: BTreeMap<String, wkt::Any>,
}

#[derive(Clone, PartialEq, Message)]
struct StableVirtualHost {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(string, repeated, tag = "2")]
    domains: Vec<String>,
    #[prost(message, repeated, tag = "3")]
    routes: Vec<StableRoute>,
    #[prost(enumeration = "rt::virtual_host::TlsRequirementType", tag = "4")]
    require_tls: i32,
    #[prost(message, repeated, tag = "6")]
    rate_limits: Vec<rt::RateLimit>,
    #[prost(bool, tag = "14")]
    include_request_attempt_count: bool,
    #[prost(btree_map = "string, message", tag = "15")]
    typed_per_filter_config: BTreeMap<String, wkt::Any>,
}

#[derive(Clone, PartialEq, Message)]
struct StableRoute {
    #[prost(message, optional, tag = "1")]
    r#match: Option<rt::RouteMatch>,
    #[prost(btree_map = "string, message", tag = "13")]
    typed_per_filter_config: BTreeMap<String, wkt::Any>,
    #[prost(string, tag = "14")]
    name: String,
    #[prost(oneof = "rt::route::Action", tags = "2, 3, 7, 17, 18")]
    action: Option<rt::route::Action>,
}

#[derive(Clone, PartialEq, Message)]
struct StableJwtAuthentication {
    #[prost(btree_map = "string, message", tag = "1")]
    providers: BTreeMap<
        String,
        envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::JwtProvider,
    >,
    #[prost(message, repeated, tag = "2")]
    rules: Vec<envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::RequirementRule>,
    #[prost(message, optional, tag = "3")]
    filter_state_rules:
        Option<envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::FilterStateRule>,
    #[prost(bool, tag = "4")]
    bypass_cors_preflight: bool,
    #[prost(btree_map = "string, message", tag = "5")]
    requirement_map: BTreeMap<
        String,
        envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::JwtRequirement,
    >,
    #[prost(bool, tag = "6")]
    strip_failure_response: bool,
    #[prost(string, tag = "7")]
    stat_prefix: String,
}

#[derive(Clone, PartialEq, Message)]
struct StableHttpRbac {
    #[prost(message, optional, tag = "1")]
    rules: Option<StableConfigRbac>,
    #[prost(message, optional, tag = "4")]
    matcher: Option<envoy_types::pb::xds::r#type::matcher::v3::Matcher>,
    #[prost(string, tag = "6")]
    rules_stat_prefix: String,
}

#[derive(Clone, PartialEq, Message)]
struct StableConfigRbac {
    #[prost(
        enumeration = "envoy_types::pb::envoy::config::rbac::v3::rbac::Action",
        tag = "1"
    )]
    action: i32,
    #[prost(btree_map = "string, message", tag = "2")]
    policies: BTreeMap<String, envoy_types::pb::envoy::config::rbac::v3::Policy>,
    #[prost(message, optional, tag = "3")]
    audit_logging_options:
        Option<envoy_types::pb::envoy::config::rbac::v3::rbac::AuditLoggingOptions>,
}

impl From<&rt::RouteConfiguration> for StableRouteConfiguration {
    fn from(proto: &rt::RouteConfiguration) -> Self {
        Self {
            name: proto.name.clone(),
            virtual_hosts: proto
                .virtual_hosts
                .iter()
                .map(StableVirtualHost::from)
                .collect(),
            typed_per_filter_config: proto
                .typed_per_filter_config
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        }
    }
}

impl From<&rt::VirtualHost> for StableVirtualHost {
    fn from(proto: &rt::VirtualHost) -> Self {
        Self {
            name: proto.name.clone(),
            domains: proto.domains.clone(),
            routes: proto.routes.iter().map(StableRoute::from).collect(),
            require_tls: proto.require_tls,
            rate_limits: proto.rate_limits.clone(),
            include_request_attempt_count: proto.include_request_attempt_count,
            typed_per_filter_config: proto
                .typed_per_filter_config
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        }
    }
}

impl From<&rt::Route> for StableRoute {
    fn from(proto: &rt::Route) -> Self {
        Self {
            r#match: proto.r#match.clone(),
            typed_per_filter_config: proto
                .typed_per_filter_config
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            name: proto.name.clone(),
            action: proto.action.clone(),
        }
    }
}

impl From<&envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::JwtAuthentication>
    for StableJwtAuthentication
{
    fn from(
        proto: &envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::JwtAuthentication,
    ) -> Self {
        Self {
            providers: proto
                .providers
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            rules: proto.rules.clone(),
            filter_state_rules: proto.filter_state_rules.clone(),
            bypass_cors_preflight: proto.bypass_cors_preflight,
            requirement_map: proto
                .requirement_map
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            strip_failure_response: proto.strip_failure_response,
            stat_prefix: proto.stat_prefix.clone(),
        }
    }
}

impl From<&envoy_types::pb::envoy::extensions::filters::http::rbac::v3::Rbac> for StableHttpRbac {
    fn from(proto: &envoy_types::pb::envoy::extensions::filters::http::rbac::v3::Rbac) -> Self {
        Self {
            rules: proto.rules.as_ref().map(StableConfigRbac::from),
            matcher: proto.matcher.clone(),
            rules_stat_prefix: proto.rules_stat_prefix.clone(),
        }
    }
}

impl From<&envoy_types::pb::envoy::config::rbac::v3::Rbac> for StableConfigRbac {
    fn from(proto: &envoy_types::pb::envoy::config::rbac::v3::Rbac) -> Self {
        Self {
            action: proto.action,
            policies: proto
                .policies
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            audit_logging_options: proto.audit_logging_options.clone(),
        }
    }
}

fn verified_stable_encode<M, S>(label: &str, original: &M, stable: S) -> DomainResult<Vec<u8>>
where
    M: Message + Default + PartialEq,
    S: Message,
{
    let bytes = stable.encode_to_vec();
    let decoded = M::decode(bytes.as_slice())
        .map_err(|err| DomainError::internal(format!("decode stable {label}: {err}")))?;
    if &decoded != original {
        return Err(DomainError::internal(format!(
            "stable {label} encoder does not cover all populated fields"
        )));
    }
    Ok(bytes)
}

pub(crate) fn encode_route_config_deterministic(
    proto: &rt::RouteConfiguration,
) -> DomainResult<Vec<u8>> {
    verified_stable_encode(
        "route configuration",
        proto,
        StableRouteConfiguration::from(proto),
    )
}

fn encode_jwt_auth_deterministic(
    proto: &envoy_types::pb::envoy::extensions::filters::http::jwt_authn::v3::JwtAuthentication,
) -> DomainResult<Vec<u8>> {
    verified_stable_encode(
        "JWT authentication",
        proto,
        StableJwtAuthentication::from(proto),
    )
}

fn encode_http_rbac_deterministic(
    proto: &envoy_types::pb::envoy::extensions::filters::http::rbac::v3::Rbac,
) -> DomainResult<Vec<u8>> {
    verified_stable_encode("HTTP RBAC", proto, StableHttpRbac::from(proto))
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

fn u64_value(value: u64) -> wkt::UInt64Value {
    wkt::UInt64Value { value }
}

fn bool_value(value: bool) -> wkt::BoolValue {
    wkt::BoolValue { value }
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

fn filename(value: String) -> core::DataSource {
    core::DataSource {
        specifier: Some(core::data_source::Specifier::Filename(value)),
        ..Default::default()
    }
}

fn ads_config_source() -> core::ConfigSource {
    core::ConfigSource {
        resource_api_version: core::ApiVersion::V3 as i32,
        config_source_specifier: Some(core::config_source::ConfigSourceSpecifier::Ads(
            core::AggregatedConfigSource {},
        )),
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
    cluster_to_proto_with_ai(name, spec, None)
}

pub fn cluster_to_proto_with_ai(
    name: &str,
    spec: &ClusterSpec,
    ai: Option<&AiUpstreamProcessorMetadata>,
) -> DomainResult<exc::Cluster> {
    let lb_policy = match spec.lb_policy {
        LbPolicy::RoundRobin => exc::cluster::LbPolicy::RoundRobin,
        LbPolicy::LeastRequest => exc::cluster::LbPolicy::LeastRequest,
        LbPolicy::Random => exc::cluster::LbPolicy::Random,
        LbPolicy::RingHash => exc::cluster::LbPolicy::RingHash,
        LbPolicy::Maglev => exc::cluster::LbPolicy::Maglev,
    };

    let lb_config = match spec.lb_policy {
        LbPolicy::LeastRequest => spec.least_request.as_ref().map(|policy| {
            exc::cluster::LbConfig::LeastRequestLbConfig(exc::cluster::LeastRequestLbConfig {
                choice_count: policy.choice_count.map(u32_value),
                ..Default::default()
            })
        }),
        LbPolicy::RingHash => spec.ring_hash.as_ref().map(|policy| {
            let hash_function = match policy.hash_function.unwrap_or(RingHashFunction::XxHash) {
                RingHashFunction::XxHash => exc::cluster::ring_hash_lb_config::HashFunction::XxHash,
                RingHashFunction::MurmurHash2 => {
                    exc::cluster::ring_hash_lb_config::HashFunction::MurmurHash2
                }
            };
            exc::cluster::LbConfig::RingHashLbConfig(exc::cluster::RingHashLbConfig {
                minimum_ring_size: policy.minimum_ring_size.map(u64_value),
                maximum_ring_size: policy.maximum_ring_size.map(u64_value),
                hash_function: hash_function as i32,
            })
        }),
        LbPolicy::Maglev => spec.maglev.as_ref().map(|policy| {
            exc::cluster::LbConfig::MaglevLbConfig(exc::cluster::MaglevLbConfig {
                table_size: policy.table_size.map(u64_value),
            })
        }),
        LbPolicy::RoundRobin | LbPolicy::Random => None,
    };

    let transport_socket = if spec.use_tls || spec.upstream_tls.is_some() {
        Some(core::TransportSocket {
            name: "envoy.transport_sockets.tls".to_string(),
            config_type: Some(core::transport_socket::ConfigType::TypedConfig(any(
                "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext",
                &upstream_tls_context(spec),
            ))),
        })
    } else {
        None
    };

    let health_checks = spec
        .health_checks
        .iter()
        .flatten()
        .map(health_check_to_proto)
        .collect();

    let circuit_breakers = spec
        .circuit_breakers
        .as_ref()
        .map(|cb| exc::CircuitBreakers {
            thresholds: [
                cb.default.as_ref().map(|thresholds| {
                    circuit_breaker_to_proto(core::RoutingPriority::Default, thresholds)
                }),
                cb.high.as_ref().map(|thresholds| {
                    circuit_breaker_to_proto(core::RoutingPriority::High, thresholds)
                }),
            ]
            .into_iter()
            .flatten()
            .collect(),
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
            success_rate_minimum_hosts: od.min_hosts.map(u32_value),
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
        lb_config,
        dns_lookup_family: dns_lookup_family(spec, cluster_uses_eds(spec)),
        typed_extension_protocol_options: upstream_protocol_options(spec, ai),
        ..Default::default()
    })
}

fn upstream_tls_context(spec: &ClusterSpec) -> tls::UpstreamTlsContext {
    let tls_spec = spec.upstream_tls.as_ref();
    tls::UpstreamTlsContext {
        common_tls_context: Some(tls::CommonTlsContext {
            validation_context_type: tls_spec
                .and_then(|tls| tls.validation_context_sds_secret_name.as_deref())
                .map(|secret| {
                    tls::common_tls_context::ValidationContextType::ValidationContextSdsSecretConfig(
                        sds_secret_config(secret),
                    )
                }),
            ..Default::default()
        }),
        sni: tls_spec.and_then(|tls| tls.sni.clone()).unwrap_or_default(),
        auto_sni_san_validation: tls_spec
            .map(|tls| tls.auto_sni_san_validation)
            .unwrap_or(false),
        ..Default::default()
    }
}

fn health_check_to_proto(hc: &HealthCheck) -> core::HealthCheck {
    match hc {
        HealthCheck::Http(hc) => core::HealthCheck {
            timeout: Some(duration(hc.timeout_seconds)),
            interval: Some(duration(hc.interval_seconds)),
            healthy_threshold: Some(u32_value(hc.healthy_threshold)),
            unhealthy_threshold: Some(u32_value(hc.unhealthy_threshold)),
            health_checker: Some(core::health_check::HealthChecker::HttpHealthCheck(
                core::health_check::HttpHealthCheck {
                    host: hc.host.clone().unwrap_or_default(),
                    path: hc.path.clone(),
                    expected_statuses: hc
                        .expected_statuses
                        .iter()
                        .map(|status| envoy_type::Int64Range {
                            start: i64::from(*status),
                            end: i64::from(*status) + 1,
                        })
                        .collect(),
                    method: hc.method.map(http_method).unwrap_or_default(),
                    ..Default::default()
                },
            )),
            ..Default::default()
        },
        HealthCheck::Tcp(hc) => core::HealthCheck {
            timeout: Some(duration(hc.timeout_seconds)),
            interval: Some(duration(hc.interval_seconds)),
            healthy_threshold: Some(u32_value(hc.healthy_threshold)),
            unhealthy_threshold: Some(u32_value(hc.unhealthy_threshold)),
            health_checker: Some(core::health_check::HealthChecker::TcpHealthCheck(
                core::health_check::TcpHealthCheck::default(),
            )),
            ..Default::default()
        },
    }
}

fn http_method(method: HttpHealthCheckMethod) -> i32 {
    (match method {
        HttpHealthCheckMethod::Get => core::RequestMethod::Get,
        HttpHealthCheckMethod::Head => core::RequestMethod::Head,
        HttpHealthCheckMethod::Post => core::RequestMethod::Post,
        HttpHealthCheckMethod::Put => core::RequestMethod::Put,
        HttpHealthCheckMethod::Delete => core::RequestMethod::Delete,
        HttpHealthCheckMethod::Options => core::RequestMethod::Options,
        HttpHealthCheckMethod::Trace => core::RequestMethod::Trace,
        HttpHealthCheckMethod::Patch => core::RequestMethod::Patch,
    }) as i32
}

fn circuit_breaker_to_proto(
    priority: core::RoutingPriority,
    cb: &CircuitBreakerThresholds,
) -> exc::circuit_breakers::Thresholds {
    exc::circuit_breakers::Thresholds {
        priority: priority as i32,
        max_connections: Some(u32_value(cb.max_connections)),
        max_pending_requests: Some(u32_value(cb.max_pending_requests)),
        max_requests: Some(u32_value(cb.max_requests)),
        max_retries: Some(u32_value(cb.max_retries)),
        ..Default::default()
    }
}

fn dns_lookup_family(spec: &ClusterSpec, uses_eds: bool) -> i32 {
    if uses_eds {
        return exc::cluster::DnsLookupFamily::Auto as i32;
    }
    (match spec.dns_lookup_family.unwrap_or(DnsLookupFamily::Auto) {
        DnsLookupFamily::Auto => exc::cluster::DnsLookupFamily::Auto,
        DnsLookupFamily::V4Only => exc::cluster::DnsLookupFamily::V4Only,
        DnsLookupFamily::V6Only => exc::cluster::DnsLookupFamily::V6Only,
        DnsLookupFamily::V4Preferred => exc::cluster::DnsLookupFamily::V4Preferred,
        DnsLookupFamily::All => exc::cluster::DnsLookupFamily::All,
    }) as i32
}

fn upstream_protocol_options(
    spec: &ClusterSpec,
    ai: Option<&AiUpstreamProcessorMetadata>,
) -> std::collections::HashMap<String, wkt::Any> {
    let Some(protocol) = spec.protocol else {
        if let Some(ai) = ai {
            return upstream_http_options(None, ai);
        }
        return std::collections::HashMap::new();
    };
    match protocol {
        UpstreamProtocol::Http1 => ai
            .map(|ai| upstream_http_options(None, ai))
            .unwrap_or_default(),
        UpstreamProtocol::Http2 | UpstreamProtocol::Grpc => {
            let protocol_options = explicit_http2_config();
            if let Some(ai) = ai {
                upstream_http_options(Some(protocol_options), ai)
            } else {
                let options = upstream_http::HttpProtocolOptions {
                    upstream_protocol_options: Some(protocol_options),
                    ..Default::default()
                };
                std::iter::once((
                    "envoy.extensions.upstreams.http.v3.HttpProtocolOptions".to_string(),
                    any(
                        "type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions",
                        &options,
                    ),
                ))
                .collect()
            }
        }
    }
}

fn upstream_http_options(
    protocol_options: Option<upstream_http::http_protocol_options::UpstreamProtocolOptions>,
    ai: &AiUpstreamProcessorMetadata,
) -> std::collections::HashMap<String, wkt::Any> {
    let options = upstream_http::HttpProtocolOptions {
        upstream_protocol_options: Some(protocol_options.unwrap_or_else(explicit_http1_config)),
        http_filters: vec![
            ai_upstream_ext_proc_filter(ai),
            hcm::HttpFilter {
                name: "envoy.filters.http.upstream_codec".to_string(),
                config_type: Some(hcm::http_filter::ConfigType::TypedConfig(any(
                    "type.googleapis.com/envoy.extensions.filters.http.upstream_codec.v3.UpstreamCodec",
                    &upstream_codec::UpstreamCodec {},
                ))),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    std::iter::once((
        "envoy.extensions.upstreams.http.v3.HttpProtocolOptions".to_string(),
        any(
            "type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions",
            &options,
        ),
    ))
    .collect()
}

fn ai_upstream_ext_proc_filter(ai: &AiUpstreamProcessorMetadata) -> hcm::HttpFilter {
    hcm::HttpFilter {
        name: format!("{AI_EXT_PROC_FILTER_NAME}.upstream"),
        config_type: Some(hcm::http_filter::ConfigType::TypedConfig(any(
            "type.googleapis.com/envoy.extensions.filters.http.ext_proc.v3.ExternalProcessor",
            &ext_proc::ExternalProcessor {
                grpc_service: Some(ai_upstream_grpc_service(ai)),
                failure_mode_allow: false,
                processing_mode: Some(ext_proc::ProcessingMode {
                    request_header_mode: ext_proc::processing_mode::HeaderSendMode::Send as i32,
                    response_header_mode: ext_proc::processing_mode::HeaderSendMode::Send as i32,
                    request_body_mode: ext_proc::processing_mode::BodySendMode::Buffered as i32,
                    response_body_mode: ext_proc::processing_mode::BodySendMode::BufferedPartial
                        as i32,
                    request_trailer_mode: ext_proc::processing_mode::HeaderSendMode::Skip as i32,
                    response_trailer_mode: ext_proc::processing_mode::HeaderSendMode::Skip as i32,
                }),
                message_timeout: Some(millis_duration(5_000)),
                stat_prefix: "flowplane_ai_upstream".into(),
                mutation_rules: Some(ai_header_mutation_rules()),
                observability_mode: false,
                disable_immediate_response: false,
                ..Default::default()
            },
        ))),
        is_optional: false,
        ..Default::default()
    }
}

fn ai_upstream_grpc_service(ai: &AiUpstreamProcessorMetadata) -> core::GrpcService {
    core::GrpcService {
        timeout: Some(millis_duration(5_000)),
        initial_metadata: vec![
            header("x-flowplane-ai-processor", "true".into()),
            header("x-flowplane-ai-upstream-processor", "true".into()),
            header("x-flowplane-team-id", ai.team_id.to_string()),
            header(
                "x-flowplane-route-config-id",
                ai.route_config_id.to_string(),
            ),
            header("x-flowplane-ai-provider-id", ai.provider_id.to_string()),
            header(
                "x-flowplane-ai-backend-position",
                ai.backend_position.to_string(),
            ),
        ],
        target_specifier: Some(core::grpc_service::TargetSpecifier::EnvoyGrpc(
            core::grpc_service::EnvoyGrpc {
                cluster_name: AI_EXT_PROC_CLUSTER.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
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
                r#match: Some(route_match_proto(rule)?),
                action: Some(route_action_proto(rule)?),
                typed_per_filter_config: overrides_to_typed_config(&rule.filter_overrides)?,
                ..Default::default()
            });
        }
        virtual_hosts.push(rt::VirtualHost {
            name: vhost.name.clone(),
            domains: vhost.domains.clone(),
            routes,
            rate_limits: rate_limits_to_proto(&vhost.rate_limits),
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

fn route_action_proto(
    rule: &fp_domain::gateway::route_config::RouteRule,
) -> DomainResult<rt::route::Action> {
    use fp_domain::gateway::route_config::RedirectResponseCode;
    if let Some(direct) = &rule.action.direct_response {
        return Ok(rt::route::Action::DirectResponse(
            rt::DirectResponseAction {
                status: u32::from(direct.status),
                body: direct.body.as_ref().map(|body| core::DataSource {
                    specifier: Some(core::data_source::Specifier::InlineString(body.clone())),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ));
    }
    if let Some(redirect) = &rule.action.redirect {
        let response_code = redirect
            .response_code
            .map(|code| match code {
                RedirectResponseCode::MovedPermanently => {
                    rt::redirect_action::RedirectResponseCode::MovedPermanently
                }
                RedirectResponseCode::Found => rt::redirect_action::RedirectResponseCode::Found,
                RedirectResponseCode::SeeOther => {
                    rt::redirect_action::RedirectResponseCode::SeeOther
                }
                RedirectResponseCode::TemporaryRedirect => {
                    rt::redirect_action::RedirectResponseCode::TemporaryRedirect
                }
                RedirectResponseCode::PermanentRedirect => {
                    rt::redirect_action::RedirectResponseCode::PermanentRedirect
                }
            })
            .unwrap_or(rt::redirect_action::RedirectResponseCode::MovedPermanently);
        let scheme_rewrite_specifier =
            match (redirect.https_redirect, redirect.scheme_redirect.as_ref()) {
                (Some(value), None) => Some(
                    rt::redirect_action::SchemeRewriteSpecifier::HttpsRedirect(value),
                ),
                (None, Some(value)) => Some(
                    rt::redirect_action::SchemeRewriteSpecifier::SchemeRedirect(value.clone()),
                ),
                _ => None,
            };
        let path_rewrite_specifier = match (
            redirect.path_redirect.as_ref(),
            redirect.prefix_rewrite.as_ref(),
        ) {
            (Some(path), None) => Some(rt::redirect_action::PathRewriteSpecifier::PathRedirect(
                path.clone(),
            )),
            (None, Some(prefix)) => Some(rt::redirect_action::PathRewriteSpecifier::PrefixRewrite(
                prefix.clone(),
            )),
            _ => None,
        };
        return Ok(rt::route::Action::Redirect(rt::RedirectAction {
            host_redirect: redirect.host_redirect.clone().unwrap_or_default(),
            response_code: response_code as i32,
            strip_query: redirect.strip_query,
            scheme_rewrite_specifier,
            path_rewrite_specifier,
            ..Default::default()
        }));
    }

    let cluster_specifier = if let Some(cluster) = &rule.action.cluster {
        rt::route_action::ClusterSpecifier::Cluster(cluster.clone())
    } else {
        rt::route_action::ClusterSpecifier::WeightedClusters(rt::WeightedCluster {
            clusters: rule
                .action
                .weighted_clusters
                .as_ref()
                .into_iter()
                .flatten()
                .map(|target| rt::weighted_cluster::ClusterWeight {
                    name: target.cluster.clone(),
                    weight: Some(u32_value(target.weight)),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    };

    let path_rewrite_policy = rule.action.template_rewrite.as_ref().map(|rewrite| {
        core::TypedExtensionConfig {
            name: "envoy.path.rewrite.uri_template.uri_template_rewriter".to_string(),
            typed_config: Some(any(
                "type.googleapis.com/envoy.extensions.path.rewrite.uri_template.v3.UriTemplateRewriteConfig",
                &uri_template_rewrite::UriTemplateRewriteConfig {
                    path_template_rewrite: rewrite.clone(),
                },
            )),
        }
    });

    Ok(rt::route::Action::Route(rt::RouteAction {
        cluster_specifier: Some(cluster_specifier),
        prefix_rewrite: rule.action.prefix_rewrite.clone().unwrap_or_default(),
        path_rewrite_policy,
        timeout: Some(duration(rule.action.timeout_secs)),
        retry_policy: rule.action.retry_policy.as_ref().map(retry_policy_to_proto),
        rate_limits: rate_limits_to_proto(&rule.action.rate_limits),
        ..Default::default()
    }))
}

fn rate_limits_to_proto(
    limits: &[fp_domain::gateway::route_config::RateLimitDefinition],
) -> Vec<rt::RateLimit> {
    limits
        .iter()
        .map(|limit| rt::RateLimit {
            stage: limit.stage.map(u32_value),
            disable_key: limit.disable_key.clone().unwrap_or_default(),
            actions: limit
                .actions
                .iter()
                .map(rate_limit_action_to_proto)
                .collect(),
            ..Default::default()
        })
        .collect()
}

fn explicit_http1_config() -> upstream_http::http_protocol_options::UpstreamProtocolOptions {
    upstream_http::http_protocol_options::UpstreamProtocolOptions::ExplicitHttpConfig(
        upstream_http::http_protocol_options::ExplicitHttpConfig {
            protocol_config: Some(
                upstream_http::http_protocol_options::explicit_http_config::ProtocolConfig::HttpProtocolOptions(
                    core::Http1ProtocolOptions::default(),
                ),
            ),
        },
    )
}

fn explicit_http2_config() -> upstream_http::http_protocol_options::UpstreamProtocolOptions {
    upstream_http::http_protocol_options::UpstreamProtocolOptions::ExplicitHttpConfig(
        upstream_http::http_protocol_options::ExplicitHttpConfig {
            protocol_config: Some(
                upstream_http::http_protocol_options::explicit_http_config::ProtocolConfig::Http2ProtocolOptions(
                    core::Http2ProtocolOptions::default(),
                ),
            ),
        },
    )
}

fn rate_limit_action_to_proto(
    action: &fp_domain::gateway::route_config::RateLimitAction,
) -> rt::rate_limit::Action {
    use fp_domain::gateway::route_config::RateLimitAction;
    let action_specifier = match action {
        RateLimitAction::RequestHeaders {
            header_name,
            descriptor_key,
            skip_if_absent,
        } => rt::rate_limit::action::ActionSpecifier::RequestHeaders(
            rt::rate_limit::action::RequestHeaders {
                header_name: header_name.clone(),
                descriptor_key: descriptor_key.clone(),
                skip_if_absent: *skip_if_absent,
            },
        ),
        RateLimitAction::GenericKey {
            descriptor_value,
            descriptor_key,
        } => rt::rate_limit::action::ActionSpecifier::GenericKey(
            rt::rate_limit::action::GenericKey {
                descriptor_value: descriptor_value.clone(),
                descriptor_key: descriptor_key.clone().unwrap_or_default(),
                default_value: String::new(),
            },
        ),
    };
    rt::rate_limit::Action {
        action_specifier: Some(action_specifier),
    }
}

fn retry_policy_to_proto(retry: &fp_domain::gateway::route_config::RetryPolicy) -> rt::RetryPolicy {
    rt::RetryPolicy {
        retry_on: retry.retry_on.clone(),
        num_retries: retry.num_retries.map(u32_value),
        per_try_timeout: retry.per_try_timeout_secs.map(duration),
        retriable_status_codes: retry
            .retriable_status_codes
            .iter()
            .map(|status| u32::from(*status))
            .collect(),
        ..Default::default()
    }
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
            any_with_value(
                "type.googleapis.com/envoy.extensions.filters.http.jwt_authn.v3.JwtAuthentication",
                encode_jwt_auth_deterministic(&jwt_auth_to_proto(c))?,
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
            any_with_value(
                "type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBAC",
                encode_http_rbac_deterministic(&rbac_to_proto(c))?,
            ),
        ),
        HttpFilterSpec::GlobalRateLimit(c) => (
            "envoy.filters.http.ratelimit",
            any(
                "type.googleapis.com/envoy.extensions.filters.http.ratelimit.v3.RateLimit",
                &global_rate_limit_to_proto(c),
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

fn global_rate_limit_to_proto(
    c: &fp_domain::gateway::filters::GlobalRateLimitConfig,
) -> rate_limit_filter::RateLimit {
    use fp_domain::gateway::filters::RateLimitRequestType;

    rate_limit_filter::RateLimit {
        domain: c.domain.clone(),
        stage: c.stage,
        request_type: match c.request_type {
            RateLimitRequestType::Both => "both",
            RateLimitRequestType::Internal => "internal",
            RateLimitRequestType::External => "external",
        }
        .to_string(),
        timeout: Some(millis_duration(c.timeout_ms)),
        failure_mode_deny: c.failure_mode_deny,
        rate_limit_service: Some(ratelimit_cfg::RateLimitServiceConfig {
            grpc_service: Some(core::GrpcService {
                timeout: Some(millis_duration(c.timeout_ms)),
                target_specifier: Some(core::grpc_service::TargetSpecifier::EnvoyGrpc(
                    core::grpc_service::EnvoyGrpc {
                        cluster_name: c.service_cluster.clone(),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }),
            transport_api_version: core::ApiVersion::V3 as i32,
        }),
        enable_x_ratelimit_headers: if c.enable_x_ratelimit_headers {
            rate_limit_filter::rate_limit::XRateLimitHeadersRfcVersion::DraftVersion03 as i32
        } else {
            rate_limit_filter::rate_limit::XRateLimitHeadersRfcVersion::Off as i32
        },
        disable_x_envoy_ratelimited_header: c.disable_x_envoy_ratelimited_header,
        rate_limited_status: c
            .rate_limited_status
            .map(|code| envoy_type::HttpStatus { code: code as i32 }),
        status_on_error: c
            .status_on_error
            .map(|code| envoy_type::HttpStatus { code: code as i32 }),
        stat_prefix: c.stat_prefix.clone().unwrap_or_default(),
        ..Default::default()
    }
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
            r#match: Some(rt::RouteMatch {
                path_specifier: Some(route_path_specifier(&rule.matcher)),
                ..Default::default()
            }),
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

/// RouteMatch from a domain route rule.
fn route_match_proto(
    rule: &fp_domain::gateway::route_config::RouteRule,
) -> DomainResult<rt::RouteMatch> {
    let path_specifier = route_path_specifier(&rule.matcher);
    Ok(rt::RouteMatch {
        path_specifier: Some(path_specifier),
        headers: rule
            .headers
            .iter()
            .map(header_match_to_proto)
            .collect::<DomainResult<Vec<_>>>()?,
        query_parameters: rule
            .query_parameters
            .iter()
            .map(query_match_to_proto)
            .collect::<DomainResult<Vec<_>>>()?,
        ..Default::default()
    })
}

fn route_path_specifier(matcher: &PathMatch) -> rt::route_match::PathSpecifier {
    match matcher {
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
        PathMatch::Regex { pattern } => {
            rt::route_match::PathSpecifier::SafeRegex(safe_regex(pattern))
        }
    }
}

#[allow(deprecated)]
fn safe_regex(pattern: &str) -> matcher_type::RegexMatcher {
    matcher_type::RegexMatcher {
        regex: pattern.to_string(),
        engine_type: Some(matcher_type::regex_matcher::EngineType::GoogleRe2(
            matcher_type::regex_matcher::GoogleRe2::default(),
        )),
    }
}

fn query_string_match_to_proto(
    matcher: &fp_domain::gateway::route_config::QueryValueMatch,
) -> DomainResult<matcher_type::StringMatcher> {
    use fp_domain::gateway::route_config::QueryValueMatch;
    let match_pattern = match matcher {
        QueryValueMatch::Exact { value } => {
            matcher_type::string_matcher::MatchPattern::Exact(value.clone())
        }
        QueryValueMatch::Prefix { value } => {
            matcher_type::string_matcher::MatchPattern::Prefix(value.clone())
        }
        QueryValueMatch::Suffix { value } => {
            matcher_type::string_matcher::MatchPattern::Suffix(value.clone())
        }
        QueryValueMatch::Contains { value } => {
            matcher_type::string_matcher::MatchPattern::Contains(value.clone())
        }
        QueryValueMatch::Regex { pattern } => {
            matcher_type::string_matcher::MatchPattern::SafeRegex(safe_regex(pattern))
        }
        QueryValueMatch::Present { .. } => {
            return Err(DomainError::internal(
                "present query matcher cannot be translated as a string matcher",
            ));
        }
    };
    Ok(matcher_type::StringMatcher {
        match_pattern: Some(match_pattern),
        ..Default::default()
    })
}

fn header_string_match_to_proto(
    matcher: &fp_domain::gateway::route_config::HeaderValueMatch,
) -> DomainResult<matcher_type::StringMatcher> {
    use fp_domain::gateway::route_config::HeaderValueMatch;
    let query_equivalent = match matcher {
        HeaderValueMatch::Exact { value } => {
            fp_domain::gateway::route_config::QueryValueMatch::Exact {
                value: value.clone(),
            }
        }
        HeaderValueMatch::Prefix { value } => {
            fp_domain::gateway::route_config::QueryValueMatch::Prefix {
                value: value.clone(),
            }
        }
        HeaderValueMatch::Suffix { value } => {
            fp_domain::gateway::route_config::QueryValueMatch::Suffix {
                value: value.clone(),
            }
        }
        HeaderValueMatch::Contains { value } => {
            fp_domain::gateway::route_config::QueryValueMatch::Contains {
                value: value.clone(),
            }
        }
        HeaderValueMatch::Regex { pattern } => {
            fp_domain::gateway::route_config::QueryValueMatch::Regex {
                pattern: pattern.clone(),
            }
        }
        HeaderValueMatch::Present { .. } => {
            return Err(DomainError::internal(
                "present header matcher cannot be translated as a string matcher",
            ));
        }
    };
    query_string_match_to_proto(&query_equivalent)
}

fn header_match_to_proto(
    header: &fp_domain::gateway::route_config::HeaderMatch,
) -> DomainResult<rt::HeaderMatcher> {
    use fp_domain::gateway::route_config::HeaderValueMatch;
    let header_match_specifier = match &header.matcher {
        HeaderValueMatch::Present { value } => {
            rt::header_matcher::HeaderMatchSpecifier::PresentMatch(*value)
        }
        other => rt::header_matcher::HeaderMatchSpecifier::StringMatch(
            header_string_match_to_proto(other)?,
        ),
    };
    Ok(rt::HeaderMatcher {
        name: header.name.clone(),
        invert_match: header.invert_match,
        header_match_specifier: Some(header_match_specifier),
        ..Default::default()
    })
}

fn query_match_to_proto(
    query: &fp_domain::gateway::route_config::QueryParameterMatch,
) -> DomainResult<rt::QueryParameterMatcher> {
    use fp_domain::gateway::route_config::QueryValueMatch;
    let query_parameter_match_specifier = match &query.matcher {
        QueryValueMatch::Present { value } => {
            rt::query_parameter_matcher::QueryParameterMatchSpecifier::PresentMatch(*value)
        }
        other => rt::query_parameter_matcher::QueryParameterMatchSpecifier::StringMatch(
            query_string_match_to_proto(other)?,
        ),
    };
    Ok(rt::QueryParameterMatcher {
        name: query.name.clone(),
        query_parameter_match_specifier: Some(query_parameter_match_specifier),
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
    listener_to_proto_with_learning(name, spec, &[])
}

pub fn listener_to_proto_with_learning(
    name: &str,
    spec: &ListenerSpec,
    captures: &[LearningCaptureInjection],
) -> DomainResult<lst::Listener> {
    listener_to_proto_with_learning_and_ai(name, spec, captures, None)
}

pub fn listener_to_proto_with_learning_and_ai(
    name: &str,
    spec: &ListenerSpec,
    captures: &[LearningCaptureInjection],
    ai: Option<&AiProcessorMetadata>,
) -> DomainResult<lst::Listener> {
    let route_config_name = spec.route_config.clone().ok_or_else(|| {
        DomainError::validation(format!(
            "listener \"{name}\" has no route_config bound; it cannot serve traffic yet"
        ))
    })?;

    // Chain: declared filters in order, router appended last (spec/04 §4.2).
    let mut http_filters = Vec::with_capacity(spec.http_filters.len() + captures.len() + 1);
    for entry in &spec.http_filters {
        http_filters.push(http_filter_to_proto(entry)?);
    }
    if ai.is_some() {
        http_filters.push(ai_ext_proc_filter(ai));
    }
    for capture in captures {
        http_filters.push(ext_proc_filter(capture));
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
        codec_type: listener_codec_type(spec) as i32,
        stat_prefix: name.to_string(),
        route_specifier: Some(hcm::http_connection_manager::RouteSpecifier::Rds(
            hcm::Rds {
                route_config_name,
                config_source: Some(ads_config_source()),
            },
        )),
        http_filters,
        access_log: access_logs_to_proto(&spec.access_logs)
            .into_iter()
            .chain(captures.iter().map(learning_access_log))
            .collect(),
        generate_request_id: Some(bool_value(true)),
        always_set_request_id_in_response: true,
        ..Default::default()
    };
    let transport_socket = spec
        .tls_context
        .as_ref()
        .map(downstream_tls_transport_socket)
        .transpose()?;

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
            transport_socket,
            ..Default::default()
        }],
        ..Default::default()
    })
}

fn grpc_service(cluster: &'static str, capture: &LearningCaptureInjection) -> core::GrpcService {
    let mut initial_metadata = vec![header("x-flowplane-team-id", capture.team_id.to_string())];
    if let Some(discovery) = &capture.discovery {
        initial_metadata.extend([
            header(
                "x-flowplane-discovery-session-id",
                capture.session_id.to_string(),
            ),
            header(
                "x-flowplane-forwarded-upstream-host",
                discovery.forwarded_upstream_host.clone(),
            ),
            header(
                "x-flowplane-forwarded-upstream-port",
                discovery.forwarded_upstream_port.to_string(),
            ),
            header(
                "x-flowplane-forwarded-upstream-ip",
                discovery.forwarded_upstream_ip.clone(),
            ),
            header(
                "x-flowplane-forwarded-upstream-tls",
                discovery.forwarded_upstream_tls.to_string(),
            ),
        ]);
        if let Some(listener_id) = capture.listener_id {
            initial_metadata.push(header(
                "x-flowplane-discovery-listener-id",
                listener_id.to_string(),
            ));
        }
    } else {
        initial_metadata.extend([
            header(
                "x-flowplane-capture-session-id",
                capture.session_id.to_string(),
            ),
            header(
                "x-flowplane-route-config-id",
                capture.route_config_id.to_string(),
            ),
        ]);
        if let Some(api_id) = capture.api_definition_id {
            initial_metadata.push(header("x-flowplane-api-definition-id", api_id.to_string()));
        }
        if let Some(listener_id) = capture.listener_id {
            initial_metadata.push(header("x-flowplane-listener-id", listener_id.to_string()));
        }
        if let Some(virtual_host) = &capture.virtual_host {
            initial_metadata.push(header("x-flowplane-virtual-host", virtual_host.clone()));
        }
        if let Some(route) = &capture.route {
            initial_metadata.push(header("x-flowplane-route", route.clone()));
        }
    }
    core::GrpcService {
        timeout: Some(millis_duration(5_000)),
        initial_metadata,
        target_specifier: Some(core::grpc_service::TargetSpecifier::EnvoyGrpc(
            core::grpc_service::EnvoyGrpc {
                cluster_name: cluster.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

fn ai_grpc_service(ai: Option<&AiProcessorMetadata>) -> core::GrpcService {
    let mut initial_metadata = vec![header("x-flowplane-ai-processor", "true".into())];
    if let Some(ai) = ai {
        initial_metadata.extend([
            header("x-flowplane-team-id", ai.team_id.to_string()),
            header("x-flowplane-listener-id", ai.listener_id.to_string()),
            header(
                "x-flowplane-route-config-id",
                ai.route_config_id.to_string(),
            ),
        ]);
    }
    core::GrpcService {
        timeout: Some(millis_duration(5_000)),
        initial_metadata,
        target_specifier: Some(core::grpc_service::TargetSpecifier::EnvoyGrpc(
            core::grpc_service::EnvoyGrpc {
                cluster_name: AI_EXT_PROC_CLUSTER.to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

fn header(key: &'static str, value: String) -> core::HeaderValue {
    core::HeaderValue {
        key: key.to_string(),
        value,
        raw_value: Vec::new(),
    }
}

fn ai_header_mutation_rules() -> mutation_rules::HeaderMutationRules {
    mutation_rules::HeaderMutationRules {
        allow_all_routing: Some(wkt::BoolValue { value: true }),
        allow_expression: Some(safe_regex("^(authorization|x-flowplane-ai-model|:path)$")),
        ..Default::default()
    }
}

fn learning_access_log(capture: &LearningCaptureInjection) -> accesslog::AccessLog {
    accesslog::AccessLog {
        name: LEARNING_ALS_NAME.to_string(),
        filter: None,
        config_type: Some(accesslog::access_log::ConfigType::TypedConfig(any(
            "type.googleapis.com/envoy.extensions.access_loggers.grpc.v3.HttpGrpcAccessLogConfig",
            &grpc_accesslog::HttpGrpcAccessLogConfig {
                common_config: Some(grpc_accesslog::CommonGrpcAccessLogConfig {
                    log_name: format!("flowplane_learning_session_{}", capture.session_id),
                    grpc_service: Some(grpc_service(LEARNING_ALS_CLUSTER, capture)),
                    transport_api_version: core::ApiVersion::V3 as i32,
                    buffer_size_bytes: Some(u32_value(16_384)),
                    ..Default::default()
                }),
                additional_request_headers_to_log: vec![
                    "content-type".into(),
                    "content-length".into(),
                    "accept".into(),
                    "user-agent".into(),
                    "x-request-id".into(),
                    "x-envoy-original-path".into(),
                ],
                additional_response_headers_to_log: vec![
                    "content-type".into(),
                    "content-length".into(),
                    "www-authenticate".into(),
                ],
                additional_response_trailers_to_log: Vec::new(),
            },
        ))),
    }
}

fn ai_ext_proc_filter(ai: Option<&AiProcessorMetadata>) -> hcm::HttpFilter {
    hcm::HttpFilter {
        name: AI_EXT_PROC_FILTER_NAME.to_string(),
        config_type: Some(hcm::http_filter::ConfigType::TypedConfig(any(
            "type.googleapis.com/envoy.extensions.filters.http.ext_proc.v3.ExternalProcessor",
            &ext_proc::ExternalProcessor {
                grpc_service: Some(ai_grpc_service(ai)),
                failure_mode_allow: false,
                processing_mode: Some(ext_proc::ProcessingMode {
                    request_header_mode: ext_proc::processing_mode::HeaderSendMode::Send as i32,
                    response_header_mode: ext_proc::processing_mode::HeaderSendMode::Send as i32,
                    request_body_mode: ext_proc::processing_mode::BodySendMode::Buffered as i32,
                    response_body_mode: ext_proc::processing_mode::BodySendMode::BufferedPartial
                        as i32,
                    request_trailer_mode: ext_proc::processing_mode::HeaderSendMode::Skip as i32,
                    response_trailer_mode: ext_proc::processing_mode::HeaderSendMode::Skip as i32,
                }),
                message_timeout: Some(millis_duration(5_000)),
                stat_prefix: "flowplane_ai".into(),
                mutation_rules: Some(ai_header_mutation_rules()),
                observability_mode: false,
                disable_immediate_response: false,
                route_cache_action: ext_proc::external_processor::RouteCacheAction::Default as i32,
                ..Default::default()
            },
        ))),
        is_optional: false,
        ..Default::default()
    }
}

fn ext_proc_filter(capture: &LearningCaptureInjection) -> hcm::HttpFilter {
    hcm::HttpFilter {
        name: format!("{LEARNING_EXT_PROC_FILTER_PREFIX}{}", capture.session_id),
        config_type: Some(hcm::http_filter::ConfigType::TypedConfig(any(
            "type.googleapis.com/envoy.extensions.filters.http.ext_proc.v3.ExternalProcessor",
            &ext_proc::ExternalProcessor {
                grpc_service: Some(grpc_service(LEARNING_EXT_PROC_CLUSTER, capture)),
                failure_mode_allow: true,
                processing_mode: Some(ext_proc::ProcessingMode {
                    request_header_mode: ext_proc::processing_mode::HeaderSendMode::Send as i32,
                    response_header_mode: ext_proc::processing_mode::HeaderSendMode::Send as i32,
                    request_body_mode: ext_proc::processing_mode::BodySendMode::BufferedPartial
                        as i32,
                    response_body_mode: ext_proc::processing_mode::BodySendMode::BufferedPartial
                        as i32,
                    request_trailer_mode: ext_proc::processing_mode::HeaderSendMode::Skip as i32,
                    response_trailer_mode: ext_proc::processing_mode::HeaderSendMode::Skip as i32,
                }),
                message_timeout: Some(millis_duration(5_000)),
                stat_prefix: format!("flowplane_learning_{}", capture.session_id.simple()),
                observability_mode: false,
                disable_immediate_response: true,
                route_cache_action: ext_proc::external_processor::RouteCacheAction::Retain as i32,
                ..Default::default()
            },
        ))),
        is_optional: true,
        ..Default::default()
    }
}

fn access_logs_to_proto(
    logs: &[fp_domain::gateway::listener::AccessLogConfig],
) -> Vec<accesslog::AccessLog> {
    logs.iter()
        .map(|log| {
            let access_log_format = log.text_format.as_ref().map(|format| {
                file_accesslog::file_access_log::AccessLogFormat::LogFormat(
                    core::SubstitutionFormatString {
                        format: Some(core::substitution_format_string::Format::TextFormatSource(
                            core::DataSource {
                                specifier: Some(core::data_source::Specifier::InlineString(
                                    format.clone(),
                                )),
                                ..Default::default()
                            },
                        )),
                        ..Default::default()
                    },
                )
            });
            let file = file_accesslog::FileAccessLog {
                path: log.path.clone(),
                access_log_format,
            };
            accesslog::AccessLog {
                name: "envoy.access_loggers.file".to_string(),
                filter: None,
                config_type: Some(accesslog::access_log::ConfigType::TypedConfig(any(
                    "type.googleapis.com/envoy.extensions.access_loggers.file.v3.FileAccessLog",
                    &file,
                ))),
            }
        })
        .collect()
}

fn listener_codec_type(spec: &ListenerSpec) -> hcm::http_connection_manager::CodecType {
    match spec.protocol {
        ListenerProtocol::Http2 => hcm::http_connection_manager::CodecType::Http2,
        ListenerProtocol::Https => hcm::http_connection_manager::CodecType::Auto,
        ListenerProtocol::Http if spec.tls_context.is_some() => {
            hcm::http_connection_manager::CodecType::Auto
        }
        ListenerProtocol::Http => hcm::http_connection_manager::CodecType::Http1,
    }
}

fn downstream_tls_transport_socket(
    config: &ListenerTlsConfig,
) -> DomainResult<core::TransportSocket> {
    let mut common = tls::CommonTlsContext::default();
    if let Some(secret_name) = &config.tls_certificate_sds_secret_name {
        common
            .tls_certificate_sds_secret_configs
            .push(sds_secret_config(secret_name));
    } else if let (Some(cert), Some(key)) = (&config.cert_chain_file, &config.private_key_file) {
        common.tls_certificates.push(tls::TlsCertificate {
            certificate_chain: Some(filename(cert.clone())),
            private_key: Some(filename(key.clone())),
            ..Default::default()
        });
    } else {
        return Err(DomainError::validation(
            "TLS context requires cert_chain_file/private_key_file or tls_certificate_sds_secret_name",
        ));
    }

    common.validation_context_type =
        if let Some(secret_name) = &config.validation_context_sds_secret_name {
            Some(
                tls::common_tls_context::ValidationContextType::ValidationContextSdsSecretConfig(
                    sds_secret_config(secret_name),
                ),
            )
        } else {
            config.ca_cert_file.as_ref().map(|path| {
                tls::common_tls_context::ValidationContextType::ValidationContext(
                    tls::CertificateValidationContext {
                        trusted_ca: Some(filename(path.clone())),
                        ..Default::default()
                    },
                )
            })
        };

    Ok(core::TransportSocket {
        name: "envoy.transport_sockets.tls".to_string(),
        config_type: Some(core::transport_socket::ConfigType::TypedConfig(any(
            "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.DownstreamTlsContext",
            &tls::DownstreamTlsContext {
                common_tls_context: Some(common),
                require_client_certificate: config
                    .require_client_certificate
                    .then(|| bool_value(true)),
                ..Default::default()
            },
        ))),
    })
}

fn sds_secret_config(name: &str) -> tls::SdsSecretConfig {
    tls::SdsSecretConfig {
        name: name.to_string(),
        sds_config: Some(ads_config_source()),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use fp_domain::gateway::cluster::{
        CircuitBreakerThresholds, CircuitBreakers, Endpoint, HealthCheck, HttpHealthCheck,
        HttpHealthCheckMethod, MaglevPolicy, OutlierDetection, UpstreamProtocol, UpstreamTlsConfig,
    };
    use fp_domain::gateway::route_config::{
        DirectResponseAction, HeaderMatch, HeaderValueMatch, QueryParameterMatch, QueryValueMatch,
        RateLimitAction, RateLimitDefinition, RedirectAction, RedirectResponseCode, RetryPolicy,
        RouteAction, RouteRule, VirtualHost, WeightedClusterTarget,
    };

    fn route_action(cluster: &str) -> RouteAction {
        RouteAction {
            cluster: Some(cluster.into()),
            weighted_clusters: None,
            redirect: None,
            direct_response: None,
            prefix_rewrite: None,
            template_rewrite: None,
            timeout_secs: 15,
            retry_policy: None,
            rate_limits: Vec::new(),
        }
    }

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
            least_request: None,
            ring_hash: None,
            maglev: None,
            dns_lookup_family: None,
            connect_timeout_secs: 7,
            use_tls: true,
            upstream_tls: None,
            protocol: None,
            health_checks: None,
            circuit_breakers: None,
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
    fn cluster_translation_carries_expanded_cluster_fields() {
        let spec = ClusterSpec {
            endpoints: vec![Endpoint {
                host: "api.example.com".into(),
                port: 443,
                weight: None,
            }],
            lb_policy: LbPolicy::Maglev,
            least_request: None,
            ring_hash: None,
            maglev: Some(MaglevPolicy {
                table_size: Some(65_537),
            }),
            dns_lookup_family: Some(DnsLookupFamily::V4Only),
            connect_timeout_secs: 5,
            use_tls: false,
            upstream_tls: Some(UpstreamTlsConfig {
                sni: Some("api.example.com".into()),
                validation_context_sds_secret_name: Some("upstream-ca".into()),
                auto_sni_san_validation: true,
            }),
            protocol: Some(UpstreamProtocol::Grpc),
            health_checks: Some(vec![HealthCheck::Http(HttpHealthCheck {
                path: "/healthz".into(),
                host: Some("api.example.com".into()),
                method: Some(HttpHealthCheckMethod::Head),
                expected_statuses: vec![200, 204],
                timeout_seconds: 1,
                interval_seconds: 10,
                healthy_threshold: 2,
                unhealthy_threshold: 3,
            })]),
            circuit_breakers: Some(CircuitBreakers {
                default: Some(CircuitBreakerThresholds {
                    max_connections: 100,
                    max_pending_requests: 200,
                    max_requests: 300,
                    max_retries: 3,
                }),
                high: None,
            }),
            outlier_detection: Some(OutlierDetection {
                consecutive_5xx: 5,
                interval_seconds: 10,
                base_ejection_seconds: 30,
                max_ejection_percent: 50,
                min_hosts: Some(3),
            }),
        };
        let proto = cluster_to_proto("api", &spec).expect("translate");
        assert_eq!(proto.lb_policy, exc::cluster::LbPolicy::Maglev as i32);
        assert!(matches!(
            proto.lb_config,
            Some(exc::cluster::LbConfig::MaglevLbConfig(_))
        ));
        assert_eq!(
            proto.dns_lookup_family,
            exc::cluster::DnsLookupFamily::V4Only as i32
        );
        assert!(proto.transport_socket.is_some(), "upstream_tls enables TLS");
        assert!(proto
            .typed_extension_protocol_options
            .contains_key("envoy.extensions.upstreams.http.v3.HttpProtocolOptions"));
        assert_eq!(proto.health_checks.len(), 1);
        assert_eq!(
            proto.health_checks[0]
                .healthy_threshold
                .as_ref()
                .expect("healthy threshold")
                .value,
            2
        );
        assert_eq!(
            proto
                .circuit_breakers
                .as_ref()
                .expect("circuit breakers")
                .thresholds
                .len(),
            1
        );
        assert_eq!(
            proto
                .outlier_detection
                .as_ref()
                .expect("outlier")
                .success_rate_minimum_hosts
                .as_ref()
                .expect("min hosts")
                .value,
            3
        );
    }

    #[test]
    fn ai_cluster_translation_adds_upstream_ext_proc_without_secret() {
        let ai = AiUpstreamProcessorMetadata {
            team_id: uuid::Uuid::now_v7(),
            route_config_id: uuid::Uuid::now_v7(),
            provider_id: uuid::Uuid::now_v7(),
            backend_position: 7,
        };
        let cluster =
            cluster_to_proto_with_ai("ai-chat-b1", &cluster_spec(), Some(&ai)).expect("cluster");
        let options_any = cluster
            .typed_extension_protocol_options
            .get("envoy.extensions.upstreams.http.v3.HttpProtocolOptions")
            .expect("http protocol options");
        let options = upstream_http::HttpProtocolOptions::decode(options_any.value.as_slice())
            .expect("decode options");

        assert!(
            matches!(
                options.upstream_protocol_options,
                Some(upstream_http::http_protocol_options::UpstreamProtocolOptions::ExplicitHttpConfig(_))
            ),
            "AI upstream filters require explicit upstream protocol options"
        );
        assert_eq!(options.http_filters.len(), 2);
        assert_eq!(
            options.http_filters[0].name,
            format!("{AI_EXT_PROC_FILTER_NAME}.upstream")
        );
        let ext_any = match options.http_filters[0]
            .config_type
            .as_ref()
            .expect("typed ext proc")
        {
            hcm::http_filter::ConfigType::TypedConfig(any) => any,
            _ => panic!("expected typed ext proc"),
        };
        let ext = ext_proc::ExternalProcessor::decode(ext_any.value.as_slice()).expect("ext proc");
        let metadata = ext
            .grpc_service
            .expect("grpc")
            .initial_metadata
            .into_iter()
            .map(|header| (header.key, header.value))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(metadata["x-flowplane-ai-upstream-processor"], "true");
        assert_eq!(metadata["x-flowplane-team-id"], ai.team_id.to_string());
        assert_eq!(
            metadata["x-flowplane-route-config-id"],
            ai.route_config_id.to_string()
        );
        assert_eq!(
            metadata["x-flowplane-ai-provider-id"],
            ai.provider_id.to_string()
        );
        assert_eq!(metadata["x-flowplane-ai-backend-position"], "7");
        let mode = ext.processing_mode.expect("mode");
        assert_eq!(
            mode.request_body_mode,
            ext_proc::processing_mode::BodySendMode::Buffered as i32
        );
        assert_eq!(
            mode.response_body_mode,
            ext_proc::processing_mode::BodySendMode::BufferedPartial as i32
        );
        assert!(
            !options_any
                .value
                .windows(b"Bearer".len())
                .any(|w| w == b"Bearer"),
            "credential values must not be serialized into xDS"
        );
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
                        headers: Vec::new(),
                        query_parameters: Vec::new(),
                        action: route_action("c1"),
                        filter_overrides: Vec::new(),
                    },
                    RouteRule {
                        name: "prefixed".into(),
                        matcher: PathMatch::Prefix {
                            prefix: "/api".into(),
                        },
                        headers: Vec::new(),
                        query_parameters: Vec::new(),
                        action: RouteAction {
                            cluster: Some("c2".into()),
                            weighted_clusters: None,
                            redirect: None,
                            direct_response: None,
                            prefix_rewrite: Some("/v2".into()),
                            template_rewrite: None,
                            timeout_secs: 30,
                            retry_policy: None,
                            rate_limits: Vec::new(),
                        },
                        filter_overrides: Vec::new(),
                    },
                    RouteRule {
                        name: "templated".into(),
                        matcher: PathMatch::Template {
                            template: "/users/{id}".into(),
                        },
                        headers: Vec::new(),
                        query_parameters: Vec::new(),
                        action: RouteAction {
                            cluster: Some("c3".into()),
                            weighted_clusters: None,
                            redirect: None,
                            direct_response: None,
                            prefix_rewrite: None,
                            template_rewrite: Some("/{id}".into()),
                            timeout_secs: 15,
                            retry_policy: None,
                            rate_limits: Vec::new(),
                        },
                        filter_overrides: Vec::new(),
                    },
                ],
                rate_limits: Vec::new(),
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
        let action = match routes[2].action.as_ref().expect("action") {
            rt::route::Action::Route(action) => action,
            _ => panic!("expected route action"),
        };
        assert!(
            action.path_rewrite_policy.is_some(),
            "template_rewrite emits URI template rewrite policy"
        );
    }

    #[test]
    fn route_config_translates_direct_response_action() {
        let spec = RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "no-backend".into(),
                    matcher: PathMatch::Exact {
                        path: "/chat".into(),
                    },
                    headers: Vec::new(),
                    query_parameters: Vec::new(),
                    action: RouteAction {
                        cluster: None,
                        weighted_clusters: None,
                        redirect: None,
                        direct_response: Some(DirectResponseAction {
                            status: 400,
                            body: Some("no backend".into()),
                        }),
                        prefix_rewrite: None,
                        template_rewrite: None,
                        timeout_secs: 15,
                        retry_policy: None,
                        rate_limits: Vec::new(),
                    },
                    filter_overrides: Vec::new(),
                }],
                rate_limits: Vec::new(),
                filter_overrides: Vec::new(),
            }],
        };

        let proto = route_config_to_proto("ai", &spec).expect("translate");
        let action = proto.virtual_hosts[0].routes[0]
            .action
            .as_ref()
            .expect("action");
        let rt::route::Action::DirectResponse(direct) = action else {
            panic!("expected direct response");
        };
        assert_eq!(direct.status, 400);
        let body = direct.body.as_ref().expect("body");
        assert_eq!(
            body.specifier,
            Some(core::data_source::Specifier::InlineString(
                "no backend".into()
            ))
        );
    }

    #[test]
    fn route_config_translates_advanced_route_fields() {
        let spec = RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![
                    RouteRule {
                        name: "split".into(),
                        matcher: PathMatch::Regex {
                            pattern: "^/v[0-9]+/items$".into(),
                        },
                        headers: vec![HeaderMatch {
                            name: "x-api-version".into(),
                            invert_match: false,
                            matcher: HeaderValueMatch::Exact { value: "2".into() },
                        }],
                        query_parameters: vec![QueryParameterMatch {
                            name: "preview".into(),
                            matcher: QueryValueMatch::Present { value: true },
                        }],
                        action: RouteAction {
                            cluster: None,
                            weighted_clusters: Some(vec![
                                WeightedClusterTarget {
                                    cluster: "primary".into(),
                                    weight: 80,
                                },
                                WeightedClusterTarget {
                                    cluster: "canary".into(),
                                    weight: 20,
                                },
                            ]),
                            redirect: None,
                            direct_response: None,
                            prefix_rewrite: None,
                            template_rewrite: None,
                            timeout_secs: 10,
                            retry_policy: Some(RetryPolicy {
                                retry_on: "5xx,connect-failure".into(),
                                num_retries: Some(2),
                                per_try_timeout_secs: Some(3),
                                retriable_status_codes: vec![502, 503],
                            }),
                            rate_limits: vec![RateLimitDefinition {
                                stage: Some(1),
                                disable_key: Some("rl.disable.preview".into()),
                                actions: vec![
                                    RateLimitAction::RequestHeaders {
                                        header_name: "x-user".into(),
                                        descriptor_key: "user".into(),
                                        skip_if_absent: true,
                                    },
                                    RateLimitAction::GenericKey {
                                        descriptor_value: "items".into(),
                                        descriptor_key: Some("route".into()),
                                    },
                                ],
                            }],
                        },
                        filter_overrides: Vec::new(),
                    },
                    RouteRule {
                        name: "redirect".into(),
                        matcher: PathMatch::Prefix {
                            prefix: "/old".into(),
                        },
                        headers: Vec::new(),
                        query_parameters: Vec::new(),
                        action: RouteAction {
                            cluster: None,
                            weighted_clusters: None,
                            redirect: Some(RedirectAction {
                                host_redirect: Some("new.example.com".into()),
                                scheme_redirect: None,
                                https_redirect: Some(true),
                                path_redirect: None,
                                prefix_rewrite: Some("/new".into()),
                                response_code: Some(RedirectResponseCode::PermanentRedirect),
                                strip_query: true,
                            }),
                            direct_response: None,
                            prefix_rewrite: None,
                            template_rewrite: None,
                            timeout_secs: 15,
                            retry_policy: None,
                            rate_limits: Vec::new(),
                        },
                        filter_overrides: Vec::new(),
                    },
                ],
                rate_limits: vec![RateLimitDefinition {
                    stage: None,
                    disable_key: None,
                    actions: vec![RateLimitAction::GenericKey {
                        descriptor_value: "default-vhost".into(),
                        descriptor_key: None,
                    }],
                }],
                filter_overrides: Vec::new(),
            }],
        };
        let proto = route_config_to_proto("advanced", &spec).expect("translate");
        assert_eq!(proto.virtual_hosts[0].rate_limits.len(), 1);
        let routes = &proto.virtual_hosts[0].routes;
        let split_match = routes[0].r#match.as_ref().expect("split match");
        assert!(matches!(
            split_match.path_specifier,
            Some(rt::route_match::PathSpecifier::SafeRegex(_))
        ));
        assert_eq!(split_match.headers.len(), 1);
        assert_eq!(split_match.query_parameters.len(), 1);

        let split_action = match routes[0].action.as_ref().expect("split action") {
            rt::route::Action::Route(action) => action,
            _ => panic!("expected route action"),
        };
        match split_action
            .cluster_specifier
            .as_ref()
            .expect("cluster specifier")
        {
            rt::route_action::ClusterSpecifier::WeightedClusters(weighted) => {
                assert_eq!(weighted.clusters.len(), 2);
                assert_eq!(weighted.clusters[0].name, "primary");
            }
            other => panic!("unexpected cluster specifier: {other:?}"),
        }
        let retry = split_action.retry_policy.as_ref().expect("retry");
        assert_eq!(retry.retry_on, "5xx,connect-failure");
        assert_eq!(retry.num_retries.as_ref().expect("retries").value, 2);
        assert_eq!(retry.retriable_status_codes, vec![502, 503]);
        assert_eq!(split_action.rate_limits.len(), 1);
        assert_eq!(
            split_action.rate_limits[0]
                .stage
                .as_ref()
                .expect("stage")
                .value,
            1
        );
        assert_eq!(split_action.rate_limits[0].actions.len(), 2);

        let redirect = match routes[1].action.as_ref().expect("redirect action") {
            rt::route::Action::Redirect(redirect) => redirect,
            _ => panic!("expected redirect action"),
        };
        assert_eq!(redirect.host_redirect, "new.example.com");
        assert_eq!(
            redirect.response_code,
            rt::redirect_action::RedirectResponseCode::PermanentRedirect as i32
        );
        assert!(redirect.strip_query);
    }

    #[test]
    fn listener_requires_a_bound_route_config() {
        let unbound = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10001,
            protocol: ListenerProtocol::Http,
            route_config: None,
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: None,
        };
        assert!(listener_to_proto("edge", &unbound).is_err());

        let bound = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10001,
            protocol: ListenerProtocol::Http,
            route_config: Some("orders".into()),
            http_filters: Vec::new(),
            access_logs: vec![fp_domain::gateway::listener::AccessLogConfig {
                path: "/var/log/envoy/access.log".into(),
                text_format: Some("%REQ(:METHOD)% %RESPONSE_CODE%\n".into()),
            }],
            tls_context: None,
        };
        let proto = listener_to_proto("edge", &bound).expect("translate");
        assert_eq!(proto.filter_chains.len(), 1);
        let manager = match &proto.filter_chains[0].filters[0].config_type {
            Some(lst::filter::ConfigType::TypedConfig(a)) => {
                hcm::HttpConnectionManager::decode(a.value.as_slice()).expect("hcm")
            }
            _ => panic!("expected typed HCM"),
        };
        assert_eq!(
            manager.codec_type,
            hcm::http_connection_manager::CodecType::Http1 as i32
        );
        assert_eq!(manager.generate_request_id, Some(bool_value(true)));
        assert!(manager.always_set_request_id_in_response);
        assert_eq!(manager.access_log.len(), 1);
        let access_log = &manager.access_log[0];
        assert_eq!(access_log.name, "envoy.access_loggers.file");
        let file = match access_log.config_type.as_ref().expect("access log config") {
            accesslog::access_log::ConfigType::TypedConfig(any) => {
                assert!(any
                    .type_url
                    .ends_with("access_loggers.file.v3.FileAccessLog"));
                file_accesslog::FileAccessLog::decode(any.value.as_slice()).expect("file log")
            }
        };
        assert_eq!(file.path, "/var/log/envoy/access.log");

        let mut http2 = bound.clone();
        http2.protocol = ListenerProtocol::Http2;
        let proto = listener_to_proto("edge-h2", &http2).expect("translate h2");
        let manager = match &proto.filter_chains[0].filters[0].config_type {
            Some(lst::filter::ConfigType::TypedConfig(a)) => {
                hcm::HttpConnectionManager::decode(a.value.as_slice()).expect("hcm")
            }
            _ => panic!("expected typed HCM"),
        };
        assert_eq!(
            manager.codec_type,
            hcm::http_connection_manager::CodecType::Http2 as i32
        );

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
    fn listener_tls_context_uses_sds_over_ads() {
        let spec = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10443,
            protocol: ListenerProtocol::Https,
            route_config: Some("orders".into()),
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: Some(ListenerTlsConfig {
                cert_chain_file: None,
                private_key_file: None,
                ca_cert_file: None,
                require_client_certificate: false,
                tls_certificate_sds_secret_name: Some("edge-cert".into()),
                validation_context_sds_secret_name: Some("edge-ca".into()),
            }),
        };
        let proto = listener_to_proto("edge-tls", &spec).expect("translate");
        let manager = match &proto.filter_chains[0].filters[0].config_type {
            Some(lst::filter::ConfigType::TypedConfig(a)) => {
                hcm::HttpConnectionManager::decode(a.value.as_slice()).expect("hcm")
            }
            _ => panic!("expected typed HCM"),
        };
        assert_eq!(
            manager.codec_type,
            hcm::http_connection_manager::CodecType::Auto as i32
        );
        let socket = proto.filter_chains[0]
            .transport_socket
            .as_ref()
            .expect("transport socket");
        assert_eq!(socket.name, "envoy.transport_sockets.tls");
        let Some(core::transport_socket::ConfigType::TypedConfig(any)) = &socket.config_type else {
            panic!("expected typed downstream tls context");
        };
        assert_eq!(
            any.type_url,
            "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.DownstreamTlsContext"
        );
        let tls_context =
            tls::DownstreamTlsContext::decode(any.value.as_slice()).expect("downstream tls");
        let common = tls_context.common_tls_context.expect("common");
        assert_eq!(common.tls_certificate_sds_secret_configs.len(), 1);
        assert_eq!(
            common.tls_certificate_sds_secret_configs[0].name,
            "edge-cert"
        );
        assert!(common.tls_certificate_sds_secret_configs[0]
            .sds_config
            .as_ref()
            .and_then(|c| c.config_source_specifier.as_ref())
            .is_some());
        match common.validation_context_type.expect("validation context") {
            tls::common_tls_context::ValidationContextType::ValidationContextSdsSecretConfig(
                config,
            ) => assert_eq!(config.name, "edge-ca"),
            other => panic!("unexpected validation context: {other:?}"),
        }
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
                HttpFilterEntry {
                    filter: HttpFilterSpec::GlobalRateLimit(GlobalRateLimitConfig {
                        domain: "flowplane".into(),
                        service_cluster: "flowplane-rls".into(),
                        timeout_ms: 50,
                        failure_mode_deny: true,
                        stage: 1,
                        request_type: RateLimitRequestType::External,
                        stat_prefix: Some("edge_rls".into()),
                        enable_x_ratelimit_headers: true,
                        disable_x_envoy_ratelimited_header: true,
                        rate_limited_status: Some(429),
                        status_on_error: Some(503),
                    }),
                    disabled: false,
                },
            ];
            let spec = ListenerSpec {
                address: "0.0.0.0".into(),
                port: 10001,
                protocol: ListenerProtocol::Http,
                route_config: Some("orders".into()),
                http_filters: chain,
                access_logs: Vec::new(),
                tls_context: None,
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
                    "envoy.filters.http.ratelimit",
                    "envoy.filters.http.router"
                ],
                "declared order, router appended last"
            );
            assert!(manager.http_filters[1].disabled, "disabled flag carried");
            let rls = match manager.http_filters[2]
                .config_type
                .as_ref()
                .expect("rls typed config")
            {
                hcm::http_filter::ConfigType::TypedConfig(any) => {
                    assert!(any.type_url.ends_with("ratelimit.v3.RateLimit"));
                    rate_limit_filter::RateLimit::decode(any.value.as_slice()).expect("rls")
                }
                other => panic!("unexpected rls filter config: {other:?}"),
            };
            assert_eq!(rls.domain, "flowplane");
            assert_eq!(rls.stage, 1);
            assert_eq!(rls.request_type, "external");
            assert!(rls.failure_mode_deny);
            assert_eq!(rls.stat_prefix, "edge_rls");
            assert_eq!(
                rls.enable_x_ratelimit_headers,
                rate_limit_filter::rate_limit::XRateLimitHeadersRfcVersion::DraftVersion03 as i32
            );
            assert!(rls.disable_x_envoy_ratelimited_header);
            assert_eq!(rls.rate_limited_status.expect("limited status").code, 429);
            assert_eq!(rls.status_on_error.expect("error status").code, 503);
            let service = rls.rate_limit_service.expect("rate limit service");
            assert_eq!(service.transport_api_version, core::ApiVersion::V3 as i32);
            let grpc = service.grpc_service.expect("grpc service");
            assert_eq!(grpc.timeout.expect("grpc timeout").seconds, 0);
            match grpc.target_specifier.expect("target") {
                core::grpc_service::TargetSpecifier::EnvoyGrpc(target) => {
                    assert_eq!(target.cluster_name, "flowplane-rls");
                }
                other => panic!("unexpected rate-limit grpc target: {other:?}"),
            }

            // cors in the chain is the empty marker; the policy travels per-scope.
            let cors_spec = ListenerSpec {
                address: "0.0.0.0".into(),
                port: 10002,
                protocol: ListenerProtocol::Http,
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
                access_logs: Vec::new(),
                tls_context: None,
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
        use fp_domain::gateway::route_config::{RouteRule, VirtualHost};
        let spec = RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "quiet".into(),
                    matcher: PathMatch::Prefix {
                        prefix: "/quiet".into(),
                    },
                    headers: Vec::new(),
                    query_parameters: Vec::new(),
                    action: route_action("c"),
                    filter_overrides: vec![FilterOverride::Disable {
                        filter_type: "local_rate_limit".into(),
                    }],
                }],
                rate_limits: Vec::new(),
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
    fn route_config_deterministic_encoding_has_golden_bytes_for_multi_entry_maps() {
        use fp_domain::gateway::filters::*;
        use fp_domain::gateway::route_config::{RouteRule, VirtualHost};
        use sha2::{Digest, Sha256};

        let spec = RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "admin".into(),
                    matcher: PathMatch::Prefix {
                        prefix: "/admin".into(),
                    },
                    headers: Vec::new(),
                    query_parameters: Vec::new(),
                    action: route_action("backend"),
                    filter_overrides: vec![
                        FilterOverride::Disable {
                            filter_type: "jwt_auth".into(),
                        },
                        FilterOverride::LocalRateLimit(LocalRateLimitConfig {
                            stat_prefix: "admin".into(),
                            token_bucket: TokenBucket {
                                max_tokens: 20,
                                tokens_per_fill: Some(10),
                                fill_interval_ms: 1000,
                            },
                            status_code: Some(429),
                        }),
                    ],
                }],
                rate_limits: Vec::new(),
                filter_overrides: vec![
                    FilterOverride::Cors(CorsConfig {
                        allow_origin: vec![OriginMatcher::Suffix {
                            value: ".example".into(),
                        }],
                        allow_methods: vec!["GET".into(), "POST".into()],
                        allow_headers: vec![],
                        expose_headers: vec![],
                        max_age_seconds: Some(600),
                        allow_credentials: true,
                    }),
                    FilterOverride::Disable {
                        filter_type: "rbac".into(),
                    },
                ],
            }],
        };
        let proto = route_config_to_proto("orders", &spec).expect("translate");
        let encoded = encode_route_config_deterministic(&proto).expect("stable encode");
        let digest = Sha256::digest(&encoded);
        let digest_hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();

        assert_eq!(encoded.len(), 538);
        assert_eq!(
            digest_hex,
            "f0a413620e23b34c8222978335e9d0c1638248950824e79b7f21b687dddee38d"
        );
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

    fn hcm_of_with_learning(
        spec: &ListenerSpec,
        captures: &[LearningCaptureInjection],
    ) -> hcm::HttpConnectionManager {
        let proto =
            listener_to_proto_with_learning("edge", spec, captures).expect("translate learning");
        match &proto.filter_chains[0].filters[0].config_type {
            Some(lst::filter::ConfigType::TypedConfig(a)) => {
                hcm::HttpConnectionManager::decode(a.value.as_slice()).expect("hcm")
            }
            _ => panic!("expected typed HCM"),
        }
    }

    fn hcm_of_named(name: &str, spec: &ListenerSpec) -> hcm::HttpConnectionManager {
        let proto = listener_to_proto(name, spec).expect("translate");
        match &proto.filter_chains[0].filters[0].config_type {
            Some(lst::filter::ConfigType::TypedConfig(a)) => {
                hcm::HttpConnectionManager::decode(a.value.as_slice()).expect("hcm")
            }
            _ => panic!("expected typed HCM"),
        }
    }

    #[test]
    fn ai_listener_injects_ai_ext_proc_before_router() {
        let spec = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10000,
            protocol: ListenerProtocol::Http,
            route_config: Some("ai-chat-routes".into()),
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: None,
        };
        let ai = AiProcessorMetadata {
            team_id: uuid::Uuid::now_v7(),
            listener_id: uuid::Uuid::now_v7(),
            route_config_id: uuid::Uuid::now_v7(),
        };

        let proto =
            listener_to_proto_with_learning_and_ai("ai-chat-listener", &spec, &[], Some(&ai))
                .expect("translate");
        let manager = match proto.filter_chains[0].filters[0]
            .config_type
            .as_ref()
            .expect("typed HCM")
        {
            lst::filter::ConfigType::TypedConfig(any) => {
                hcm::HttpConnectionManager::decode(any.value.as_slice()).expect("hcm")
            }
            _ => panic!("expected typed HCM"),
        };
        let names = manager
            .http_filters
            .iter()
            .map(|filter| filter.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![AI_EXT_PROC_FILTER_NAME, "envoy.filters.http.router"]
        );
        let ext_any = match &manager.http_filters[0].config_type {
            Some(hcm::http_filter::ConfigType::TypedConfig(any)) => any,
            _ => panic!("expected ext proc typed config"),
        };
        let ext = ext_proc::ExternalProcessor::decode(ext_any.value.as_slice()).expect("ext proc");
        assert!(!ext.failure_mode_allow);
        let mode = ext.processing_mode.expect("mode");
        assert_eq!(
            mode.request_body_mode,
            ext_proc::processing_mode::BodySendMode::Buffered as i32
        );
        assert_eq!(
            mode.response_header_mode,
            ext_proc::processing_mode::HeaderSendMode::Send as i32
        );
        let metadata = ext
            .grpc_service
            .expect("grpc")
            .initial_metadata
            .into_iter()
            .map(|header| (header.key, header.value))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(metadata["x-flowplane-ai-processor"], "true");
    }

    #[test]
    fn ai_prefixed_user_listener_does_not_inject_ai_ext_proc() {
        let spec = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10000,
            protocol: ListenerProtocol::Http,
            route_config: Some("routes".into()),
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: None,
        };

        let manager = hcm_of_named("ai-user-listener", &spec);
        let names = manager
            .http_filters
            .iter()
            .map(|filter| filter.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["envoy.filters.http.router"]);
    }

    #[test]
    fn ai_listener_can_include_processor_identity_metadata() {
        let spec = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10000,
            protocol: ListenerProtocol::Http,
            route_config: Some("ai-chat-routes".into()),
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: None,
        };
        let ai = AiProcessorMetadata {
            team_id: uuid::Uuid::now_v7(),
            listener_id: uuid::Uuid::now_v7(),
            route_config_id: uuid::Uuid::now_v7(),
        };

        let proto =
            listener_to_proto_with_learning_and_ai("ai-chat-listener", &spec, &[], Some(&ai))
                .expect("translate");
        let manager = match proto.filter_chains[0].filters[0]
            .config_type
            .as_ref()
            .expect("typed HCM")
        {
            lst::filter::ConfigType::TypedConfig(any) => {
                hcm::HttpConnectionManager::decode(any.value.as_slice()).expect("hcm")
            }
            _ => panic!("expected typed HCM"),
        };
        let ext_any = match &manager.http_filters[0].config_type {
            Some(hcm::http_filter::ConfigType::TypedConfig(any)) => any,
            _ => panic!("expected ext proc typed config"),
        };
        let ext = ext_proc::ExternalProcessor::decode(ext_any.value.as_slice()).expect("ext proc");
        let metadata = ext
            .grpc_service
            .expect("grpc")
            .initial_metadata
            .into_iter()
            .map(|header| (header.key, header.value))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(metadata["x-flowplane-team-id"], ai.team_id.to_string());
        assert_eq!(
            metadata["x-flowplane-listener-id"],
            ai.listener_id.to_string()
        );
        assert_eq!(
            metadata["x-flowplane-route-config-id"],
            ai.route_config_id.to_string()
        );
    }

    #[test]
    fn learning_capture_injects_als_and_ext_proc_before_router() {
        let session_id = uuid::Uuid::now_v7();
        let team_id = uuid::Uuid::now_v7();
        let api_id = uuid::Uuid::now_v7();
        let route_config_id = uuid::Uuid::now_v7();
        let listener_id = uuid::Uuid::now_v7();
        let spec = ListenerSpec {
            address: "0.0.0.0".into(),
            port: 10000,
            protocol: ListenerProtocol::Http,
            route_config: Some("orders".into()),
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: None,
        };
        let capture = LearningCaptureInjection {
            session_id,
            team_id,
            api_definition_id: Some(api_id),
            route_config_id,
            listener_id: Some(listener_id),
            virtual_host: Some("default".into()),
            route: Some("all".into()),
            discovery: None,
        };
        let manager = hcm_of_with_learning(&spec, &[capture]);
        let names = manager
            .http_filters
            .iter()
            .map(|filter| filter.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                format!("{LEARNING_EXT_PROC_FILTER_PREFIX}{session_id}"),
                "envoy.filters.http.router".to_string()
            ]
        );
        assert_eq!(manager.access_log.len(), 1);
        let accesslog::access_log::ConfigType::TypedConfig(log_any) = manager.access_log[0]
            .config_type
            .as_ref()
            .expect("als config");
        assert!(log_any.type_url.ends_with("HttpGrpcAccessLogConfig"));
        let als =
            grpc_accesslog::HttpGrpcAccessLogConfig::decode(log_any.value.as_slice()).expect("als");
        assert_eq!(
            als.additional_request_headers_to_log,
            vec![
                "content-type",
                "content-length",
                "accept",
                "user-agent",
                "x-request-id",
                "x-envoy-original-path"
            ]
        );
        for sensitive in [
            "authorization",
            "proxy-authorization",
            "x-api-key",
            "x-auth-token",
        ] {
            assert!(
                !als.additional_request_headers_to_log
                    .iter()
                    .any(|header| header == sensitive),
                "learning ALS must not export credential header values"
            );
        }
        let common = als.common_config.expect("common");
        assert_eq!(
            common.log_name,
            format!("flowplane_learning_session_{session_id}")
        );
        let grpc = common.grpc_service.expect("grpc");
        let metadata = grpc
            .initial_metadata
            .iter()
            .map(|header| (header.key.as_str(), header.value.as_str()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(metadata["x-flowplane-team-id"], team_id.to_string());
        assert_eq!(
            metadata["x-flowplane-capture-session-id"],
            session_id.to_string()
        );
        assert_eq!(
            metadata["x-flowplane-api-definition-id"],
            api_id.to_string()
        );
        match grpc.target_specifier.expect("als target") {
            core::grpc_service::TargetSpecifier::EnvoyGrpc(target) => {
                assert_eq!(target.cluster_name, LEARNING_ALS_CLUSTER);
            }
            other => panic!("unexpected ALS target: {other:?}"),
        }

        let ext_any = match manager.http_filters[0]
            .config_type
            .as_ref()
            .expect("ext proc config")
        {
            hcm::http_filter::ConfigType::TypedConfig(any) => any,
            other => panic!("unexpected ExtProc config: {other:?}"),
        };
        assert!(ext_any.type_url.ends_with("ExternalProcessor"));
        let ext = ext_proc::ExternalProcessor::decode(ext_any.value.as_slice()).expect("ext proc");
        assert!(ext.failure_mode_allow);
        assert!(!ext.observability_mode);
        assert_eq!(
            ext.processing_mode.expect("mode").request_body_mode,
            ext_proc::processing_mode::BodySendMode::BufferedPartial as i32
        );
        match ext
            .grpc_service
            .expect("ext grpc")
            .target_specifier
            .expect("ext target")
        {
            core::grpc_service::TargetSpecifier::EnvoyGrpc(target) => {
                assert_eq!(target.cluster_name, LEARNING_EXT_PROC_CLUSTER);
            }
            other => panic!("unexpected ExtProc target: {other:?}"),
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
            protocol: ListenerProtocol::Http,
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
            access_logs: Vec::new(),
            tls_context: None,
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
        use fp_domain::gateway::route_config::{RouteRule, VirtualHost};
        let spec = RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "admin".into(),
                    matcher: PathMatch::Prefix {
                        prefix: "/admin".into(),
                    },
                    headers: Vec::new(),
                    query_parameters: Vec::new(),
                    action: route_action("c"),
                    filter_overrides: vec![FilterOverride::JwtAuth {
                        requirement_name: "admins-only".into(),
                    }],
                }],
                rate_limits: Vec::new(),
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

    fn route_action(cluster: &str) -> RouteAction {
        RouteAction {
            cluster: Some(cluster.into()),
            weighted_clusters: None,
            redirect: None,
            direct_response: None,
            prefix_rewrite: None,
            template_rewrite: None,
            timeout_secs: 15,
            retry_policy: None,
            rate_limits: Vec::new(),
        }
    }

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
                    headers: Vec::new(),
                    query_parameters: Vec::new(),
                    action: route_action("c"),
                    filter_overrides: Vec::new(),
                }],
                rate_limits: Vec::new(),
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
