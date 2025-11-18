use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{read_json, send_request, setup_platform_api_app};
use flowplane::storage::repository::{CreateListenerRequest, ListenerRepository};

#[tokio::test]
async fn create_isolated_api_creates_dedicated_listener() {
    let app = setup_platform_api_app().await;
    let token = app
        .issue_token(
            "platform-admin",
            &["api-definitions:write", "api-definitions:read", "listeners:read"],
        )
        .await;

    let payload = json!({
        "team": "payments",
        "domain": "iso.flowplane.dev",
        "listener": {
            "name": "iso-listener-1",
            "bindAddress": "0.0.0.0",
            "port": 10011,
            "protocol": "HTTP"
        },
        "routes": [
            {
                "match": { "prefix": "/api" },
                "cluster": { "name": "backend", "endpoint": "backend.svc:8080" },
                "timeoutSeconds": 3
            }
        ]
    });

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/api-definitions",
        Some(&token.token),
        Some(payload),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let body: serde_json::Value = read_json(response).await;
    let api_id = body["id"].as_str().expect("id");
    assert!(body.get("bootstrapUri").is_some());

    // Verify listener exists via repository query
    let repo = ListenerRepository::new(app.pool.clone());
    let listeners = repo.list(Some(100), None).await.expect("list listeners");
    assert!(listeners.iter().any(|l| l.name == "iso-listener-1" && l.port == Some(10011)));

    // Verify definition is returned by GET list
    let list_resp =
        send_request(&app, Method::GET, "/api/v1/api-definitions", Some(&token.token), None).await;
    assert_eq!(list_resp.status(), StatusCode::OK);
    let items: serde_json::Value = read_json(list_resp).await;
    assert!(items.as_array().unwrap().iter().any(|v| v["id"] == api_id));
}

#[tokio::test]
async fn isolated_port_conflict_rolls_back_definition() {
    let app = setup_platform_api_app().await;
    let token =
        app.issue_token("platform-admin", &["api-definitions:write", "listeners:write"]).await;

    // Seed a conflicting listener at 0.0.0.0:10012
    let repo = ListenerRepository::new(app.pool.clone());
    let _existing = repo
        .create(CreateListenerRequest {
            name: "pre-existing".into(),
            address: "0.0.0.0".into(),
            port: Some(10012),
            protocol: Some("HTTP".into()),
            configuration: serde_json::json!({"note":"seed"}),
            team: Some("test".into()),
        })
        .await
        .expect("seed listener");

    let payload = json!({
        "team": "payments",
        "domain": "conflict.flowplane.dev",
        "listener": { "bindAddress": "0.0.0.0", "port": 10012, "protocol": "HTTP" },
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
    assert!(resp.status().is_client_error(), "should fail on port conflict");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_definitions")
        .fetch_one(&app.pool)
        .await
        .expect("count definitions");
    assert_eq!(count, 0, "definition should not be persisted on conflict");
}
