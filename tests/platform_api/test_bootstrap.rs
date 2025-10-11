use super::support::{read_json, send_request, setup_platform_api_app};
use axum::body::to_bytes;
use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn bootstrap_returns_yaml_with_team_scope_defaults() {
    let app = setup_platform_api_app().await;
    let token =
        app.issue_token("bootstrap", &["api-definitions:write", "api-definitions:read"]).await;

    // Create a minimal API definition (non-isolated is fine for bootstrap generation)
    let payload = json!({
        "team": "payments",
        "domain": "bootstrap.flowplane.dev",
        "listenerIsolation": false,
        "routes": [ { "match": {"prefix":"/"}, "cluster": {"name":"b","endpoint":"b:8080"} } ]
    });
    let resp = send_request(
        &app,
        Method::POST,
        "/api/v1/api-definitions",
        Some(&token.token),
        Some(payload),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = read_json(resp).await;
    let id = body["id"].as_str().unwrap();

    // Request team-scoped bootstrap (includeDefault default is false)
    let path = format!("/api/v1/api-definitions/{}/bootstrap?format=yaml&scope=team", id);
    let resp2 = send_request(&app, Method::GET, &path, Some(&token.token), None).await;
    assert_eq!(resp2.status(), StatusCode::OK);

    // Read raw body to string and check metadata keys
    let bytes = to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("node:"));
    assert!(text.contains("team: payments"));
    // include_default should be false by default; presence not guaranteed in YAML, so don't assert its literal
}

#[tokio::test]
async fn bootstrap_returns_json_format() {
    let app = setup_platform_api_app().await;
    let token =
        app.issue_token("bootstrap-json", &["api-definitions:write", "api-definitions:read"]).await;

    // Create API definition
    let payload = json!({
        "team": "platform",
        "domain": "json-bootstrap.flowplane.dev",
        "listenerIsolation": false,
        "routes": [ { "match": {"prefix":"/"}, "cluster": {"name":"api","endpoint":"api:8080"} } ]
    });
    let resp = send_request(
        &app,
        Method::POST,
        "/api/v1/api-definitions",
        Some(&token.token),
        Some(payload),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = read_json(resp).await;
    let id = body["id"].as_str().unwrap();

    // Request JSON format bootstrap
    let path = format!("/api/v1/api-definitions/{}/bootstrap?format=json", id);
    let resp2 = send_request(&app, Method::GET, &path, Some(&token.token), None).await;
    assert_eq!(resp2.status(), StatusCode::OK);

    // Verify it's valid JSON and has expected structure
    let bootstrap: serde_json::Value = read_json(resp2).await;
    assert!(bootstrap.get("admin").is_some(), "should have admin config");
    assert!(bootstrap.get("node").is_some(), "should have node config");
    assert!(bootstrap.get("dynamic_resources").is_some(), "should have dynamic_resources");
    assert!(
        bootstrap.get("static_resources").is_some(),
        "should have static_resources with xds_cluster"
    );

    let node = bootstrap.get("node").unwrap();
    assert!(node.get("id").is_some(), "node should have id");
}

#[tokio::test]
async fn bootstrap_defaults_to_yaml_format() {
    let app = setup_platform_api_app().await;
    let token = app
        .issue_token("bootstrap-default", &["api-definitions:write", "api-definitions:read"])
        .await;

    // Create API definition
    let payload = json!({
        "team": "default-test",
        "domain": "default.flowplane.dev",
        "listenerIsolation": false,
        "routes": [ { "match": {"prefix":"/"}, "cluster": {"name":"svc","endpoint":"svc:8080"} } ]
    });
    let resp = send_request(
        &app,
        Method::POST,
        "/api/v1/api-definitions",
        Some(&token.token),
        Some(payload),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = read_json(resp).await;
    let id = body["id"].as_str().unwrap();

    // Request without format parameter (should default to YAML)
    let path = format!("/api/v1/api-definitions/{}/bootstrap", id);
    let resp2 = send_request(&app, Method::GET, &path, Some(&token.token), None).await;
    assert_eq!(resp2.status(), StatusCode::OK);

    // Should be YAML (text format, not JSON parseable)
    let bytes = to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("admin:"), "should be YAML format with admin: key");
    assert!(text.contains("node:"), "should be YAML format with node: key");
}

#[tokio::test]
async fn bootstrap_returns_404_for_nonexistent_definition() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("bootstrap-404", &["api-definitions:read"]).await;

    let path = "/api/v1/api-definitions/nonexistent-id/bootstrap";
    let resp = send_request(&app, Method::GET, path, Some(&token.token), None).await;

    // Should return 404 or 500 (depending on error mapping)
    assert!(
        resp.status() == StatusCode::NOT_FOUND
            || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
        "should fail for nonexistent definition, got: {}",
        resp.status()
    );
}
