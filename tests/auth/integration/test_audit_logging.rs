use axum::http::{Method, StatusCode};
use serde_json::json;

use crate::support::{read_json, send_request, setup_test_app};
use flowplane::auth::{models::PersonalAccessToken, token_service::TokenSecretResponse};

#[tokio::test]
async fn integration_audit_logging_records_events() {
    let app = setup_test_app().await;
    let admin = app.issue_token("audit-admin", &["tokens:write", "tokens:read"]).await;

    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/tokens",
        Some(&admin.token),
        Some(json!({
            "name": "audit-sample",
            "scopes": ["clusters:read"]
        })),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let created: TokenSecretResponse = read_json(create_response).await;

    let rotate_url = format!("/api/v1/tokens/{}/rotate", created.id);
    let rotate_response =
        send_request(&app, Method::POST, &rotate_url, Some(&admin.token), None).await;
    assert_eq!(rotate_response.status(), StatusCode::OK);
    let revoke_url = format!("/api/v1/tokens/{}", created.id);
    let revoke_response =
        send_request(&app, Method::DELETE, &revoke_url, Some(&admin.token), None).await;
    assert_eq!(revoke_response.status(), StatusCode::OK);
    let _: PersonalAccessToken = read_json(revoke_response).await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE resource_type = 'auth.token' AND resource_id = ?",
    )
    .bind(&created.id)
    .fetch_one(&app.pool)
    .await
    .expect("count audit events");

    // Expect create, rotate, revoke, and authenticate entries (authenticate occurs for each API call).
    assert!(count >= 3);
}
