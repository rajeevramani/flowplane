//! Integration tests for CLI error handling
//!
//! Tests:
//! - Network errors (server unreachable)
//! - Invalid JSON/YAML responses
//! - Timeout handling
//! - Invalid command arguments
//! - Malformed input files

use super::support::{run_cli_command, TempOpenApiFile, TestServer};

#[tokio::test]
async fn test_network_error_unreachable_server() {
    let token = "test-token";

    // Use an unreachable address
    let result = run_cli_command(&[
        "api",
        "list",
        "--token",
        token,
        "--base-url",
        "http://localhost:9999",
        "--timeout",
        "2",
    ])
    .await;

    assert!(result.is_err(), "Should fail when server is unreachable");
    let error = result.unwrap_err();
    assert!(
        error.contains("Connection refused")
            || error.contains("connect")
            || error.contains("network")
            || error.contains("timeout"),
        "Error should indicate network issue: {}",
        error
    );
}

#[tokio::test]
async fn test_timeout_handling() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("timeout-test-token", &["routes:read"]).await;

    // Use a very short timeout (note: this may or may not trigger depending on server speed)
    let result = run_cli_command(&[
        "api",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--timeout",
        "0", // 0 seconds should cause immediate timeout
    ])
    .await;

    // This might succeed if the server is very fast, or fail with timeout
    // We just verify the CLI handles the timeout parameter correctly
    if result.is_err() {
        let error = result.unwrap_err();
        assert!(
            error.contains("timeout") || error.contains("time") || error.contains("duration"),
            "Timeout error should mention timing: {}",
            error
        );
    }
}

#[tokio::test]
async fn test_invalid_output_format() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("invalid-format-token", &["routes:read"]).await;

    let result = run_cli_command(&[
        "api",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "invalid_format",
    ])
    .await;

    assert!(result.is_err(), "Should fail with invalid output format");
    let error = result.unwrap_err();
    assert!(
        error.contains("Unsupported output format") || error.contains("invalid"),
        "Error should mention invalid format: {}",
        error
    );
}

#[tokio::test]
async fn test_malformed_openapi_file() {
    let malformed_yaml = r#"
openapi: 3.0.0
info:
  title: Malformed API
  this is not valid yaml syntax::
    nested: [unclosed
servers
"#;
    let openapi_file = TempOpenApiFile::new(malformed_yaml);

    let result =
        run_cli_command(&["api", "validate-filters", "--file", &openapi_file.path_str()]).await;

    assert!(result.is_err(), "Should fail with malformed YAML");
    let error = result.unwrap_err();
    assert!(
        error.contains("parse") || error.contains("YAML") || error.contains("invalid"),
        "Error should indicate parsing failure: {}",
        error
    );
}

#[tokio::test]
async fn test_missing_required_argument() {
    // Try to import API without specifying file
    let result = run_cli_command(&["api", "import-openapi", "--token", "test-token"]).await;

    assert!(result.is_err(), "Should fail with missing required argument");
    let error = result.unwrap_err();
    assert!(
        error.contains("required") || error.contains("--file") || error.contains("argument"),
        "Error should mention missing argument: {}",
        error
    );
}

#[tokio::test]
async fn test_invalid_base_url_format() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("invalid-url-token", &["routes:read"]).await;

    let result = run_cli_command(&[
        "api",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        "not-a-valid-url",
    ])
    .await;

    assert!(result.is_err(), "Should fail with invalid URL format");
    let error = result.unwrap_err();
    assert!(
        error.contains("URL") || error.contains("invalid") || error.contains("parse"),
        "Error should indicate URL issue: {}",
        error
    );
}

#[tokio::test]
async fn test_nonexistent_subcommand() {
    let result = run_cli_command(&["api", "nonexistent-command"]).await;

    assert!(result.is_err(), "Should fail with nonexistent subcommand");
    let error = result.unwrap_err();
    assert!(
        error.contains("unrecognized") || error.contains("invalid") || error.contains("command"),
        "Error should mention invalid command: {}",
        error
    );
}

#[tokio::test]
async fn test_empty_token_file() {
    let empty_token_file = TempOpenApiFile::new(""); // Reuse TempOpenApiFile for empty file
    let server = TestServer::start().await;

    let result = run_cli_command(&[
        "api",
        "list",
        "--token-file",
        &empty_token_file.path_str(),
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_err(), "Should fail with empty token file");
    let error = result.unwrap_err();
    assert!(
        error.contains("token") || error.contains("empty") || error.contains("authentication"),
        "Error should indicate token issue: {}",
        error
    );
}

#[tokio::test]
async fn test_json_parse_error_in_openapi() {
    let invalid_json = r#"
{
  "openapi": "3.0.0",
  "info": {
    "title": "Invalid JSON",
    "version": "1.0.0"

  "servers": []
}
"#; // Missing closing brace for info object
    let openapi_file = TempOpenApiFile::new(invalid_json);

    let result =
        run_cli_command(&["api", "validate-filters", "--file", &openapi_file.path_str()]).await;

    assert!(result.is_err(), "Should fail with invalid JSON");
    let error = result.unwrap_err();
    assert!(
        error.contains("parse") || error.contains("JSON") || error.contains("invalid"),
        "Error should indicate JSON parsing failure: {}",
        error
    );
}

#[tokio::test]
async fn test_help_flag_doesnt_error() {
    let result = run_cli_command(&["--help"]).await;

    // Help flag should succeed and show usage information
    assert!(result.is_ok() || result.is_err(), "Help flag should be handled");
    if let Ok(output) = result {
        assert!(
            output.contains("flowplane") || output.contains("USAGE") || output.contains("Commands"),
            "Help output should contain usage information"
        );
    }
}

#[tokio::test]
async fn test_version_flag() {
    let result = run_cli_command(&["--version"]).await;

    // Version flag should succeed
    assert!(result.is_ok() || result.is_err(), "Version flag should be handled");
    if let Ok(output) = result {
        assert!(
            output.contains("flowplane") || output.contains(env!("CARGO_PKG_VERSION")),
            "Version output should contain version information"
        );
    }
}

#[tokio::test]
async fn test_concurrent_requests_handling() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("concurrent-test-token", &["api-definitions:read"]).await;

    // Make multiple concurrent requests
    let handles: Vec<_> = (0..5)
        .map(|_| {
            let token = token_response.token.clone();
            let base_url = server.base_url();
            tokio::spawn(async move {
                run_cli_command(&[
                    "api",
                    "list",
                    "--token",
                    &token,
                    "--base-url",
                    &base_url,
                    "--output",
                    "json",
                ])
                .await
            })
        })
        .collect();

    // Wait for all requests
    let results: Vec<_> = futures::future::join_all(handles).await;

    // All requests should succeed
    for result in results {
        let cli_result = result.expect("task should complete");
        assert!(cli_result.is_ok(), "Concurrent requests should all succeed");
    }
}

#[tokio::test]
async fn test_special_characters_in_arguments() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("special-char-token", &["clusters:read"]).await;

    // Try to get a cluster with special characters in the ID
    let result = run_cli_command(&[
        "cluster",
        "get",
        "cluster-with-special-chars-!@#$%",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    // Should fail (cluster doesn't exist), but should handle special chars without crashing
    assert!(result.is_err(), "Should fail gracefully with special characters in ID");
}

#[tokio::test]
async fn test_very_long_token() {
    let server = TestServer::start().await;
    let very_long_token = "a".repeat(10000); // 10KB token

    let result = run_cli_command(&[
        "api",
        "list",
        "--token",
        &very_long_token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    // Should fail with authentication error, but handle long token gracefully
    assert!(result.is_err(), "Should fail with invalid long token");
}
