use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{read_json, send_request, setup_platform_api_app};

#[tokio::test]
async fn append_route_to_existing_definition() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("platform-admin", &["routes:write"]).await;

    let create_payload = json!({
        "team": "payments",
        "domain": "payments.flowplane.dev",
        "listenerIsolation": false,
        "routes": [
            {
                "match": { "prefix": "/v1/" },
                "cluster": {
                    "name": "payments-backend",
                    "endpoint": "payments.svc.cluster.local:8443"
                },
                "timeoutSeconds": 3
            }
        ]
    });

    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/api-definitions",
        Some(&token.token),
        Some(create_payload),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let created_body: serde_json::Value = read_json(create_response).await;
    let api_id = created_body["id"].as_str().expect("api id present").to_string();

    let append_payload = json!({
        "route": {
            "match": { "prefix": "/v2/" },
            "cluster": {
                "name": "payments-backend",
                "endpoint": "payments.svc.cluster.local:8443"
            },
            "timeoutSeconds": 5,
            "rewrite": { "prefix": "/internal/v2/" }
        },
        "deploymentNote": "enable /v2 rollout"
    });

    let append_path = format!("/api/v1/api-definitions/{}/routes", api_id);
    let append_response =
        send_request(&app, Method::POST, &append_path, Some(&token.token), Some(append_payload))
            .await;
    assert_eq!(append_response.status(), StatusCode::ACCEPTED);
    let append_body: serde_json::Value = read_json(append_response).await;
    assert!(append_body.get("routeId").and_then(|v| v.as_str()).is_some());
    assert!(append_body.get("revision").and_then(|v| v.as_i64()).is_some());

    let route_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_routes")
        .fetch_one(&app.pool)
        .await
        .expect("count routes after append");
    assert_eq!(route_count, 2, "append should only add one additional route");

    let version: i64 = sqlx::query_scalar("SELECT version FROM api_definitions WHERE id = ?")
        .bind(&api_id)
        .fetch_one(&app.pool)
        .await
        .expect("fetch definition version");
    assert!(version >= 2, "version should bump after append");
}
