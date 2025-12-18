//! Cluster handler DTOs and type definitions

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::xds::ClusterSpec;

fn default_health_check_type() -> String {
    "http".to_string()
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "team": "payments",
    "name": "my-service-cluster",
    "serviceName": "inventory-service",
    "endpoints": [
        {"host": "service1.example.com", "port": 8080},
        {"host": "service2.example.com", "port": 8080}
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "service.example.com",
    "lbPolicy": "ROUND_ROBIN",
    "healthChecks": [
        {
            "type": "http",
            "path": "/health",
            "intervalSeconds": 10,
            "timeoutSeconds": 5,
            "healthyThreshold": 2,
            "unhealthyThreshold": 3
        }
    ],
    "circuitBreakers": {
        "default": {
            "maxConnections": 100,
            "maxPendingRequests": 50,
            "maxRequests": 200,
            "maxRetries": 3
        }
    },
    "outlierDetection": {
        "consecutive5xx": 5,
        "intervalSeconds": 30,
        "baseEjectionTimeSeconds": 30,
        "maxEjectionPercent": 50
    }
}))]
pub struct CreateClusterBody {
    /// Team identifier for ownership.
    #[validate(length(min = 1, max = 100))]
    #[schema(example = "payments")]
    pub team: String,

    /// Unique name for the cluster.
    #[validate(length(min = 1, max = 50))]
    #[schema(example = "my-service-cluster")]
    pub name: String,

    /// Optional service identifier exposed to clients (defaults to `name`).
    #[serde(default)]
    #[validate(length(min = 1, max = 50))]
    #[schema(example = "inventory-service")]
    pub service_name: Option<String>,

    /// Upstream endpoints (host & port) for this cluster. At least one is required.
    #[validate(length(min = 1))]
    #[schema(min_items = 1, value_type = Vec<EndpointRequest>)]
    pub endpoints: Vec<EndpointRequest>,

    /// Connection timeout in seconds (default: 5).
    #[serde(default)]
    #[schema(default = 5)]
    pub connect_timeout_seconds: Option<u64>,

    /// Enable TLS for upstream connections (default: false).
    #[serde(default)]
    #[schema(default = false)]
    pub use_tls: Option<bool>,

    /// Optional SNI server name to present during TLS handshake.
    #[serde(default)]
    pub tls_server_name: Option<String>,

    /// DNS lookup family for hostname endpoints (`AUTO`, `V4_ONLY`, `V6_ONLY`, `V4_PREFERRED`, `ALL`).
    #[serde(default)]
    #[schema(example = "AUTO")]
    pub dns_lookup_family: Option<String>,

    /// Load-balancer policy (`ROUND_ROBIN`, `LEAST_REQUEST`, `RANDOM`, `RING_HASH`, `MAGLEV`, `CLUSTER_PROVIDED`).
    #[serde(default)]
    #[schema(example = "ROUND_ROBIN")]
    pub lb_policy: Option<String>,

    /// Active health-check definitions.
    #[serde(default)]
    #[schema(value_type = Vec<HealthCheckRequest>)]
    pub health_checks: Vec<HealthCheckRequest>,

    /// Circuit breaker thresholds applied to the cluster.
    #[serde(default)]
    #[schema(value_type = CircuitBreakersRequest)]
    pub circuit_breakers: Option<CircuitBreakersRequest>,

    /// Passive outlier detection configuration.
    #[serde(default)]
    pub outlier_detection: Option<OutlierDetectionRequest>,

    /// Protocol type for upstream connections.
    /// Use "HTTP2" or "GRPC" for gRPC/HTTP2 upstreams (e.g., OTEL collectors, gRPC services).
    /// Defaults to HTTP/1.1 if not specified.
    #[serde(default)]
    #[schema(example = "GRPC")]
    pub protocol_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({"host": "httpbin.org", "port": 443}))]
pub struct EndpointRequest {
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "httpbin.org")]
    pub host: String,

    #[validate(range(min = 1, max = 65535))]
    #[schema(example = 443)]
    pub port: u16,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "type": "http",
    "path": "/health",
    "method": "GET",
    "intervalSeconds": 10,
    "timeoutSeconds": 5,
    "healthyThreshold": 2,
    "unhealthyThreshold": 3,
    "expectedStatuses": [200, 204]
}))]
pub struct HealthCheckRequest {
    #[serde(default = "default_health_check_type")]
    #[schema(example = "http")]
    pub r#type: String,

    /// HTTP path probed when `type` is `http` (default: `/health`).
    pub path: Option<String>,
    /// Host header override for HTTP health checks.
    pub host: Option<String>,
    /// HTTP method used for health probes.
    pub method: Option<String>,
    /// Interval between health probes in seconds.
    #[schema(example = 10)]
    pub interval_seconds: Option<u64>,
    /// Timeout for health probes in seconds.
    #[schema(example = 5)]
    pub timeout_seconds: Option<u64>,
    /// Number of consecutive successful probes before marking healthy.
    pub healthy_threshold: Option<u32>,
    /// Number of consecutive failed probes before marking unhealthy.
    pub unhealthy_threshold: Option<u32>,
    /// HTTP status codes treated as successful responses.
    pub expected_statuses: Option<Vec<u32>>,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "default": {
        "maxConnections": 100,
        "maxPendingRequests": 50,
        "maxRequests": 200,
        "maxRetries": 3
    }
}))]
pub struct CircuitBreakersRequest {
    /// Thresholds applied to default-priority traffic.
    pub default: Option<CircuitBreakerThresholdsRequest>,
    /// Thresholds applied to high-priority traffic.
    pub high: Option<CircuitBreakerThresholdsRequest>,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "maxConnections": 100,
    "maxPendingRequests": 50,
    "maxRequests": 200,
    "maxRetries": 3
}))]
pub struct CircuitBreakerThresholdsRequest {
    /// Maximum concurrent upstream connections.
    pub max_connections: Option<u32>,
    /// Maximum number of pending requests allowed.
    pub max_pending_requests: Option<u32>,
    /// Maximum in-flight requests.
    pub max_requests: Option<u32>,
    /// Maximum simultaneous retries.
    pub max_retries: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "consecutive5xx": 5,
    "intervalSeconds": 30,
    "baseEjectionTimeSeconds": 30,
    "maxEjectionPercent": 50,
    "minHosts": 3
}))]
pub struct OutlierDetectionRequest {
    /// Number of consecutive 5xx responses before ejecting a host.
    pub consecutive_5xx: Option<u32>,
    /// Interval between outlier analysis sweeps (seconds).
    pub interval_seconds: Option<u64>,
    /// Base ejection time (seconds).
    pub base_ejection_time_seconds: Option<u64>,
    /// Maximum percentage of hosts that can be ejected simultaneously.
    pub max_ejection_percent: Option<u32>,
    /// Minimum number of hosts required before ejection is allowed.
    pub min_hosts: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "name": "my-service-cluster",
    "team": "payments",
    "serviceName": "my-service-cluster",
    "config": {
        "endpoints": [
            {"host": "service1.example.com", "port": 8080},
            {"host": "service2.example.com", "port": 8080}
        ],
        "connectTimeoutSeconds": 5,
        "useTls": true,
        "tlsServerName": "service.example.com",
        "lbPolicy": "ROUND_ROBIN",
        "healthChecks": [
            {
                "type": "http",
                "path": "/health",
                "method": "GET",
                "intervalSeconds": 10,
                "timeoutSeconds": 5,
                "healthyThreshold": 2,
                "unhealthyThreshold": 3
            }
        ],
        "circuitBreakers": {
            "default": {
                "maxConnections": 100,
                "maxPendingRequests": 50,
                "maxRequests": 200,
                "maxRetries": 3
            }
        },
        "outlierDetection": {
            "consecutive5xx": 5,
            "intervalSeconds": 30,
            "baseEjectionTimeSeconds": 30,
            "maxEjectionPercent": 50
        }
    }
}))]
pub struct ClusterResponse {
    pub name: String,
    pub team: String,
    pub service_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_id: Option<String>,
    pub config: ClusterSpec,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListClustersQuery {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}
