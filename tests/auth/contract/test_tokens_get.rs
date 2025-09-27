use axum::http::{Method, StatusCode};
use flowplane::auth::models::PersonalAccessToken;

use crate::support::{read_json, send_request, setup_test_app};

#[tokio::test]
async fn contract_get_tokens_id_returns_token() {
    let app = setup_test_app().await;

    let admin = app.issue_token("token-reader", &["tokens:read", "tokens:write"]).await;

    let created = app
        .token_service
        .create_token(flowplane::auth::validation::CreateTokenRequest {
            name: "get-token".into(),
            description: Some("sample".into()),
            expires_at: None,
            scopes: vec!["routes:read".into()],
            created_by: Some("tests".into()),
        })
        .await
        .unwrap();

    let url = format!("/api/v1/tokens/{}", created.id);
    let response = send_request(&app, Method::GET, &url, Some(&admin.token), None).await;
    assert_eq!(response.status(), StatusCode::OK);

    let token: PersonalAccessToken = read_json(response).await;
    assert_eq!(token.name, "get-token");
    assert_eq!(token.description.as_deref(), Some("sample"));
}
