//! Cluster domain types
//!
//! This module contains pure domain entities for cluster (upstream) configurations.
//! These types encapsulate service discovery, load balancing, health checking,
//! and resilience patterns without any infrastructure dependencies.

use std::net::IpAddr;

/// Cluster specification defining upstream service configuration.
///
/// A cluster represents a logical group of endpoints that provide
/// the same service, along with policies for routing, health checking,
/// and failure handling.
#[derive(Debug, Clone)]
pub struct ClusterSpec {
    /// Cluster name (must be unique)
    pub name: String,

    /// Endpoints providing this service
    pub endpoints: Vec<Endpoint>,

    /// Load balancing policy
    pub load_balancing: LoadBalancingPolicy,

    /// Connection timeout in seconds
    pub connect_timeout_seconds: u64,

    /// TLS configuration for upstream connections
    pub tls_config: Option<UpstreamTlsConfig>,

    /// Health check configuration
    pub health_checks: Vec<HealthCheck>,

    /// Circuit breaker configuration
    pub circuit_breaker: Option<CircuitBreaker>,

    /// Outlier detection configuration
    pub outlier_detection: Option<OutlierDetection>,
}

impl ClusterSpec {
    /// Create a basic cluster with static endpoints
    pub fn static_cluster(name: impl Into<String>, endpoints: Vec<Endpoint>) -> Self {
        Self {
            name: name.into(),
            endpoints,
            load_balancing: LoadBalancingPolicy::RoundRobin,
            connect_timeout_seconds: 5,
            tls_config: None,
            health_checks: vec![],
            circuit_breaker: None,
            outlier_detection: None,
        }
    }

    /// Set load balancing policy
    pub fn with_load_balancing(mut self, policy: LoadBalancingPolicy) -> Self {
        self.load_balancing = policy;
        self
    }

    /// Set connection timeout
    pub fn with_connect_timeout(mut self, seconds: u64) -> Self {
        self.connect_timeout_seconds = seconds;
        self
    }

    /// Enable TLS for upstream connections
    pub fn with_tls(mut self, tls_config: UpstreamTlsConfig) -> Self {
        self.tls_config = Some(tls_config);
        self
    }

    /// Add health check
    pub fn with_health_check(mut self, health_check: HealthCheck) -> Self {
        self.health_checks.push(health_check);
        self
    }

    /// Set circuit breaker
    pub fn with_circuit_breaker(mut self, circuit_breaker: CircuitBreaker) -> Self {
        self.circuit_breaker = Some(circuit_breaker);
        self
    }

    /// Set outlier detection
    pub fn with_outlier_detection(mut self, outlier_detection: OutlierDetection) -> Self {
        self.outlier_detection = Some(outlier_detection);
        self
    }

    /// Validate cluster configuration
    pub fn validate(&self) -> Result<(), ClusterValidationError> {
        if self.name.is_empty() {
            return Err(ClusterValidationError::EmptyName);
        }

        if self.endpoints.is_empty() {
            return Err(ClusterValidationError::NoEndpoints);
        }

        if self.connect_timeout_seconds == 0 {
            return Err(ClusterValidationError::InvalidTimeout);
        }

        Ok(())
    }
}

/// Endpoint representing a single instance of a service
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Endpoint address
    pub address: EndpointAddress,

    /// Port number
    pub port: u16,

    /// Health status (for active health checking)
    pub health_status: HealthStatus,
}

impl Endpoint {
    /// Create endpoint from IP address and port
    pub fn from_ip(ip: IpAddr, port: u16) -> Self {
        Self { address: EndpointAddress::Ip(ip), port, health_status: HealthStatus::Unknown }
    }

    /// Create endpoint from hostname and port
    pub fn from_hostname(hostname: impl Into<String>, port: u16) -> Self {
        Self {
            address: EndpointAddress::Hostname(hostname.into()),
            port,
            health_status: HealthStatus::Unknown,
        }
    }

    /// Get endpoint as "host:port" string
    pub fn to_socket_string(&self) -> String {
        match &self.address {
            EndpointAddress::Ip(IpAddr::V4(ip)) => format!("{}:{}", ip, self.port),
            EndpointAddress::Ip(IpAddr::V6(ip)) => format!("[{}]:{}", ip, self.port),
            EndpointAddress::Hostname(host) => format!("{}:{}", host, self.port),
        }
    }
}

/// Endpoint address (IP or hostname)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointAddress {
    /// IP address (v4 or v6)
    Ip(IpAddr),

    /// Hostname (requires DNS resolution)
    Hostname(String),
}

/// Health status of an endpoint
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Health status unknown
    Unknown,

    /// Endpoint is healthy
    Healthy,

    /// Endpoint is unhealthy
    Unhealthy,

    /// Endpoint is being health checked
    Checking,
}

/// Load balancing policy
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadBalancingPolicy {
    /// Round-robin distribution
    RoundRobin,

    /// Least request (least active connections)
    LeastRequest { choice_count: u32 },

    /// Random selection
    Random,

    /// Ring hash (consistent hashing)
    RingHash { minimum_ring_size: u64 },

    /// Maglev (consistent hashing)
    Maglev { table_size: u64 },
}

/// TLS configuration for upstream connections
#[derive(Debug, Clone)]
pub struct UpstreamTlsConfig {
    /// Server name for SNI
    pub server_name: Option<String>,

    /// Whether to verify server certificate
    pub verify_certificate: bool,

    /// Optional client certificate for mutual TLS
    pub client_certificate: Option<ClientCertificate>,

    /// Minimum TLS version
    pub min_tls_version: TlsVersion,
}

/// Client certificate for mutual TLS
#[derive(Debug, Clone)]
pub struct ClientCertificate {
    /// Certificate chain (PEM format)
    pub certificate_chain: String,

    /// Private key (PEM format)
    pub private_key: String,
}

/// TLS version for upstream connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TlsVersion {
    /// TLS 1.2
    V1_2,

    /// TLS 1.3
    V1_3,
}

/// Health check configuration
#[derive(Debug, Clone)]
pub struct HealthCheck {
    /// Health check protocol
    pub protocol: HealthCheckProtocol,

    /// Interval between checks in seconds
    pub interval_seconds: u64,

    /// Timeout for each check in seconds
    pub timeout_seconds: u64,

    /// Unhealthy threshold (consecutive failures)
    pub unhealthy_threshold: u32,

    /// Healthy threshold (consecutive successes)
    pub healthy_threshold: u32,
}

impl HealthCheck {
    /// Create a basic HTTP health check
    pub fn http(path: impl Into<String>) -> Self {
        Self {
            protocol: HealthCheckProtocol::Http { path: path.into(), expected_status: 200 },
            interval_seconds: 10,
            timeout_seconds: 5,
            unhealthy_threshold: 3,
            healthy_threshold: 2,
        }
    }

    /// Create a TCP health check
    pub fn tcp() -> Self {
        Self {
            protocol: HealthCheckProtocol::Tcp,
            interval_seconds: 10,
            timeout_seconds: 5,
            unhealthy_threshold: 3,
            healthy_threshold: 2,
        }
    }
}

/// Health check protocol
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthCheckProtocol {
    /// HTTP health check
    Http {
        /// Health check path
        path: String,

        /// Expected HTTP status code
        expected_status: u16,
    },

    /// TCP health check (connection test)
    Tcp,

    /// gRPC health check
    Grpc {
        /// Service name
        service_name: Option<String>,
    },
}

/// Circuit breaker configuration
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    /// Maximum number of connections
    pub max_connections: u32,

    /// Maximum pending requests
    pub max_pending_requests: u32,

    /// Maximum requests per connection
    pub max_requests: u32,

    /// Maximum active requests
    pub max_retries: u32,
}

impl CircuitBreaker {
    /// Create default circuit breaker configuration
    pub fn default_config() -> Self {
        Self {
            max_connections: 1024,
            max_pending_requests: 1024,
            max_requests: 1024,
            max_retries: 3,
        }
    }
}

/// Outlier detection configuration (passive health checking)
#[derive(Debug, Clone)]
pub struct OutlierDetection {
    /// Consecutive 5xx errors before ejection
    pub consecutive_5xx: u32,

    /// Time interval for measuring errors (seconds)
    pub interval_seconds: u64,

    /// Base ejection time (seconds)
    pub base_ejection_time_seconds: u64,

    /// Maximum ejection percentage
    pub max_ejection_percent: u32,

    /// Minimum number of hosts
    pub min_hosts: u32,
}

impl OutlierDetection {
    /// Create default outlier detection configuration
    pub fn default_config() -> Self {
        Self {
            consecutive_5xx: 5,
            interval_seconds: 10,
            base_ejection_time_seconds: 30,
            max_ejection_percent: 10,
            min_hosts: 1,
        }
    }
}

/// Cluster validation errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterValidationError {
    /// Empty cluster name
    EmptyName,

    /// No endpoints defined
    NoEndpoints,

    /// Invalid timeout
    InvalidTimeout,
}

/// Represents a dependency on a cluster from another resource.
///
/// Used to track what resources reference a cluster, enabling
/// proper deletion protection and dependency visualization.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ClusterDependency {
    /// Type of the resource that depends on the cluster
    pub resource_type: String,

    /// ID of the dependent resource
    pub resource_id: String,

    /// Name of the dependent resource
    pub resource_name: String,

    /// Path within the resource where the cluster is referenced
    pub reference_path: String,
}

impl ClusterDependency {
    /// Create a new cluster dependency
    pub fn new(
        resource_type: impl Into<String>,
        resource_id: impl Into<String>,
        resource_name: impl Into<String>,
        reference_path: impl Into<String>,
    ) -> Self {
        Self {
            resource_type: resource_type.into(),
            resource_id: resource_id.into(),
            resource_name: resource_name.into(),
            reference_path: reference_path.into(),
        }
    }

    /// Create a dependency for a route config
    pub fn route_config(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::new("route_config", id, name, "cluster_name")
    }

    /// Create a dependency for a filter
    pub fn filter(
        id: impl Into<String>,
        name: impl Into<String>,
        reference_path: impl Into<String>,
    ) -> Self {
        Self::new("filter", id, name, reference_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn endpoint_from_ip() {
        let endpoint = Endpoint::from_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 8080);
        assert_eq!(endpoint.port, 8080);
        assert_eq!(endpoint.to_socket_string(), "192.168.1.1:8080");
    }

    #[test]
    fn endpoint_from_hostname() {
        let endpoint = Endpoint::from_hostname("backend.example.com", 443);
        assert_eq!(endpoint.port, 443);
        assert_eq!(endpoint.to_socket_string(), "backend.example.com:443");
    }

    #[test]
    fn cluster_spec_basic() {
        let cluster = ClusterSpec::static_cluster(
            "backend",
            vec![Endpoint::from_hostname("backend.svc.local", 8080)],
        );

        assert_eq!(cluster.name, "backend");
        assert_eq!(cluster.endpoints.len(), 1);
        assert_eq!(cluster.load_balancing, LoadBalancingPolicy::RoundRobin);
        assert_eq!(cluster.connect_timeout_seconds, 5);
    }

    #[test]
    fn cluster_spec_builder() {
        let cluster = ClusterSpec::static_cluster(
            "backend",
            vec![Endpoint::from_hostname("backend.svc.local", 8080)],
        )
        .with_load_balancing(LoadBalancingPolicy::LeastRequest { choice_count: 2 })
        .with_connect_timeout(10)
        .with_health_check(HealthCheck::http("/health"));

        assert_eq!(cluster.load_balancing, LoadBalancingPolicy::LeastRequest { choice_count: 2 });
        assert_eq!(cluster.connect_timeout_seconds, 10);
        assert_eq!(cluster.health_checks.len(), 1);
    }

    #[test]
    fn cluster_validation_success() {
        let cluster = ClusterSpec::static_cluster(
            "valid",
            vec![Endpoint::from_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 8080)],
        );
        assert!(cluster.validate().is_ok());
    }

    #[test]
    fn cluster_validation_empty_name() {
        let cluster = ClusterSpec::static_cluster(
            "",
            vec![Endpoint::from_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 8080)],
        );
        assert_eq!(cluster.validate(), Err(ClusterValidationError::EmptyName));
    }

    #[test]
    fn cluster_validation_no_endpoints() {
        let cluster = ClusterSpec::static_cluster("test", vec![]);
        assert_eq!(cluster.validate(), Err(ClusterValidationError::NoEndpoints));
    }

    #[test]
    fn health_check_http() {
        let hc = HealthCheck::http("/healthz");
        assert_eq!(hc.interval_seconds, 10);
        assert_eq!(hc.timeout_seconds, 5);
        assert!(matches!(hc.protocol, HealthCheckProtocol::Http { .. }));
    }

    #[test]
    fn health_check_tcp() {
        let hc = HealthCheck::tcp();
        assert!(matches!(hc.protocol, HealthCheckProtocol::Tcp));
    }

    #[test]
    fn load_balancing_policies() {
        assert_eq!(LoadBalancingPolicy::RoundRobin, LoadBalancingPolicy::RoundRobin);
        assert_ne!(
            LoadBalancingPolicy::RoundRobin,
            LoadBalancingPolicy::LeastRequest { choice_count: 2 }
        );
    }

    #[test]
    fn circuit_breaker_defaults() {
        let cb = CircuitBreaker::default_config();
        assert_eq!(cb.max_connections, 1024);
        assert_eq!(cb.max_pending_requests, 1024);
        assert_eq!(cb.max_retries, 3);
    }

    #[test]
    fn outlier_detection_defaults() {
        let od = OutlierDetection::default_config();
        assert_eq!(od.consecutive_5xx, 5);
        assert_eq!(od.base_ejection_time_seconds, 30);
        assert_eq!(od.max_ejection_percent, 10);
    }

    #[test]
    fn health_status_states() {
        assert_eq!(HealthStatus::Healthy, HealthStatus::Healthy);
        assert_ne!(HealthStatus::Healthy, HealthStatus::Unhealthy);
    }

    #[test]
    fn cluster_with_tls() {
        let tls_config = UpstreamTlsConfig {
            server_name: Some("backend.example.com".to_string()),
            verify_certificate: true,
            client_certificate: None,
            min_tls_version: TlsVersion::V1_2,
        };

        let cluster = ClusterSpec::static_cluster(
            "secure-backend",
            vec![Endpoint::from_hostname("backend.example.com", 443)],
        )
        .with_tls(tls_config);

        assert!(cluster.tls_config.is_some());
        let tls = cluster.tls_config.unwrap();
        assert_eq!(tls.server_name, Some("backend.example.com".to_string()));
        assert!(tls.verify_certificate);
    }

    #[test]
    fn cluster_with_mutual_tls() {
        let client_cert = ClientCertificate {
            certificate_chain: "client-cert".to_string(),
            private_key: "client-key".to_string(),
        };

        let tls_config = UpstreamTlsConfig {
            server_name: Some("backend.example.com".to_string()),
            verify_certificate: true,
            client_certificate: Some(client_cert),
            min_tls_version: TlsVersion::V1_3,
        };

        assert!(tls_config.client_certificate.is_some());
        assert_eq!(tls_config.min_tls_version, TlsVersion::V1_3);
    }

    #[test]
    fn load_balancing_policy_variants() {
        let rr = LoadBalancingPolicy::RoundRobin;
        let lr = LoadBalancingPolicy::LeastRequest { choice_count: 2 };
        let random = LoadBalancingPolicy::Random;
        let ring_hash = LoadBalancingPolicy::RingHash { minimum_ring_size: 1024 };
        let maglev = LoadBalancingPolicy::Maglev { table_size: 65537 };

        assert!(matches!(rr, LoadBalancingPolicy::RoundRobin));
        assert!(matches!(lr, LoadBalancingPolicy::LeastRequest { .. }));
        assert!(matches!(random, LoadBalancingPolicy::Random));
        assert!(matches!(ring_hash, LoadBalancingPolicy::RingHash { .. }));
        assert!(matches!(maglev, LoadBalancingPolicy::Maglev { .. }));
    }

    #[test]
    fn health_check_protocol_variants() {
        let http = HealthCheckProtocol::Http { path: "/health".to_string(), expected_status: 200 };
        let tcp = HealthCheckProtocol::Tcp;
        let grpc = HealthCheckProtocol::Grpc { service_name: Some("my.service.v1".to_string()) };

        assert!(matches!(http, HealthCheckProtocol::Http { .. }));
        assert!(matches!(tcp, HealthCheckProtocol::Tcp));
        assert!(matches!(grpc, HealthCheckProtocol::Grpc { .. }));
    }

    #[test]
    fn endpoint_address_variants() {
        let ip_v4 = EndpointAddress::Ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        let hostname = EndpointAddress::Hostname("api.example.com".to_string());

        assert!(matches!(ip_v4, EndpointAddress::Ip(_)));
        assert!(matches!(hostname, EndpointAddress::Hostname(_)));
    }

    #[test]
    fn cluster_with_all_features() {
        let cluster = ClusterSpec::static_cluster(
            "production-backend",
            vec![
                Endpoint::from_hostname("backend-1.example.com", 8080),
                Endpoint::from_hostname("backend-2.example.com", 8080),
            ],
        )
        .with_load_balancing(LoadBalancingPolicy::LeastRequest { choice_count: 2 })
        .with_connect_timeout(10)
        .with_health_check(HealthCheck::http("/healthz"))
        .with_circuit_breaker(CircuitBreaker::default_config())
        .with_outlier_detection(OutlierDetection::default_config());

        assert_eq!(cluster.name, "production-backend");
        assert_eq!(cluster.endpoints.len(), 2);
        assert_eq!(cluster.connect_timeout_seconds, 10);
        assert_eq!(cluster.health_checks.len(), 1);
        assert!(cluster.circuit_breaker.is_some());
        assert!(cluster.outlier_detection.is_some());
    }

    #[test]
    fn circuit_breaker_custom_config() {
        let cb = CircuitBreaker {
            max_connections: 2048,
            max_pending_requests: 2048,
            max_requests: 2048,
            max_retries: 5,
        };

        assert_eq!(cb.max_connections, 2048);
        assert_eq!(cb.max_retries, 5);
    }

    #[test]
    fn outlier_detection_custom_config() {
        let od = OutlierDetection {
            consecutive_5xx: 10,
            interval_seconds: 30,
            base_ejection_time_seconds: 60,
            max_ejection_percent: 20,
            min_hosts: 2,
        };

        assert_eq!(od.consecutive_5xx, 10);
        assert_eq!(od.max_ejection_percent, 20);
        assert_eq!(od.min_hosts, 2);
    }

    #[test]
    fn tls_version_comparison() {
        assert!(TlsVersion::V1_3 > TlsVersion::V1_2);
    }
}
