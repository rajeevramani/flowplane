//! Tests for CLI config commands and config key management.
//!
//! Covers:
//! - Config init / save / load round-trips via CliConfigPaths::from_base(TempDir)
//! - All config keys including Phase 1+2 OIDC keys (oidc_issuer, oidc_client_id, callback_url)
//! - Unknown key rejection
//! - Config file permissions on Unix
//! - Dockerfile.backend CMD verification
//! - `flowplane serve` subcommand parsing

use flowplane::cli::config::{CliConfig, CliConfigPaths};
use flowplane::cli::config_cmd::validate_config_key;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// CliConfigPaths::from_base
// ---------------------------------------------------------------------------

#[test]
fn config_paths_from_base_creates_expected_structure() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    assert_eq!(paths.flowplane_dir, tmp.path().join(".flowplane"));
    assert_eq!(paths.config_path, tmp.path().join(".flowplane/config.toml"));
    assert_eq!(
        paths.credentials_path,
        tmp.path().join(".flowplane/credentials")
    );
}

// ---------------------------------------------------------------------------
// Round-trip: save then load returns identical values
// ---------------------------------------------------------------------------

#[test]
fn config_round_trip_all_keys() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    let config = CliConfig {
        token: Some("test-token-abc".into()),
        base_url: Some("http://localhost:9090".into()),
        timeout: Some(45),
        team: Some("platform".into()),
        org: Some("acme".into()),
        oidc_issuer: Some("https://auth.example.com".into()),
        oidc_client_id: Some("client-id-xyz".into()),
        callback_url: Some("http://localhost:3000/callback".into()),
    };

    config.save_to_paths(&paths).unwrap();

    let loaded = CliConfig::load_from_paths(&paths).unwrap();
    assert_eq!(loaded.token.as_deref(), Some("test-token-abc"));
    assert_eq!(loaded.base_url.as_deref(), Some("http://localhost:9090"));
    assert_eq!(loaded.timeout, Some(45));
    assert_eq!(loaded.team.as_deref(), Some("platform"));
    assert_eq!(loaded.org.as_deref(), Some("acme"));
    assert_eq!(
        loaded.oidc_issuer.as_deref(),
        Some("https://auth.example.com")
    );
    assert_eq!(loaded.oidc_client_id.as_deref(), Some("client-id-xyz"));
    assert_eq!(
        loaded.callback_url.as_deref(),
        Some("http://localhost:3000/callback")
    );
}

// ---------------------------------------------------------------------------
// Default config has all None
// ---------------------------------------------------------------------------

#[test]
fn config_default_all_none() {
    let config = CliConfig::default();
    assert!(config.token.is_none());
    assert!(config.base_url.is_none());
    assert!(config.timeout.is_none());
    assert!(config.team.is_none());
    assert!(config.org.is_none());
    assert!(config.oidc_issuer.is_none());
    assert!(config.oidc_client_id.is_none());
    assert!(config.callback_url.is_none());
}

// ---------------------------------------------------------------------------
// Load from nonexistent path returns default
// ---------------------------------------------------------------------------

#[test]
fn config_load_nonexistent_returns_default() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    let loaded = CliConfig::load_from_paths(&paths).unwrap();
    assert!(loaded.token.is_none());
    assert!(loaded.oidc_issuer.is_none());
}

// ---------------------------------------------------------------------------
// Backward compatibility: old config without OIDC fields parses OK
// ---------------------------------------------------------------------------

#[test]
fn config_backward_compat_no_oidc_fields() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    std::fs::create_dir_all(&paths.flowplane_dir).unwrap();
    std::fs::write(
        &paths.config_path,
        r#"
token = "old-token"
base_url = "http://old-host:8080"
timeout = 30
"#,
    )
    .unwrap();

    let loaded = CliConfig::load_from_paths(&paths).unwrap();
    assert_eq!(loaded.token.as_deref(), Some("old-token"));
    assert!(loaded.oidc_issuer.is_none());
    assert!(loaded.oidc_client_id.is_none());
    assert!(loaded.callback_url.is_none());
    assert!(loaded.team.is_none());
    assert!(loaded.org.is_none());
}

// ---------------------------------------------------------------------------
// validate_config_key: all valid keys accepted
// ---------------------------------------------------------------------------

#[test]
fn validate_config_key_accepts_all_valid_keys() {
    let valid = [
        "token",
        "base_url",
        "timeout",
        "team",
        "org",
        "oidc_issuer",
        "oidc_client_id",
        "callback_url",
    ];
    for key in &valid {
        assert!(
            validate_config_key(key).is_ok(),
            "Expected key '{}' to be valid",
            key
        );
    }
}

// ---------------------------------------------------------------------------
// validate_config_key: unknown keys rejected
// ---------------------------------------------------------------------------

#[test]
fn validate_config_key_rejects_unknown() {
    let invalid = [
        "unknown",
        "Token",
        "BASE_URL",
        "oidc",
        "",
        "auth_mode",
        "dev_token",
    ];
    for key in &invalid {
        let result = validate_config_key(key);
        assert!(
            result.is_err(),
            "Expected key '{}' to be rejected",
            key
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Unknown configuration key"),
            "Error for '{}' should mention 'Unknown configuration key', got: {}",
            key,
            msg
        );
    }
}

// ---------------------------------------------------------------------------
// Config file created in correct directory
// ---------------------------------------------------------------------------

#[test]
fn config_save_creates_directory_and_file() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    // Directory should not exist yet
    assert!(!paths.flowplane_dir.exists());

    let config = CliConfig {
        base_url: Some("http://test:1234".into()),
        ..Default::default()
    };
    config.save_to_paths(&paths).unwrap();

    assert!(paths.flowplane_dir.exists());
    assert!(paths.config_path.exists());
}

// ---------------------------------------------------------------------------
// Config file permissions (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn config_file_is_readable_after_save() {
    use std::os::unix::fs::MetadataExt;

    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    let config = CliConfig {
        token: Some("secret".into()),
        ..Default::default()
    };
    config.save_to_paths(&paths).unwrap();

    let metadata = std::fs::metadata(&paths.config_path).unwrap();
    let mode = metadata.mode() & 0o777;
    // File should be readable by owner at minimum
    assert!(
        mode & 0o400 != 0,
        "Config file should be owner-readable, got mode {:o}",
        mode
    );
    // Config contains secrets (tokens) — no group or other access
    assert!(
        mode & 0o077 == 0,
        "Config file should have no group/other access, got mode {:o}",
        mode
    );
}

// ---------------------------------------------------------------------------
// OIDC keys serialization: None fields are omitted from TOML
// ---------------------------------------------------------------------------

#[test]
fn config_omits_none_fields_in_toml() {
    let config = CliConfig {
        base_url: Some("http://localhost:8080".into()),
        ..Default::default()
    };

    let toml_str = toml::to_string_pretty(&config).unwrap();
    assert!(toml_str.contains("base_url"));
    assert!(!toml_str.contains("token"));
    assert!(!toml_str.contains("oidc_issuer"));
    assert!(!toml_str.contains("oidc_client_id"));
    assert!(!toml_str.contains("callback_url"));
}

// ---------------------------------------------------------------------------
// OIDC keys round-trip: set individually and verify
// ---------------------------------------------------------------------------

#[test]
fn config_oidc_issuer_round_trip() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    let config = CliConfig {
        oidc_issuer: Some("https://zitadel.example.com".into()),
        ..Default::default()
    };
    config.save_to_paths(&paths).unwrap();

    let loaded = CliConfig::load_from_paths(&paths).unwrap();
    assert_eq!(
        loaded.oidc_issuer.as_deref(),
        Some("https://zitadel.example.com")
    );
}

#[test]
fn config_oidc_client_id_round_trip() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    let config = CliConfig {
        oidc_client_id: Some("my-client-id-12345".into()),
        ..Default::default()
    };
    config.save_to_paths(&paths).unwrap();

    let loaded = CliConfig::load_from_paths(&paths).unwrap();
    assert_eq!(
        loaded.oidc_client_id.as_deref(),
        Some("my-client-id-12345")
    );
}

#[test]
fn config_callback_url_round_trip() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    let config = CliConfig {
        callback_url: Some("http://localhost:3000/auth/callback".into()),
        ..Default::default()
    };
    config.save_to_paths(&paths).unwrap();

    let loaded = CliConfig::load_from_paths(&paths).unwrap();
    assert_eq!(
        loaded.callback_url.as_deref(),
        Some("http://localhost:3000/auth/callback")
    );
}

// ---------------------------------------------------------------------------
// Incremental update: load, modify one key, save, reload
// ---------------------------------------------------------------------------

#[test]
fn config_incremental_update_preserves_other_keys() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    // Initial save with base_url and team
    let config = CliConfig {
        base_url: Some("http://original:8080".into()),
        team: Some("eng".into()),
        ..Default::default()
    };
    config.save_to_paths(&paths).unwrap();

    // Load, add oidc_issuer, save
    let mut loaded = CliConfig::load_from_paths(&paths).unwrap();
    loaded.oidc_issuer = Some("https://auth.test".into());
    loaded.save_to_paths(&paths).unwrap();

    // Reload and verify both old and new keys present
    let final_config = CliConfig::load_from_paths(&paths).unwrap();
    assert_eq!(
        final_config.base_url.as_deref(),
        Some("http://original:8080")
    );
    assert_eq!(final_config.team.as_deref(), Some("eng"));
    assert_eq!(
        final_config.oidc_issuer.as_deref(),
        Some("https://auth.test")
    );
}

// ---------------------------------------------------------------------------
// Malformed TOML returns error
// ---------------------------------------------------------------------------

#[test]
fn config_load_malformed_toml_returns_error() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    std::fs::create_dir_all(&paths.flowplane_dir).unwrap();
    std::fs::write(&paths.config_path, "this is not valid [[ toml {{").unwrap();

    let result = CliConfig::load_from_paths(&paths);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Empty config file loads as default
// ---------------------------------------------------------------------------

#[test]
fn config_load_empty_file_returns_default() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    std::fs::create_dir_all(&paths.flowplane_dir).unwrap();
    std::fs::write(&paths.config_path, "").unwrap();

    let loaded = CliConfig::load_from_paths(&paths).unwrap();
    assert!(loaded.token.is_none());
    assert!(loaded.base_url.is_none());
}

// ---------------------------------------------------------------------------
// CLI parsing: `flowplane serve` is a valid subcommand
// ---------------------------------------------------------------------------

#[test]
fn cli_serve_subcommand_parses() {
    use clap::Parser;
    use flowplane::cli::Cli;

    let result = Cli::try_parse_from(["flowplane", "serve"]);
    assert!(result.is_ok(), "Expected 'serve' to be a valid subcommand");
}

#[test]
fn cli_serve_dev_subcommand_parses() {
    use clap::Parser;
    use flowplane::cli::Cli;

    let result = Cli::try_parse_from(["flowplane", "serve", "--dev"]);
    assert!(
        result.is_ok(),
        "Expected 'serve --dev' to be a valid subcommand"
    );
}

// ---------------------------------------------------------------------------
// CLI parsing: config subcommands parse correctly
// ---------------------------------------------------------------------------

#[test]
fn cli_config_set_valid_key_parses() {
    use clap::Parser;
    use flowplane::cli::Cli;

    let keys_and_values = [
        ("token", "my-token"),
        ("base_url", "http://localhost:9090"),
        ("timeout", "60"),
        ("team", "engineering"),
        ("org", "acme-corp"),
        ("oidc_issuer", "https://auth.example.com"),
        ("oidc_client_id", "client-123"),
        ("callback_url", "http://localhost:3000/cb"),
    ];

    for (key, value) in &keys_and_values {
        let result = Cli::try_parse_from(["flowplane", "config", "set", key, value]);
        assert!(
            result.is_ok(),
            "Expected 'config set {} {}' to parse, got: {:?}",
            key,
            value,
            result.err()
        );
    }
}

#[test]
fn cli_config_set_invalid_key_rejected_by_clap() {
    use clap::Parser;
    use flowplane::cli::Cli;

    let result = Cli::try_parse_from(["flowplane", "config", "set", "bogus_key", "value"]);
    assert!(
        result.is_err(),
        "Expected 'config set bogus_key' to be rejected by clap value_parser"
    );
}

#[test]
fn cli_config_init_parses() {
    use clap::Parser;
    use flowplane::cli::Cli;

    let result = Cli::try_parse_from(["flowplane", "config", "init"]);
    assert!(result.is_ok());
}

#[test]
fn cli_config_show_parses() {
    use clap::Parser;
    use flowplane::cli::Cli;

    let result = Cli::try_parse_from(["flowplane", "config", "show"]);
    assert!(result.is_ok());
}

#[test]
fn cli_config_path_parses() {
    use clap::Parser;
    use flowplane::cli::Cli;

    let result = Cli::try_parse_from(["flowplane", "config", "path"]);
    assert!(result.is_ok());
}

// ---------------------------------------------------------------------------
// Dockerfile.backend CMD verification (static assertion)
// ---------------------------------------------------------------------------

#[test]
fn dockerfile_backend_cmd_uses_flowplane_serve() {
    let dockerfile = include_str!("../Dockerfile.backend");

    // Must contain the CMD line with flowplane serve
    assert!(
        dockerfile.contains(r#"CMD ["flowplane", "serve"]"#),
        "Dockerfile.backend CMD should be 'flowplane serve', got:\n{}",
        dockerfile
            .lines()
            .filter(|l| l.starts_with("CMD") || l.starts_with("ENTRYPOINT"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Should NOT use old binary names
    assert!(
        !dockerfile.contains("flowplane-cli"),
        "Dockerfile.backend should not reference old 'flowplane-cli' binary"
    );
}

// ---------------------------------------------------------------------------
// Adversarial inputs: special characters in config values
// ---------------------------------------------------------------------------

#[test]
fn config_handles_special_characters_in_values() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    let config = CliConfig {
        base_url: Some("http://host:8080/path?q=1&r=2".into()),
        team: Some("team with spaces".into()),
        oidc_issuer: Some("https://auth.example.com/realms/my-realm".into()),
        callback_url: Some("http://localhost:3000/callback#fragment".into()),
        ..Default::default()
    };

    config.save_to_paths(&paths).unwrap();
    let loaded = CliConfig::load_from_paths(&paths).unwrap();

    assert_eq!(
        loaded.base_url.as_deref(),
        Some("http://host:8080/path?q=1&r=2")
    );
    assert_eq!(loaded.team.as_deref(), Some("team with spaces"));
    assert_eq!(
        loaded.oidc_issuer.as_deref(),
        Some("https://auth.example.com/realms/my-realm")
    );
    assert_eq!(
        loaded.callback_url.as_deref(),
        Some("http://localhost:3000/callback#fragment")
    );
}

// ---------------------------------------------------------------------------
// Extra/unknown fields in TOML are silently ignored (no deny_unknown_fields)
// ---------------------------------------------------------------------------

#[test]
fn config_with_unknown_toml_fields() {
    let tmp = TempDir::new().unwrap();
    let paths = CliConfigPaths::from_base(tmp.path()).unwrap();

    std::fs::create_dir_all(&paths.flowplane_dir).unwrap();
    std::fs::write(
        &paths.config_path,
        r#"
base_url = "http://localhost:8080"
totally_unknown_field = "should this fail?"
"#,
    )
    .unwrap();

    // CliConfig does NOT use #[serde(deny_unknown_fields)], so unknown fields
    // are silently ignored by toml::from_str. Load must succeed.
    let loaded = CliConfig::load_from_paths(&paths)
        .expect("unknown fields should be ignored (no deny_unknown_fields)");
    assert_eq!(
        loaded.base_url.as_deref(),
        Some("http://localhost:8080"),
    );
}
