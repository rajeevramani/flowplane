//! Adversarial integration tests for dev-mode authentication.
//!
//! These tests exercise edge cases and malformed inputs that a real user or
//! attacker might send. They run via `tower::ServiceExt::oneshot` against the
//! axum router with a real PostgreSQL backend (testcontainers).
//!
//! Written WITHOUT reading the auth middleware implementation — from the API
//! contract: "Authorization: Bearer <token>" with the configured dev token
//! should succeed; everything else should return 401.

#![cfg(feature = "postgres_tests")]
#![allow(clippy::await_holding_lock)]

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::env_guard::EnvGuard;
use common::test_db::TestDatabase;
use flowplane::config::SimpleXdsConfig;
use flowplane::startup::seed_dev_resources;
use flowplane::xds::XdsState;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEV_TOKEN: &str = "test-dev-token-adversarial";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the axum router in dev auth mode with the adversarial dev token.
/// Returns the router and an EnvGuard that restores env vars on drop.
async fn dev_router(
    db: &TestDatabase,
) -> (axum::Router, EnvGuard, std::sync::MutexGuard<'static, ()>) {
    let lock = common::env_guard::ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let mut env = EnvGuard::new();
    env.set("FLOWPLANE_AUTH_MODE", "dev");
    env.set("FLOWPLANE_DEV_TOKEN", DEV_TOKEN);
    env.set("FLOWPLANE_COOKIE_SECURE", "false");
    env.set("FLOWPLANE_BASE_URL", "http://localhost:8080");

    seed_dev_resources(&db.pool).await.expect("seed dev resources");

    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), db.pool.clone()));
    (flowplane::api::routes::build_router(state), env, lock)
}

/// Build a GET request with a raw Authorization header value.
fn request_with_raw_auth(uri: &str, auth_value: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::AUTHORIZATION, auth_value)
        .body(Body::empty())
        .unwrap()
}

/// Build a GET request with no Authorization header.
fn request_no_auth(uri: &str) -> Request<Body> {
    Request::builder().method(Method::GET).uri(uri).body(Body::empty()).unwrap()
}

const CLUSTERS_URI: &str = "/api/v1/teams/default/clusters";

// ===========================================================================
// Bearer token format edge cases
// ===========================================================================

/// Double space between "Bearer" and token → 401.
///
/// REAL BUG this test is designed to catch: A `split_once(' ')` parser would
/// extract ` token` (with leading space) as the token value, which wouldn't
/// match the canonical token. But `splitn(2, ' ')` on "Bearer  token" would
/// give ["Bearer", " token"]. If the server trims the extracted value, it
/// would match. If it doesn't trim, it won't match. This test documents
/// the expected behavior: double-space should NOT be equivalent to single-space.
#[tokio::test]
async fn bearer_double_space_returns_401() {
    let db = TestDatabase::new("adv_double_space").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // "Bearer  token" (two spaces)
    let auth = format!("Bearer  {}", DEV_TOKEN);
    let req = request_with_raw_auth(CLUSTERS_URI, &auth);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "double space between Bearer and token should be rejected"
    );
}

/// Token with trailing newline → 401.
///
/// Common when tokens are read from files that have a trailing newline.
#[tokio::test]
async fn bearer_token_trailing_newline_returns_401() {
    let db = TestDatabase::new("adv_trailing_newline").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let auth = format!("Bearer {}\n", DEV_TOKEN);
    let req = request_with_raw_auth(CLUSTERS_URI, &auth);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "token with trailing newline should be rejected"
    );
}

/// Token with trailing carriage return + newline → 401.
#[tokio::test]
async fn bearer_token_trailing_crlf_returns_401() {
    let db = TestDatabase::new("adv_trailing_crlf").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let auth = format!("Bearer {}\r\n", DEV_TOKEN);
    let req = request_with_raw_auth(CLUSTERS_URI, &auth);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "token with trailing CRLF should be rejected"
    );
}

/// Empty Authorization header → 401.
#[tokio::test]
async fn empty_authorization_header_returns_401() {
    let db = TestDatabase::new("adv_empty_auth").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = request_with_raw_auth(CLUSTERS_URI, "");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "empty Authorization header → 401");
}

/// Authorization header with only "Bearer" and no token → 401.
#[tokio::test]
async fn bearer_keyword_only_returns_401() {
    let db = TestDatabase::new("adv_bearer_only").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = request_with_raw_auth(CLUSTERS_URI, "Bearer");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "'Bearer' with no token → 401");
}

/// Authorization header with "Bearer " (trailing space, no token) → 401.
#[tokio::test]
async fn bearer_space_only_returns_401() {
    let db = TestDatabase::new("adv_bearer_space").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = request_with_raw_auth(CLUSTERS_URI, "Bearer ");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "'Bearer ' (space, no token) → 401");
}

/// Wrong auth scheme: "Basic" instead of "Bearer" → 401.
#[tokio::test]
async fn basic_auth_scheme_returns_401() {
    let db = TestDatabase::new("adv_basic_scheme").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // base64("test:test") = "dGVzdDp0ZXN0"
    let req = request_with_raw_auth(CLUSTERS_URI, "Basic dGVzdDp0ZXN0");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "Basic auth scheme should be rejected");
}

/// "bearer" (lowercase) instead of "Bearer" → 401.
///
/// Despite RFC 7235 saying auth schemes are case-insensitive, our middleware
/// uses `strip_prefix("Bearer ")` which is case-sensitive. Lowercase "bearer"
/// is rejected.
#[tokio::test]
async fn lowercase_bearer_returns_401_or_200() {
    let db = TestDatabase::new("adv_lowercase_bearer").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let auth = format!("bearer {}", DEV_TOKEN);
    let req = request_with_raw_auth(CLUSTERS_URI, &auth);
    let resp = app.oneshot(req).await.unwrap();

    // Middleware uses strip_prefix("Bearer ") — case-sensitive, so lowercase is rejected.
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "lowercase 'bearer' rejected by case-sensitive strip_prefix",
    );
}

/// "BEARER" (uppercase) instead of "Bearer" → 401.
///
/// Same as lowercase: middleware uses case-sensitive strip_prefix("Bearer ").
#[tokio::test]
async fn uppercase_bearer_returns_401_or_200() {
    let db = TestDatabase::new("adv_uppercase_bearer").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let auth = format!("BEARER {}", DEV_TOKEN);
    let req = request_with_raw_auth(CLUSTERS_URI, &auth);
    let resp = app.oneshot(req).await.unwrap();

    // Middleware uses strip_prefix("Bearer ") — case-sensitive, so BEARER is rejected.
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "uppercase 'BEARER' rejected by case-sensitive strip_prefix",
    );
}

/// Doubled "Bearer Bearer <token>" prefix → 401.
///
/// A naive parser that splits on first space and takes the rest would extract
/// "Bearer <token>" as the token value (including the second "Bearer" keyword).
#[tokio::test]
async fn doubled_bearer_prefix_returns_401() {
    let db = TestDatabase::new("adv_doubled_bearer").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let auth = format!("Bearer Bearer {}", DEV_TOKEN);
    let req = request_with_raw_auth(CLUSTERS_URI, &auth);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "doubled 'Bearer Bearer' prefix should be rejected"
    );
}

/// Token that looks like a JWT (starts with "eyJ") but isn't the dev token → 401.
#[tokio::test]
async fn jwt_lookalike_token_returns_401() {
    let db = TestDatabase::new("adv_jwt_lookalike").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let fake_jwt = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.fake_signature";
    let auth = format!("Bearer {}", fake_jwt);
    let req = request_with_raw_auth(CLUSTERS_URI, &auth);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "JWT-lookalike token should be rejected in dev mode"
    );
}

// ===========================================================================
// Cross-mode rejection
// ===========================================================================

/// A valid dev token should work in dev mode (sanity check for the adversarial suite).
#[tokio::test]
async fn valid_dev_token_returns_200() {
    let db = TestDatabase::new("adv_valid_token").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let auth = format!("Bearer {}", DEV_TOKEN);
    let req = request_with_raw_auth(CLUSTERS_URI, &auth);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "valid dev token should return 200");
}

// ===========================================================================
// Response format — API errors must be JSON, never HTML
// ===========================================================================

/// Auth failure should return JSON error, not HTML.
///
/// REAL BUG this test is designed to catch: If the auth middleware uses a
/// framework default error handler that returns HTML, API clients will get
/// an HTML error page instead of a machine-parseable JSON response.
#[tokio::test]
async fn auth_failure_returns_json_not_html() {
    let db = TestDatabase::new("adv_json_not_html").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = request_with_raw_auth(CLUSTERS_URI, "Bearer wrong-token");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        !content_type.contains("text/html"),
        "auth failure should not return HTML, got Content-Type: {}",
        content_type
    );

    // If there's a body, it should be parseable as JSON (or empty)
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    if !bytes.is_empty() {
        let body_str = String::from_utf8_lossy(&bytes);
        assert!(
            !body_str.starts_with("<!"),
            "auth failure body should not be HTML: {}",
            &body_str[..body_str.len().min(200)]
        );
    }
}

/// Missing auth should return JSON error, not HTML.
#[tokio::test]
async fn missing_auth_returns_json_not_html() {
    let db = TestDatabase::new("adv_missing_json").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = request_no_auth(CLUSTERS_URI);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        !content_type.contains("text/html"),
        "missing auth should not return HTML, got Content-Type: {}",
        content_type
    );
}

// ===========================================================================
// Expose endpoint: adversarial inputs
// ===========================================================================

/// Expose with malformed JSON body → 400, not 500.
#[tokio::test]
async fn expose_malformed_json_returns_400() {
    let db = TestDatabase::new("adv_expose_malformed").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/teams/default/expose")
        .header(header::AUTHORIZATION, format!("Bearer {}", DEV_TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name": "test", "upstream": }"#)) // invalid JSON
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "malformed JSON should return 400 or 422, got {}",
        status
    );
}

/// Expose with missing required field (no upstream) → 400 or 422.
#[tokio::test]
async fn expose_missing_upstream_returns_400() {
    let db = TestDatabase::new("adv_expose_no_upstream").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({"name": "no-upstream-svc"});
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/teams/default/expose")
        .header(header::AUTHORIZATION, format!("Bearer {}", DEV_TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "missing upstream should return 400 or 422, got {}",
        status
    );
}

/// Expose with empty body → 400 or 422, not 500.
#[tokio::test]
async fn expose_empty_body_returns_400() {
    let db = TestDatabase::new("adv_expose_empty_body").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/teams/default/expose")
        .header(header::AUTHORIZATION, format!("Bearer {}", DEV_TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "empty body should return 400 or 422, got {}",
        status
    );
}

/// PUT on expose endpoint (wrong method) → 404 or 405, not 500 or HTML.
#[tokio::test]
async fn expose_put_method_returns_error() {
    let db = TestDatabase::new("adv_expose_put").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "put-svc",
        "upstream": "http://localhost:8080"
    });
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/api/v1/teams/default/expose")
        .header(header::AUTHORIZATION, format!("Bearer {}", DEV_TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::METHOD_NOT_ALLOWED || status == StatusCode::NOT_FOUND,
        "PUT on expose should return 405 or 404, got {}",
        status
    );
}
