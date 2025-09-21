//! # Configuration Management
//!
//! This module provides comprehensive configuration management for the Magaya control plane.
//! It supports multiple configuration sources including files, environment variables,
//! and command-line arguments.

pub mod settings;

pub use settings::{AppConfig, DatabaseConfig, ServerConfig, ObservabilityConfig, AuthConfig};

use crate::errors::{MagayaError, Result};
use config::{Config, Environment, File};
use std::path::Path;

/// Load application configuration from multiple sources
///
/// Configuration is loaded in the following order (later sources override earlier ones):
/// 1. Default values
/// 2. Configuration file (if specified)
/// 3. Environment variables with MAGAYA_ prefix
/// 4. Command line arguments (via clap, handled separately)
pub fn load_config<P: AsRef<Path>>(config_path: Option<P>) -> Result<AppConfig> {
    let mut builder = Config::builder();

    // Add default configuration
    builder = builder.add_source(Config::try_from(&AppConfig::default())?);

    // Add configuration file if specified
    if let Some(path) = config_path {
        let path = path.as_ref();
        if path.exists() {
            builder = builder.add_source(File::from(path));
        } else {
            return Err(MagayaError::config(format!(
                "Configuration file not found: {}",
                path.display()
            )));
        }
    }

    // Add environment variables with MAGAYA_ prefix
    builder = builder.add_source(
        Environment::with_prefix("MAGAYA")
            .separator("_")
            .try_parsing(true),
    );

    // Build the configuration
    let config = builder
        .build()
        .map_err(|e| MagayaError::config_with_source("Failed to build configuration", Box::new(e)))?;

    // Deserialize into AppConfig
    let app_config: AppConfig = config
        .try_deserialize()
        .map_err(|e| MagayaError::config_with_source("Failed to deserialize configuration", Box::new(e)))?;

    // Validate the configuration
    app_config.validate()?;

    Ok(app_config)
}

/// Load configuration from environment variables only
/// Useful for containerized deployments
pub fn load_config_from_env() -> Result<AppConfig> {
    load_config::<&str>(None)
}

/// Load configuration from a YAML file
pub fn load_config_from_file<P: AsRef<Path>>(path: P) -> Result<AppConfig> {
    load_config(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_load_default_config() {
        let config = load_config_from_env().unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8080);
    }

    #[test]
    fn test_load_config_from_env() {
        // Set environment variables
        env::set_var("MAGAYA_SERVER_PORT", "9090");
        env::set_var("MAGAYA_DATABASE_URL", "postgresql://test:test@localhost/test");

        let config = load_config_from_env().unwrap();
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.database.url, "postgresql://test:test@localhost/test");

        // Clean up
        env::remove_var("MAGAYA_SERVER_PORT");
        env::remove_var("MAGAYA_DATABASE_URL");
    }

    #[test]
    fn test_load_config_from_file() {
        let yaml_content = r#"
server:
  host: "0.0.0.0"
  port: 8081
database:
  url: "postgresql://localhost/magaya"
  max_connections: 20
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(yaml_content.as_bytes()).unwrap();

        let config = load_config_from_file(temp_file.path()).unwrap();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8081);
        assert_eq!(config.database.max_connections, 20);
    }

    #[test]
    fn test_load_config_nonexistent_file() {
        let result = load_config_from_file("/nonexistent/file.yaml");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Configuration file not found"));
    }

    #[test]
    fn test_config_precedence() {
        // Set an environment variable
        env::set_var("MAGAYA_SERVER_PORT", "7777");

        let yaml_content = r#"
server:
  host: "0.0.0.0"
  port: 8888
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(yaml_content.as_bytes()).unwrap();

        let config = load_config_from_file(temp_file.path()).unwrap();

        // Environment variable should override file
        assert_eq!(config.server.port, 7777);
        // File should override default
        assert_eq!(config.server.host, "0.0.0.0");

        // Clean up
        env::remove_var("MAGAYA_SERVER_PORT");
    }
}