use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use super::support::{read_json, send_request, setup_platform_api_app};

#[tokio::test]
async fn platform_api_create_append_and_persist_routes() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("lifecycle", &["api-definitions:write"]).await;

    let create_payload = json!({
        "team": "payments",
        "domain": "docs.flowplane.dev",
        "listenerIsolation": false,
        "routes": [
            {
                "match": { "prefix": "/v1/" },
                "cluster": {
                    "name": "payments-api",
                    "endpoint": "payments.svc.cluster.local:8443"
                },
                "timeoutSeconds": 5,
                "filters": { "cors": "allow-authenticated" }
            }
        ]
    });

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/api-definitions",
        Some(&token.token),
        Some(create_payload),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = read_json(response).await;
    let api_id = body.get("id").and_then(|v| v.as_str()).expect("api id");
    let bootstrap_uri = body.get("bootstrapUri").and_then(|v| v.as_str()).expect("bootstrap uri");

    // Bootstrap is now served dynamically via API endpoint (no file on disk)
    assert_eq!(
        bootstrap_uri,
        format!("/api/v1/api-definitions/{}/bootstrap", api_id),
        "bootstrap URI should point to API endpoint"
    );

    let append_payload = json!({
        "route": {
            "match": { "prefix": "/admin/" },
            "cluster": {
                "name": "payments-admin",
                "endpoint": "payments-admin.svc.cluster.local:8080"
            },
            "timeoutSeconds": 3
        },
        "deploymentNote": "add admin surface"
    });

    let append_response = send_request(
        &app,
        Method::POST,
        &format!("/api/v1/api-definitions/{}/routes", api_id),
        Some(&token.token),
        Some(append_payload),
    )
    .await;

    assert_eq!(append_response.status(), StatusCode::ACCEPTED);

    // Verify both routes are persisted with canonical override structure
    let route_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM api_routes WHERE api_definition_id = $1")
            .bind(api_id)
            .fetch_one(&app.pool)
            .await
            .expect("count routes");
    assert_eq!(route_count, 2);

    let stored_override: Option<String> = sqlx::query_scalar(
        "SELECT override_config FROM api_routes WHERE api_definition_id = $1 ORDER BY route_order LIMIT 1",
    )
    .bind(api_id)
    .fetch_one(&app.pool)
    .await
    .expect("fetch override config");

    let override_json: Value =
        serde_json::from_str(&stored_override.expect("override config exists"))
            .expect("parse override config");
    assert!(
        override_json.get("cors").and_then(|value| value.get("policy")).is_some(),
        "canonical CORS policy should be stored"
    );
}
