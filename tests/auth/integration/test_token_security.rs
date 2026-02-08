use axum::http::{Method, StatusCode};
use serde_json::json;

use crate::support::{read_json, send_request, setup_test_app};
use flowplane::auth::token_service::TokenSecretResponse;

#[tokio::test]
async fn integration_token_security_verifies_rotation_and_revocation() {
    let app = setup_test_app().await;

    let admin =
        app.issue_token("security-admin", &["admin:all", "tokens:write", "tokens:read"]).await;

    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/tokens",
        Some(&admin.token),
        Some(json!({
            "name": "security-target",
            "scopes": ["admin:all", "tokens:read"]
        })),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let created: TokenSecretResponse = read_json(create_response).await;

    // Rotate the token and capture the new secret.
    let rotate_url = format!("/api/v1/tokens/{}/rotate", created.id);
    let rotate_response =
        send_request(&app, Method::POST, &rotate_url, Some(&admin.token), None).await;
    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotated: TokenSecretResponse = read_json(rotate_response).await;

    // Old secret should fail authentication.
    let fail_response =
        send_request(&app, Method::GET, "/api/v1/tokens", Some(&created.token), None).await;
    assert_eq!(fail_response.status(), StatusCode::UNAUTHORIZED);

    // New secret succeeds.
    let success_response =
        send_request(&app, Method::GET, "/api/v1/tokens", Some(&rotated.token), None).await;
    assert_eq!(success_response.status(), StatusCode::OK);

    // Revoke the token and ensure it can no longer access the API.
    let revoke_url = format!("/api/v1/tokens/{}", created.id);
    let revoke_response =
        send_request(&app, Method::DELETE, &revoke_url, Some(&admin.token), None).await;
    assert_eq!(revoke_response.status(), StatusCode::OK);

    let after_revoke =
        send_request(&app, Method::GET, "/api/v1/tokens", Some(&rotated.token), None).await;
    assert_eq!(after_revoke.status(), StatusCode::UNAUTHORIZED);
}
