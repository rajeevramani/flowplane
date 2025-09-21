//! # Configuration Management
//!
//! This module provides minimal configuration management for the Magaya control plane.
//! For Checkpoint 1, we only need basic configuration support.

use crate::Result;

/// Minimal application configuration for Checkpoint 1
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub xds: XdsConfig,
}

/// XDS server configuration
#[derive(Debug, Clone)]
pub struct XdsConfig {
    pub bind_address: String,
    pub port: u16,
}

impl Default for XdsConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0".to_string(),
            port: 18000,
        }
    }
}

impl Config {
    /// Create configuration from environment variables
    pub fn from_env() -> Result<Self> {
        let xds_port_str = std::env::var("MAGAYA_XDS_PORT").unwrap_or_else(|_| "18000".to_string());

        let xds_port: u16 = xds_port_str.parse().map_err(|e| {
            crate::Error::config(format!("Invalid XDS port '{}': {}", xds_port_str, e))
        })?;

        // Validate port range
        if xds_port == 0 {
            return Err(crate::Error::config("XDS port cannot be 0".to_string()));
        }

        let xds_bind_address =
            std::env::var("MAGAYA_XDS_BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0".to_string());

        Ok(Self {
            xds: XdsConfig {
                bind_address: xds_bind_address,
                port: xds_port,
            },
        })
    }
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
    }

    #[test]
    fn test_config_from_env() {
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original values to restore later
        let original_port = env::var("MAGAYA_XDS_PORT").ok();
        let original_bind = env::var("MAGAYA_XDS_BIND_ADDRESS").ok();

        // Set environment variables
        env::set_var("MAGAYA_XDS_PORT", "9090");
        env::set_var("MAGAYA_XDS_BIND_ADDRESS", "127.0.0.1");

        let config = Config::from_env().unwrap();
        assert_eq!(config.xds.port, 9090);
        assert_eq!(config.xds.bind_address, "127.0.0.1");

        // Restore original environment
        match original_port {
            Some(port) => env::set_var("MAGAYA_XDS_PORT", port),
            None => env::remove_var("MAGAYA_XDS_PORT"),
        }
        match original_bind {
            Some(bind) => env::set_var("MAGAYA_XDS_BIND_ADDRESS", bind),
            None => env::remove_var("MAGAYA_XDS_BIND_ADDRESS"),
        }
    }

    #[test]
    fn test_config_from_env_defaults() {
        let _guard = ENV_MUTEX.lock().unwrap();

        // Save original values
        let original_port = env::var("MAGAYA_XDS_PORT").ok();
        let original_bind = env::var("MAGAYA_XDS_BIND_ADDRESS").ok();

        // Ensure no env vars are set
        env::remove_var("MAGAYA_XDS_PORT");
        env::remove_var("MAGAYA_XDS_BIND_ADDRESS");

        let config = Config::from_env().unwrap();
        assert_eq!(config.xds.port, 18000);
        assert_eq!(config.xds.bind_address, "0.0.0.0");

        // Restore original environment
        match original_port {
            Some(port) => env::set_var("MAGAYA_XDS_PORT", port),
            None => env::remove_var("MAGAYA_XDS_PORT"),
        }
        match original_bind {
            Some(bind) => env::set_var("MAGAYA_XDS_BIND_ADDRESS", bind),
            None => env::remove_var("MAGAYA_XDS_BIND_ADDRESS"),
        }
    }
}
