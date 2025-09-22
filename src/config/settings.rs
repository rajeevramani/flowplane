//! # Configuration Settings
//!
//! Defines the configuration structure for the Magaya control plane.

use crate::errors::{MagayaError, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use validator::Validate;

/// Main application configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate, Default)]
pub struct AppConfig {
    /// Server configuration
    #[validate(nested)]
    pub server: ServerConfig,

    /// Database configuration
    #[validate(nested)]
    pub database: DatabaseConfig,

    /// Observability configuration
    #[validate(nested)]
    pub observability: ObservabilityConfig,

    /// Authentication configuration
    #[validate(nested)]
    pub auth: AuthConfig,

    /// xDS server configuration
    #[validate(nested)]
    pub xds: XdsConfig,
}

// impl Default for AppConfig {
//     fn default() -> Self {
//         Self {
//             server: ServerConfig::default(),
//             database: DatabaseConfig::default(),
//             observability: ObservabilityConfig::default(),
//             auth: AuthConfig::default(),
//             xds: XdsConfig::default(),
//         }
//     }
// }

impl AppConfig {
    /// Validate the entire configuration
    pub fn validate(&self) -> Result<()> {
        // Use validator crate for basic validation
        Validate::validate(self).map_err(MagayaError::from)?;

        // Custom validation logic
        self.validate_custom()?;

        Ok(())
    }

    /// Custom validation logic that goes beyond what the validator crate can do
    fn validate_custom(&self) -> Result<()> {
        // Validate that ports don't conflict
        if self.server.port == self.xds.port {
            return Err(MagayaError::validation(
                "Server and xDS ports cannot be the same",
            ));
        }

        // Validate database URL format
        if !self.database.url.starts_with("postgresql://")
            && !self.database.url.starts_with("sqlite://")
        {
            return Err(MagayaError::validation(
                "Database URL must start with 'postgresql://' or 'sqlite://'",
            ));
        }

        // Validate JWT secret length
        if self.auth.jwt_secret.len() < 32 {
            return Err(MagayaError::validation(
                "JWT secret must be at least 32 characters long",
            ));
        }

        Ok(())
    }
}

/// HTTP server configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ServerConfig {
    /// Server bind address
    #[validate(length(min = 1, message = "Host cannot be empty"))]
    pub host: String,

    /// Server port
    #[validate(range(min = 1, max = 65535, message = "Port must be between 1 and 65535"))]
    pub port: u16,

    /// Request timeout in seconds
    #[validate(range(
        min = 1,
        max = 300,
        message = "Timeout must be between 1 and 300 seconds"
    ))]
    pub timeout_seconds: u64,

    /// Maximum request body size in bytes
    #[validate(range(min = 1024, message = "Max body size must be at least 1KB"))]
    pub max_body_size: usize,

    /// Enable CORS
    pub enable_cors: bool,

    /// CORS allowed origins (empty = allow all)
    pub cors_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            timeout_seconds: 30,
            max_body_size: 1024 * 1024, // 1MB
            enable_cors: true,
            cors_origins: vec![],
        }
    }
}

impl ServerConfig {
    /// Get the server bind address
    pub fn bind_address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Get request timeout as Duration
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct DatabaseConfig {
    /// Database connection URL
    #[validate(length(min = 1, message = "Database URL cannot be empty"))]
    pub url: String,

    /// Maximum number of connections in the pool
    #[validate(range(
        min = 1,
        max = 100,
        message = "Max connections must be between 1 and 100"
    ))]
    pub max_connections: u32,

    /// Minimum number of connections in the pool
    #[validate(range(
        min = 0,
        max = 50,
        message = "Min connections must be between 0 and 50"
    ))]
    pub min_connections: u32,

    /// Connection timeout in seconds
    #[validate(range(
        min = 1,
        max = 60,
        message = "Connect timeout must be between 1 and 60 seconds"
    ))]
    pub connect_timeout_seconds: u64,

    /// Idle timeout in seconds (0 = no timeout)
    pub idle_timeout_seconds: u64,

    /// Enable automatic migrations
    pub auto_migrate: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite://./data/magaya.db".to_string(),
            max_connections: 10,
            min_connections: 0,
            connect_timeout_seconds: 10,
            idle_timeout_seconds: 600, // 10 minutes
            auto_migrate: true,
        }
    }
}

impl DatabaseConfig {
    /// Get connection timeout as Duration
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_seconds)
    }

    /// Get idle timeout as Duration (None if 0)
    pub fn idle_timeout(&self) -> Option<Duration> {
        if self.idle_timeout_seconds == 0 {
            None
        } else {
            Some(Duration::from_secs(self.idle_timeout_seconds))
        }
    }

    /// Check if this is a SQLite configuration
    pub fn is_sqlite(&self) -> bool {
        self.url.starts_with("sqlite://")
    }

    /// Check if this is a PostgreSQL configuration
    pub fn is_postgresql(&self) -> bool {
        self.url.starts_with("postgresql://")
    }

    /// Create DatabaseConfig from environment variables
    pub fn from_env() -> Self {
        let url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://./data/magaya.db".to_string());

        let max_connections = std::env::var("DATABASE_MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(10);

        let min_connections = std::env::var("DATABASE_MIN_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        let connect_timeout_seconds = std::env::var("DATABASE_CONNECT_TIMEOUT_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(10);

        let idle_timeout_seconds = std::env::var("DATABASE_IDLE_TIMEOUT_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(600);

        let auto_migrate = std::env::var("DATABASE_AUTO_MIGRATE")
            .map(|s| s.to_lowercase() == "true" || s == "1")
            .unwrap_or(true);

        Self {
            url,
            max_connections,
            min_connections,
            connect_timeout_seconds,
            idle_timeout_seconds,
            auto_migrate,
        }
    }
}

/// Observability configuration for metrics, tracing, and health checks
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ObservabilityConfig {
    /// Enable metrics collection
    pub enable_metrics: bool,

    /// Metrics server port (0 = disabled)
    #[validate(range(max = 65535, message = "Metrics port must be <= 65535"))]
    pub metrics_port: u16,

    /// Enable distributed tracing
    pub enable_tracing: bool,

    /// Jaeger collector endpoint
    pub jaeger_endpoint: Option<String>,

    /// Tracing service name
    #[validate(length(min = 1, message = "Service name cannot be empty"))]
    pub service_name: String,

    /// Log level (trace, debug, info, warn, error)
    #[validate(length(min = 1, message = "Log level cannot be empty"))]
    pub log_level: String,

    /// Enable JSON structured logging
    pub json_logging: bool,

    /// Health check interval in seconds
    #[validate(range(
        min = 1,
        max = 300,
        message = "Health check interval must be between 1 and 300 seconds"
    ))]
    pub health_check_interval_seconds: u64,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            enable_metrics: true,
            metrics_port: 9090,
            enable_tracing: true,
            jaeger_endpoint: Some("http://localhost:14268/api/traces".to_string()),
            service_name: "magaya".to_string(),
            log_level: "info".to_string(),
            json_logging: false,
            health_check_interval_seconds: 30,
        }
    }
}

impl ObservabilityConfig {
    /// Get health check interval as Duration
    pub fn health_check_interval(&self) -> Duration {
        Duration::from_secs(self.health_check_interval_seconds)
    }

    /// Get metrics bind address (None if disabled)
    pub fn metrics_bind_address(&self) -> Option<String> {
        if self.metrics_port == 0 {
            None
        } else {
            Some(format!("0.0.0.0:{}", self.metrics_port))
        }
    }
}

/// Authentication and authorization configuration
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AuthConfig {
    /// Enable authentication
    pub enable_auth: bool,

    /// JWT secret for token signing/verification
    #[validate(length(min = 1, message = "JWT secret cannot be empty"))]
    pub jwt_secret: String,

    /// JWT token expiry in seconds
    #[validate(range(
        min = 300,
        max = 86400,
        message = "Token expiry must be between 5 minutes and 24 hours"
    ))]
    pub token_expiry_seconds: u64,

    /// JWT issuer
    #[validate(length(min = 1, message = "JWT issuer cannot be empty"))]
    pub jwt_issuer: String,

    /// JWT audience
    #[validate(length(min = 1, message = "JWT audience cannot be empty"))]
    pub jwt_audience: String,

    /// Enable role-based access control
    pub enable_rbac: bool,

    /// Default user role for new users
    #[validate(length(min = 1, message = "Default role cannot be empty"))]
    pub default_role: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enable_auth: true,
            jwt_secret: "magaya-default-secret-please-change-in-production".to_string(),
            token_expiry_seconds: 3600, // 1 hour
            jwt_issuer: "magaya".to_string(),
            jwt_audience: "magaya-api".to_string(),
            enable_rbac: true,
            default_role: "user".to_string(),
        }
    }
}

impl AuthConfig {
    /// Get token expiry as Duration
    pub fn token_expiry(&self) -> Duration {
        Duration::from_secs(self.token_expiry_seconds)
    }
}

/// xDS server configuration for Envoy communication
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct XdsConfig {
    /// xDS server bind address
    #[validate(length(min = 1, message = "xDS host cannot be empty"))]
    pub host: String,

    /// xDS server port
    #[validate(range(min = 1, max = 65535, message = "xDS port must be between 1 and 65535"))]
    pub port: u16,

    /// Enable mTLS for xDS connections
    pub enable_mtls: bool,

    /// Path to TLS certificate file
    pub cert_file: Option<String>,

    /// Path to TLS private key file
    pub key_file: Option<String>,

    /// Path to CA certificate file for client verification
    pub ca_file: Option<String>,

    /// Node discovery cache TTL in seconds
    #[validate(range(
        min = 1,
        max = 3600,
        message = "Cache TTL must be between 1 and 3600 seconds"
    ))]
    pub cache_ttl_seconds: u64,

    /// Maximum concurrent xDS streams
    #[validate(range(
        min = 1,
        max = 10000,
        message = "Max streams must be between 1 and 10000"
    ))]
    pub max_concurrent_streams: u32,
}

impl Default for XdsConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 18000,
            enable_mtls: false,
            cert_file: None,
            key_file: None,
            ca_file: None,
            cache_ttl_seconds: 300, // 5 minutes
            max_concurrent_streams: 1000,
        }
    }
}

impl XdsConfig {
    /// Get the xDS server bind address
    pub fn bind_address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Get cache TTL as Duration
    pub fn cache_ttl(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_seconds)
    }

    /// Check if TLS is properly configured
    pub fn has_tls_config(&self) -> bool {
        self.cert_file.is_some() && self.key_file.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_validation() {
        let config = AppConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_server_config_bind_address() {
        let config = ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8080,
            ..Default::default()
        };
        assert_eq!(config.bind_address(), "0.0.0.0:8080");
    }

    #[test]
    fn test_server_config_timeout() {
        let config = ServerConfig {
            timeout_seconds: 45,
            ..Default::default()
        };
        assert_eq!(config.timeout(), Duration::from_secs(45));
    }

    #[test]
    fn test_database_config_timeouts() {
        let config = DatabaseConfig {
            connect_timeout_seconds: 15,
            idle_timeout_seconds: 300,
            ..Default::default()
        };
        assert_eq!(config.connect_timeout(), Duration::from_secs(15));
        assert_eq!(config.idle_timeout(), Some(Duration::from_secs(300)));

        let config_no_idle = DatabaseConfig {
            idle_timeout_seconds: 0,
            ..Default::default()
        };
        assert_eq!(config_no_idle.idle_timeout(), None);
    }

    #[test]
    fn test_database_config_type_detection() {
        let sqlite_config = DatabaseConfig {
            url: "sqlite://./test.db".to_string(),
            ..Default::default()
        };
        assert!(sqlite_config.is_sqlite());
        assert!(!sqlite_config.is_postgresql());

        let pg_config = DatabaseConfig {
            url: "postgresql://localhost/test".to_string(),
            ..Default::default()
        };
        assert!(!pg_config.is_sqlite());
        assert!(pg_config.is_postgresql());
    }

    #[test]
    fn test_observability_config_metrics_address() {
        let config = ObservabilityConfig {
            metrics_port: 9090,
            ..Default::default()
        };
        assert_eq!(
            config.metrics_bind_address(),
            Some("0.0.0.0:9090".to_string())
        );

        let disabled_config = ObservabilityConfig {
            metrics_port: 0,
            ..Default::default()
        };
        assert_eq!(disabled_config.metrics_bind_address(), None);
    }

    #[test]
    fn test_auth_config_token_expiry() {
        let config = AuthConfig {
            token_expiry_seconds: 7200,
            ..Default::default()
        };
        assert_eq!(config.token_expiry(), Duration::from_secs(7200));
    }

    #[test]
    fn test_xds_config() {
        let config = XdsConfig {
            host: "localhost".to_string(),
            port: 18000,
            cert_file: Some("/path/to/cert.pem".to_string()),
            key_file: Some("/path/to/key.pem".to_string()),
            ..Default::default()
        };

        assert_eq!(config.bind_address(), "localhost:18000");
        assert!(config.has_tls_config());

        let no_tls_config = XdsConfig::default();
        assert!(!no_tls_config.has_tls_config());
    }

    #[test]
    fn test_config_validation_errors() {
        // Test port conflict
        let mut config = AppConfig::default();
        config.server.port = 8080;
        config.xds.port = 8080;
        assert!(config.validate().is_err());

        // Test invalid database URL
        let mut config = AppConfig::default();
        config.database.url = "invalid://url".to_string();
        assert!(config.validate().is_err());

        // Test short JWT secret
        let mut config = AppConfig::default();
        config.auth.jwt_secret = "short".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_ranges() {
        let mut config = AppConfig::default();

        // Test invalid port
        config.server.port = 0;
        assert!(config.validate().is_err());

        // config.server.port = 70000;
        // assert!(config.validate().is_err());

        // Test invalid max connections
        config = AppConfig::default();
        config.database.max_connections = 0;
        assert!(config.validate().is_err());

        config.database.max_connections = 200;
        assert!(config.validate().is_err());
    }
}
