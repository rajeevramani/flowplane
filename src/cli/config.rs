//! Configuration file handling for Flowplane CLI
//!
//! Manages loading and saving CLI configuration from ~/.flowplane/config.toml
//! and resolving authentication credentials from multiple sources.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Resolved paths for CLI configuration files.
///
/// In production: derived from `$HOME`. In tests: derived from a `TempDir`.
#[derive(Debug, Clone)]
pub struct CliConfigPaths {
    pub config_path: PathBuf,
    pub credentials_path: PathBuf,
    pub flowplane_dir: PathBuf,
}

impl CliConfigPaths {
    /// Production paths from `$HOME` (or `USERPROFILE` on Windows).
    pub fn from_home() -> Result<Self> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .context("Unable to determine home directory")?;
        Self::from_base(PathBuf::from(home))
    }

    /// Paths rooted at an arbitrary base directory (e.g. a `TempDir` in tests).
    pub fn from_base(base: impl Into<PathBuf>) -> Result<Self> {
        let fp_dir = base.into().join(".flowplane");
        Ok(Self {
            config_path: fp_dir.join("config.toml"),
            credentials_path: fp_dir.join("credentials"),
            flowplane_dir: fp_dir,
        })
    }
}

/// CLI configuration stored in ~/.flowplane/config.toml
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

    /// Default team context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,

    /// Default organization context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,

    /// OIDC issuer URL (set after `auth login`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oidc_issuer: Option<String>,

    /// OIDC client ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oidc_client_id: Option<String>,

    /// Callback URL for OIDC PKCE login
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
}

impl CliConfig {
    /// Get the default configuration file path (~/.flowplane/config.toml)
    pub fn config_path() -> Result<PathBuf> {
        CliConfigPaths::from_home().map(|p| p.config_path)
    }

    /// Get the credentials file path (~/.flowplane/credentials)
    pub fn credentials_path() -> Result<PathBuf> {
        CliConfigPaths::from_home().map(|p| p.credentials_path)
    }

    /// Load configuration from the default `$HOME`-based paths.
    pub fn load() -> Result<Self> {
        let paths = CliConfigPaths::from_home()?;
        Self::load_from_paths(&paths)
    }

    /// Load configuration from resolved [`CliConfigPaths`].
    pub fn load_from_paths(paths: &CliConfigPaths) -> Result<Self> {
        Self::load_from_path(&paths.config_path)
    }

    /// Load configuration from a specific file path.
    pub fn load_from_path(path: &(impl AsRef<Path> + ?Sized)) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
    }

    /// Save configuration to the default `$HOME`-based paths.
    pub fn save(&self) -> Result<()> {
        let paths = CliConfigPaths::from_home()?;
        self.save_to_paths(&paths)
    }

    /// Save configuration to resolved [`CliConfigPaths`].
    pub fn save_to_paths(&self, paths: &CliConfigPaths) -> Result<()> {
        self.save_to_path(&paths.config_path)
    }

    /// Save configuration to a specific file path.
    pub fn save_to_path(&self, path: &(impl AsRef<Path> + ?Sized)) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let contents = toml::to_string_pretty(self).context("Failed to serialize configuration")?;

        std::fs::write(path, &contents)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        // Restrict permissions to owner-only (0600) since config may contain sensitive data
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms)
                .with_context(|| format!("Failed to set permissions on: {}", path.display()))?;
        }

        Ok(())
    }
}

/// Resolve the authentication token from multiple sources
///
/// Checks sources in the following priority order:
/// 1. --token command line flag
/// 2. --token-file command line flag
/// 3. ~/.flowplane/credentials file
/// 4. ~/.flowplane/config.toml (deprecated, emits warning)
/// 5. FLOWPLANE_TOKEN environment variable
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

    // 3. Check credentials file (supports both OIDC JSON and plain-text formats)
    if let Ok(creds_path) = CliConfig::credentials_path() {
        if creds_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&creds_path) {
                let trimmed = contents.trim().to_string();
                if !trimmed.is_empty() {
                    // Try JSON format first (OIDC credentials)
                    if trimmed.starts_with('{') {
                        if let Ok(creds) =
                            serde_json::from_str::<super::auth::OidcCredentials>(&trimmed)
                        {
                            debug!("Using token from OIDC credentials file");
                            return Ok(creds.access_token);
                        }
                    }
                    // Fall back to plain-text token
                    debug!("Using token from credentials file");
                    return Ok(trimmed);
                }
            }
        }
    }

    // 4. Check config file (deprecated)
    if let Ok(config) = CliConfig::load() {
        if let Some(token) = config.token {
            if !token.is_empty() {
                warn!(
                    "Token found in config.toml is deprecated. \
                     Move to ~/.flowplane/credentials or run 'flowplane init' to regenerate."
                );
                debug!("Using token from config file (deprecated)");
                return Ok(token);
            }
        }
    }

    // 5. Check environment variable
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
         - ~/.flowplane/credentials\n\
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

/// Resolve the team from multiple sources
///
/// Checks sources in the following priority order:
/// 1. --team command line flag
/// 2. ~/.flowplane/config.toml
/// 3. FLOWPLANE_TEAM environment variable
/// 4. Error if none found
pub fn resolve_team(team_flag: Option<String>) -> Result<String> {
    // 1. Check --team flag
    if let Some(team) = team_flag {
        debug!("Using team from --team flag: {}", team);
        return Ok(team);
    }

    // 2. Check config file
    if let Ok(config) = CliConfig::load() {
        if let Some(team) = config.team {
            if !team.is_empty() {
                debug!("Using team from config file: {}", team);
                return Ok(team);
            }
        }
    }

    // 3. Check environment variable
    if let Ok(team) = std::env::var("FLOWPLANE_TEAM") {
        if !team.is_empty() {
            debug!("Using team from FLOWPLANE_TEAM environment variable: {}", team);
            return Ok(team);
        }
    }

    anyhow::bail!(
        "No team specified. Please provide a team via:\n\
         - --team flag\n\
         - ~/.flowplane/config.toml (team field)\n\
         - FLOWPLANE_TEAM environment variable"
    )
}

/// Resolve the organization from multiple sources
///
/// Checks sources in the following priority order:
/// 1. --org command line flag
/// 2. ~/.flowplane/config.toml
/// 3. FLOWPLANE_ORG environment variable
/// 4. Error if none found
pub fn resolve_org(org_flag: Option<String>) -> Result<String> {
    // 1. Check --org flag
    if let Some(org) = org_flag {
        debug!("Using org from --org flag: {}", org);
        return Ok(org);
    }

    // 2. Check config file
    if let Ok(config) = CliConfig::load() {
        if let Some(org) = config.org {
            if !org.is_empty() {
                debug!("Using org from config file: {}", org);
                return Ok(org);
            }
        }
    }

    // 3. Check environment variable
    if let Ok(org) = std::env::var("FLOWPLANE_ORG") {
        if !org.is_empty() {
            debug!("Using org from FLOWPLANE_ORG environment variable: {}", org);
            return Ok(org);
        }
    }

    anyhow::bail!(
        "No organization specified. Please provide an organization via:\n\
         - --org flag\n\
         - ~/.flowplane/config.toml (org field)\n\
         - FLOWPLANE_ORG environment variable"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Serialize tests that mutate shared env vars (HOME, FLOWPLANE_TOKEN, etc.)
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_config_default() {
        let config = CliConfig::default();
        assert!(config.token.is_none());
        assert!(config.base_url.is_none());
        assert!(config.timeout.is_none());
        assert!(config.team.is_none());
        assert!(config.org.is_none());
    }

    #[test]
    fn test_config_serialization() {
        let config = CliConfig {
            token: Some("fake-token-for-testing".to_string()),
            base_url: Some("http://example.com".to_string()),
            timeout: Some(60),
            team: Some("engineering".to_string()),
            org: Some("acme-corp".to_string()),
            oidc_issuer: None,
            oidc_client_id: None,
            callback_url: None,
        };

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("token = \"fake-token-for-testing\""));
        assert!(toml_str.contains("base_url = \"http://example.com\""));
        assert!(toml_str.contains("timeout = 60"));
        assert!(toml_str.contains("team = \"engineering\""));
        assert!(toml_str.contains("org = \"acme-corp\""));
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            token = "fake-token-for-testing"
            base_url = "http://example.com"
            timeout = 60
            team = "engineering"
            org = "acme-corp"
        "#;

        let config: CliConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.token, Some("fake-token-for-testing".to_string()));
        assert_eq!(config.base_url, Some("http://example.com".to_string()));
        assert_eq!(config.timeout, Some(60));
        assert_eq!(config.team, Some("engineering".to_string()));
        assert_eq!(config.org, Some("acme-corp".to_string()));
    }

    #[test]
    fn test_config_deserialization_without_new_fields() {
        // Ensure backward compatibility: config files without team/org still parse
        let toml_str = r#"
            token = "fake-token-for-testing"
            base_url = "http://example.com"
            timeout = 60
        "#;

        let config: CliConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.token, Some("fake-token-for-testing".to_string()));
        assert!(config.team.is_none());
        assert!(config.org.is_none());
    }

    #[test]
    fn test_config_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let config = CliConfig {
            token: Some("fake-token-for-testing".to_string()),
            base_url: Some("http://example.com".to_string()),
            timeout: Some(60),
            team: Some("engineering".to_string()),
            org: Some("acme-corp".to_string()),
            oidc_issuer: None,
            oidc_client_id: None,
            callback_url: None,
        };

        // Save
        config.save_to_path(&config_path).unwrap();
        assert!(config_path.exists());

        // Load
        let loaded = CliConfig::load_from_path(&config_path).unwrap();
        assert_eq!(loaded.token, config.token);
        assert_eq!(loaded.base_url, config.base_url);
        assert_eq!(loaded.timeout, config.timeout);
        assert_eq!(loaded.team, config.team);
        assert_eq!(loaded.org, config.org);
    }

    #[test]
    fn test_config_load_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("nonexistent.toml");

        let loaded = CliConfig::load_from_path(&config_path).unwrap();
        assert!(loaded.token.is_none());
        assert!(loaded.base_url.is_none());
        assert!(loaded.timeout.is_none());
        assert!(loaded.team.is_none());
        assert!(loaded.org.is_none());
    }

    #[test]
    fn test_credentials_path() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path());

        let path = CliConfig::credentials_path().unwrap();

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }

        assert!(path.ends_with(".flowplane/credentials"));
    }

    // T7a: resolve_team() flag takes highest priority
    #[test]
    fn test_resolve_team_flag_priority() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // Set env to verify flag wins
        std::env::set_var("FLOWPLANE_TEAM", "env-team");
        let result = resolve_team(Some("flag-team".to_string()));
        std::env::remove_var("FLOWPLANE_TEAM");
        assert_eq!(result.unwrap(), "flag-team");
    }

    // T7b: resolve_team() config fallback
    #[test]
    fn test_resolve_team_config_fallback() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let config = CliConfig { team: Some("config-team".to_string()), ..Default::default() };
        config.save_to_path(&config_path).unwrap();

        // Override HOME so CliConfig::load() reads our temp config
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path().join(".flowplane").parent().unwrap());

        // Create .flowplane dir structure in temp
        let fp_dir = temp_dir.path().join(".flowplane");
        std::fs::create_dir_all(&fp_dir).unwrap();
        std::fs::copy(&config_path, fp_dir.join("config.toml")).unwrap();
        std::env::set_var("HOME", temp_dir.path());
        std::env::remove_var("FLOWPLANE_TEAM");

        let result = resolve_team(None);

        // Restore HOME
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }

        assert_eq!(result.unwrap(), "config-team");
    }

    // T7c: resolve_team() env var fallback
    #[test]
    fn test_resolve_team_env_fallback() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // Point HOME to a temp dir with no config
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path());
        std::env::set_var("FLOWPLANE_TEAM", "env-team");

        let result = resolve_team(None);

        std::env::remove_var("FLOWPLANE_TEAM");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }

        assert_eq!(result.unwrap(), "env-team");
    }

    // T7d: resolve_team() all absent returns error
    #[test]
    fn test_resolve_team_all_absent_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path());
        std::env::remove_var("FLOWPLANE_TEAM");

        let result = resolve_team(None);

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No team specified"));
    }

    // T7e: resolve_org() same priority chain
    #[test]
    fn test_resolve_org_flag_priority() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("FLOWPLANE_ORG", "env-org");
        let result = resolve_org(Some("flag-org".to_string()));
        std::env::remove_var("FLOWPLANE_ORG");
        assert_eq!(result.unwrap(), "flag-org");
    }

    #[test]
    fn test_resolve_org_env_fallback() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path());
        std::env::set_var("FLOWPLANE_ORG", "env-org");

        let result = resolve_org(None);

        std::env::remove_var("FLOWPLANE_ORG");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }

        assert_eq!(result.unwrap(), "env-org");
    }

    #[test]
    fn test_resolve_org_all_absent_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path());
        std::env::remove_var("FLOWPLANE_ORG");

        let result = resolve_org(None);

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No organization specified"));
    }

    // T7f: resolve_token() prefers credentials file over config.toml
    #[test]
    fn test_resolve_token_prefers_credentials_file() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path());
        std::env::remove_var("FLOWPLANE_TOKEN");

        // Set up credentials file
        let fp_dir = temp_dir.path().join(".flowplane");
        std::fs::create_dir_all(&fp_dir).unwrap();
        std::fs::write(fp_dir.join("credentials"), "creds-token\n").unwrap();

        // Also set up config.toml with a different token
        let config = CliConfig { token: Some("config-token".to_string()), ..Default::default() };
        config.save_to_path(&fp_dir.join("config.toml")).unwrap();

        let result = resolve_token(None, None);

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }

        assert_eq!(result.unwrap(), "creds-token");
    }

    // T7g: resolve_token() config.toml fallback emits deprecation warning
    #[test]
    fn test_resolve_token_config_fallback_works() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path());
        std::env::remove_var("FLOWPLANE_TOKEN");

        // Set up config.toml with token but NO credentials file
        let fp_dir = temp_dir.path().join(".flowplane");
        std::fs::create_dir_all(&fp_dir).unwrap();
        let config = CliConfig { token: Some("config-token".to_string()), ..Default::default() };
        config.save_to_path(&fp_dir.join("config.toml")).unwrap();

        // The deprecation warning is emitted via tracing::warn, which we
        // can't easily capture here — but we verify the fallback works
        let result = resolve_token(None, None);

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }

        assert_eq!(result.unwrap(), "config-token");
    }

    // T7h: config set unknown-key is rejected
    #[test]
    fn test_config_set_unknown_key_rejected() {
        use super::super::config_cmd::validate_config_key;

        assert!(validate_config_key("token").is_ok());
        assert!(validate_config_key("base_url").is_ok());
        assert!(validate_config_key("timeout").is_ok());
        assert!(validate_config_key("team").is_ok());
        assert!(validate_config_key("org").is_ok());

        let result = validate_config_key("unknown-key");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unknown configuration key"));
    }
}
