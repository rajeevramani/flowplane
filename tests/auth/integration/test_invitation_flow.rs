/// Integration tests for the invitation registration flow.
///
/// Tests cover:
/// 1. Full invite-accept flow (create invite → validate → accept → verify user+session)
/// 2. Expired invitation rejection
/// 3. Revoked invitation rejection
/// 4. Duplicate pending invite detection (PG 23505)
/// 5. Re-invite after revocation succeeds
/// 6. Accept returns valid session cookie + CSRF token
/// 7. Login rate limiting (429)
/// 8. Login enumeration fix: non-existent user gets same error as wrong password
/// 9. Cross-org isolation: admin of org-A cannot revoke invitation from org-B (403)
/// 10. Cross-org isolation: admin of org-A cannot list invitations from org-B
/// 11. Accept race: revoke between validate and accept → rejection
/// 12. Accept with existing email → conflict
/// 13. Org admin cannot invite admin role (role hierarchy)
/// 14. Org admin CAN invite member and viewer roles
/// 15. Owner role is never invitable
/// 16. Token parsing: invalid format returns generic error
use axum::http::{Method, StatusCode};
use axum_extra::extract::cookie::Cookie;
use serde_json::json;
use tower::ServiceExt;

use crate::support::{read_json, setup_test_app, TestApp};

/// Helper: bootstrap the system, creating admin user + setup token.
async fn bootstrap(app: &TestApp) -> (String, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/bootstrap/initialize")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "email": "admin@example.com",
                "password": "SecurePassword123!",
                "name": "Admin User"
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "bootstrap should succeed");
    let body: serde_json::Value = read_json(resp).await;
    let setup_token = body["setupToken"].as_str().unwrap().to_string();
    (setup_token, body)
}

/// Helper: create an organization via admin API.
async fn create_org(
    app: &TestApp,
    session_cookie: &str,
    csrf_token: &str,
    org_name: &str,
) -> serde_json::Value {
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/admin/organizations")
        .header("Cookie", format!("fp_session={}", session_cookie))
        .header("X-CSRF-Token", csrf_token)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "name": org_name,
                "displayName": format!("{} Org", org_name)
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    let body: serde_json::Value = read_json(resp).await;
    assert!(
        status == StatusCode::CREATED || status == StatusCode::CONFLICT,
        "create org should succeed or conflict: status={}, body={:?}",
        status,
        body
    );
    body
}

/// Helper: login with email/password, returning (session_cookie, csrf_token, body).
async fn login(
    app: &TestApp,
    email: &str,
    password: &str,
) -> Result<(String, String, serde_json::Value), StatusCode> {
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({ "email": email, "password": password })).unwrap(),
        ))
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    if status != StatusCode::OK {
        let _body: serde_json::Value = read_json(resp).await;
        return Err(status);
    }

    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("should have set-cookie header")
        .to_string();
    let cookie = Cookie::parse(&set_cookie).unwrap();
    let session_cookie = cookie.value().to_string();

    let body: serde_json::Value = read_json(resp).await;
    let csrf_token = body["csrfToken"].as_str().unwrap().to_string();

    Ok((session_cookie, csrf_token, body))
}

/// Helper: create an invitation. Returns (status, body).
async fn create_invitation(
    app: &TestApp,
    session_cookie: &str,
    csrf_token: &str,
    org_name: &str,
    email: &str,
    role: &str,
) -> (StatusCode, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/orgs/{}/invitations", org_name))
        .header("Cookie", format!("fp_session={}", session_cookie))
        .header("X-CSRF-Token", csrf_token)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({ "email": email, "role": role })).unwrap(),
        ))
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    let body: serde_json::Value = read_json(resp).await;
    (status, body)
}

/// Helper: list invitations for an org. Returns (status, body).
async fn list_invitations(
    app: &TestApp,
    session_cookie: &str,
    org_name: &str,
) -> (StatusCode, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/orgs/{}/invitations", org_name))
        .header("Cookie", format!("fp_session={}", session_cookie))
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    let body: serde_json::Value = read_json(resp).await;
    (status, body)
}

/// Helper: revoke an invitation. Returns status.
async fn revoke_invitation(
    app: &TestApp,
    session_cookie: &str,
    csrf_token: &str,
    org_name: &str,
    invitation_id: &str,
) -> StatusCode {
    let request = axum::http::Request::builder()
        .method(Method::DELETE)
        .uri(format!("/api/v1/orgs/{}/invitations/{}", org_name, invitation_id))
        .header("Cookie", format!("fp_session={}", session_cookie))
        .header("X-CSRF-Token", csrf_token)
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    resp.status()
}

/// Helper: validate an invitation token. Returns (status, body).
async fn validate_invite(app: &TestApp, token: &str) -> (StatusCode, serde_json::Value) {
    // Token is base64url-safe (no padding) so safe to use directly in query string
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/invitations/validate?token={}", token))
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    let body: serde_json::Value = read_json(resp).await;
    (status, body)
}

/// Helper: accept an invitation. Returns (status, response headers, body).
async fn accept_invite(
    app: &TestApp,
    token: &str,
    name: &str,
    password: &str,
) -> (StatusCode, Option<String>, Option<String>, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/invitations/accept")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "token": token,
                "name": name,
                "password": password
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    let set_cookie =
        resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let csrf_header =
        resp.headers().get("x-csrf-token").and_then(|v| v.to_str().ok()).map(|s| s.to_string());

    let body: serde_json::Value = read_json(resp).await;
    (status, set_cookie, csrf_header, body)
}

/// Extract the raw token from an invite_url (everything after #token=).
fn extract_token_from_url(invite_url: &str) -> String {
    invite_url.split("#token=").nth(1).expect("invite_url should contain #token=").to_string()
}

/// Full setup: bootstrap -> login as admin (gets admin:all) -> create org -> return session info.
async fn full_admin_setup(app: &TestApp, org_name: &str) -> (String, String) {
    // Bootstrap creates the admin user
    let (_, _) = bootstrap(app).await;
    // Login as admin to get admin:all scope (setup token only has bootstrap:initialize)
    let (session_cookie, csrf_token, _) =
        login(app, "admin@example.com", "SecurePassword123!").await.unwrap();
    create_org(app, &session_cookie, &csrf_token, org_name).await;
    (session_cookie, csrf_token)
}

// ========================================================================
// Test 1: Full flow — bootstrap → create org → create invitation →
//         validate token → accept invitation → verify user + session
// ========================================================================

#[tokio::test]
async fn test_full_invitation_flow() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "invite-org").await;

    // Create invitation
    let (status, body) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "invite-org",
        "newuser@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite creation failed: {:?}", body);
    let invite_url = body["inviteUrl"].as_str().unwrap();
    let token = extract_token_from_url(invite_url);

    // Validate the token
    let (status, info) = validate_invite(&app, &token).await;
    assert_eq!(status, StatusCode::OK, "validate should succeed: {:?}", info);
    assert_eq!(info["email"].as_str().unwrap(), "newuser@example.com");
    assert_eq!(info["orgName"].as_str().unwrap(), "invite-org");
    assert_eq!(info["role"].as_str().unwrap(), "member");

    // Accept the invitation
    let (status, set_cookie, csrf, accept_body) =
        accept_invite(&app, &token, "New User", "SecurePassword123!").await;
    assert_eq!(status, StatusCode::CREATED, "accept should succeed: {:?}", accept_body);
    assert!(set_cookie.is_some(), "accept should set session cookie");
    assert!(csrf.is_some(), "accept should return CSRF token");
    assert!(accept_body.get("sessionId").is_some(), "accept should return session ID");
    assert_eq!(accept_body["userEmail"].as_str().unwrap(), "newuser@example.com");

    // Verify the new user can log in
    let login_result = login(&app, "newuser@example.com", "SecurePassword123!").await;
    assert!(login_result.is_ok(), "new user should be able to login");
}

// ========================================================================
// Test 2: Expired invitation rejected
// ========================================================================

#[tokio::test]
async fn test_expired_invitation_rejected() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "expire-org").await;

    // Create invitation
    let (status, body) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "expire-org",
        "expiry@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let invite_url = body["inviteUrl"].as_str().unwrap();
    let token = extract_token_from_url(invite_url);
    let invitation_id = body["id"].as_str().unwrap();

    // Manually expire the invitation in the database
    sqlx::query("UPDATE invitations SET expires_at = NOW() - INTERVAL '1 hour' WHERE id = $1")
        .bind(invitation_id)
        .execute(&app.pool)
        .await
        .unwrap();

    // Validate should fail
    let (status, _) = validate_invite(&app, &token).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED,
        "expired invite should be rejected, got {}",
        status
    );

    // Accept should also fail
    let (status, _, _, _) = accept_invite(&app, &token, "Expired User", "SecurePassword123!").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED,
        "expired invite accept should be rejected, got {}",
        status
    );
}

// ========================================================================
// Test 3: Revoked invitation rejected
// ========================================================================

#[tokio::test]
async fn test_revoked_invitation_rejected() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "revoke-org").await;

    // Create invitation
    let (status, body) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "revoke-org",
        "revoked@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let invite_url = body["inviteUrl"].as_str().unwrap();
    let token = extract_token_from_url(invite_url);
    let invitation_id = body["id"].as_str().unwrap();

    // Revoke the invitation
    let revoke_status =
        revoke_invitation(&app, &session_cookie, &csrf_token, "revoke-org", invitation_id).await;
    assert_eq!(revoke_status, StatusCode::NO_CONTENT, "revocation should succeed");

    // Validate should fail
    let (status, _) = validate_invite(&app, &token).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED,
        "revoked invite should be rejected, got {}",
        status
    );

    // Accept should also fail
    let (status, _, _, _) = accept_invite(&app, &token, "Revoked User", "SecurePassword123!").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED,
        "revoked invite accept should be rejected, got {}",
        status
    );
}

// ========================================================================
// Test 4: Duplicate pending invite returns friendly error (PG 23505)
// ========================================================================

#[tokio::test]
async fn test_duplicate_pending_invite_returns_conflict() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "dup-org").await;

    // First invite succeeds
    let (status, _) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "dup-org",
        "duplicate@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Second invite for same email+org should conflict
    let (status, body) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "dup-org",
        "duplicate@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate invite should return 409: {:?}", body);
}

// ========================================================================
// Test 5: Re-invite after revocation succeeds
// ========================================================================

#[tokio::test]
async fn test_reinvite_after_revocation_succeeds() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "reinvite-org").await;

    // Create first invitation
    let (status, body) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "reinvite-org",
        "reinvite@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let invitation_id = body["id"].as_str().unwrap();

    // Revoke it
    let revoke_status =
        revoke_invitation(&app, &session_cookie, &csrf_token, "reinvite-org", invitation_id).await;
    assert_eq!(revoke_status, StatusCode::NO_CONTENT);

    // Re-invite should succeed (partial unique index only covers pending)
    let (status, body) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "reinvite-org",
        "reinvite@example.com",
        "member",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "re-invite after revocation should succeed: {:?}",
        body
    );
}

// ========================================================================
// Test 6: Accept returns valid session cookie + CSRF token (auto-login)
// ========================================================================

#[tokio::test]
async fn test_accept_returns_session_and_csrf() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "session-org").await;

    let (status, body) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "session-org",
        "session@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = extract_token_from_url(body["inviteUrl"].as_str().unwrap());

    let (status, set_cookie, csrf_header, body) =
        accept_invite(&app, &token, "Session User", "SecurePassword123!").await;
    assert_eq!(status, StatusCode::CREATED);

    // Verify session cookie is set
    let set_cookie_val = set_cookie.expect("must have set-cookie");
    assert!(set_cookie_val.contains("fp_session="), "cookie should be fp_session");

    // Verify CSRF token is returned
    let csrf = csrf_header.expect("must have x-csrf-token");
    assert!(!csrf.is_empty(), "CSRF token should not be empty");

    // Verify body contains session info
    assert!(body.get("sessionId").is_some());
    assert!(body.get("csrfToken").is_some());
    assert!(body.get("expiresAt").is_some());
    assert!(body.get("userId").is_some());
    assert_eq!(body["userEmail"].as_str().unwrap(), "session@example.com");

    // Verify the session cookie works for a GET request
    let cookie = Cookie::parse(&set_cookie_val).unwrap();
    let session_token_val = cookie.value();
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/auth/sessions/me")
        .header("Cookie", format!("fp_session={}", session_token_val))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(request).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "session cookie should be valid for /me");
}

// ========================================================================
// Test 7: Login rate limit returns 429
// ========================================================================

#[tokio::test]
async fn test_login_rate_limit_returns_429() {
    // Set a very low rate limit for testing
    std::env::set_var("FLOWPLANE_RATE_LIMIT_LOGIN_PER_MIN", "2");
    let app = setup_test_app().await;
    let (_, _) = bootstrap(&app).await;

    // Build the router ONCE so rate limiter state is shared across requests
    let router = app.router();

    // Exhaust the rate limit (2 attempts allowed per minute)
    // Use X-Forwarded-For to simulate a consistent IP for rate limiting
    let mut got_429 = false;
    for i in 0..5 {
        let request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/login")
            .header("Content-Type", "application/json")
            .header("X-Forwarded-For", "10.0.0.99")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "email": "rate@example.com",
                    "password": "wrongpassword"
                }))
                .unwrap(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(request).await.unwrap();
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            got_429 = true;
            break;
        }
        assert!(
            i < 4,
            "should hit rate limit within 5 attempts (attempt {} returned {})",
            i + 1,
            resp.status()
        );
    }

    assert!(got_429, "should have received 429 after exceeding rate limit");
    std::env::remove_var("FLOWPLANE_RATE_LIMIT_LOGIN_PER_MIN");
}

// ========================================================================
// Test 8: Login enumeration fix — non-existent user gets same error as wrong password
// ========================================================================

#[tokio::test]
async fn test_login_enumeration_fix() {
    let app = setup_test_app().await;
    let (_, _) = bootstrap(&app).await;

    // Login with non-existent user
    let request1 = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "email": "nonexistent@example.com",
                "password": "SomePassword123!"
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp1 = app.router().oneshot(request1).await.unwrap();
    let status1 = resp1.status();
    let body1: serde_json::Value = read_json(resp1).await;

    // Login with existing user but wrong password
    let request2 = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "email": "admin@example.com",
                "password": "WrongPassword456!"
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp2 = app.router().oneshot(request2).await.unwrap();
    let status2 = resp2.status();
    let body2: serde_json::Value = read_json(resp2).await;

    // Both should return 401 with the same generic message
    assert_eq!(status1, StatusCode::UNAUTHORIZED, "non-existent user should return 401");
    assert_eq!(status2, StatusCode::UNAUTHORIZED, "wrong password should return 401");

    // The error messages should be identical (preventing enumeration)
    let msg1 = body1["error"].as_str().or_else(|| body1["message"].as_str());
    let msg2 = body2["error"].as_str().or_else(|| body2["message"].as_str());
    assert_eq!(msg1, msg2, "error messages should be identical for enumeration prevention");
}

// ========================================================================
// Test 9: Cross-org isolation — admin of org-A cannot revoke invite from org-B
// ========================================================================

#[tokio::test]
async fn test_cross_org_revoke_forbidden() {
    let app = setup_test_app().await;
    let (admin_cookie, admin_csrf) = full_admin_setup(&app, "cross-a").await;

    // Create a second org
    create_org(&app, &admin_cookie, &admin_csrf, "cross-b").await;

    // Create invitation in org cross-b
    let (status, body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "cross-b",
        "crossuser@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let invitation_id = body["id"].as_str().unwrap();

    // Platform admin can access both — but let's create a non-admin user in org-A
    // to test cross-org isolation. We need to create a user that only has org-A admin.
    // Since the bootstrap admin is platform admin (admin:all), they bypass all checks.
    // So we'll create a user in org-A, give them admin role, then test from their session.

    // Create invitation for org-A admin user
    let (status, body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "cross-a",
        "orgaadmin@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let org_a_invite_token = extract_token_from_url(body["inviteUrl"].as_str().unwrap());

    // Accept the invitation to create the org-A user
    let (status, _, _, _) =
        accept_invite(&app, &org_a_invite_token, "Org A Admin", "SecurePassword123!").await;
    assert_eq!(status, StatusCode::CREATED);

    // Login as org-A user
    let login_result = login(&app, "orgaadmin@example.com", "SecurePassword123!").await;
    let (org_a_cookie, org_a_csrf, _) = login_result.unwrap();

    // Try to revoke org-B's invitation with org-A's session — should be forbidden
    let revoke_status =
        revoke_invitation(&app, &org_a_cookie, &org_a_csrf, "cross-b", invitation_id).await;
    assert_eq!(
        revoke_status,
        StatusCode::FORBIDDEN,
        "org-A user should not be able to revoke org-B invitations"
    );
}

// ========================================================================
// Test 10: Cross-org isolation — admin of org-A cannot list invitations from org-B
// ========================================================================

#[tokio::test]
async fn test_cross_org_list_forbidden() {
    let app = setup_test_app().await;
    let (admin_cookie, admin_csrf) = full_admin_setup(&app, "list-a").await;
    create_org(&app, &admin_cookie, &admin_csrf, "list-b").await;

    // Create a member user in org list-a
    let (_, body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "list-a",
        "listauser@example.com",
        "member",
    )
    .await;
    let token = extract_token_from_url(body["inviteUrl"].as_str().unwrap());
    accept_invite(&app, &token, "List A User", "SecurePassword123!").await;

    let (org_a_cookie, _, _) =
        login(&app, "listauser@example.com", "SecurePassword123!").await.unwrap();

    // Try to list org-B's invitations — should be forbidden
    let (status, _) = list_invitations(&app, &org_a_cookie, "list-b").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "org-A user should not list org-B invitations");
}

// ========================================================================
// Test 11: Accept race — revoke between validate and accept
// ========================================================================

#[tokio::test]
async fn test_accept_race_after_revocation() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "race-org").await;

    // Create invitation
    let (_, body) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "race-org",
        "raceuser@example.com",
        "member",
    )
    .await;
    let invite_url = body["inviteUrl"].as_str().unwrap();
    let token = extract_token_from_url(invite_url);
    let invitation_id = body["id"].as_str().unwrap();

    // Validate succeeds
    let (status, _) = validate_invite(&app, &token).await;
    assert_eq!(status, StatusCode::OK);

    // Revoke between validate and accept
    let revoke_status =
        revoke_invitation(&app, &session_cookie, &csrf_token, "race-org", invitation_id).await;
    assert_eq!(revoke_status, StatusCode::NO_CONTENT);

    // Now accept should fail gracefully
    let (status, _, _, _) = accept_invite(&app, &token, "Race User", "SecurePassword123!").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED,
        "accept after revocation should fail, got {}",
        status
    );
}

// ========================================================================
// Test 12: Accept with existing email → conflict
// ========================================================================

#[tokio::test]
async fn test_accept_with_existing_email_returns_conflict() {
    let app = setup_test_app().await;
    let (admin_cookie, admin_csrf) = full_admin_setup(&app, "existing-org").await;

    // Create first invitation and accept it
    let (_, body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "existing-org",
        "existing@example.com",
        "member",
    )
    .await;
    let token1 = extract_token_from_url(body["inviteUrl"].as_str().unwrap());
    let (status, _, _, _) = accept_invite(&app, &token1, "First User", "SecurePassword123!").await;
    assert_eq!(status, StatusCode::CREATED);

    // Create a second org and invite the same email
    create_org(&app, &admin_cookie, &admin_csrf, "existing-org2").await;
    let (status, body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "existing-org2",
        "existing@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token2 = extract_token_from_url(body["inviteUrl"].as_str().unwrap());

    // Accept should fail because the email is already registered
    let (status, _, _, body) =
        accept_invite(&app, &token2, "Second User", "SecurePassword123!").await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::CONFLICT,
        "accept for existing email should fail, got {}: {:?}",
        status,
        body
    );
}

// ========================================================================
// Test 13: Org admin cannot invite admin role (role hierarchy enforcement)
// ========================================================================

#[tokio::test]
async fn test_org_admin_cannot_invite_admin_role() {
    let app = setup_test_app().await;
    let (admin_cookie, admin_csrf) = full_admin_setup(&app, "hierarchy-org").await;

    // Platform admin creates an org admin user (platform admin bypasses hierarchy check)
    // First, create a member user and upgrade them to admin via org membership
    let (_, body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "hierarchy-org",
        "orgadmin@example.com",
        "admin",
    )
    .await;
    // Platform admin can invite admin (they have admin:all bypass)
    // Accept the invitation
    let token = extract_token_from_url(body["inviteUrl"].as_str().unwrap());
    let (status, _, _, _) = accept_invite(&app, &token, "Org Admin", "SecurePassword123!").await;
    assert_eq!(status, StatusCode::CREATED);

    // Login as the org admin
    let (org_admin_cookie, org_admin_csrf, _) =
        login(&app, "orgadmin@example.com", "SecurePassword123!").await.unwrap();

    // Org admin tries to invite another admin — should be rejected (hierarchy enforcement)
    let (status, body) = create_invitation(
        &app,
        &org_admin_cookie,
        &org_admin_csrf,
        "hierarchy-org",
        "anotheradmin@example.com",
        "admin",
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
        "org admin should not be able to invite admin role, got {}: {:?}",
        status,
        body
    );
}

// ========================================================================
// Test 14: Org admin CAN invite member and viewer roles
// ========================================================================

#[tokio::test]
async fn test_org_admin_can_invite_member_and_viewer() {
    let app = setup_test_app().await;
    let (admin_cookie, admin_csrf) = full_admin_setup(&app, "roles-org").await;

    // Create an org admin
    let (_, body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "roles-org",
        "rolesadmin@example.com",
        "admin",
    )
    .await;
    let token = extract_token_from_url(body["inviteUrl"].as_str().unwrap());
    accept_invite(&app, &token, "Roles Admin", "SecurePassword123!").await;

    let (org_admin_cookie, org_admin_csrf, _) =
        login(&app, "rolesadmin@example.com", "SecurePassword123!").await.unwrap();

    // Org admin should be able to invite member
    let (status, _) = create_invitation(
        &app,
        &org_admin_cookie,
        &org_admin_csrf,
        "roles-org",
        "member@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "org admin should invite member");

    // Org admin should be able to invite viewer
    let (status, _) = create_invitation(
        &app,
        &org_admin_cookie,
        &org_admin_csrf,
        "roles-org",
        "viewer@example.com",
        "viewer",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "org admin should invite viewer");
}

// ========================================================================
// Test 15: Owner role is never invitable
// ========================================================================

#[tokio::test]
async fn test_owner_role_not_invitable() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "owner-org").await;

    // Even platform admin cannot invite owner
    let (status, body) = create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "owner-org",
        "owner@example.com",
        "owner",
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
        "owner role should not be invitable, got {}: {:?}",
        status,
        body
    );
}

// ========================================================================
// Test 16: Token parsing — invalid format returns generic error
// ========================================================================

#[tokio::test]
async fn test_invalid_token_format_returns_error() {
    let app = setup_test_app().await;

    // No dot separator
    let (status, _) = validate_invite(&app, "fp_invite_abc123noseparator").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED,
        "invalid token format should be rejected, got {}",
        status
    );

    // Wrong prefix
    let (status, _) = validate_invite(&app, "fp_session_abc123.secret").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED,
        "wrong prefix should be rejected, got {}",
        status
    );

    // Empty token
    let (status, _) = validate_invite(&app, "").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED,
        "empty token should be rejected, got {}",
        status
    );

    // Completely random string
    let (status, _) = validate_invite(&app, "not_a_valid_token_at_all").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED,
        "random string should be rejected, got {}",
        status
    );
}

// ========================================================================
// Additional: List invitations returns created invitations
// ========================================================================

#[tokio::test]
async fn test_list_invitations_returns_created() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "listinv-org").await;

    // Create two invitations
    create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "listinv-org",
        "list1@example.com",
        "member",
    )
    .await;
    create_invitation(
        &app,
        &session_cookie,
        &csrf_token,
        "listinv-org",
        "list2@example.com",
        "viewer",
    )
    .await;

    // List invitations
    let (status, body) = list_invitations(&app, &session_cookie, "listinv-org").await;
    assert_eq!(status, StatusCode::OK, "list should succeed: {:?}", body);

    let invitations = body["invitations"].as_array().unwrap();
    assert_eq!(invitations.len(), 2, "should have 2 invitations");
    assert_eq!(body["total"].as_i64().unwrap(), 2);
}

// ========================================================================
// Test: Invited user can access resource endpoints (regression for org scope bug)
//
// This test verifies the full lifecycle: bootstrap → create org → create team →
// invite user → accept → login → access resource endpoints.
// Without the fix to check_resource_access (org scope recognition) and
// InternalAuthContext::from_rest (is_admin fix), the invited user would get 403.
// ========================================================================

#[tokio::test]
async fn test_invited_user_can_access_resource_endpoints() {
    let app = setup_test_app().await;
    let (admin_cookie, admin_csrf) = full_admin_setup(&app, "resource-org").await;

    // Invite a member to the org
    let (status, body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "resource-org",
        "resourceuser@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite creation failed: {:?}", body);
    let token = extract_token_from_url(body["inviteUrl"].as_str().unwrap());

    // Accept the invitation
    let (status, _, _, _) =
        accept_invite(&app, &token, "Resource User", "SecurePassword123!").await;
    assert_eq!(status, StatusCode::CREATED, "accept should succeed");

    // Login as the invited user
    let (user_cookie, _user_csrf, _) =
        login(&app, "resourceuser@example.com", "SecurePassword123!").await.unwrap();

    // Verify: GET /api/v1/auth/sessions/me should return 200
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/auth/sessions/me")
        .header("Cookie", format!("fp_session={}", user_cookie))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(request).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "invited user should access /auth/sessions/me");

    // Verify: GET /api/v1/route-configs should return 200 (not 403)
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/route-configs")
        .header("Cookie", format!("fp_session={}", user_cookie))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(request).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "invited org member should access /route-configs (got {})",
        resp.status()
    );

    // Verify: GET /api/v1/listeners should return 200 (not 403)
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/listeners")
        .header("Cookie", format!("fp_session={}", user_cookie))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(request).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "invited org member should access /listeners (got {})",
        resp.status()
    );

    // Verify: GET /api/v1/clusters should return 200 (not 403)
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/v1/clusters")
        .header("Cookie", format!("fp_session={}", user_cookie))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router().oneshot(request).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "invited org member should access /clusters (got {})",
        resp.status()
    );
}

// ========================================================================
// Additional: Revoke non-existent invitation returns 404
// ========================================================================

#[tokio::test]
async fn test_revoke_nonexistent_invitation_returns_404() {
    let app = setup_test_app().await;
    let (session_cookie, csrf_token) = full_admin_setup(&app, "notfound-org").await;

    let status =
        revoke_invitation(&app, &session_cookie, &csrf_token, "notfound-org", "nonexistent-id")
            .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "revoking non-existent invitation should return 404");
}

// ========================================================================
// Test: Full org admin lifecycle (Fix 7 from invite-registration-review)
//
// Verifies the complete happy-path flow after Fixes 1-4:
//   bootstrap → create org → verify default team created →
//   invite admin → accept → verify team membership in default team →
//   create team "engineering" → verify membership auto-created →
//   session refresh → create cluster in "engineering" → 201
// ========================================================================

/// Helper: POST to create a team within an org (session-auth).
async fn create_org_team(
    app: &TestApp,
    session_cookie: &str,
    csrf_token: &str,
    org_name: &str,
    team_name: &str,
) -> (StatusCode, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/orgs/{}/teams", org_name))
        .header("Cookie", format!("fp_session={}", session_cookie))
        .header("X-CSRF-Token", csrf_token)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "name": team_name,
                "displayName": format!("{} Team", team_name),
                "description": format!("Team {} in org {}", team_name, org_name)
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"_raw": String::from_utf8_lossy(&bytes).to_string(), "_status": status.as_u16()}));
    (status, body)
}

/// Helper: GET teams for an org (session-auth).
async fn list_org_teams(
    app: &TestApp,
    session_cookie: &str,
    org_name: &str,
) -> (StatusCode, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/orgs/{}/teams", org_name))
        .header("Cookie", format!("fp_session={}", session_cookie))
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    let body: serde_json::Value = read_json(resp).await;
    (status, body)
}

/// Helper: POST /api/v1/auth/sessions/refresh (session-auth).
async fn refresh_session(
    app: &TestApp,
    session_cookie: &str,
    csrf_token: &str,
) -> (StatusCode, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/sessions/refresh")
        .header("Cookie", format!("fp_session={}", session_cookie))
        .header("X-CSRF-Token", csrf_token)
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    let body: serde_json::Value = read_json(resp).await;
    (status, body)
}

/// Helper: POST /api/v1/clusters (session-auth).
async fn create_cluster(
    app: &TestApp,
    session_cookie: &str,
    csrf_token: &str,
    team_name: &str,
    cluster_name: &str,
) -> (StatusCode, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/v1/clusters")
        .header("Cookie", format!("fp_session={}", session_cookie))
        .header("X-CSRF-Token", csrf_token)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "team": team_name,
                "name": cluster_name,
                "endpoints": [{"host": "127.0.0.1", "port": 8080}]
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app.router().oneshot(request).await.unwrap();
    let status = resp.status();
    let body: serde_json::Value = read_json(resp).await;
    (status, body)
}

#[tokio::test]
async fn test_org_admin_full_lifecycle() {
    let app = setup_test_app().await;

    // ── Step 1: Bootstrap ──
    let (_, _) = bootstrap(&app).await;
    let (admin_cookie, admin_csrf, _) =
        login(&app, "admin@example.com", "SecurePassword123!").await.unwrap();

    // ── Step 2: Create org "lifecycle" → should auto-create default team ──
    let org_body = create_org(&app, &admin_cookie, &admin_csrf, "lifecycle").await;
    assert!(org_body.get("id").is_some(), "create_org should return org with id: {:?}", org_body);

    // Verify the default team "lifecycle-default" was created
    // Platform admin can list org teams (admin:all bypasses org membership check)
    let (status, teams_body) = list_org_teams(&app, &admin_cookie, "lifecycle").await;
    assert_eq!(status, StatusCode::OK, "list_org_teams failed: {:?}", teams_body);
    let teams = teams_body["teams"].as_array().expect("teams should be array");
    assert_eq!(teams.len(), 1, "new org should have exactly 1 default team, got: {:?}", teams);
    let default_team = &teams[0];
    assert_eq!(
        default_team["name"].as_str().unwrap(),
        "lifecycle-default",
        "default team should be named lifecycle-default"
    );

    // ── Step 3: Invite an org admin ──
    let (status, invite_body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "lifecycle",
        "orgadmin@lifecycle.io",
        "admin",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "invite creation failed: {:?}", invite_body);
    let invite_url = invite_body["inviteUrl"].as_str().unwrap();
    let token = extract_token_from_url(invite_url);

    // ── Step 4: Accept invitation → user gets default team membership ──
    let (status, set_cookie, csrf_header, accept_body) =
        accept_invite(&app, &token, "Lifecycle Admin", "SecurePassword123!").await;
    assert_eq!(status, StatusCode::CREATED, "accept should succeed: {:?}", accept_body);
    assert!(set_cookie.is_some(), "accept should set session cookie");

    // Parse the session cookie for authenticated requests
    let set_cookie_val = set_cookie.unwrap();
    let cookie = Cookie::parse(&set_cookie_val).unwrap();
    let org_admin_cookie = cookie.value().to_string();
    let org_admin_csrf = csrf_header.expect("accept should return CSRF token");

    // Verify scopes include the default team
    let scopes = accept_body["scopes"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        scopes.iter().any(|s| s.contains("org:lifecycle:admin")),
        "scopes should contain org:lifecycle:admin, got: {:?}",
        scopes
    );
    assert!(
        scopes.iter().any(|s| s.contains("lifecycle-default")),
        "scopes should contain lifecycle-default team scope, got: {:?}",
        scopes
    );

    // ── Step 5: Org admin creates team "engineering" → auto-memberships created ──
    let (status, team_body) =
        create_org_team(&app, &org_admin_cookie, &org_admin_csrf, "lifecycle", "engineering").await;
    assert_eq!(status, StatusCode::CREATED, "create team should succeed: {:?}", team_body);
    assert_eq!(team_body["name"].as_str().unwrap(), "engineering");

    // Verify both teams now listed
    let (status, teams_body) = list_org_teams(&app, &org_admin_cookie, "lifecycle").await;
    assert_eq!(status, StatusCode::OK);
    let teams = teams_body["teams"].as_array().unwrap();
    assert_eq!(teams.len(), 2, "should have 2 teams: default + engineering, got: {:?}", teams);

    // ── Step 6: Refresh session → picks up new team:engineering:*:* scopes ──
    let (status, refresh_body) = refresh_session(&app, &org_admin_cookie, &org_admin_csrf).await;
    assert_eq!(status, StatusCode::OK, "session refresh failed: {:?}", refresh_body);

    let refreshed_scopes = refresh_body["scopes"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        refreshed_scopes.iter().any(|s| s.contains("engineering")),
        "refreshed scopes should contain engineering team scope, got: {:?}",
        refreshed_scopes
    );

    let refreshed_teams = refresh_body["teams"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        refreshed_teams.contains(&"engineering"),
        "refreshed teams should contain engineering, got: {:?}",
        refreshed_teams
    );

    // ── Step 7: Create cluster in "engineering" → 201 (was 403 before fixes) ──
    // Must re-login after refresh so the session middleware sees updated scopes
    let (org_admin_cookie, org_admin_csrf, _) =
        login(&app, "orgadmin@lifecycle.io", "SecurePassword123!").await.unwrap();

    let (status, cluster_body) = create_cluster(
        &app,
        &org_admin_cookie,
        &org_admin_csrf,
        "engineering",
        "lifecycle-cluster",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "org admin should create cluster in engineering team (got {}): {:?}",
        status,
        cluster_body
    );

    // ── Step 8: Verify cluster in default team also works ──
    let (status, cluster_body2) = create_cluster(
        &app,
        &org_admin_cookie,
        &org_admin_csrf,
        "lifecycle-default",
        "default-cluster",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "org admin should create cluster in default team (got {}): {:?}",
        status,
        cluster_body2
    );
}

// ========================================================================
// Test: Invited member gets correct scopes (not just admin)
// Verifies scope assignment for member role specifically.
// ========================================================================

#[tokio::test]
async fn test_invited_member_gets_team_scopes() {
    let app = setup_test_app().await;
    let (admin_cookie, admin_csrf) = full_admin_setup(&app, "member-scope-org").await;

    // Invite a member (not admin)
    let (status, body) = create_invitation(
        &app,
        &admin_cookie,
        &admin_csrf,
        "member-scope-org",
        "member@scope-test.io",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = extract_token_from_url(body["inviteUrl"].as_str().unwrap());

    // Accept
    let (status, _, _, accept_body) =
        accept_invite(&app, &token, "Scope Member", "SecurePassword123!").await;
    assert_eq!(status, StatusCode::CREATED);

    // Verify scopes include the default team (member-scope-org-default)
    let scopes = accept_body["scopes"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        scopes.iter().any(|s| s.contains("org:member-scope-org:member")),
        "member scopes should contain org:member-scope-org:member, got: {:?}",
        scopes
    );
    assert!(
        scopes.iter().any(|s| s.contains("member-scope-org-default")),
        "member scopes should contain default team scope, got: {:?}",
        scopes
    );
    // Members should have read access but the exact scope format depends on scopes_for_role
    assert!(
        scopes.iter().any(|s| s.contains(":read")),
        "member should have at least read scopes, got: {:?}",
        scopes
    );
}
