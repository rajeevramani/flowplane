use axum::http::{Method, StatusCode};

use crate::support::{send_request, setup_test_app};

#[tokio::test]
async fn integration_auth_middleware_enforces_bearer_tokens() {
    let app = setup_test_app().await;

    // Missing bearer header returns 401.
    let response = send_request(&app, Method::GET, "/api/v1/tokens", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Malformed bearer header also returns 401.
    let response =
        send_request(&app, Method::GET, "/api/v1/tokens", Some("not-a-valid-token"), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
