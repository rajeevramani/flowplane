//! # Configuration Management
//!
//! This module provides configuration management for the Flowplane control plane,
//! integrating both simple and comprehensive configuration approaches.

use crate::Result;

pub mod settings;

pub use settings::{
    AppConfig, AuthConfig, DatabaseConfig, ObservabilityConfig, ServerConfig, XdsConfig,
};

/// Simplified configuration for backwards compatibility and development
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub xds: SimpleXdsConfig,
    pub api: ApiServerConfig,
}

/// Simple XDS server configuration for development
#[derive(Debug, Clone)]
pub struct SimpleXdsConfig {
    pub bind_address: String,
    pub port: u16,
    pub resources: XdsResourceConfig,
    pub tls: Option<XdsTlsConfig>,
}

/// TLS configuration for the xDS server
#[derive(Debug, Clone)]
pub struct XdsTlsConfig {
    pub cert_path: String,
    pub key_path: String,
    pub client_ca_path: Option<String>,
    pub require_client_cert: bool,
}

/// Configuration for HTTP API server
#[derive(Debug, Clone)]
pub struct ApiServerConfig {
    pub bind_address: String,
    pub port: u16,
}

/// Configuration for Envoy resources served by XDS
#[derive(Debug, Clone)]
pub struct XdsResourceConfig {
    pub cluster_name: String,
    pub route_name: String,
    pub listener_name: String,
    pub backend_address: String,
    pub backend_port: u16,
    pub listener_port: u16,
}

impl Default for XdsResourceConfig {
    fn default() -> Self {
        Self {
            cluster_name: "demo_cluster".to_string(),
            route_name: "demo_route".to_string(),
            listener_name: "demo_listener".to_string(),
            backend_address: "127.0.0.1".to_string(),
            backend_port: 8080,
            listener_port: 10000,
        }
    }
}

impl Default for SimpleXdsConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0".to_string(),
            port: 18000,
            resources: XdsResourceConfig::default(),
            tls: None,
        }
    }
}

impl Default for ApiServerConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".to_string(),
            port: 8080,
        }
    }
}

impl Config {
    /// Create configuration from environment variables
    pub fn from_env() -> Result<Self> {
        let xds_port_str =
            std::env::var("FLOWPLANE_XDS_PORT").unwrap_or_else(|_| "18000".to_string());

        let xds_port: u16 = xds_port_str.parse().map_err(|e| {
            crate::Error::config(format!("Invalid XDS port '{}': {}", xds_port_str, e))
        })?;

        // Validate port range
        if xds_port == 0 {
            return Err(crate::Error::config("XDS port cannot be 0".to_string()));
        }

        let xds_bind_address =
            std::env::var("FLOWPLANE_XDS_BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0".to_string());

        // Load resource configuration from environment variables
        let cluster_name =
            std::env::var("FLOWPLANE_CLUSTER_NAME").unwrap_or_else(|_| "demo_cluster".to_string());

        let route_name =
            std::env::var("FLOWPLANE_ROUTE_NAME").unwrap_or_else(|_| "demo_route".to_string());

        let listener_name = std::env::var("FLOWPLANE_LISTENER_NAME")
            .unwrap_or_else(|_| "demo_listener".to_string());

        let backend_address =
            std::env::var("FLOWPLANE_BACKEND_ADDRESS").unwrap_or_else(|_| "127.0.0.1".to_string());

        let backend_port_str =
            std::env::var("FLOWPLANE_BACKEND_PORT").unwrap_or_else(|_| "8080".to_string());
        let backend_port: u16 = backend_port_str.parse().map_err(|e| {
            crate::Error::config(format!(
                "Invalid backend port '{}': {}",
                backend_port_str, e
            ))
        })?;

        let listener_port_str =
            std::env::var("FLOWPLANE_LISTENER_PORT").unwrap_or_else(|_| "10000".to_string());
        let listener_port: u16 = listener_port_str.parse().map_err(|e| {
            crate::Error::config(format!(
                "Invalid listener port '{}': {}",
                listener_port_str, e
            ))
        })?;

        // Validate port ranges
        if backend_port == 0 {
            return Err(crate::Error::config("Backend port cannot be 0".to_string()));
        }
        if listener_port == 0 {
            return Err(crate::Error::config(
                "Listener port cannot be 0".to_string(),
            ));
        }

        // API server configuration
        let api_port_str =
            std::env::var("FLOWPLANE_API_PORT").unwrap_or_else(|_| "8080".to_string());
        let api_port: u16 = api_port_str.parse().map_err(|e| {
            crate::Error::config(format!("Invalid API port '{}': {}", api_port_str, e))
        })?;

        if api_port == 0 {
            return Err(crate::Error::config("API port cannot be 0".to_string()));
        }

        let api_bind_address =
            std::env::var("FLOWPLANE_API_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1".to_string());

        Ok(Self {
            xds: SimpleXdsConfig {
                bind_address: xds_bind_address,
                port: xds_port,
                resources: XdsResourceConfig {
                    cluster_name,
                    route_name,
                    listener_name,
                    backend_address,
                    backend_port,
                    listener_port,
                },
                tls: load_xds_tls_config_from_env()?,
            },
            api: ApiServerConfig {
                bind_address: api_bind_address,
                port: api_port,
            },
        })
    }

    /// Convert to comprehensive AppConfig with database support
    pub fn to_app_config(&self) -> AppConfig {
        AppConfig {
            xds: XdsConfig {
                host: self.xds.bind_address.clone(),
                port: self.xds.port,
                enable_mtls: self.xds.tls.is_some(),
                cert_file: self.xds.tls.as_ref().map(|tls| tls.cert_path.clone()),
                key_file: self.xds.tls.as_ref().map(|tls| tls.key_path.clone()),
                ca_file: self
                    .xds
                    .tls
                    .as_ref()
                    .and_then(|tls| tls.client_ca_path.clone()),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

fn load_xds_tls_config_from_env() -> Result<Option<XdsTlsConfig>> {
    let cert_path = match std::env::var("FLOWPLANE_XDS_TLS_CERT_PATH") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return Ok(None),
    };

    let key_path = std::env::var("FLOWPLANE_XDS_TLS_KEY_PATH").map_err(|_| {
        crate::Error::config(
            "FLOWPLANE_XDS_TLS_KEY_PATH must be set when FLOWPLANE_XDS_TLS_CERT_PATH is provided",
        )
    })?;

    let client_ca_path = std::env::var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let require_client_cert = std::env::var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT")
        .ok()
        .map(|value| match value.to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(crate::Error::config(
                "FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT must be a boolean value",
            )),
        })
        .transpose()? // convert Option<Result<bool>> into Result<Option<bool>>
        .unwrap_or(true);

    if require_client_cert && client_ca_path.is_none() {
        return Err(crate::Error::config(
            "Client certificate verification is enabled but FLOWPLANE_XDS_TLS_CLIENT_CA_PATH is not set",
        ));
    }

    Ok(Some(XdsTlsConfig {
        cert_path,
        key_path,
        client_ca_path,
        require_client_cert,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Use a mutex to serialize tests that modify environment variables
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.xds.bind_address, "0.0.0.0");
        assert_eq!(config.xds.port, 18000);
        assert_eq!(config.api.bind_address, "127.0.0.1");
        assert_eq!(config.api.port, 8080);
    }

    #[test]
    fn test_config_from_env() {
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original values to restore later
        let original_port = env::var("FLOWPLANE_XDS_PORT").ok();
        let original_bind = env::var("FLOWPLANE_XDS_BIND_ADDRESS").ok();
        let original_api_port = env::var("FLOWPLANE_API_PORT").ok();
        let original_api_bind = env::var("FLOWPLANE_API_BIND_ADDRESS").ok();

        // Set environment variables
        env::set_var("FLOWPLANE_XDS_PORT", "9090");
        env::set_var("FLOWPLANE_XDS_BIND_ADDRESS", "127.0.0.1");
        env::set_var("FLOWPLANE_API_PORT", "7070");
        env::set_var("FLOWPLANE_API_BIND_ADDRESS", "0.0.0.0");

        let config = Config::from_env().unwrap();
        assert_eq!(config.xds.port, 9090);
        assert_eq!(config.xds.bind_address, "127.0.0.1");
        assert_eq!(config.api.port, 7070);
        assert_eq!(config.api.bind_address, "0.0.0.0");

        // Restore original environment
        match original_port {
            Some(port) => env::set_var("FLOWPLANE_XDS_PORT", port),
            None => env::remove_var("FLOWPLANE_XDS_PORT"),
        }
        match original_bind {
            Some(bind) => env::set_var("FLOWPLANE_XDS_BIND_ADDRESS", bind),
            None => env::remove_var("FLOWPLANE_XDS_BIND_ADDRESS"),
        }
        match original_api_port {
            Some(port) => env::set_var("FLOWPLANE_API_PORT", port),
            None => env::remove_var("FLOWPLANE_API_PORT"),
        }
        match original_api_bind {
            Some(bind) => env::set_var("FLOWPLANE_API_BIND_ADDRESS", bind),
            None => env::remove_var("FLOWPLANE_API_BIND_ADDRESS"),
        }
    }

    #[test]
    fn test_config_from_env_defaults() {
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original values
        let original_port = env::var("FLOWPLANE_XDS_PORT").ok();
        let original_bind = env::var("FLOWPLANE_XDS_BIND_ADDRESS").ok();

        // Ensure no env vars are set
        env::remove_var("FLOWPLANE_XDS_PORT");
        env::remove_var("FLOWPLANE_XDS_BIND_ADDRESS");
        env::remove_var("FLOWPLANE_XDS_TLS_CERT_PATH");
        env::remove_var("FLOWPLANE_XDS_TLS_KEY_PATH");
        env::remove_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH");
        env::remove_var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT");

        let config = Config::from_env().unwrap();
        assert_eq!(config.xds.port, 18000);
        assert_eq!(config.xds.bind_address, "0.0.0.0");
        assert!(config.xds.tls.is_none());

        // Restore original environment
        match original_port {
            Some(port) => env::set_var("FLOWPLANE_XDS_PORT", port),
            None => env::remove_var("FLOWPLANE_XDS_PORT"),
        }
        match original_bind {
            Some(bind) => env::set_var("FLOWPLANE_XDS_BIND_ADDRESS", bind),
            None => env::remove_var("FLOWPLANE_XDS_BIND_ADDRESS"),
        }
    }

    #[test]
    fn test_config_from_env_with_tls() {
        let _guard = ENV_MUTEX.lock().unwrap();

        let original_cert = env::var("FLOWPLANE_XDS_TLS_CERT_PATH").ok();
        let original_key = env::var("FLOWPLANE_XDS_TLS_KEY_PATH").ok();
        let original_ca = env::var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH").ok();
        let original_require = env::var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT").ok();

        env::set_var("FLOWPLANE_XDS_TLS_CERT_PATH", "/tmp/server.pem");
        env::set_var("FLOWPLANE_XDS_TLS_KEY_PATH", "/tmp/server.key");
        env::set_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH", "/tmp/ca.pem");
        env::set_var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT", "true");

        let config = Config::from_env().unwrap();
        let tls = config.xds.tls.expect("TLS config should be populated");
        assert_eq!(tls.cert_path, "/tmp/server.pem");
        assert_eq!(tls.key_path, "/tmp/server.key");
        assert_eq!(tls.client_ca_path.as_deref(), Some("/tmp/ca.pem"));
        assert!(tls.require_client_cert);

        match original_cert {
            Some(value) => env::set_var("FLOWPLANE_XDS_TLS_CERT_PATH", value),
            None => env::remove_var("FLOWPLANE_XDS_TLS_CERT_PATH"),
        }
        match original_key {
            Some(value) => env::set_var("FLOWPLANE_XDS_TLS_KEY_PATH", value),
            None => env::remove_var("FLOWPLANE_XDS_TLS_KEY_PATH"),
        }
        match original_ca {
            Some(value) => env::set_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH", value),
            None => env::remove_var("FLOWPLANE_XDS_TLS_CLIENT_CA_PATH"),
        }
        match original_require {
            Some(value) => env::set_var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT", value),
            None => env::remove_var("FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT"),
        }
    }
}
