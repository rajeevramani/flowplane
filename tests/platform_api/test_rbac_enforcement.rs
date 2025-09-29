use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{send_request, setup_platform_api_app};

#[tokio::test]
async fn creating_api_without_required_scope_is_forbidden() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("limited-user", &["clusters:write"]).await;

    let payload = json!({
        "team": "payments",
        "domain": "payments.flowplane.dev",
        "routes": [
            {
                "match": { "prefix": "/v1/" },
                "cluster": {
                    "name": "payments-backend",
                    "endpoint": "payments.svc.cluster.local:8443"
                }
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

    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let definition_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_definitions")
        .fetch_one(&app.pool)
        .await
        .expect("count definitions after forbidden call");
    assert_eq!(definition_count, 0);
}
