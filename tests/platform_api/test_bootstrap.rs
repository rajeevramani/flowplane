use super::support::{read_json, send_request, setup_platform_api_app};
use axum::body::to_bytes;
use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn bootstrap_returns_yaml_with_team_scope_defaults() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("bootstrap", &["routes:write", "routes:read"]).await;

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
