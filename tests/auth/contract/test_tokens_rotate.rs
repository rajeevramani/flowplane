use axum::http::{Method, StatusCode};
use flowplane::auth::token_service::TokenSecretResponse;

use crate::support::{read_json, send_request, setup_test_app};

#[tokio::test]
async fn contract_post_tokens_rotate_creates_new_secret() {
    let app = setup_test_app().await;

    let admin = app.issue_token("token-rotator", &["tokens:write", "tokens:read"]).await;

    let created = app
        .token_service
        .create_token(
            flowplane::auth::validation::CreateTokenRequest {
                name: "rotate-me".into(),
                description: None,
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

    let url = format!("/api/v1/tokens/{}/rotate", created.id);
    let response = send_request(&app, Method::POST, &url, Some(&admin.token), None).await;
    assert_eq!(response.status(), StatusCode::OK);

    let rotated: TokenSecretResponse = read_json(response).await;
    assert_ne!(rotated.token, created.token);
    assert!(rotated.token.starts_with(&format!("fp_pat_{}", created.id)));
}
