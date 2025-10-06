//! Configuration file handling for Flowplane CLI
//!
//! Manages loading and saving CLI configuration from ~/.flowplane/config.toml
//! and resolving authentication credentials from multiple sources.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::debug;

/// CLI configuration stored in ~/.flowplane/config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// Personal access token
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,

    /// Base URL for the Flowplane API
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Request timeout in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            token: None,
            base_url: None,
            timeout: None,
        }
    }
}

impl CliConfig {
    /// Get the default configuration file path (~/.flowplane/config.toml)
    pub fn config_path() -> Result<PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .context("Unable to determine home directory")?;

        let mut path = PathBuf::from(home);
        path.push(".flowplane");
        path.push("config.toml");

        Ok(path)
    }

    /// Load configuration from the default path
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
    }

    /// Load configuration from a specific path
    pub fn load_from_path(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
    }

    /// Save configuration to the default path
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;

        // Create the .flowplane directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let contents = toml::to_string_pretty(self)
            .context("Failed to serialize configuration")?;

        std::fs::write(&path, contents)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        Ok(())
    }

    /// Save configuration to a specific path
    pub fn save_to_path(&self, path: &PathBuf) -> Result<()> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let contents = toml::to_string_pretty(self)
            .context("Failed to serialize configuration")?;

        std::fs::write(path, contents)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        Ok(())
    }
}

/// Resolve the authentication token from multiple sources
///
/// Checks sources in the following priority order:
/// 1. --token command line flag
/// 2. --token-file command line flag
/// 3. ~/.flowplane/config.toml
/// 4. FLOWPLANE_TOKEN environment variable
pub fn resolve_token(
    token_flag: Option<String>,
    token_file_flag: Option<PathBuf>,
) -> Result<String> {
    // 1. Check --token flag
    if let Some(token) = token_flag {
        debug!("Using token from --token flag");
        return Ok(token);
    }

    // 2. Check --token-file flag
    if let Some(token_file) = token_file_flag {
        debug!("Reading token from file: {}", token_file.display());
        let token = std::fs::read_to_string(&token_file)
            .with_context(|| format!("Failed to read token file: {}", token_file.display()))?
            .trim()
            .to_string();

        if token.is_empty() {
            anyhow::bail!("Token file is empty: {}", token_file.display());
        }

        return Ok(token);
    }

    // 3. Check config file
    if let Ok(config) = CliConfig::load() {
        if let Some(token) = config.token {
            if !token.is_empty() {
                debug!("Using token from config file");
                return Ok(token);
            }
        }
    }

    // 4. Check environment variable
    if let Ok(token) = std::env::var("FLOWPLANE_TOKEN") {
        if !token.is_empty() {
            debug!("Using token from FLOWPLANE_TOKEN environment variable");
            return Ok(token);
        }
    }

    anyhow::bail!(
        "No authentication token found. Please provide a token via:\n\
         - --token flag\n\
         - --token-file flag\n\
         - ~/.flowplane/config.toml\n\
         - FLOWPLANE_TOKEN environment variable"
    )
}

/// Resolve the base URL from multiple sources
///
/// Checks sources in the following priority order:
/// 1. --base-url command line flag
/// 2. ~/.flowplane/config.toml
/// 3. FLOWPLANE_BASE_URL environment variable
/// 4. Default: http://localhost:8080
pub fn resolve_base_url(base_url_flag: Option<String>) -> String {
    // 1. Check --base-url flag
    if let Some(url) = base_url_flag {
        debug!("Using base URL from --base-url flag: {}", url);
        return url;
    }

    // 2. Check config file
    if let Ok(config) = CliConfig::load() {
        if let Some(url) = config.base_url {
            if !url.is_empty() {
                debug!("Using base URL from config file: {}", url);
                return url;
            }
        }
    }

    // 3. Check environment variable
    if let Ok(url) = std::env::var("FLOWPLANE_BASE_URL") {
        if !url.is_empty() {
            debug!("Using base URL from FLOWPLANE_BASE_URL environment variable: {}", url);
            return url;
        }
    }

    // 4. Default
    let default_url = "http://localhost:8080".to_string();
    debug!("Using default base URL: {}", default_url);
    default_url
}

/// Resolve the timeout from multiple sources
///
/// Checks sources in the following priority order:
/// 1. --timeout command line flag
/// 2. ~/.flowplane/config.toml
/// 3. Default: 30 seconds
pub fn resolve_timeout(timeout_flag: Option<u64>) -> u64 {
    // 1. Check --timeout flag
    if let Some(timeout) = timeout_flag {
        debug!("Using timeout from --timeout flag: {} seconds", timeout);
        return timeout;
    }

    // 2. Check config file
    if let Ok(config) = CliConfig::load() {
        if let Some(timeout) = config.timeout {
            debug!("Using timeout from config file: {} seconds", timeout);
            return timeout;
        }
    }

    // 3. Default
    let default_timeout = 30;
    debug!("Using default timeout: {} seconds", default_timeout);
    default_timeout
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_config_default() {
        let config = CliConfig::default();
        assert!(config.token.is_none());
        assert!(config.base_url.is_none());
        assert!(config.timeout.is_none());
    }

    #[test]
    fn test_config_serialization() {
        let config = CliConfig {
            token: Some("test_token".to_string()),
            base_url: Some("http://example.com".to_string()),
            timeout: Some(60),
        };

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("token = \"test_token\""));
        assert!(toml_str.contains("base_url = \"http://example.com\""));
        assert!(toml_str.contains("timeout = 60"));
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            token = "test_token"
            base_url = "http://example.com"
            timeout = 60
        "#;

        let config: CliConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.token, Some("test_token".to_string()));
        assert_eq!(config.base_url, Some("http://example.com".to_string()));
        assert_eq!(config.timeout, Some(60));
    }

    #[test]
    fn test_config_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let config = CliConfig {
            token: Some("test_token".to_string()),
            base_url: Some("http://example.com".to_string()),
            timeout: Some(60),
        };

        // Save
        config.save_to_path(&config_path).unwrap();
        assert!(config_path.exists());

        // Load
        let loaded = CliConfig::load_from_path(&config_path).unwrap();
        assert_eq!(loaded.token, config.token);
        assert_eq!(loaded.base_url, config.base_url);
        assert_eq!(loaded.timeout, config.timeout);
    }

    #[test]
    fn test_config_load_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("nonexistent.toml");

        let loaded = CliConfig::load_from_path(&config_path).unwrap();
        assert!(loaded.token.is_none());
        assert!(loaded.base_url.is_none());
        assert!(loaded.timeout.is_none());
    }
}
