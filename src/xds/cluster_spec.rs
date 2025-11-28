use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::errors::Error;

#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClusterSpec {
    #[serde(default, alias = "connect_timeout_seconds")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout_seconds: Option<u64>,

    #[serde(default)]
    pub endpoints: Vec<EndpointSpec>,

    #[serde(default, alias = "use_tls")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_tls: Option<bool>,

    #[serde(default, alias = "tls_server_name")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_server_name: Option<String>,

    #[serde(default, alias = "dns_lookup_family")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_lookup_family: Option<String>,

    #[serde(default, alias = "lb_policy")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lb_policy: Option<String>,

    #[serde(default, alias = "least_request")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub least_request: Option<LeastRequestPolicy>,

    #[serde(default, alias = "ring_hash")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ring_hash: Option<RingHashPolicy>,

    #[serde(default, alias = "maglev")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maglev: Option<MaglevPolicy>,

    #[serde(default, alias = "circuit_breakers")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circuit_breakers: Option<CircuitBreakersSpec>,

    #[serde(default, alias = "health_checks")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub health_checks: Vec<HealthCheckSpec>,

    #[serde(default, alias = "outlier_detection")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outlier_detection: Option<OutlierDetectionSpec>,

    /// Protocol type for upstream connections.
    /// Set to "HTTP2" or "GRPC" for gRPC/HTTP2 upstreams (e.g., OTEL collectors).
    /// Defaults to HTTP/1.1 if not specified.
    #[serde(default, alias = "protocol_type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_type: Option<String>,
}

impl ClusterSpec {
    pub fn from_value(value: Value) -> Result<Self, Error> {
        let spec: ClusterSpec = serde_json::from_value(value.clone())
            .map_err(|e| Error::config(format!("Invalid cluster configuration JSON: {}", e)))?;
        spec.validate_model()?;
        Ok(spec)
    }

    pub fn to_value(&self) -> Result<Value, Error> {
        serde_json::to_value(self).map_err(|e| {
            Error::internal(format!("Failed to serialize cluster configuration: {}", e))
        })
    }

    fn ensure_endpoints(&self) -> Result<(), Error> {
        if self.endpoints.is_empty() {
            return Err(Error::validation("Cluster must define at least one endpoint"));
        }

        if let Some(invalid) = self.endpoints.iter().find(|ep| ep.to_host_port().is_none()) {
            return Err(Error::validation(format!("Invalid endpoint definition: {}", invalid)));
        }

        Ok(())
    }

    pub fn use_tls(&self) -> bool {
        self.use_tls.unwrap_or(false)
    }

    pub fn validate_model(&self) -> Result<(), Error> {
        self.ensure_endpoints()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum EndpointSpec {
    String(String),
    Address { host: String, port: u16 },
}

impl std::fmt::Display for EndpointSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EndpointSpec::String(s) => write!(f, "{}", s),
            EndpointSpec::Address { host, port } => write!(f, "{}:{}", host, port),
        }
    }
}

impl EndpointSpec {
    pub fn to_host_port(&self) -> Option<(String, u32)> {
        match self {
            EndpointSpec::String(value) => {
                let parts: Vec<&str> = value.split(':').collect();
                if parts.len() != 2 {
                    return None;
                }
                let host = parts[0].trim();
                let port = parts[1].trim().parse::<u32>().ok()?;
                if host.is_empty() {
                    return None;
                }
                Some((host.to_string(), port))
            }
            EndpointSpec::Address { host, port } => {
                if host.trim().is_empty() {
                    return None;
                }
                Some((host.trim().to_string(), *port as u32))
            }
        }
    }

    pub fn is_hostname(&self) -> bool {
        self.to_host_port()
            .and_then(|(host, _)| host.parse::<IpAddr>().ok().map(|_| false))
            .map(|is_ip| !is_ip)
            .unwrap_or(false)
    }

    pub fn host_port_or_error(&self) -> Result<(String, u32), Error> {
        self.to_host_port().ok_or_else(|| Error::validation(format!("Invalid endpoint: {}", self)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LeastRequestPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RingHashPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_ring_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_ring_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_function: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaglevPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakersSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<CircuitBreakerThresholdsSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<CircuitBreakerThresholdsSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CircuitBreakerThresholdsSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_connections: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_pending_requests: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_requests: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HealthCheckSpec {
    Http {
        path: String,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        method: Option<String>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        interval_seconds: Option<u64>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        healthy_threshold: Option<u32>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        unhealthy_threshold: Option<u32>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_statuses: Option<Vec<u32>>,
    },
    Tcp {
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        interval_seconds: Option<u64>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        healthy_threshold: Option<u32>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        unhealthy_threshold: Option<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutlierDetectionSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consecutive_5xx: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ejection_time_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_ejection_percent: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_hosts: Option<u32>,
}

impl std::fmt::Display for HealthCheckSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthCheckSpec::Http { path, .. } => write!(f, "http {}", path),
            HealthCheckSpec::Tcp { .. } => write!(f, "tcp"),
        }
    }
}

impl std::fmt::Display for CircuitBreakerThresholdsSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "connections={:?} pending={:?} requests={:?} retries={:?}",
            self.max_connections, self.max_pending_requests, self.max_requests, self.max_retries
        )
    }
}
