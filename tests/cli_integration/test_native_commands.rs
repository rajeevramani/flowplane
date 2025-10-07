//! Integration tests for native resource CLI commands
//!
//! Tests:
//! - cluster list/get
//! - listener list/get
//! - route list/get

use super::support::{run_cli_command, TestServer};

#[tokio::test]
async fn test_cluster_list_json() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("cluster-list-token", &["clusters:read"]).await;

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

    assert!(result.is_ok(), "Cluster list should succeed: {:?}", result);
    let output = result.unwrap();

    // Verify JSON output
    let json: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    assert!(json.is_array(), "Output should be a JSON array");
}

#[tokio::test]
async fn test_cluster_list_yaml() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("cluster-list-yaml-token", &["clusters:read"]).await;

    let result = run_cli_command(&[
        "cluster",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "yaml",
    ])
    .await;

    assert!(result.is_ok(), "Cluster list with YAML output should succeed");
    let output = result.unwrap();

    // Verify YAML output
    let yaml: serde_yaml::Value = serde_yaml::from_str(&output).expect("valid YAML");
    assert!(yaml.is_sequence(), "Output should be a YAML sequence");
}

#[tokio::test]
async fn test_cluster_list_table() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("cluster-list-table-token", &["clusters:read"]).await;

    let result = run_cli_command(&[
        "cluster",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "table",
    ])
    .await;

    assert!(result.is_ok(), "Cluster list with table output should succeed");
    let output = result.unwrap();

    // Table output should have headers
    assert!(
        output.contains("Name") || output.contains("ID") || output.is_empty(),
        "Table output should have headers or be empty: {}",
        output
    );
}

#[tokio::test]
async fn test_cluster_get_with_invalid_id() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("cluster-get-token", &["clusters:read"]).await;

    let result = run_cli_command(&[
        "cluster",
        "get",
        "nonexistent-cluster-id",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_err(), "Cluster get should fail with invalid ID");
    let error = result.unwrap_err();
    assert!(
        error.contains("404") || error.contains("Not found") || error.contains("not found"),
        "Error should indicate not found: {}",
        error
    );
}

#[tokio::test]
async fn test_listener_list_json() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("listener-list-token", &["listeners:read"]).await;

    let result = run_cli_command(&[
        "listener",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await;

    assert!(result.is_ok(), "Listener list should succeed");
    let output = result.unwrap();

    // Verify JSON output
    let json: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    assert!(json.is_array(), "Output should be a JSON array");
}

#[tokio::test]
async fn test_listener_list_yaml() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("listener-list-yaml-token", &["listeners:read"]).await;

    let result = run_cli_command(&[
        "listener",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "yaml",
    ])
    .await;

    assert!(result.is_ok(), "Listener list with YAML output should succeed");
    let output = result.unwrap();

    // Verify YAML output
    let yaml: serde_yaml::Value = serde_yaml::from_str(&output).expect("valid YAML");
    assert!(yaml.is_sequence(), "Output should be a YAML sequence");
}

#[tokio::test]
async fn test_listener_list_table() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("listener-list-table-token", &["listeners:read"]).await;

    let result = run_cli_command(&[
        "listener",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "table",
    ])
    .await;

    assert!(result.is_ok(), "Listener list with table output should succeed");
}

#[tokio::test]
async fn test_listener_get_with_invalid_id() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("listener-get-token", &["listeners:read"]).await;

    let result = run_cli_command(&[
        "listener",
        "get",
        "nonexistent-listener-id",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_err(), "Listener get should fail with invalid ID");
    let error = result.unwrap_err();
    assert!(
        error.contains("404") || error.contains("Not found") || error.contains("not found"),
        "Error should indicate not found: {}",
        error
    );
}

#[tokio::test]
async fn test_route_list_json() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("route-list-token", &["routes:read"]).await;

    let result = run_cli_command(&[
        "route",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await;

    assert!(result.is_ok(), "Route list should succeed");
    let output = result.unwrap();

    // Verify JSON output
    let json: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    assert!(json.is_array(), "Output should be a JSON array");
}

#[tokio::test]
async fn test_route_list_yaml() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("route-list-yaml-token", &["routes:read"]).await;

    let result = run_cli_command(&[
        "route",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "yaml",
    ])
    .await;

    assert!(result.is_ok(), "Route list with YAML output should succeed");
    let output = result.unwrap();

    // Verify YAML output
    let yaml: serde_yaml::Value = serde_yaml::from_str(&output).expect("valid YAML");
    assert!(yaml.is_sequence(), "Output should be a YAML sequence");
}

#[tokio::test]
async fn test_route_list_table() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("route-list-table-token", &["routes:read"]).await;

    let result = run_cli_command(&[
        "route",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "table",
    ])
    .await;

    assert!(result.is_ok(), "Route list with table output should succeed");
}

#[tokio::test]
async fn test_route_get_with_invalid_id() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("route-get-token", &["routes:read"]).await;

    let result = run_cli_command(&[
        "route",
        "get",
        "nonexistent-route-id",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_err(), "Route get should fail with invalid ID");
    let error = result.unwrap_err();
    assert!(
        error.contains("404") || error.contains("Not found") || error.contains("not found"),
        "Error should indicate not found: {}",
        error
    );
}

#[tokio::test]
async fn test_native_resource_without_auth() {
    let server = TestServer::start().await;

    // Remove any env var
    std::env::remove_var("FLOWPLANE_TOKEN");

    let result = run_cli_command(&["cluster", "list", "--base-url", &server.base_url()]).await;

    assert!(result.is_err(), "Native resource commands should fail without auth");
    let error = result.unwrap_err();
    assert!(
        error.contains("No authentication token found") || error.contains("token"),
        "Error should indicate missing token: {}",
        error
    );
}

#[tokio::test]
async fn test_native_resource_with_insufficient_scope() {
    let server = TestServer::start().await;
    // Create token with only routes:read, not clusters:read
    let token_response = server.issue_token("limited-token", &["routes:read"]).await;

    let result = run_cli_command(&[
        "cluster",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_err(), "Cluster commands should fail with insufficient scopes");
    let error = result.unwrap_err();
    assert!(
        error.contains("403") || error.contains("Forbidden") || error.contains("permission"),
        "Error should indicate permission denied: {}",
        error
    );
}
