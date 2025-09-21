//! # Configuration Management
//!
//! This module provides minimal configuration management for the Magaya control plane.
//! For Checkpoint 1, we only need basic configuration support.

use crate::Result;

/// Minimal application configuration for Checkpoint 1
#[derive(Debug, Clone)]
pub struct Config {
    pub xds: XdsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            xds: XdsConfig::default(),
        }
    }
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
        let xds_port = std::env::var("MAGAYA_XDS_PORT")
            .unwrap_or_else(|_| "18000".to_string())
            .parse()
            .map_err(|e| crate::Error::config(format!("Invalid XDS port: {}", e)))?;

        let xds_bind_address = std::env::var("MAGAYA_XDS_BIND_ADDRESS")
            .unwrap_or_else(|_| "0.0.0.0".to_string());

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

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.xds.bind_address, "0.0.0.0");
        assert_eq!(config.xds.port, 18000);
    }

    #[test]
    fn test_config_from_env() {
        // Set environment variables
        env::set_var("MAGAYA_XDS_PORT", "9090");
        env::set_var("MAGAYA_XDS_BIND_ADDRESS", "127.0.0.1");

        let config = Config::from_env().unwrap();
        assert_eq!(config.xds.port, 9090);
        assert_eq!(config.xds.bind_address, "127.0.0.1");

        // Clean up
        env::remove_var("MAGAYA_XDS_PORT");
        env::remove_var("MAGAYA_XDS_BIND_ADDRESS");
    }

    #[test]
    fn test_config_from_env_defaults() {
        // Ensure no env vars are set
        env::remove_var("MAGAYA_XDS_PORT");
        env::remove_var("MAGAYA_XDS_BIND_ADDRESS");

        let config = Config::from_env().unwrap();
        assert_eq!(config.xds.port, 18000);
        assert_eq!(config.xds.bind_address, "0.0.0.0");
    }
}