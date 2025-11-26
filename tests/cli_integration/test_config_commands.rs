//! Integration tests for config management CLI commands
//!
//! Tests:
//! - config init
//! - config show
//! - config set
//! - config path

use super::support::{run_cli_command, run_cli_command_with_env, TempConfig};
use std::fs;

#[tokio::test]
async fn test_config_init_creates_file() {
    let temp_config = TempConfig::new();

    // Ensure config doesn't exist
    if temp_config.path.exists() {
        fs::remove_file(&temp_config.path).ok();
    }

    let home_dir = temp_config.home_dir().to_str().unwrap();
    let result = run_cli_command_with_env(&["config", "init"], Some(&[("HOME", home_dir)])).await;

    assert!(result.is_ok(), "Config init should succeed: {:?}", result);
}

#[tokio::test]
async fn test_config_set_token() {
    let temp_config = TempConfig::new();
    let home_dir = temp_config.home_dir().to_str().unwrap();

    // Initialize config with isolated HOME
    run_cli_command_with_env(&["config", "init"], Some(&[("HOME", home_dir)]))
        .await
        .expect("init config");

    // Set token with isolated HOME
    let result = run_cli_command_with_env(
        &["config", "set", "token", "test-token-12345"],
        Some(&[("HOME", home_dir)]),
    )
    .await;

    assert!(result.is_ok(), "Config set token should succeed");

    // Verify token was written
    let config_content = fs::read_to_string(&temp_config.path).expect("read config file");
    assert!(
        config_content.contains("test-token-12345"),
        "Config should contain set token: {}",
        config_content
    );
}

#[tokio::test]
async fn test_config_set_base_url() {
    let temp_config = TempConfig::new();
    let home_dir = temp_config.home_dir().to_str().unwrap();

    run_cli_command_with_env(&["config", "init"], Some(&[("HOME", home_dir)]))
        .await
        .expect("init config");

    let result = run_cli_command_with_env(
        &["config", "set", "base_url", "https://api.custom.com"],
        Some(&[("HOME", home_dir)]),
    )
    .await;

    assert!(result.is_ok(), "Config set base_url should succeed");

    let config_content = fs::read_to_string(&temp_config.path).expect("read config file");
    assert!(
        config_content.contains("https://api.custom.com"),
        "Config should contain set base_url: {}",
        config_content
    );
}

#[tokio::test]
async fn test_config_set_timeout() {
    let temp_config = TempConfig::new();
    let home_dir = temp_config.home_dir().to_str().unwrap();

    run_cli_command_with_env(&["config", "init"], Some(&[("HOME", home_dir)]))
        .await
        .expect("init config");

    let result =
        run_cli_command_with_env(&["config", "set", "timeout", "60"], Some(&[("HOME", home_dir)]))
            .await;

    assert!(result.is_ok(), "Config set timeout should succeed");

    let config_content = fs::read_to_string(&temp_config.path).expect("read config file");
    assert!(
        config_content.contains("timeout = 60") || config_content.contains("timeout=60"),
        "Config should contain timeout: {}",
        config_content
    );
}

#[tokio::test]
async fn test_config_set_invalid_timeout() {
    let temp_config = TempConfig::new();
    let home_dir = temp_config.home_dir().to_str().unwrap();

    run_cli_command_with_env(&["config", "init"], Some(&[("HOME", home_dir)]))
        .await
        .expect("init config");

    let result = run_cli_command_with_env(
        &["config", "set", "timeout", "not-a-number"],
        Some(&[("HOME", home_dir)]),
    )
    .await;

    assert!(result.is_err(), "Config set should fail with invalid timeout value");
    let error = result.unwrap_err();
    assert!(
        error.contains("Invalid timeout") || error.contains("parse"),
        "Error should indicate invalid value: {}",
        error
    );
}

#[tokio::test]
async fn test_config_set_unknown_key() {
    let temp_config = TempConfig::new();
    let home_dir = temp_config.home_dir().to_str().unwrap();

    run_cli_command_with_env(&["config", "init"], Some(&[("HOME", home_dir)]))
        .await
        .expect("init config");

    let result = run_cli_command_with_env(
        &["config", "set", "unknown_key", "value"],
        Some(&[("HOME", home_dir)]),
    )
    .await;

    assert!(result.is_err(), "Config set should fail with unknown key");
    let error = result.unwrap_err();
    assert!(
        error.contains("Unknown configuration key") || error.contains("unknown_key"),
        "Error should mention unknown key: {}",
        error
    );
}

#[tokio::test]
async fn test_config_show_json() {
    let temp_config = TempConfig::new();
    temp_config.write_config("show-test-token", "https://test.example.com");
    let home_dir = temp_config.home_dir().to_str().unwrap();

    let result = run_cli_command_with_env(
        &["config", "show", "--output", "json"],
        Some(&[("HOME", home_dir)]),
    )
    .await;

    assert!(result.is_ok(), "Config show should succeed");
    let output = result.unwrap();

    // Verify JSON output
    let json: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    assert!(json.get("token").is_some(), "JSON should have token field");
    assert!(json.get("base_url").is_some(), "JSON should have base_url field");
}

#[tokio::test]
async fn test_config_show_yaml() {
    let temp_config = TempConfig::new();
    temp_config.write_config("show-yaml-token", "https://yaml.example.com");
    let home_dir = temp_config.home_dir().to_str().unwrap();

    let result = run_cli_command_with_env(
        &["config", "show", "--output", "yaml"],
        Some(&[("HOME", home_dir)]),
    )
    .await;

    assert!(result.is_ok(), "Config show with YAML should succeed");
    let output = result.unwrap();

    // Verify YAML output
    let yaml: serde_yaml::Value = serde_yaml::from_str(&output).expect("valid YAML");
    assert!(yaml.get("token").is_some(), "YAML should have token field");
    assert!(yaml.get("base_url").is_some(), "YAML should have base_url field");
}

#[tokio::test]
async fn test_config_path() {
    let result = run_cli_command(&["config", "path"]).await;

    assert!(result.is_ok(), "Config path should succeed");
    let output = result.unwrap();
    assert!(
        output.contains(".flowplane") && output.contains("config.toml"),
        "Output should show config file path: {}",
        output
    );
}

#[tokio::test]
async fn test_config_init_force_overwrites() {
    let temp_config = TempConfig::new();
    temp_config.write_config("existing-token", "https://existing.com");
    let home_dir = temp_config.home_dir().to_str().unwrap();

    // Force reinitialize
    let result =
        run_cli_command_with_env(&["config", "init", "--force"], Some(&[("HOME", home_dir)])).await;

    assert!(result.is_ok(), "Config init --force should succeed");

    // Verify old config was overwritten
    let config_content = fs::read_to_string(&temp_config.path).expect("read config file");
    assert!(
        !config_content.contains("existing-token"),
        "Force init should overwrite existing config"
    );
}

#[tokio::test]
async fn test_config_show_nonexistent() {
    // Use a temp directory that definitely doesn't have a config
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let home_dir = temp_dir.path().to_str().unwrap();

    let result = run_cli_command_with_env(&["config", "show"], Some(&[("HOME", home_dir)])).await;

    // Config show should handle missing config gracefully
    // Either it returns an error, or it returns output containing "not found" or "No configuration file"
    match result {
        Err(error) => {
            // Error is expected when config doesn't exist - verify message is helpful
            assert!(
                error.contains("not found") || error.contains("No configuration file"),
                "Error should indicate config not found: {}",
                error
            );
        }
        Ok(output) => {
            // If it succeeds, output should contain helpful message
            assert!(
                output.contains("not found")
                    || output.contains("No configuration file")
                    || output.is_empty(),
                "Config show should handle missing config: {}",
                output
            );
        }
    }
}
