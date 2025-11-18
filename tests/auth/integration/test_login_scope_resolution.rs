/// Integration tests for login with scope resolution
///
/// Tests the complete login flow including:
/// 1. Admin user login → receives admin:all scope
/// 2. Regular user with single team → receives team-scoped permissions
/// 3. Regular user with multiple teams → receives combined scopes (deduplicated)
/// 4. Regular user with no teams → receives empty scopes
/// 5. Login with inactive/suspended user → fails
/// 6. Login with invalid credentials → fails
use axum::http::{Method, StatusCode};
use serde_json::json;

use crate::support::{create_team, read_json, send_request, setup_test_app};
use flowplane::api::handlers::auth::LoginResponseBody;
use flowplane::auth::user::UserResponse;

#[tokio::test]
async fn login_admin_user_receives_admin_all_scope() {
    let app = setup_test_app().await;

    // Create an admin user
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "admin@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Admin User",
            "isAdmin": true
        })),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let admin_user: UserResponse = read_json(create_response).await;
    assert!(admin_user.is_admin, "User should be created as admin");

    // Login as admin user
    let login_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "admin@example.com",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    assert_eq!(login_response.status(), StatusCode::OK);
    let login_body: LoginResponseBody = read_json(login_response).await;

    // Admin should receive admin:all scope
    assert_eq!(login_body.scopes, vec!["admin:all"]);
    assert_eq!(login_body.user_id, admin_user.id.to_string());
    assert_eq!(login_body.user_email, "admin@example.com");
    assert!(login_body.teams.is_empty(), "Admin with admin:all should have no team-scoped access");
}

#[tokio::test]
async fn login_regular_user_single_team() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create the team first
    create_team(&app, &admin_token.token, "engineering").await;

    // Create a regular user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "user@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Regular User",
            "isAdmin": false
        })),
    )
    .await;
    let user: UserResponse = read_json(create_response).await;

    // Add team membership with scopes
    let teams_url = format!("/api/v1/users/{}/teams", user.id);
    send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&admin_token.token),
        Some(json!({
            "userId": user.id,
            "team": "engineering",
            "scopes": ["team:engineering:clusters:read", "team:engineering:routes:write"]
        })),
    )
    .await;

    // Login as regular user
    let login_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "user@example.com",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    assert_eq!(login_response.status(), StatusCode::OK);
    let login_body: LoginResponseBody = read_json(login_response).await;

    // Should receive team-scoped permissions
    assert_eq!(login_body.scopes.len(), 2);
    assert!(login_body.scopes.contains(&"team:engineering:clusters:read".to_string()));
    assert!(login_body.scopes.contains(&"team:engineering:routes:write".to_string()));
    assert_eq!(login_body.teams, vec!["engineering"]);
    assert_eq!(login_body.user_id, user.id.to_string());
}

#[tokio::test]
async fn login_regular_user_multiple_teams_deduplicates_scopes() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create teams first
    create_team(&app, &admin_token.token, "engineering").await;
    create_team(&app, &admin_token.token, "platform").await;
    create_team(&app, &admin_token.token, "security").await;

    // Create a regular user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "multiteam@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Multi Team User",
            "isAdmin": false
        })),
    )
    .await;
    let user: UserResponse = read_json(create_response).await;

    // Add multiple team memberships with overlapping scopes
    let teams_url = format!("/api/v1/users/{}/teams", user.id);

    // Engineering team
    send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&admin_token.token),
        Some(json!({
            "userId": user.id,
            "team": "engineering",
            "scopes": ["team:engineering:clusters:read", "team:engineering:routes:write"]
        })),
    )
    .await;

    // Platform team (with overlapping clusters:read)
    send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&admin_token.token),
        Some(json!({
            "userId": user.id,
            "team": "platform",
            "scopes": ["team:platform:listeners:read", "team:platform:clusters:read"]
        })),
    )
    .await;

    // Security team
    send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&admin_token.token),
        Some(json!({
            "userId": user.id,
            "team": "security",
            "scopes": ["team:security:routes:read"]
        })),
    )
    .await;

    // Login as multi-team user
    let login_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "multiteam@example.com",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    assert_eq!(login_response.status(), StatusCode::OK);
    let login_body: LoginResponseBody = read_json(login_response).await;

    // Should receive all scopes from all teams (deduplicated and sorted)
    assert_eq!(login_body.scopes.len(), 5);
    let mut expected_scopes = vec![
        "team:engineering:clusters:read",
        "team:engineering:routes:write",
        "team:platform:clusters:read",
        "team:platform:listeners:read",
        "team:security:routes:read",
    ];
    expected_scopes.sort();

    let mut actual_scopes = login_body.scopes.clone();
    actual_scopes.sort();

    assert_eq!(actual_scopes, expected_scopes);

    // Should have access to all three teams
    let mut teams = login_body.teams.clone();
    teams.sort();
    assert_eq!(teams, vec!["engineering", "platform", "security"]);
}

#[tokio::test]
async fn login_user_with_no_teams_receives_empty_scopes() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create a regular user without any team memberships
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "noteams@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "No Teams User",
            "isAdmin": false
        })),
    )
    .await;
    let user: UserResponse = read_json(create_response).await;

    // Login as user with no teams
    let login_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "noteams@example.com",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    assert_eq!(login_response.status(), StatusCode::OK);
    let login_body: LoginResponseBody = read_json(login_response).await;

    // Should receive empty scopes and teams
    assert!(login_body.scopes.is_empty(), "User with no teams should have no scopes");
    assert!(login_body.teams.is_empty(), "User with no teams should have no teams");
    assert_eq!(login_body.user_id, user.id.to_string());
}

#[tokio::test]
async fn login_fails_with_invalid_email() {
    let app = setup_test_app().await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "nonexistent@example.com",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_fails_with_invalid_password() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create a user
    send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "user@example.com",
            "password": "CorrectPassword123",
            "name": "Test User",
            "isAdmin": false
        })),
    )
    .await;

    // Try to login with wrong password
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "user@example.com",
            "password": "WrongPassword456"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_fails_with_inactive_user() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create a user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "inactive@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Inactive User",
            "isAdmin": false
        })),
    )
    .await;
    let user: UserResponse = read_json(create_response).await;

    // Update user to inactive status
    let update_url = format!("/api/v1/users/{}", user.id);
    send_request(
        &app,
        Method::PUT,
        &update_url,
        Some(&admin_token.token),
        Some(json!({
            "status": "inactive"
        })),
    )
    .await;

    // Try to login as inactive user
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "inactive@example.com",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_fails_with_suspended_user() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create a user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "suspended@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Suspended User",
            "isAdmin": false
        })),
    )
    .await;
    let user: UserResponse = read_json(create_response).await;

    // Update user to suspended status
    let update_url = format!("/api/v1/users/{}", user.id);
    send_request(
        &app,
        Method::PUT,
        &update_url,
        Some(&admin_token.token),
        Some(json!({
            "status": "suspended"
        })),
    )
    .await;

    // Try to login as suspended user
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "suspended@example.com",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_validates_email_format() {
    let app = setup_test_app().await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "not-an-email",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn login_session_can_be_used_for_authenticated_requests() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create team first
    create_team(&app, &admin_token.token, "engineering").await;

    // Create a user with token permissions
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "tokenuser@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Token User",
            "isAdmin": false
        })),
    )
    .await;
    let user: UserResponse = read_json(create_response).await;

    // Add team membership with token read permission
    let teams_url = format!("/api/v1/users/{}/teams", user.id);
    send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&admin_token.token),
        Some(json!({
            "userId": user.id,
            "team": "engineering",
            "scopes": ["team:engineering:tokens:read"]
        })),
    )
    .await;

    // Login
    let login_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "tokenuser@example.com",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    // Extract session token from Set-Cookie header
    let set_cookie_header = login_response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("should have set-cookie header")
        .to_string();

    let login_body: LoginResponseBody = read_json(login_response).await;

    // Extract session token value
    let cookie = axum_extra::extract::cookie::Cookie::parse(&set_cookie_header).unwrap();
    let session_token = cookie.value();

    // Use session to access session info endpoint
    let request = axum::http::Request::builder()
        .method(axum::http::Method::GET)
        .uri("/api/v1/auth/sessions/me")
        .header("Cookie", format!("fp_session={}", session_token))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = tower::ServiceExt::oneshot(app.router(), request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let session_info: serde_json::Value = read_json(response).await;
    assert_eq!(session_info["sessionId"].as_str().unwrap(), login_body.session_id);
    assert_eq!(session_info["scopes"].as_array().unwrap().len(), 1);
}
