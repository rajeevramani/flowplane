//! Listener domain types
//!
//! This module contains pure domain entities for listener configurations.
//! These types encapsulate network binding configuration and listener
//! behavior without any infrastructure dependencies.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Network protocol supported by listeners
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Protocol {
    /// HTTP/1.1 protocol
    Http,

    /// HTTP/2 protocol
    Http2,

    /// HTTPS (HTTP over TLS)
    Https,

    /// TCP protocol (Layer 4)
    Tcp,
}

impl Protocol {
    /// Check if this protocol requires TLS
    pub fn requires_tls(&self) -> bool {
        matches!(self, Protocol::Https)
    }

    /// Check if this protocol supports HTTP
    pub fn is_http(&self) -> bool {
        matches!(self, Protocol::Http | Protocol::Http2 | Protocol::Https)
    }

    /// Get the default port for this protocol
    pub fn default_port(&self) -> u16 {
        match self {
            Protocol::Http | Protocol::Http2 => 80,
            Protocol::Https => 443,
            Protocol::Tcp => 8080,
        }
    }
}

/// Listener bind address configuration
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindAddress {
    /// IP address to bind to
    pub address: IpAddr,

    /// Port number
    pub port: u16,
}

impl BindAddress {
    /// Create a new bind address
    pub fn new(address: IpAddr, port: u16) -> Self {
        Self { address, port }
    }

    /// Create a bind address for all IPv4 interfaces
    pub fn ipv4_all(port: u16) -> Self {
        Self { address: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port }
    }

    /// Create a bind address for all IPv6 interfaces
    pub fn ipv6_all(port: u16) -> Self {
        Self { address: IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0)), port }
    }

    /// Create a localhost bind address (IPv4)
    pub fn localhost(port: u16) -> Self {
        Self { address: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port }
    }

    /// Format as "address:port" string
    pub fn to_socket_addr_string(&self) -> String {
        match self.address {
            IpAddr::V4(addr) => format!("{}:{}", addr, self.port),
            IpAddr::V6(addr) => format!("[{}]:{}", addr, self.port),
        }
    }
}

/// TLS configuration for secure listeners
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Certificate chain (PEM format)
    pub certificate_chain: String,

    /// Private key (PEM format)
    pub private_key: String,

    /// Optional CA certificate for mutual TLS
    pub ca_certificate: Option<String>,

    /// Whether to require client certificates (mutual TLS)
    pub require_client_cert: bool,

    /// Minimum TLS version
    pub min_tls_version: TlsVersion,

    /// Cipher suites (empty = use defaults)
    pub cipher_suites: Vec<String>,
}

impl TlsConfig {
    /// Create a basic TLS configuration (server-side only)
    pub fn server_only(certificate_chain: String, private_key: String) -> Self {
        Self {
            certificate_chain,
            private_key,
            ca_certificate: None,
            require_client_cert: false,
            min_tls_version: TlsVersion::V1_2,
            cipher_suites: vec![],
        }
    }

    /// Create a mutual TLS configuration (client cert required)
    pub fn mutual_tls(
        certificate_chain: String,
        private_key: String,
        ca_certificate: String,
    ) -> Self {
        Self {
            certificate_chain,
            private_key,
            ca_certificate: Some(ca_certificate),
            require_client_cert: true,
            min_tls_version: TlsVersion::V1_2,
            cipher_suites: vec![],
        }
    }

    /// Set minimum TLS version
    pub fn with_min_version(mut self, version: TlsVersion) -> Self {
        self.min_tls_version = version;
        self
    }

    /// Set cipher suites
    pub fn with_cipher_suites(mut self, suites: Vec<String>) -> Self {
        self.cipher_suites = suites;
        self
    }
}

/// TLS protocol version
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TlsVersion {
    /// TLS 1.0 (deprecated, not recommended)
    V1_0,

    /// TLS 1.1 (deprecated, not recommended)
    V1_1,

    /// TLS 1.2
    V1_2,

    /// TLS 1.3
    V1_3,
}

/// Listener isolation mode.
///
/// Determines whether a listener is dedicated to a specific API
/// or shared across multiple APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationMode {
    /// Shared listener used by multiple APIs
    Shared,

    /// Dedicated listener for a single API
    Isolated,
}

/// Complete listener specification.
///
/// This represents the full configuration for a network listener,
/// including binding, protocol, and security settings.
#[derive(Debug, Clone)]
pub struct ListenerSpec {
    /// Listener name (must be unique)
    pub name: String,

    /// Bind address and port
    pub bind_address: BindAddress,

    /// Network protocol
    pub protocol: Protocol,

    /// Optional TLS configuration
    pub tls_config: Option<TlsConfig>,

    /// Isolation mode
    pub isolation_mode: IsolationMode,

    /// Connection idle timeout in seconds
    pub idle_timeout_seconds: Option<u64>,

    /// Maximum concurrent connections (0 = unlimited)
    pub max_connections: u32,
}

impl ListenerSpec {
    /// Create a basic HTTP listener
    pub fn http(name: impl Into<String>, port: u16) -> Self {
        Self {
            name: name.into(),
            bind_address: BindAddress::ipv4_all(port),
            protocol: Protocol::Http,
            tls_config: None,
            isolation_mode: IsolationMode::Shared,
            idle_timeout_seconds: Some(300),
            max_connections: 0,
        }
    }

    /// Create an HTTPS listener
    pub fn https(
        name: impl Into<String>,
        port: u16,
        certificate_chain: String,
        private_key: String,
    ) -> Self {
        Self {
            name: name.into(),
            bind_address: BindAddress::ipv4_all(port),
            protocol: Protocol::Https,
            tls_config: Some(TlsConfig::server_only(certificate_chain, private_key)),
            isolation_mode: IsolationMode::Shared,
            idle_timeout_seconds: Some(300),
            max_connections: 0,
        }
    }

    /// Set the bind address
    pub fn with_bind_address(mut self, bind_address: BindAddress) -> Self {
        self.bind_address = bind_address;
        self
    }

    /// Set isolation mode
    pub fn with_isolation_mode(mut self, mode: IsolationMode) -> Self {
        self.isolation_mode = mode;
        self
    }

    /// Set idle timeout
    pub fn with_idle_timeout(mut self, seconds: u64) -> Self {
        self.idle_timeout_seconds = Some(seconds);
        self
    }

    /// Set maximum connections
    pub fn with_max_connections(mut self, max: u32) -> Self {
        self.max_connections = max;
        self
    }

    /// Validate the listener configuration
    pub fn validate(&self) -> Result<(), ListenerValidationError> {
        // Check protocol and TLS consistency
        if self.protocol.requires_tls() && self.tls_config.is_none() {
            return Err(ListenerValidationError::TlsRequired);
        }

        // Check port number is valid
        if self.bind_address.port == 0 {
            return Err(ListenerValidationError::InvalidPort);
        }

        // Check name is not empty
        if self.name.is_empty() {
            return Err(ListenerValidationError::EmptyName);
        }

        Ok(())
    }
}

/// Listener validation errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenerValidationError {
    /// TLS configuration required but not provided
    TlsRequired,

    /// Invalid port number
    InvalidPort,

    /// Empty listener name
    EmptyName,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_tls_requirement() {
        assert!(Protocol::Https.requires_tls());
        assert!(!Protocol::Http.requires_tls());
        assert!(!Protocol::Http2.requires_tls());
        assert!(!Protocol::Tcp.requires_tls());
    }

    #[test]
    fn protocol_http_detection() {
        assert!(Protocol::Http.is_http());
        assert!(Protocol::Http2.is_http());
        assert!(Protocol::Https.is_http());
        assert!(!Protocol::Tcp.is_http());
    }

    #[test]
    fn protocol_default_ports() {
        assert_eq!(Protocol::Http.default_port(), 80);
        assert_eq!(Protocol::Http2.default_port(), 80);
        assert_eq!(Protocol::Https.default_port(), 443);
        assert_eq!(Protocol::Tcp.default_port(), 8080);
    }

    #[test]
    fn bind_address_ipv4_all() {
        let bind = BindAddress::ipv4_all(8080);
        assert_eq!(bind.port, 8080);
        assert_eq!(bind.address, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(bind.to_socket_addr_string(), "0.0.0.0:8080");
    }

    #[test]
    fn bind_address_ipv6_all() {
        let bind = BindAddress::ipv6_all(8443);
        assert_eq!(bind.port, 8443);
        assert_eq!(bind.to_socket_addr_string(), "[::]:8443");
    }

    #[test]
    fn bind_address_localhost() {
        let bind = BindAddress::localhost(3000);
        assert_eq!(bind.port, 3000);
        assert_eq!(bind.address, IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(bind.to_socket_addr_string(), "127.0.0.1:3000");
    }

    #[test]
    fn tls_config_server_only() {
        let tls = TlsConfig::server_only("cert".to_string(), "key".to_string());
        assert!(!tls.require_client_cert);
        assert!(tls.ca_certificate.is_none());
        assert_eq!(tls.min_tls_version, TlsVersion::V1_2);
    }

    #[test]
    fn tls_config_mutual() {
        let tls = TlsConfig::mutual_tls("cert".to_string(), "key".to_string(), "ca".to_string());
        assert!(tls.require_client_cert);
        assert!(tls.ca_certificate.is_some());
        assert_eq!(tls.ca_certificate.unwrap(), "ca");
    }

    #[test]
    fn tls_config_with_min_version() {
        let tls = TlsConfig::server_only("cert".to_string(), "key".to_string())
            .with_min_version(TlsVersion::V1_3);
        assert_eq!(tls.min_tls_version, TlsVersion::V1_3);
    }

    #[test]
    fn tls_version_ordering() {
        assert!(TlsVersion::V1_3 > TlsVersion::V1_2);
        assert!(TlsVersion::V1_2 > TlsVersion::V1_1);
        assert!(TlsVersion::V1_1 > TlsVersion::V1_0);
    }

    #[test]
    fn listener_spec_http() {
        let listener = ListenerSpec::http("api-listener", 8080);
        assert_eq!(listener.name, "api-listener");
        assert_eq!(listener.bind_address.port, 8080);
        assert_eq!(listener.protocol, Protocol::Http);
        assert!(listener.tls_config.is_none());
        assert_eq!(listener.isolation_mode, IsolationMode::Shared);
    }

    #[test]
    fn listener_spec_https() {
        let listener = ListenerSpec::https(
            "secure-listener",
            8443,
            "cert-chain".to_string(),
            "private-key".to_string(),
        );
        assert_eq!(listener.name, "secure-listener");
        assert_eq!(listener.bind_address.port, 8443);
        assert_eq!(listener.protocol, Protocol::Https);
        assert!(listener.tls_config.is_some());
    }

    #[test]
    fn listener_spec_builder_pattern() {
        let listener = ListenerSpec::http("test", 8080)
            .with_isolation_mode(IsolationMode::Isolated)
            .with_idle_timeout(600)
            .with_max_connections(1000);

        assert_eq!(listener.isolation_mode, IsolationMode::Isolated);
        assert_eq!(listener.idle_timeout_seconds, Some(600));
        assert_eq!(listener.max_connections, 1000);
    }

    #[test]
    fn listener_validation_success() {
        let listener = ListenerSpec::http("valid", 8080);
        assert!(listener.validate().is_ok());
    }

    #[test]
    fn listener_validation_requires_tls() {
        let listener = ListenerSpec {
            name: "broken".to_string(),
            bind_address: BindAddress::ipv4_all(8443),
            protocol: Protocol::Https,
            tls_config: None,
            isolation_mode: IsolationMode::Shared,
            idle_timeout_seconds: Some(300),
            max_connections: 0,
        };

        assert_eq!(listener.validate(), Err(ListenerValidationError::TlsRequired));
    }

    #[test]
    fn listener_validation_invalid_port() {
        let listener = ListenerSpec {
            name: "test".to_string(),
            bind_address: BindAddress::ipv4_all(0),
            protocol: Protocol::Http,
            tls_config: None,
            isolation_mode: IsolationMode::Shared,
            idle_timeout_seconds: Some(300),
            max_connections: 0,
        };

        assert_eq!(listener.validate(), Err(ListenerValidationError::InvalidPort));
    }

    #[test]
    fn listener_validation_empty_name() {
        let listener = ListenerSpec {
            name: "".to_string(),
            bind_address: BindAddress::ipv4_all(8080),
            protocol: Protocol::Http,
            tls_config: None,
            isolation_mode: IsolationMode::Shared,
            idle_timeout_seconds: Some(300),
            max_connections: 0,
        };

        assert_eq!(listener.validate(), Err(ListenerValidationError::EmptyName));
    }
}
