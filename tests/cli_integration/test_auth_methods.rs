// NOTE: Requires PostgreSQL - disabled until Phase 4
#![cfg(feature = "postgres_tests")]

//! Integration tests for CLI authentication methods
//!
//! Tests all supported authentication methods:
//! - --token flag
//! - --token-file flag
//! - Config file (~/.flowplane/config.toml)
//! - FLOWPLANE_TOKEN environment variable
//!
//! Also tests authentication precedence and error handling.

use super::support::{run_cli_command, TempTokenFile, TestServer};

#[tokio::test]
async fn test_auth_with_token_flag() {
    let server = TestServer::start().await;
    let token_response = server.issue_admin_token("test-token-flag", &["clusters:read"]).await;

    let result = run_cli_command(&[
        "cluster",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await;

    assert!(result.is_ok(), "CLI command should succeed with --token flag. Error: {:?}", result);
}

#[tokio::test]
async fn test_auth_with_token_file() {
    let server = TestServer::start().await;
    let token_response = server.issue_admin_token("test-token-file", &["clusters:read"]).await;
    let token_file = TempTokenFile::new(&token_response.token);

    let result = run_cli_command(&[
        "cluster",
        "list",
        "--token-file",
        &token_file.path_str(),
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await;

    assert!(result.is_ok(), "CLI command should succeed with --token-file flag");
}

#[tokio::test]
async fn test_auth_with_env_var() {
    use super::support::run_cli_command_with_env;

    let server = TestServer::start().await;
    let token_response = server.issue_admin_token("test-env-var", &["clusters:read"]).await;

    let result = run_cli_command_with_env(
        &["cluster", "list", "--base-url", &server.base_url(), "--output", "json"],
        Some(&[("FLOWPLANE_TOKEN", &token_response.token)]),
    )
    .await;

    assert!(
        result.is_ok(),
        "CLI command should succeed with FLOWPLANE_TOKEN env var: {:?}",
        result
    );
}

#[tokio::test]
async fn test_auth_precedence_token_flag_over_file() {
    let server = TestServer::start().await;
    let valid_token = server.issue_admin_token("valid-token", &["clusters:read"]).await;
    let invalid_token_file = TempTokenFile::new("invalid-token");

    // --token should take precedence over --token-file
    let result = run_cli_command(&[
        "cluster",
        "list",
        "--token",
        &valid_token.token,
        "--token-file",
        &invalid_token_file.path_str(),
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await;

    assert!(result.is_ok(), "CLI should use --token flag over --token-file when both are provided");
}

#[tokio::test]
async fn test_auth_precedence_token_file_over_env() {
    use super::support::run_cli_command_with_env;

    let server = TestServer::start().await;
    let valid_token = server.issue_admin_token("valid-file-token", &["clusters:read"]).await;
    let token_file = TempTokenFile::new(&valid_token.token);

    // --token-file should take precedence over env var
    let result = run_cli_command_with_env(
        &[
            "cluster",
            "list",
            "--token-file",
            &token_file.path_str(),
            "--base-url",
            &server.base_url(),
            "--output",
            "json",
        ],
        Some(&[("FLOWPLANE_TOKEN", "invalid-env-token")]),
    )
    .await;

    assert!(
        result.is_ok(),
        "CLI should use --token-file over FLOWPLANE_TOKEN env var when both are provided"
    );
}

#[tokio::test]
async fn test_auth_failure_with_invalid_token() {
    let server = TestServer::start().await;

    let result = run_cli_command(&[
        "cluster",
        "list",
        "--token",
        "invalid-token-12345",
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await;

    assert!(result.is_err(), "CLI should fail with invalid token");
    let error = result.unwrap_err();
    assert!(
        error.contains("401") || error.contains("Unauthorized") || error.contains("authentication"),
        "Error should indicate authentication failure: {}",
        error
    );
}

#[tokio::test]
async fn test_auth_failure_with_no_token() {
    let server = TestServer::start().await;

    let result =
        run_cli_command(&["cluster", "list", "--base-url", &server.base_url(), "--output", "json"])
            .await;

    assert!(result.is_err(), "CLI should fail when no authentication is provided");
    let error = result.unwrap_err();
    assert!(
        error.contains("No authentication token found") || error.contains("token"),
        "Error should indicate missing token: {}",
        error
    );
}

#[tokio::test]
async fn test_auth_with_insufficient_scopes() {
    use super::support::TempOpenApiFile;

    let server = TestServer::start().await;
    // Create token with only routes:read scope, not routes:write
    let token_response = server.issue_token("limited-scope-token", &["routes:read"]).await;

    // Create a temp file for route config
    let route_config = r#"
{
  "name": "test-route",
  "path_prefix": "/api",
  "cluster_name": "test-cluster",
  "configuration": {
    "name": "test"
  },
  "team": "test-team"
}
"#;
    let config_file = TempOpenApiFile::new(route_config);

    let result = run_cli_command(&[
        "route",
        "create",
        "--file",
        &config_file.path_str(),
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_err(), "CLI should fail with insufficient scopes");
    let error = result.unwrap_err();
    assert!(
        error.contains("403") || error.contains("Forbidden") || error.contains("permission"),
        "Error should indicate permission denied: {}",
        error
    );
}

#[tokio::test]
async fn test_auth_token_file_not_found() {
    let server = TestServer::start().await;

    let result = run_cli_command(&[
        "cluster",
        "list",
        "--token-file",
        "/nonexistent/path/to/token.txt",
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_err(), "CLI should fail when token file doesn't exist");
    let error = result.unwrap_err();
    assert!(
        error.contains("Failed to read token file") || error.contains("No such file"),
        "Error should indicate file not found: {}",
        error
    );
}
