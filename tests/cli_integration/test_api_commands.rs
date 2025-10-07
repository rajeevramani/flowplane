//! Integration tests for API definition CLI commands
//!
//! Tests:
//! - api import
//! - api list
//! - api get
//! - api delete
//! - api bootstrap
//! - api validate-filters
//! - api show-filters

use super::support::{run_cli_command, TempOpenApiFile, TestServer};

const SIMPLE_OPENAPI: &str = r#"
openapi: 3.0.0
info:
  title: Test API
  version: 1.0.0
servers:
  - url: https://httpbin.org
paths:
  /get:
    get:
      summary: Get endpoint
      responses:
        '200':
          description: Success
"#;

const OPENAPI_WITH_FILTERS: &str = r#"
openapi: 3.0.0
info:
  title: API with Filters
  version: 1.0.0
x-flowplane-filters:
  - filter:
      type: cors
      policy:
        allow_origin:
          - type: exact
            value: "https://example.com"
  - filter:
      type: header_mutation
      request_headers_to_add:
        - key: x-api-version
          value: "v1"
servers:
  - url: https://api.example.com
paths:
  /api/v1/users:
    get:
      summary: List users
      responses:
        '200':
          description: Success
"#;

#[tokio::test]
async fn test_api_import_simple_openapi() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("api-import-token", &["routes:write"]).await;
    let openapi_file = TempOpenApiFile::new(SIMPLE_OPENAPI);

    let result = run_cli_command(&[
        "api",
        "import-openapi",
        "--file",
        &openapi_file.path_str(),
        "--team",
        "test-team",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_ok(), "API import should succeed: {:?}", result);
    let output = result.unwrap();
    assert!(output.contains("API definition imported successfully"), "Output: {}", output);
}

#[tokio::test]
async fn test_api_list_json_output() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("api-list-token", &["routes:read", "routes:write"]).await;
    let openapi_file = TempOpenApiFile::new(SIMPLE_OPENAPI);

    // Import an API first
    run_cli_command(&[
        "api",
        "import-openapi",
        "--file",
        &openapi_file.path_str(),
        "--team",
        "test-team",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await
    .expect("import API");

    // List APIs
    let result = run_cli_command(&[
        "api",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await;

    assert!(result.is_ok(), "API list should succeed");
    let output = result.unwrap();

    // Verify JSON output is valid
    let json: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    assert!(json.is_array(), "Output should be a JSON array");
    assert!(!json.as_array().unwrap().is_empty(), "Should have at least one API");
}

#[tokio::test]
async fn test_api_list_yaml_output() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("api-list-yaml-token", &["routes:read"]).await;

    let result = run_cli_command(&[
        "api",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "yaml",
    ])
    .await;

    assert!(result.is_ok(), "API list with YAML output should succeed");
    let output = result.unwrap();

    // Verify YAML output is valid
    let yaml: serde_yaml::Value = serde_yaml::from_str(&output).expect("valid YAML");
    assert!(yaml.is_sequence(), "Output should be a YAML sequence");
}

#[tokio::test]
async fn test_api_get_by_id() {
    let server = TestServer::start().await;
    let token_response =
        server.issue_token("api-get-token", &["routes:read", "routes:write"]).await;
    let openapi_file = TempOpenApiFile::new(SIMPLE_OPENAPI);

    // Import an API
    let import_result = run_cli_command(&[
        "api",
        "import-openapi",
        "--file",
        &openapi_file.path_str(),
        "--team",
        "test-team",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await
    .expect("import API");

    // Extract API ID from import output (assumes format includes ID)
    let import_output = import_result;
    let api_id = if import_output.contains("ID:") {
        import_output
            .lines()
            .find(|line| line.contains("ID:"))
            .and_then(|line| line.split(':').nth(1))
            .map(|s| s.trim().to_string())
            .expect("extract API ID")
    } else {
        // Fallback: list APIs and get the first one
        let list_output = run_cli_command(&[
            "api",
            "list",
            "--token",
            &token_response.token,
            "--base-url",
            &server.base_url(),
            "--output",
            "json",
        ])
        .await
        .expect("list APIs");

        let apis: Vec<serde_json::Value> = serde_json::from_str(&list_output).expect("parse JSON");
        apis[0]["id"].as_str().expect("get first API ID").to_string()
    };

    // Get specific API
    let result = run_cli_command(&[
        "api",
        "get",
        &api_id,
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await;

    assert!(result.is_ok(), "API get should succeed");
    let output = result.unwrap();
    assert!(output.contains(&api_id), "Output should contain API ID");
}

#[tokio::test]
async fn test_api_delete() {
    let server = TestServer::start().await;
    let token_response =
        server.issue_token("api-delete-token", &["routes:read", "routes:write"]).await;
    let openapi_file = TempOpenApiFile::new(SIMPLE_OPENAPI);

    // Import an API
    run_cli_command(&[
        "api",
        "import-openapi",
        "--file",
        &openapi_file.path_str(),
        "--team",
        "test-team",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await
    .expect("import API");

    // Get API ID
    let list_output = run_cli_command(&[
        "api",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await
    .expect("list APIs");

    let apis: Vec<serde_json::Value> = serde_json::from_str(&list_output).expect("parse JSON");
    let api_id = apis[0]["id"].as_str().expect("get API ID").to_string();

    // Delete API
    let result = run_cli_command(&[
        "api",
        "delete",
        &api_id,
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_ok(), "API delete should succeed");
}

#[tokio::test]
async fn test_api_bootstrap() {
    let server = TestServer::start().await;
    let token_response =
        server.issue_token("api-bootstrap-token", &["routes:read", "routes:write"]).await;
    let openapi_file = TempOpenApiFile::new(SIMPLE_OPENAPI);

    // Import an API
    run_cli_command(&[
        "api",
        "import-openapi",
        "--file",
        &openapi_file.path_str(),
        "--team",
        "test-team",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await
    .expect("import API");

    // Get API ID
    let list_output = run_cli_command(&[
        "api",
        "list",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await
    .expect("list APIs");

    let apis: Vec<serde_json::Value> = serde_json::from_str(&list_output).expect("parse JSON");
    let api_id = apis[0]["id"].as_str().expect("get API ID").to_string();

    // Get bootstrap config
    let result = run_cli_command(&[
        "api",
        "bootstrap",
        &api_id,
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
        "--output",
        "json",
    ])
    .await;

    assert!(result.is_ok(), "API bootstrap should succeed");
    let output = result.unwrap();

    // Verify bootstrap config structure
    let bootstrap: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    assert!(bootstrap.get("static_resources").is_some(), "Bootstrap should have static_resources");
}

#[tokio::test]
async fn test_api_validate_filters_success() {
    let openapi_file = TempOpenApiFile::new(OPENAPI_WITH_FILTERS);

    let result = run_cli_command(&[
        "api",
        "validate-filters",
        "--file",
        &openapi_file.path_str(),
        "--output",
        "table",
    ])
    .await;

    assert!(result.is_ok(), "Filter validation should succeed");
    let output = result.unwrap();
    assert!(output.contains("Successfully validated"), "Output: {}", output);
    assert!(output.contains("cors"), "Should mention cors filter");
    assert!(output.contains("header_mutation"), "Should mention header_mutation filter");
}

#[tokio::test]
async fn test_api_validate_filters_no_filters() {
    let openapi_file = TempOpenApiFile::new(SIMPLE_OPENAPI);

    let result =
        run_cli_command(&["api", "validate-filters", "--file", &openapi_file.path_str()]).await;

    assert!(result.is_ok(), "Validation should succeed even with no filters");
    let output = result.unwrap();
    assert!(output.contains("No x-flowplane-filters found"), "Output: {}", output);
}

#[tokio::test]
async fn test_api_validate_filters_invalid_syntax() {
    let invalid_filters = r#"
openapi: 3.0.0
info:
  title: Invalid Filters API
  version: 1.0.0
x-flowplane-filters:
  - filter:
      type: invalid_filter_type
      some_config: value
servers:
  - url: https://api.example.com
paths:
  /test:
    get:
      responses:
        '200':
          description: OK
"#;
    let openapi_file = TempOpenApiFile::new(invalid_filters);

    let result =
        run_cli_command(&["api", "validate-filters", "--file", &openapi_file.path_str()]).await;

    assert!(result.is_err(), "Validation should fail with invalid filter syntax");
    let error = result.unwrap_err();
    assert!(error.contains("validation failed") || error.contains("invalid"), "Error: {}", error);
}

#[tokio::test]
async fn test_api_import_invalid_file() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("api-import-invalid-token", &["routes:write"]).await;

    let result = run_cli_command(&[
        "api",
        "import-openapi",
        "--file",
        "/nonexistent/file.yaml",
        "--team",
        "test-team",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_err(), "Import should fail with nonexistent file");
    let error = result.unwrap_err();
    assert!(
        error.contains("No such file") || error.contains("not found"),
        "Error should indicate file not found: {}",
        error
    );
}

#[tokio::test]
async fn test_api_get_nonexistent_id() {
    let server = TestServer::start().await;
    let token_response = server.issue_token("api-get-404-token", &["routes:read"]).await;

    let result = run_cli_command(&[
        "api",
        "get",
        "nonexistent-id-12345",
        "--token",
        &token_response.token,
        "--base-url",
        &server.base_url(),
    ])
    .await;

    assert!(result.is_err(), "Get should fail with nonexistent ID");
    let error = result.unwrap_err();
    assert!(
        error.contains("404") || error.contains("Not found") || error.contains("not found"),
        "Error should indicate not found: {}",
        error
    );
}
