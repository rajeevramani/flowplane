use axum::http::{Method, StatusCode};
use flowplane::auth::models::{PersonalAccessToken, TokenStatus};

use crate::support::{read_json, send_request, setup_test_app};

#[tokio::test]
async fn contract_delete_tokens_revokes_token() {
    let app = setup_test_app().await;

    let admin = app.issue_token("token-revoker", &["tokens:write", "tokens:read"]).await;

    let created = app
        .token_service
        .create_token(flowplane::auth::validation::CreateTokenRequest {
            name: "revoke-me".into(),
            description: None,
            expires_at: None,
            scopes: vec!["routes:read".into()],
            created_by: Some("tests".into()),
            user_id: None,
            user_email: None,
        })
        .await
        .unwrap();

    let url = format!("/api/v1/tokens/{}", created.id);
    let response = send_request(&app, Method::DELETE, &url, Some(&admin.token), None).await;
    assert_eq!(response.status(), StatusCode::OK);

    let token: PersonalAccessToken = read_json(response).await;
    assert_eq!(token.status, TokenStatus::Revoked);
    assert!(token.scopes.is_empty());
}
