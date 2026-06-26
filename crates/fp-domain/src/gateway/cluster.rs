//! Cluster: an upstream backend definition (spec/03 §7.2 business rules carried from v1,
//! with one deliberate change — TLS to the upstream is always explicit, never inferred
//! from port 443 (v1 smell, spec/04 §8.13).

use crate::error::{DomainError, DomainResult};
use crate::id::{ClusterId, TeamId};
use crate::identity::validate_name;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Name prefixes reserved for Flowplane-internal resources (v1 rule, kept). `rate_limit_` is
/// reserved for the built-in global rate-limit cluster (fpv2-4ht S6) — defense-in-depth, since
/// [`validate_name`] already rejects underscores, so the guard also covers any future loosening.
pub const RESERVED_NAME_PREFIXES: &[&str] = &[
    "envoy-",
    "xds-",
    "internal-",
    "system-",
    "flowplane-",
    "rate_limit_",
];

/// The single reserved name of the CP-synthesized built-in rate-limit cluster (spec/04:212,345).
/// The CP injects this cluster into CDS when `FLOWPLANE_RLS_GRPC_URL` is set (S6) and defaults
/// `GlobalRateLimitConfig.service_cluster` to it (filters.rs); S7 composes the Envoy filter
/// against the same name. ONE source of truth so the three sites never drift. It contains
/// underscores (the spec name) and is therefore exempt from the user-facing [`validate_name`]
/// slug rule wherever it appears as a CP-owned value.
pub const RESERVED_RATE_LIMIT_CLUSTER: &str = "rate_limit_cluster";

pub const MAX_ENDPOINTS: usize = 100;
pub const MAX_AGGREGATE_CLUSTERS: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cluster {
    pub id: ClusterId,
    pub team_id: TeamId,
    pub name: String,
    pub spec: ClusterSpec,
    /// Optimistic-concurrency revision; every update bumps it (spec/10 §3.4.4).
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClusterSpec {
    pub endpoints: Vec<Endpoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aggregate_clusters: Vec<String>,
    #[serde(default)]
    pub lb_policy: LbPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub least_request: Option<LeastRequestPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring_hash: Option<RingHashPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maglev: Option<MaglevPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_lookup_family: Option<DnsLookupFamily>,
    /// Connection timeout to the upstream, seconds (1–300).
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u32,
    /// TLS to the upstream — always explicit (no port-443 inference).
    #[serde(default)]
    pub use_tls: bool,
    /// Optional upstream TLS details. Setting this also enables upstream TLS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_tls: Option<UpstreamTlsConfig>,
    /// Optional upstream protocol selection. `Http2`/`Grpc` force Envoy's HTTP/2
    /// upstream protocol options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<UpstreamProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_checks: Option<Vec<HealthCheck>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_breakers: Option<CircuitBreakers>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outlier_detection: Option<OutlierDetection>,
}

fn default_connect_timeout() -> u32 {
    5
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// Load-balancing weight (1–1000). All endpoints weighted or none (v1 rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LbPolicy {
    #[default]
    RoundRobin,
    LeastRequest,
    Random,
    RingHash,
    Maglev,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LeastRequestPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RingHashPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_ring_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_ring_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_function: Option<RingHashFunction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RingHashFunction {
    XxHash,
    MurmurHash2,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MaglevPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DnsLookupFamily {
    Auto,
    V4Only,
    V6Only,
    V4Preferred,
    All,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpstreamTlsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_context_sds_secret_name: Option<String>,
    /// Filesystem path (in the Envoy/dataplane container) to a PEM CA bundle used to verify the
    /// upstream certificate. Alternative to `validation_context_sds_secret_name`. When neither is
    /// set, the translator falls back to the default system CA bundle (verify-by-default); see
    /// `upstream_tls_context`. Ignored when `insecure_skip_verify` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert_file: Option<String>,
    #[serde(default)]
    pub auto_sni_san_validation: bool,
    /// Explicit opt-out of upstream certificate verification. Default false: TLS upstreams verify
    /// the server cert against a CA bundle. Set true only when the upstream cannot be verified and
    /// the risk is accepted — this disables peer/SAN validation (issue #125).
    #[serde(default)]
    pub insecure_skip_verify: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamProtocol {
    Http1,
    Http2,
    Grpc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HealthCheck {
    Http(HttpHealthCheck),
    Tcp(TcpHealthCheck),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpHealthCheck {
    /// HTTP path probed; must start with `/`, no `..`, ≤200 chars.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<HttpHealthCheckMethod>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_statuses: Vec<u16>,
    /// 1–60 and strictly less than `interval_seconds`.
    pub timeout_seconds: u32,
    /// 1–300.
    pub interval_seconds: u32,
    /// 1–10.
    #[serde(default = "default_threshold")]
    pub healthy_threshold: u32,
    /// 1–10.
    #[serde(default = "default_threshold")]
    pub unhealthy_threshold: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TcpHealthCheck {
    /// 1–60 and strictly less than `interval_seconds`.
    pub timeout_seconds: u32,
    /// 1–300.
    pub interval_seconds: u32,
    /// 1–10.
    #[serde(default = "default_threshold")]
    pub healthy_threshold: u32,
    /// 1–10.
    #[serde(default = "default_threshold")]
    pub unhealthy_threshold: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HttpHealthCheckMethod {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Options,
    Trace,
    Patch,
}

fn default_threshold() -> u32 {
    3
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CircuitBreakers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<CircuitBreakerThresholds>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<CircuitBreakerThresholds>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CircuitBreakerThresholds {
    /// Each 1–10000.
    pub max_connections: u32,
    pub max_pending_requests: u32,
    pub max_requests: u32,
    /// ≤10.
    #[serde(default)]
    pub max_retries: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutlierDetection {
    /// 1–1000.
    pub consecutive_5xx: u32,
    /// 1–300 seconds.
    pub interval_seconds: u32,
    /// 1–3600 seconds.
    pub base_ejection_seconds: u32,
    /// 1–100.
    pub max_ejection_percent: u32,
    /// 1–100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_hosts: Option<u32>,
}

fn range(label: &str, value: u32, min: u32, max: u32) -> DomainResult<()> {
    if value < min || value > max {
        return Err(DomainError::validation(format!(
            "{label} must be between {min} and {max}, got {value}"
        )));
    }
    Ok(())
}

pub fn validate_cluster_name(name: &str) -> DomainResult<()> {
    validate_name(name)?;
    for prefix in RESERVED_NAME_PREFIXES {
        if name.starts_with(prefix) {
            return Err(DomainError::validation(format!(
                "the \"{prefix}\" name prefix is reserved for internal resources"
            )));
        }
    }
    Ok(())
}

fn validate_host(host: &str) -> DomainResult<()> {
    if host.is_empty()
        || host.len() > 253
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        || host.contains("..")
    {
        return Err(DomainError::validation(format!(
            "\"{}\" is not a valid endpoint host",
            host.chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect::<String>()
        )));
    }
    Ok(())
}

impl ClusterSpec {
    pub fn validate(&self) -> DomainResult<()> {
        if !self.aggregate_clusters.is_empty() {
            if !self.endpoints.is_empty() {
                return Err(DomainError::validation(
                    "aggregate clusters must not define endpoints",
                ));
            }
            if self.aggregate_clusters.len() > MAX_AGGREGATE_CLUSTERS {
                return Err(DomainError::validation(format!(
                    "at most {MAX_AGGREGATE_CLUSTERS} aggregate member clusters, got {}",
                    self.aggregate_clusters.len()
                )));
            }
            for cluster in &self.aggregate_clusters {
                crate::identity::validate_name(cluster)?;
            }
            range("connect_timeout_secs", self.connect_timeout_secs, 1, 300)?;
            return Ok(());
        }
        if self.endpoints.is_empty() {
            return Err(
                DomainError::validation("a cluster needs at least one endpoint")
                    .with_hint("add endpoints: [{host, port}]"),
            );
        }
        if self.endpoints.len() > MAX_ENDPOINTS {
            return Err(DomainError::validation(format!(
                "at most {MAX_ENDPOINTS} endpoints per cluster, got {}",
                self.endpoints.len()
            )));
        }
        let weighted = self.endpoints.iter().filter(|e| e.weight.is_some()).count();
        if weighted != 0 && weighted != self.endpoints.len() {
            return Err(DomainError::validation(
                "either all endpoints carry a weight or none do",
            ));
        }
        let mut total_weight: u64 = 0;
        for endpoint in &self.endpoints {
            validate_host(&endpoint.host)?;
            if endpoint.port == 0 {
                return Err(DomainError::validation("endpoint port must be 1-65535"));
            }
            if let Some(weight) = endpoint.weight {
                range("endpoint weight", weight, 1, 1000)?;
                total_weight += u64::from(weight);
            }
        }
        if total_weight > 10_000 {
            return Err(DomainError::validation(
                "total endpoint weight must be <= 10000",
            ));
        }
        range("connect_timeout_secs", self.connect_timeout_secs, 1, 300)?;

        if self.lb_policy != LbPolicy::LeastRequest && self.least_request.is_some() {
            return Err(DomainError::validation(
                "least_request options require lb_policy = least-request",
            ));
        }
        if self.lb_policy != LbPolicy::RingHash && self.ring_hash.is_some() {
            return Err(DomainError::validation(
                "ring_hash options require lb_policy = ring-hash",
            ));
        }
        if self.lb_policy != LbPolicy::Maglev && self.maglev.is_some() {
            return Err(DomainError::validation(
                "maglev options require lb_policy = maglev",
            ));
        }

        if let Some(policy) = &self.least_request {
            if let Some(choice_count) = policy.choice_count {
                range("least_request.choice_count", choice_count, 2, 100)?;
            }
        }
        if let Some(policy) = &self.ring_hash {
            if let Some(min) = policy.minimum_ring_size {
                range_u64("ring_hash.minimum_ring_size", min, 1, 8_388_608)?;
            }
            if let Some(max) = policy.maximum_ring_size {
                range_u64("ring_hash.maximum_ring_size", max, 1, 8_388_608)?;
            }
            if let (Some(min), Some(max)) = (policy.minimum_ring_size, policy.maximum_ring_size) {
                if min > max {
                    return Err(DomainError::validation(
                        "ring_hash.minimum_ring_size must be <= maximum_ring_size",
                    ));
                }
            }
        }
        if let Some(policy) = &self.maglev {
            if let Some(table_size) = policy.table_size {
                range_u64("maglev.table_size", table_size, 1, 5_000_011)?;
            }
        }
        if let Some(tls) = &self.upstream_tls {
            if let Some(sni) = &tls.sni {
                validate_host(sni)?;
            }
            if let Some(secret) = &tls.validation_context_sds_secret_name {
                crate::identity::validate_name(secret)?;
            }
            if let Some(ca) = &tls.ca_cert_file {
                if tls.validation_context_sds_secret_name.is_some() {
                    return Err(DomainError::validation(
                        "upstream_tls validation source must be either ca_cert_file or validation_context_sds_secret_name, not both",
                    ));
                }
                if ca.trim().is_empty() {
                    return Err(DomainError::validation(
                        "upstream_tls ca_cert_file must not be empty",
                    ));
                }
                if ca.chars().any(char::is_control) {
                    return Err(DomainError::validation(
                        "upstream_tls ca_cert_file must not contain control characters",
                    ));
                }
            }
        }
        for hc in self.health_checks.iter().flatten() {
            validate_health_check(hc)?;
        }
        if let Some(cb) = &self.circuit_breakers {
            if cb.default.is_none() && cb.high.is_none() {
                return Err(DomainError::validation(
                    "circuit_breakers must set default or high thresholds",
                ));
            }
            if let Some(default) = &cb.default {
                validate_circuit_breaker("circuit_breakers.default", default)?;
            }
            if let Some(high) = &cb.high {
                validate_circuit_breaker("circuit_breakers.high", high)?;
            }
        }
        if let Some(od) = &self.outlier_detection {
            range("consecutive_5xx", od.consecutive_5xx, 1, 1000)?;
            range("outlier interval_seconds", od.interval_seconds, 1, 300)?;
            range("base_ejection_seconds", od.base_ejection_seconds, 1, 3600)?;
            range("max_ejection_percent", od.max_ejection_percent, 1, 100)?;
            if let Some(min_hosts) = od.min_hosts {
                range("min_hosts", min_hosts, 1, 100)?;
            }
        }
        Ok(())
    }
}

fn range_u64(label: &str, value: u64, min: u64, max: u64) -> DomainResult<()> {
    if value < min || value > max {
        return Err(DomainError::validation(format!(
            "{label} must be between {min} and {max}, got {value}"
        )));
    }
    Ok(())
}

fn validate_health_check(hc: &HealthCheck) -> DomainResult<()> {
    let (label, timeout, interval, healthy, unhealthy) = match hc {
        HealthCheck::Http(hc) => {
            if !hc.path.starts_with('/') || hc.path.contains("..") || hc.path.len() > 200 {
                return Err(DomainError::validation(
                    "health-check path must start with '/', contain no '..', and be <= 200 chars",
                ));
            }
            if let Some(host) = &hc.host {
                validate_host(host)?;
            }
            for status in &hc.expected_statuses {
                if !(100..600).contains(status) {
                    return Err(DomainError::validation(
                        "health-check expected_statuses must be HTTP status codes 100-599",
                    ));
                }
            }
            (
                "http health-check",
                hc.timeout_seconds,
                hc.interval_seconds,
                hc.healthy_threshold,
                hc.unhealthy_threshold,
            )
        }
        HealthCheck::Tcp(hc) => (
            "tcp health-check",
            hc.timeout_seconds,
            hc.interval_seconds,
            hc.healthy_threshold,
            hc.unhealthy_threshold,
        ),
    };
    range(&format!("{label} timeout_seconds"), timeout, 1, 60)?;
    range(&format!("{label} interval_seconds"), interval, 1, 300)?;
    if timeout >= interval {
        return Err(DomainError::validation(format!(
            "{label} timeout must be strictly less than its interval"
        )));
    }
    range(&format!("{label} healthy_threshold"), healthy, 1, 10)?;
    range(&format!("{label} unhealthy_threshold"), unhealthy, 1, 10)?;
    Ok(())
}

fn validate_circuit_breaker(label: &str, cb: &CircuitBreakerThresholds) -> DomainResult<()> {
    range(
        &format!("{label}.max_connections"),
        cb.max_connections,
        1,
        10_000,
    )?;
    range(
        &format!("{label}.max_pending_requests"),
        cb.max_pending_requests,
        1,
        10_000,
    )?;
    range(&format!("{label}.max_requests"), cb.max_requests, 1, 10_000)?;
    range(&format!("{label}.max_retries"), cb.max_retries, 0, 10)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn minimal() -> ClusterSpec {
        ClusterSpec {
            endpoints: vec![Endpoint {
                host: "10.0.0.1".into(),
                port: 8080,
                weight: None,
            }],
            aggregate_clusters: Vec::new(),
            lb_policy: LbPolicy::RoundRobin,
            least_request: None,
            ring_hash: None,
            maglev: None,
            dns_lookup_family: None,
            connect_timeout_secs: 5,
            use_tls: false,
            upstream_tls: None,
            protocol: None,
            health_checks: None,
            circuit_breakers: None,
            outlier_detection: None,
        }
    }

    #[test]
    fn minimal_spec_validates() {
        assert!(minimal().validate().is_ok());
    }

    #[test]
    fn upstream_tls_ca_cert_file_validation() {
        let with_ca = |ca: Option<&str>, secret: Option<&str>| ClusterSpec {
            upstream_tls: Some(UpstreamTlsConfig {
                sni: Some("api.example.com".into()),
                validation_context_sds_secret_name: secret.map(Into::into),
                ca_cert_file: ca.map(Into::into),
                auto_sni_san_validation: true,
                insecure_skip_verify: false,
            }),
            ..minimal()
        };
        assert!(with_ca(Some("/etc/ssl/upstream-ca.pem"), None)
            .validate()
            .is_ok());
        assert!(
            with_ca(Some(""), None).validate().is_err(),
            "empty ca_cert_file"
        );
        assert!(
            with_ca(Some("/etc/\nca.pem"), None).validate().is_err(),
            "control char"
        );
        assert!(
            with_ca(Some("/etc/ssl/ca.pem"), Some("upstream-ca"))
                .validate()
                .is_err(),
            "ca_cert_file and SDS secret are mutually exclusive"
        );
    }

    #[test]
    fn reserved_prefixes_rejected() {
        for name in [
            "envoy-edge",
            "xds-x",
            "internal-db",
            "system-a",
            "flowplane-self",
            "rate_limit_cluster",
            "rate_limit_foo",
        ] {
            assert!(
                validate_cluster_name(name).is_err(),
                "{name} must be reserved"
            );
        }
        assert!(validate_cluster_name("payments-db").is_ok());
    }

    #[test]
    fn adversarial_specs_rejected() {
        let cases: Vec<(&str, ClusterSpec)> = vec![
            (
                "no endpoints",
                ClusterSpec {
                    endpoints: vec![],
                    ..minimal()
                },
            ),
            (
                "zero port",
                ClusterSpec {
                    endpoints: vec![Endpoint {
                        host: "h".into(),
                        port: 0,
                        weight: None,
                    }],
                    ..minimal()
                },
            ),
            (
                "hostile host",
                ClusterSpec {
                    endpoints: vec![Endpoint {
                        host: "evil..host/$(rm -rf)".into(),
                        port: 80,
                        weight: None,
                    }],
                    ..minimal()
                },
            ),
            (
                "mixed weights",
                ClusterSpec {
                    endpoints: vec![
                        Endpoint {
                            host: "a".into(),
                            port: 1,
                            weight: Some(10),
                        },
                        Endpoint {
                            host: "b".into(),
                            port: 1,
                            weight: None,
                        },
                    ],
                    ..minimal()
                },
            ),
            (
                "weight over cap",
                ClusterSpec {
                    endpoints: vec![Endpoint {
                        host: "a".into(),
                        port: 1,
                        weight: Some(1001),
                    }],
                    ..minimal()
                },
            ),
            (
                "timeout zero",
                ClusterSpec {
                    connect_timeout_secs: 0,
                    ..minimal()
                },
            ),
            (
                "hc timeout >= interval",
                ClusterSpec {
                    health_checks: Some(vec![HealthCheck::Http(HttpHealthCheck {
                        path: "/healthz".into(),
                        host: None,
                        method: None,
                        expected_statuses: Vec::new(),
                        timeout_seconds: 10,
                        interval_seconds: 10,
                        healthy_threshold: 3,
                        unhealthy_threshold: 3,
                    })]),
                    ..minimal()
                },
            ),
            (
                "hc path traversal",
                ClusterSpec {
                    health_checks: Some(vec![HealthCheck::Http(HttpHealthCheck {
                        path: "/../admin".into(),
                        host: None,
                        method: None,
                        expected_statuses: Vec::new(),
                        timeout_seconds: 1,
                        interval_seconds: 10,
                        healthy_threshold: 3,
                        unhealthy_threshold: 3,
                    })]),
                    ..minimal()
                },
            ),
            (
                "invalid expected status",
                ClusterSpec {
                    health_checks: Some(vec![HealthCheck::Http(HttpHealthCheck {
                        path: "/healthz".into(),
                        host: None,
                        method: Some(HttpHealthCheckMethod::Get),
                        expected_statuses: vec![99],
                        timeout_seconds: 1,
                        interval_seconds: 10,
                        healthy_threshold: 3,
                        unhealthy_threshold: 3,
                    })]),
                    ..minimal()
                },
            ),
            (
                "retries over cap",
                ClusterSpec {
                    circuit_breakers: Some(CircuitBreakers {
                        default: Some(CircuitBreakerThresholds {
                            max_connections: 10,
                            max_pending_requests: 10,
                            max_requests: 10,
                            max_retries: 11,
                        }),
                        high: None,
                    }),
                    ..minimal()
                },
            ),
            (
                "empty circuit breaker config",
                ClusterSpec {
                    circuit_breakers: Some(CircuitBreakers {
                        default: None,
                        high: None,
                    }),
                    ..minimal()
                },
            ),
            (
                "ring hash min over max",
                ClusterSpec {
                    lb_policy: LbPolicy::RingHash,
                    ring_hash: Some(RingHashPolicy {
                        minimum_ring_size: Some(1024),
                        maximum_ring_size: Some(128),
                        hash_function: None,
                    }),
                    ..minimal()
                },
            ),
            (
                "lb option for wrong policy",
                ClusterSpec {
                    lb_policy: LbPolicy::RoundRobin,
                    maglev: Some(MaglevPolicy {
                        table_size: Some(65_537),
                    }),
                    ..minimal()
                },
            ),
        ];
        for (label, spec) in cases {
            assert!(spec.validate().is_err(), "{label} must be rejected");
        }
    }

    #[test]
    fn expanded_cluster_options_validate() {
        let spec = ClusterSpec {
            lb_policy: LbPolicy::Maglev,
            maglev: Some(MaglevPolicy {
                table_size: Some(65_537),
            }),
            dns_lookup_family: Some(DnsLookupFamily::V4Only),
            upstream_tls: Some(UpstreamTlsConfig {
                sni: Some("api.example.com".into()),
                validation_context_sds_secret_name: Some("upstream-ca".into()),
                ca_cert_file: None,
                auto_sni_san_validation: true,
                insecure_skip_verify: false,
            }),
            protocol: Some(UpstreamProtocol::Grpc),
            health_checks: Some(vec![
                HealthCheck::Http(HttpHealthCheck {
                    path: "/healthz".into(),
                    host: Some("api.example.com".into()),
                    method: Some(HttpHealthCheckMethod::Get),
                    expected_statuses: vec![200, 204],
                    timeout_seconds: 1,
                    interval_seconds: 10,
                    healthy_threshold: 2,
                    unhealthy_threshold: 3,
                }),
                HealthCheck::Tcp(TcpHealthCheck {
                    timeout_seconds: 1,
                    interval_seconds: 10,
                    healthy_threshold: 2,
                    unhealthy_threshold: 3,
                }),
            ]),
            circuit_breakers: Some(CircuitBreakers {
                default: Some(CircuitBreakerThresholds {
                    max_connections: 100,
                    max_pending_requests: 200,
                    max_requests: 300,
                    max_retries: 3,
                }),
                high: Some(CircuitBreakerThresholds {
                    max_connections: 1000,
                    max_pending_requests: 2000,
                    max_requests: 3000,
                    max_retries: 5,
                }),
            }),
            outlier_detection: Some(OutlierDetection {
                consecutive_5xx: 5,
                interval_seconds: 10,
                base_ejection_seconds: 30,
                max_ejection_percent: 50,
                min_hosts: Some(3),
            }),
            ..minimal()
        };
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn unknown_spec_fields_rejected_at_deserialization() {
        let err = serde_json::from_value::<ClusterSpec>(serde_json::json!({
            "endpoints": [{"host": "h", "port": 80}],
            "use_tls_maybe": true,
        }));
        assert!(
            err.is_err(),
            "typos in spec fields must not be silently dropped"
        );
    }
}
