use std::collections::HashSet;

use axum::http::{Method, StatusCode};
use flowplane::auth::token_service::TokenSecretResponse;
use serde_json::json;

use crate::support::{read_json, send_request, setup_test_app};

#[tokio::test]
async fn contract_post_tokens_creates_token() {
    let app = setup_test_app().await;

    let admin =
        app.issue_token("admin-writer", &["tokens:write", "tokens:read", "clusters:read"]).await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/tokens",
        Some(&admin.token),
        Some(json!({
            "name": "ci-token",
            "description": "Continuous integration",
            "scopes": ["tokens:read", "clusters:read"],
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);

    let created: TokenSecretResponse = read_json(response).await;
    assert!(created.token.starts_with(&format!("fp_pat_{}", created.id)));

    let stored = app.token_service.get_token(&created.id).await.unwrap();
    assert_eq!(stored.name, "ci-token");
    let scopes: HashSet<_> = stored.scopes.iter().cloned().collect();
    assert!(scopes.contains("tokens:read"));
    assert!(scopes.contains("clusters:read"));
}
