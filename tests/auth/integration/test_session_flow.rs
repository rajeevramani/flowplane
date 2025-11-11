/// Integration tests for session management flow
///
/// Tests:
/// 1. Bootstrap initialization generates setup token
/// 2. Setup token can be exchanged for session
/// 3. Session includes CSRF token
/// 4. Session can be used for authenticated requests
/// 5. Logout revokes session
/// 6. Revoked session cannot be used
use axum::http::{Method, StatusCode};
use axum_extra::extract::cookie::Cookie;
use serde_json::json;
use tower::ServiceExt;

use crate::support::{read_json, send_request, setup_test_app};

#[tokio::test]
async fn bootstrap_generates_setup_token_on_uninitialized_system() {
    let app = setup_test_app().await;

    // Bootstrap should succeed when no tokens exist
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/bootstrap/initialize",
        None,
        Some(json!({
            "adminEmail": "admin@example.com"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);

    let body: serde_json::Value = read_json(response).await;
    assert!(body.get("setupToken").is_some(), "should have setup token");
    assert!(body["setupToken"].as_str().unwrap().starts_with("fp_setup_"));
    assert!(body.get("expiresAt").is_some(), "should have expiration");
    assert!(body.get("maxUsageCount").is_some(), "should have usage count");
    assert_eq!(body["maxUsageCount"].as_i64().unwrap(), 1, "should be single-use");
}

#[tokio::test]
async fn bootstrap_fails_when_system_already_initialized() {
    let app = setup_test_app().await;

    // Create a token first (system is now initialized)
    app.issue_token("existing-token", &["tokens:read", "tokens:write"]).await;

    // Bootstrap should now fail
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/bootstrap/initialize",
        None,
        Some(json!({
            "adminEmail": "admin@example.com"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bootstrap_validates_email_format() {
    let app = setup_test_app().await;

    // Invalid email should fail validation
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/bootstrap/initialize",
        None,
        Some(json!({
            "adminEmail": "not-an-email"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn setup_token_can_be_exchanged_for_session() {
    let app = setup_test_app().await;

    // 1. Bootstrap to get setup token
    let bootstrap_response = send_request(
        &app,
        Method::POST,
        "/api/v1/bootstrap/initialize",
        None,
        Some(json!({
            "adminEmail": "admin@example.com"
        })),
    )
    .await;
    let bootstrap_body: serde_json::Value = read_json(bootstrap_response).await;
    let setup_token = bootstrap_body["setupToken"].as_str().unwrap();

    // 2. Exchange setup token for session
    let session_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/sessions",
        None,
        Some(json!({
            "setupToken": setup_token
        })),
    )
    .await;

    assert_eq!(session_response.status(), StatusCode::CREATED);

    // 3. Verify session response
    let session_body: serde_json::Value = read_json(session_response).await;
    assert!(session_body.get("sessionId").is_some(), "should have session ID");
    assert!(session_body.get("csrfToken").is_some(), "should have CSRF token");
    assert!(session_body.get("expiresAt").is_some(), "should have expiration");
    assert!(session_body.get("teams").is_some(), "should have teams");
    assert!(session_body.get("scopes").is_some(), "should have scopes");
}

#[tokio::test]
async fn session_creation_fails_with_invalid_setup_token() {
    let app = setup_test_app().await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/sessions",
        None,
        Some(json!({
            "setupToken": "fp_setup_invalid.badtoken123"
        })),
    )
    .await;

    // Could be 401 (unauthorized) or 404 (not found) depending on implementation
    assert!(
        response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::NOT_FOUND,
        "Expected 401 or 404, got {}",
        response.status()
    );
}

#[tokio::test]
async fn session_creation_fails_with_expired_setup_token() {
    let app = setup_test_app().await;

    // Create an expired setup token directly in the database
    use chrono::{Duration, Utc};
    use flowplane::auth::models::{NewPersonalAccessToken, TokenStatus};
    use flowplane::auth::setup_token::SetupToken;
    use flowplane::domain::TokenId;
    use flowplane::storage::repository::{SqlxTokenRepository, TokenRepository};

    let generator = SetupToken::new();
    let (token_value, hashed_secret, _) = generator.generate(Some(1), Some(7)).unwrap();

    let token_id = token_value.strip_prefix("fp_setup_").and_then(|s| s.split('.').next()).unwrap();

    let new_token = NewPersonalAccessToken {
        id: TokenId::from_string(token_id.to_string()),
        name: "expired-setup".to_string(),
        description: Some("Expired test setup token".to_string()),
        hashed_secret,
        status: TokenStatus::Active,
        expires_at: Some(Utc::now() - Duration::hours(1)), // Already expired
        created_by: Some("test".to_string()),
        scopes: vec!["bootstrap:initialize".to_string()],
        is_setup_token: true,
        max_usage_count: Some(1),
        usage_count: 0,
        failed_attempts: 0,
        locked_until: None,
    };

    let token_repo = SqlxTokenRepository::new(app.pool.clone());
    token_repo.create_token(new_token).await.unwrap();

    // Try to use the expired setup token
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/sessions",
        None,
        Some(json!({
            "setupToken": token_value
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn session_can_be_used_for_get_requests_without_csrf() {
    let app = setup_test_app().await;

    // 1. Bootstrap and create session
    let bootstrap_response = send_request(
        &app,
        Method::POST,
        "/api/v1/bootstrap/initialize",
        None,
        Some(json!({
            "adminEmail": "admin@example.com"
        })),
    )
    .await;
    let bootstrap_body: serde_json::Value = read_json(bootstrap_response).await;
    let setup_token = bootstrap_body["setupToken"].as_str().unwrap();

    let session_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/sessions",
        None,
        Some(json!({
            "setupToken": setup_token
        })),
    )
    .await;

    // Extract session token from Set-Cookie header BEFORE consuming body
    let set_cookie_header = session_response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("should have set-cookie header")
        .to_string();

    let session_body: serde_json::Value = read_json(session_response).await;

    let cookie = Cookie::parse(&set_cookie_header).unwrap();
    let session_token = cookie.value();

    // 2. Use session token for GET request (no CSRF needed)
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/auth/sessions/me")
        .header("Cookie", format!("fp_session={}", session_token))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let session_info: serde_json::Value = read_json(response).await;
    assert_eq!(
        session_info["sessionId"].as_str().unwrap(),
        session_body["sessionId"].as_str().unwrap()
    );
}

#[tokio::test]
async fn session_post_request_requires_csrf_token() {
    let app = setup_test_app().await;

    // 1. Bootstrap and create session
    let bootstrap_response = send_request(
        &app,
        Method::POST,
        "/api/v1/bootstrap/initialize",
        None,
        Some(json!({
            "adminEmail": "admin@example.com"
        })),
    )
    .await;
    let bootstrap_body: serde_json::Value = read_json(bootstrap_response).await;
    let setup_token = bootstrap_body["setupToken"].as_str().unwrap();

    let session_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/sessions",
        None,
        Some(json!({
            "setupToken": setup_token
        })),
    )
    .await;

    // Extract session token from Set-Cookie header
    let set_cookie_header = session_response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("should have set-cookie header");

    let cookie = Cookie::parse(set_cookie_header).unwrap();
    let session_token = cookie.value();

    // 2. Try POST to a protected endpoint without CSRF token (should fail)
    // Use tokens endpoint instead of logout since logout is public
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/tokens")
        .header("Cookie", format!("fp_session={}", session_token))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "name": "test-token",
                "scopes": ["tokens:read"]
            }))
            .unwrap(),
        ))
        .unwrap();

    let response = app.router().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "POST request without CSRF should be forbidden"
    );
}

#[tokio::test]
async fn logout_revokes_session() {
    let app = setup_test_app().await;

    // 1. Bootstrap and create session
    let bootstrap_response = send_request(
        &app,
        Method::POST,
        "/api/v1/bootstrap/initialize",
        None,
        Some(json!({
            "adminEmail": "admin@example.com"
        })),
    )
    .await;
    let bootstrap_body: serde_json::Value = read_json(bootstrap_response).await;
    let setup_token = bootstrap_body["setupToken"].as_str().unwrap();

    let session_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/sessions",
        None,
        Some(json!({
            "setupToken": setup_token
        })),
    )
    .await;

    // Extract session token from Set-Cookie header BEFORE consuming body
    let set_cookie_header = session_response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("should have set-cookie header")
        .to_string();

    let session_body: serde_json::Value = read_json(session_response).await;
    let csrf_token = session_body["csrfToken"].as_str().unwrap();

    let cookie = Cookie::parse(&set_cookie_header).unwrap();
    let session_token = cookie.value();

    // 2. Verify session works before logout
    let get_request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/auth/sessions/me")
        .header("Cookie", format!("fp_session={}", session_token))
        .body(axum::body::Body::empty())
        .unwrap();

    let get_response = app.router().oneshot(get_request).await.unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);

    // 3. Logout with CSRF token
    let logout_request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/sessions/logout")
        .header("Cookie", format!("fp_session={}", session_token))
        .header("X-CSRF-Token", csrf_token)
        .body(axum::body::Body::empty())
        .unwrap();

    let logout_response = app.router().oneshot(logout_request).await.unwrap();
    assert_eq!(logout_response.status(), StatusCode::NO_CONTENT);

    // 4. Verify session no longer works after logout
    let verify_request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/auth/sessions/me")
        .header("Cookie", format!("fp_session={}", session_token))
        .body(axum::body::Body::empty())
        .unwrap();

    let verify_response = app.router().oneshot(verify_request).await.unwrap();
    assert_eq!(verify_response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_without_session_returns_unauthorized() {
    let app = setup_test_app().await;

    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/sessions/logout")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
