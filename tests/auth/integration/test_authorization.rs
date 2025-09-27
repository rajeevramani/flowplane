use axum::http::{Method, StatusCode};
use serde_json::json;

use crate::support::{send_request, setup_test_app};

#[tokio::test]
async fn integration_authorization_checks_scopes() {
    let app = setup_test_app().await;

    let read_only = app.issue_token("read-only", &["tokens:read"]).await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/tokens",
        Some(&read_only.token),
        Some(json!({
            "name": "should-fail",
            "scopes": ["clusters:read"]
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
