//! Cluster DTOs for API request/response handling

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// Request body for creating a cluster
#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
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
pub struct CreateClusterDto {
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
    #[schema(min_items = 1, value_type = Vec<EndpointDto>)]
    pub endpoints: Vec<EndpointDto>,

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
    #[schema(value_type = Vec<HealthCheckDto>)]
    pub health_checks: Vec<HealthCheckDto>,

    /// Circuit breaker thresholds applied to the cluster.
    #[serde(default)]
    #[schema(value_type = CircuitBreakersDto)]
    pub circuit_breakers: Option<CircuitBreakersDto>,

    /// Passive outlier detection configuration.
    #[serde(default)]
    pub outlier_detection: Option<OutlierDetectionDto>,
}

/// Request body for updating a cluster
#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateClusterDto {
    /// Updated service identifier
    #[serde(default)]
    #[validate(length(min = 1, max = 50))]
    pub service_name: Option<String>,

    /// Updated upstream endpoints
    #[validate(length(min = 1))]
    #[schema(min_items = 1, value_type = Vec<EndpointDto>)]
    pub endpoints: Vec<EndpointDto>,

    /// Updated connection timeout
    #[serde(default)]
    pub connect_timeout_seconds: Option<u64>,

    /// Updated TLS configuration
    #[serde(default)]
    pub use_tls: Option<bool>,

    #[serde(default)]
    pub tls_server_name: Option<String>,

    #[serde(default)]
    pub dns_lookup_family: Option<String>,

    #[serde(default)]
    pub lb_policy: Option<String>,

    #[serde(default)]
    pub health_checks: Option<Vec<HealthCheckDto>>,

    #[serde(default)]
    pub circuit_breakers: Option<CircuitBreakersDto>,

    #[serde(default)]
    pub outlier_detection: Option<OutlierDetectionDto>,
}

/// Endpoint DTO
#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({"host": "httpbin.org", "port": 443}))]
pub struct EndpointDto {
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "httpbin.org")]
    pub host: String,

    #[validate(range(min = 1, max = 65535))]
    #[schema(example = 443)]
    pub port: u16,
}

/// Health check configuration DTO
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
pub struct HealthCheckDto {
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

fn default_health_check_type() -> String {
    "http".to_string()
}

/// Circuit breakers configuration DTO
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
pub struct CircuitBreakersDto {
    /// Thresholds applied to default-priority traffic.
    pub default: Option<CircuitBreakerThresholdsDto>,
    /// Thresholds applied to high-priority traffic.
    pub high: Option<CircuitBreakerThresholdsDto>,
}

/// Circuit breaker thresholds DTO
#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "maxConnections": 100,
    "maxPendingRequests": 50,
    "maxRequests": 200,
    "maxRetries": 3
}))]
pub struct CircuitBreakerThresholdsDto {
    /// Maximum concurrent upstream connections.
    pub max_connections: Option<u32>,
    /// Maximum number of pending requests allowed.
    pub max_pending_requests: Option<u32>,
    /// Maximum in-flight requests.
    pub max_requests: Option<u32>,
    /// Maximum concurrent retry operations.
    pub max_retries: Option<u32>,
}

/// Outlier detection configuration DTO
#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "consecutive5xx": 5,
    "intervalSeconds": 30,
    "baseEjectionTimeSeconds": 30,
    "maxEjectionPercent": 50,
    "minHosts": 3
}))]
pub struct OutlierDetectionDto {
    /// Number of consecutive 5xx errors before ejection.
    pub consecutive_5xx: Option<u32>,
    /// Time interval for outlier detection in seconds.
    pub interval_seconds: Option<u64>,
    /// Minimum ejection duration in seconds.
    pub base_ejection_time_seconds: Option<u64>,
    /// Maximum percent of endpoints that can be ejected.
    pub max_ejection_percent: Option<u32>,
    /// Minimum number of hosts required before ejection is allowed.
    pub min_hosts: Option<u32>,
}
