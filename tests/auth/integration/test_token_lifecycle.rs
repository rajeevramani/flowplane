use axum::http::{Method, StatusCode};
use serde_json::json;

use crate::support::{read_json, send_request, setup_test_app};
use flowplane::auth::{
    models::{PersonalAccessToken, TokenStatus},
    token_service::TokenSecretResponse,
};

#[tokio::test]
async fn integration_token_lifecycle_flow() {
    let app = setup_test_app().await;

    let admin = app.issue_token("token-admin", &["tokens:write", "tokens:read"]).await;

    // Create a new token via API.
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/tokens",
        Some(&admin.token),
        Some(json!({
            "name": "lifecycle-api",
            "description": "api created",
            "scopes": ["clusters:read"]
        })),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let created: TokenSecretResponse = read_json(create_response).await;

    // Fetch the token.
    let get_url = format!("/api/v1/tokens/{}", created.id);
    let get_response = send_request(&app, Method::GET, &get_url, Some(&admin.token), None).await;
    assert_eq!(get_response.status(), StatusCode::OK);
    let fetched: PersonalAccessToken = read_json(get_response).await;
    assert_eq!(fetched.name, "lifecycle-api");

    // Update metadata.
    let update_response = send_request(
        &app,
        Method::PATCH,
        &get_url,
        Some(&admin.token),
        Some(json!({ "description": "updated", "scopes": ["clusters:read", "routes:read"] })),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);
    let updated: PersonalAccessToken = read_json(update_response).await;
    assert!(updated.scopes.contains(&"routes:read".into()));
    assert_eq!(updated.description.as_deref(), Some("updated"));

    // Rotate the secret and ensure we receive a new value.
    let rotate_url = format!("/api/v1/tokens/{}/rotate", created.id);
    let rotate_response =
        send_request(&app, Method::POST, &rotate_url, Some(&admin.token), None).await;
    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotated: TokenSecretResponse = read_json(rotate_response).await;
    assert_ne!(rotated.token, created.token);

    // Revoke the token.
    let revoke_response =
        send_request(&app, Method::DELETE, &get_url, Some(&admin.token), None).await;
    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoked: PersonalAccessToken = read_json(revoke_response).await;
    assert_eq!(revoked.status, TokenStatus::Revoked);
}
