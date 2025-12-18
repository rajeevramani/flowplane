use axum::http::{Method, StatusCode};
use flowplane::auth::models::{PersonalAccessToken, TokenStatus};
use serde_json::json;

use crate::support::{read_json, send_request, setup_test_app};

#[tokio::test]
async fn contract_patch_tokens_updates_metadata() {
    let app = setup_test_app().await;

    let admin = app.issue_token("token-writer", &["tokens:write", "tokens:read"]).await;

    let created = app
        .token_service
        .create_token(
            flowplane::auth::validation::CreateTokenRequest {
                name: "update-token".into(),
                description: Some("before".into()),
                expires_at: None,
                scopes: vec!["routes:read".into()],
                created_by: Some("tests".into()),
                user_id: None,
                user_email: None,
            },
            None,
        )
        .await
        .unwrap();

    let url = format!("/api/v1/tokens/{}", created.id);
    let response = send_request(
        &app,
        Method::PATCH,
        &url,
        Some(&admin.token),
        Some(json!({
            "description": "after",
            "status": "active",
            "scopes": ["routes:read", "clusters:read"]
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let token: PersonalAccessToken = read_json(response).await;
    assert_eq!(token.description.as_deref(), Some("after"));
    assert!(token.scopes.contains(&"clusters:read".into()));
    assert_eq!(token.status, TokenStatus::Active);
}
