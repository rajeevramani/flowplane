use axum::http::{Method, StatusCode};
use flowplane::auth::models::PersonalAccessToken;

use crate::support::{read_json, send_request, setup_test_app};

#[tokio::test]
async fn contract_get_tokens_lists_tokens() {
    let app = setup_test_app().await;

    let admin = app.issue_token("tokens-reader", &["tokens:read", "tokens:write"]).await;

    let _ = app
        .token_service
        .create_token(flowplane::auth::validation::CreateTokenRequest {
            name: "list-one".into(),
            description: None,
            expires_at: None,
            scopes: vec!["clusters:read".into()],
            created_by: Some("tests".into()),
            user_id: None,
            user_email: None,
        })
        .await
        .unwrap();

    let _ = app
        .token_service
        .create_token(flowplane::auth::validation::CreateTokenRequest {
            name: "list-two".into(),
            description: None,
            expires_at: None,
            scopes: vec!["routes:read".into()],
            created_by: Some("tests".into()),
            user_id: None,
            user_email: None,
        })
        .await
        .unwrap();

    let response =
        send_request(&app, Method::GET, "/api/v1/tokens?limit=10", Some(&admin.token), None).await;
    assert_eq!(response.status(), StatusCode::OK);

    let tokens: Vec<PersonalAccessToken> = read_json(response).await;
    assert!(tokens.iter().any(|t| t.name == "list-one"));
    assert!(tokens.iter().any(|t| t.name == "list-two"));
}
