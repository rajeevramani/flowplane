//! Cluster: an upstream backend definition (spec/03 §7.2 business rules carried from v1,
//! with one deliberate change — TLS to the upstream is always explicit, never inferred
//! from port 443 (v1 smell, spec/04 §8.13).

use crate::error::{DomainError, DomainResult};
use crate::id::{ClusterId, TeamId};
use crate::identity::validate_name;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Name prefixes reserved for Flowplane-internal resources (v1 rule, kept).
pub const RESERVED_NAME_PREFIXES: &[&str] =
    &["envoy-", "xds-", "internal-", "system-", "flowplane-"];

pub const MAX_ENDPOINTS: usize = 100;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterSpec {
    pub endpoints: Vec<Endpoint>,
    #[serde(default)]
    pub lb_policy: LbPolicy,
    /// Connection timeout to the upstream, seconds (1–300).
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u32,
    /// TLS to the upstream — always explicit (no port-443 inference).
    #[serde(default)]
    pub use_tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<HealthCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_breaker: Option<CircuitBreaker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outlier_detection: Option<OutlierDetection>,
}

fn default_connect_timeout() -> u32 {
    5
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// Load-balancing weight (1–1000). All endpoints weighted or none (v1 rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LbPolicy {
    #[default]
    RoundRobin,
    LeastRequest,
    Random,
    RingHash,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    /// HTTP path probed; must start with `/`, no `..`, ≤200 chars.
    pub path: String,
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

fn default_threshold() -> u32 {
    3
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircuitBreaker {
    /// Each 1–10000.
    pub max_connections: u32,
    pub max_pending_requests: u32,
    pub max_requests: u32,
    /// ≤10.
    #[serde(default)]
    pub max_retries: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

        if let Some(hc) = &self.health_check {
            if !hc.path.starts_with('/') || hc.path.contains("..") || hc.path.len() > 200 {
                return Err(DomainError::validation(
                    "health-check path must start with '/', contain no '..', and be <= 200 chars",
                ));
            }
            range("health-check timeout_seconds", hc.timeout_seconds, 1, 60)?;
            range("health-check interval_seconds", hc.interval_seconds, 1, 300)?;
            if hc.timeout_seconds >= hc.interval_seconds {
                return Err(DomainError::validation(
                    "health-check timeout must be strictly less than its interval",
                ));
            }
            range("healthy_threshold", hc.healthy_threshold, 1, 10)?;
            range("unhealthy_threshold", hc.unhealthy_threshold, 1, 10)?;
        }
        if let Some(cb) = &self.circuit_breaker {
            range("max_connections", cb.max_connections, 1, 10_000)?;
            range("max_pending_requests", cb.max_pending_requests, 1, 10_000)?;
            range("max_requests", cb.max_requests, 1, 10_000)?;
            range("max_retries", cb.max_retries, 0, 10)?;
        }
        if let Some(od) = &self.outlier_detection {
            range("consecutive_5xx", od.consecutive_5xx, 1, 1000)?;
            range("outlier interval_seconds", od.interval_seconds, 1, 300)?;
            range("base_ejection_seconds", od.base_ejection_seconds, 1, 3600)?;
            range("max_ejection_percent", od.max_ejection_percent, 1, 100)?;
        }
        Ok(())
    }
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
            lb_policy: LbPolicy::RoundRobin,
            connect_timeout_secs: 5,
            use_tls: false,
            health_check: None,
            circuit_breaker: None,
            outlier_detection: None,
        }
    }

    #[test]
    fn minimal_spec_validates() {
        assert!(minimal().validate().is_ok());
    }

    #[test]
    fn reserved_prefixes_rejected() {
        for name in [
            "envoy-edge",
            "xds-x",
            "internal-db",
            "system-a",
            "flowplane-self",
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
                    health_check: Some(HealthCheck {
                        path: "/healthz".into(),
                        timeout_seconds: 10,
                        interval_seconds: 10,
                        healthy_threshold: 3,
                        unhealthy_threshold: 3,
                    }),
                    ..minimal()
                },
            ),
            (
                "hc path traversal",
                ClusterSpec {
                    health_check: Some(HealthCheck {
                        path: "/../admin".into(),
                        timeout_seconds: 1,
                        interval_seconds: 10,
                        healthy_threshold: 3,
                        unhealthy_threshold: 3,
                    }),
                    ..minimal()
                },
            ),
            (
                "retries over cap",
                ClusterSpec {
                    circuit_breaker: Some(CircuitBreaker {
                        max_connections: 10,
                        max_pending_requests: 10,
                        max_requests: 10,
                        max_retries: 11,
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
