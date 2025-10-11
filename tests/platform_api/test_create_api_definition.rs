use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{read_json, send_request, setup_platform_api_app};

#[tokio::test]
async fn create_api_definition_persists_record_and_returns_bootstrap() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("platform-admin", &["api-definitions:write"]).await;

    let payload = json!({
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
                "timeoutSeconds": 3,
                "filters": {
                    "cors": "allow-authenticated"
                }
            },
            {
                "match": { "path": "/healthz" },
                "cluster": {
                    "name": "payments-admin",
                    "endpoint": "payments-admin.svc.cluster.local:8080"
                },
                "timeoutSeconds": 1
            }
        ],
        "tls": {
            "mode": "mutual",
            "cert": "arn:aws:secretsmanager:us-east-1:123456789012:secret:payments-cert",
            "key": "arn:aws:secretsmanager:us-east-1:123456789012:secret:payments-key"
        }
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
    assert!(body.get("id").and_then(|v| v.as_str()).is_some(), "response id missing");
    assert!(body.get("bootstrapUri").and_then(|v| v.as_str()).is_some(), "bootstrap uri missing");

    let definition_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_definitions")
        .fetch_one(&app.pool)
        .await
        .expect("count api definitions");
    assert_eq!(definition_count, 1);

    let stored_domain: String =
        sqlx::query_scalar("SELECT domain FROM api_definitions WHERE team = 'payments'")
            .fetch_one(&app.pool)
            .await
            .expect("fetch domain");
    assert_eq!(stored_domain, "payments.flowplane.dev");

    let route_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_routes")
        .fetch_one(&app.pool)
        .await
        .expect("count api routes");
    assert_eq!(route_count, 2, "all requested routes should be stored");
}
