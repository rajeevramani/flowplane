use axum::http::{Method, StatusCode};
use serde_json::json;

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

#[tokio::test]
async fn dynamic_scope_derivation_get_requires_read() {
    let app = setup_test_app().await;

    // Create a token with only clusters:read scope
    let token = app.issue_token("read-only", &["clusters:read"]).await;

    // GET /api/v1/clusters should pass scope check (requires clusters:read)
    // Note: may get 500 if clusters table doesn't exist, but should NOT get 403
    let response =
        send_request(&app, Method::GET, "/api/v1/clusters", Some(&token.token), None).await;
    assert_ne!(response.status(), StatusCode::FORBIDDEN);

    // POST /api/v1/clusters should fail (requires clusters:write)
    let payload = json!({
        "name": "test-cluster",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let response =
        send_request(&app, Method::POST, "/api/v1/clusters", Some(&token.token), Some(payload))
            .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn dynamic_scope_derivation_post_requires_write() {
    let app = setup_test_app().await;

    // Create a token with only routes:write scope
    let token = app.issue_token("write-only", &["routes:write"]).await;

    // POST /api/v1/route-configs should succeed (requires routes:write)
    let payload = json!({
        "name": "test-routes",
        "virtual_hosts": [{
            "name": "default",
            "domains": ["*"],
            "routes": [{
                "name": "test",
                "match": {"path": {"Prefix": {"value": "/"}}},
                "action": {"Forward": {"cluster": "test-cluster"}}
            }]
        }]
    });
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/route-configs",
        Some(&token.token),
        Some(payload),
    )
    .await;
    // Expect 201 CREATED or error due to missing cluster, but NOT 403 (scope check passes)
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn dynamic_scope_derivation_delete_requires_write() {
    let app = setup_test_app().await;

    // Create a token with only listeners:read scope (not listeners:write)
    let token = app.issue_token("read-only", &["listeners:read"]).await;

    // DELETE /api/v1/listeners/test should fail (requires listeners:write)
    let response = send_request(
        &app,
        Method::DELETE,
        "/api/v1/listeners/test-listener",
        Some(&token.token),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn dynamic_scope_derivation_put_requires_write() {
    let app = setup_test_app().await;

    // Create a token with clusters:write scope
    let token = app.issue_token("cluster-writer", &["clusters:write"]).await;

    // PUT /api/v1/clusters/test should succeed (requires clusters:write)
    let payload = json!({
        "name": "test-cluster",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let response = send_request(
        &app,
        Method::PUT,
        "/api/v1/clusters/test-cluster",
        Some(&token.token),
        Some(payload),
    )
    .await;
    // Expect NOT 403 (scope check passes, may get 404 if cluster doesn't exist)
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn dynamic_scope_derivation_patch_requires_write() {
    let app = setup_test_app().await;

    // Create a token with tokens:write scope
    let token = app.issue_token("token-manager", &["tokens:write"]).await;

    // PATCH /api/v1/tokens/{id} should succeed (requires tokens:write)
    let payload = json!({
        "status": "revoked"
    });
    let response = send_request(
        &app,
        Method::PATCH,
        "/api/v1/tokens/some-token-id",
        Some(&token.token),
        Some(payload),
    )
    .await;
    // Expect NOT 403 (scope check passes, may get 404 if token doesn't exist)
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn dynamic_scope_derivation_admin_all_bypasses_checks() {
    let app = setup_test_app().await;

    // Create a token with admin:all scope
    let token = app.issue_token("admin", &["admin:all"]).await;

    // Should be able to access any endpoint regardless of resource/action
    let response =
        send_request(&app, Method::GET, "/api/v1/clusters", Some(&token.token), None).await;
    assert_ne!(response.status(), StatusCode::FORBIDDEN);

    let response =
        send_request(&app, Method::GET, "/api/v1/route-configs", Some(&token.token), None).await;
    assert_ne!(response.status(), StatusCode::FORBIDDEN);

    let response =
        send_request(&app, Method::GET, "/api/v1/listeners", Some(&token.token), None).await;
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn dynamic_scope_derivation_wrong_resource_fails() {
    let app = setup_test_app().await;

    // Create a token with only clusters:read scope
    let token = app.issue_token("cluster-reader", &["clusters:read"]).await;

    // GET /api/v1/route-configs should fail (requires routes:read, not clusters:read)
    let response =
        send_request(&app, Method::GET, "/api/v1/route-configs", Some(&token.token), None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // GET /api/v1/listeners should fail (requires listeners:read, not clusters:read)
    let response =
        send_request(&app, Method::GET, "/api/v1/listeners", Some(&token.token), None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
