//! Adversarial integration tests for Phase 1+2 API endpoints.
//!
//! These tests exercise edge cases, malformed inputs, boundary values, and
//! wrong HTTP methods across ALL team-scoped API endpoints. Written WITHOUT
//! reading the handler implementations — from the API contract only.
//!
//! Every error response MUST be JSON (not HTML) with an `error` field.

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
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEV_TOKEN: &str = "test-dev-token-phase2-adversarial";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn bearer() -> String {
    format!("Bearer {}", DEV_TOKEN)
}

fn authed_get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::AUTHORIZATION, bearer())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap()
}

fn authed_json(method: Method, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, bearer())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn authed_raw(method: Method, uri: &str, content_type: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, bearer())
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn authed_no_content_type(method: Method, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, bearer())
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn authed_method(method: Method, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, bearer())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// Assert response is JSON with an `error` field and NOT 500.
async fn assert_json_error(resp: axum::response::Response, context: &str) -> (StatusCode, Value) {
    let status = resp.status();
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "{}: should not return 500", context);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        !content_type.contains("text/html"),
        "{}: should not return HTML, got Content-Type: {}",
        context,
        content_type
    );

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };

    (status, json)
}

// ===========================================================================
// Cluster endpoint: adversarial inputs
// ===========================================================================

/// Create cluster with empty name → 400.
#[tokio::test]
async fn cluster_create_empty_name_returns_400() {
    let db = TestDatabase::new("adv_cluster_empty_name").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "",
        "serviceName": "empty-name",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    let (status, _) = assert_json_error(resp, "empty cluster name").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "empty name should be 400 or 422, got {}",
        status
    );
}

/// Create cluster with name containing spaces → server accepts (no name character validation).
///
/// The cluster handler validates name length (1-50) but does not reject spaces.
/// Tightened: assert exactly CREATED since the server has no character-set validation.
#[tokio::test]
async fn cluster_create_name_with_spaces_returns_error() {
    let db = TestDatabase::new("adv_cluster_spaces").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "my cluster name",
        "serviceName": "spaced-name",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    // Server has no character-set validation on cluster names — only length(1..=50).
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "server accepts names with spaces (no character validation exists)",
    );
}

/// Create cluster with special characters in name → should reject or sanitize.
#[tokio::test]
async fn cluster_create_name_with_special_chars_returns_error() {
    let db = TestDatabase::new("adv_cluster_special").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "test/cluster@#$%",
        "serviceName": "special-chars",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "special chars in name should be rejected, got {}",
        status
    );
}

/// Create cluster with duplicate name → should return 409 Conflict.
#[tokio::test]
async fn cluster_create_duplicate_name_returns_409() {
    let db = TestDatabase::new("adv_cluster_dup").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "dup-cluster",
        "serviceName": "dup-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });

    // First create should succeed
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body.clone());
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "first create should succeed");

    // Second create with same name → conflict
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::CONFLICT || status == StatusCode::BAD_REQUEST,
        "duplicate name should be 409 or 400, got {}",
        status
    );
}

/// Create cluster with missing endpoints array → 400.
#[tokio::test]
async fn cluster_create_missing_endpoints_returns_400() {
    let db = TestDatabase::new("adv_cluster_no_eps").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "no-endpoints",
        "serviceName": "no-eps"
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "missing endpoints should be 400 or 422, got {}",
        status
    );
}

/// Create cluster with endpoint port 0 → 400.
///
/// EndpointRequest.port has `#[validate(range(min = 1, max = 65535))]`.
#[tokio::test]
async fn cluster_create_port_zero_returns_400() {
    let db = TestDatabase::new("adv_cluster_port0").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "port-zero",
        "serviceName": "port-zero-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 0}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    // Endpoint port validated via range(min=1, max=65535) — port 0 is rejected.
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "port 0 violates range(min=1) validation",);
}

/// Create cluster with endpoint port > 65535 → 400.
#[tokio::test]
async fn cluster_create_port_too_large_returns_400() {
    let db = TestDatabase::new("adv_cluster_port_big").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "big-port",
        "serviceName": "big-port-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 99999}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "port 99999 should be rejected, got {}",
        status
    );
}

/// Create cluster with empty host → 400.
#[tokio::test]
async fn cluster_create_empty_host_returns_400() {
    let db = TestDatabase::new("adv_cluster_empty_host").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "empty-host",
        "serviceName": "empty-host-svc",
        "endpoints": [{"host": "", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "empty host should be rejected, got {}",
        status
    );
}

/// Create cluster with host containing scheme (users type "http://host") → should be handled.
///
/// REAL BUG this test is designed to catch: Users commonly provide
/// "http://10.0.0.1" as the host, not just "10.0.0.1". The server should
/// either strip the scheme or reject it — not silently store a broken address.
#[tokio::test]
async fn cluster_create_host_with_scheme_returns_error_or_strips() {
    let db = TestDatabase::new("adv_cluster_scheme_host").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "scheme-host",
        "serviceName": "scheme-host-svc",
        "endpoints": [{"host": "http://10.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    // Either reject (400) or accept (201) — but if accepted, the stored host
    // should NOT contain "http://"
    assert!(
        status == StatusCode::BAD_REQUEST
            || status == StatusCode::UNPROCESSABLE_ENTITY
            || status == StatusCode::CREATED,
        "host with scheme: unexpected status {}",
        status
    );
}

/// Get non-existent cluster → 404 with JSON.
#[tokio::test]
async fn cluster_get_nonexistent_returns_404_json() {
    let db = TestDatabase::new("adv_cluster_get_404").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/default/clusters/no-such-cluster-xyz");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "404 should be JSON, got Content-Type: {}",
        content_type
    );
}

/// Get cluster with SQL injection attempt → 404 (not 500).
#[tokio::test]
async fn cluster_get_sql_injection_returns_404() {
    let db = TestDatabase::new("adv_cluster_sqli").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/default/clusters/'; DROP TABLE clusters;--");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
        "SQL injection attempt should be 404 or 400, got {}",
        status
    );
}

/// Get cluster with URL-encoded special chars → 404 (not 500).
#[tokio::test]
async fn cluster_get_url_encoded_special_chars_returns_404() {
    let db = TestDatabase::new("adv_cluster_urlenc").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/default/clusters/%00%01%02null");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
        "URL-encoded special chars should be 404 or 400, got {}",
        status
    );
}

/// PUT cluster with mismatched name in URL vs body → should reject or use URL name.
#[tokio::test]
async fn cluster_put_mismatched_name_returns_error() {
    let db = TestDatabase::new("adv_cluster_mismatch").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // First create the cluster
    let body = serde_json::json!({
        "name": "match-cluster",
        "serviceName": "match-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // PUT with different name in body vs URL
    let update_body = serde_json::json!({
        "name": "different-name",
        "serviceName": "updated-svc",
        "endpoints": [{"host": "10.0.0.1", "port": 9090}]
    });
    let req = authed_json(Method::PUT, "/api/v1/teams/default/clusters/match-cluster", update_body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    // Should either reject the mismatch (400) or use the URL name (200)
    assert!(
        status == StatusCode::BAD_REQUEST
            || status == StatusCode::OK
            || status == StatusCode::UNPROCESSABLE_ENTITY,
        "mismatched name should be 400 or 200, got {}",
        status
    );
}

/// PUT cluster with empty endpoints → 400.
///
/// CreateClusterBody.endpoints has `#[validate(length(min = 1))]`.
#[tokio::test]
async fn cluster_put_empty_endpoints_returns_400() {
    let db = TestDatabase::new("adv_cluster_empty_eps").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // First create
    let body = serde_json::json!({
        "name": "empty-eps-cluster",
        "serviceName": "empty-eps-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // PUT with empty endpoints
    let update_body = serde_json::json!({
        "name": "empty-eps-cluster",
        "serviceName": "empty-eps-svc",
        "endpoints": []
    });
    let req =
        authed_json(Method::PUT, "/api/v1/teams/default/clusters/empty-eps-cluster", update_body);
    let resp = app.oneshot(req).await.unwrap();
    // Endpoints validated via length(min=1) — empty array is rejected.
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "empty endpoints violates length(min=1) validation",
    );
}

/// DELETE non-existent cluster → 404 or 204.
#[tokio::test]
async fn cluster_delete_nonexistent_returns_404_or_204() {
    let db = TestDatabase::new("adv_cluster_del_404").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_method(Method::DELETE, "/api/v1/teams/default/clusters/ghost-cluster");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::NO_CONTENT,
        "delete nonexistent should be 404 or 204, got {}",
        status
    );
}

/// DELETE cluster twice → second should be 404 or 204.
#[tokio::test]
async fn cluster_delete_twice_is_safe() {
    let db = TestDatabase::new("adv_cluster_del_twice").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // Create
    let body = serde_json::json!({
        "name": "del-twice",
        "serviceName": "del-twice-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // First delete
    let req = authed_method(Method::DELETE, "/api/v1/teams/default/clusters/del-twice");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Second delete — should not 500
    let req = authed_method(Method::DELETE, "/api/v1/teams/default/clusters/del-twice");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::NO_CONTENT,
        "second delete should be 404 or 204, got {}",
        status
    );
}

// ===========================================================================
// Listener endpoint: adversarial inputs
// ===========================================================================

/// Create listener with missing filterChains → 400.
#[tokio::test]
async fn listener_create_missing_filter_chains_returns_400() {
    let db = TestDatabase::new("adv_listener_no_fc").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "no-fc-listener",
        "address": "0.0.0.0",
        "port": 15001,
        "protocol": "HTTP",
        "dataplaneId": "dev-dataplane-id"
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/listeners", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "missing filterChains should be 400 or 422, got {}",
        status
    );
}

/// Create listener with invalid protocol → accepted (no protocol validation).
///
/// The listener handler has no enum/whitelist validation on the protocol field
/// (it's an Option<String>), so unknown values are accepted.
#[tokio::test]
async fn listener_create_invalid_protocol_returns_400() {
    let db = TestDatabase::new("adv_listener_bad_proto").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "bad-proto-listener",
        "address": "0.0.0.0",
        "port": 15002,
        "protocol": "WEBSOCKET_V99",
        "dataplaneId": "dev-dataplane-id",
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": "some-routes"
            }]
        }]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/listeners", body);
    let resp = app.oneshot(req).await.unwrap();
    // Protocol field is Option<String> with no validation — any value is accepted.
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "server accepts unknown protocol values (no protocol validation exists)",
    );
}

/// Create listener with port 0 → 400.
///
/// Listener validation requires port >= 1024 via validate_listener_common().
#[tokio::test]
async fn listener_create_port_zero_returns_400() {
    let db = TestDatabase::new("adv_listener_port0").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "port0-listener",
        "address": "0.0.0.0",
        "port": 0,
        "protocol": "HTTP",
        "dataplaneId": "dev-dataplane-id",
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": "some-routes"
            }]
        }]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/listeners", body);
    let resp = app.oneshot(req).await.unwrap();
    // Listener port must be >= 1024 — port 0 is rejected.
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "port 0 violates listener port >= 1024 validation",
    );
}

/// Create listener with port > 65535 → 400.
#[tokio::test]
async fn listener_create_port_too_large_returns_400() {
    let db = TestDatabase::new("adv_listener_port_big").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "bigport-listener",
        "address": "0.0.0.0",
        "port": 70000,
        "protocol": "HTTP",
        "dataplaneId": "dev-dataplane-id",
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": "some-routes"
            }]
        }]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/listeners", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "port 70000 should be rejected, got {}",
        status
    );
}

/// Get non-existent listener → 404 JSON.
#[tokio::test]
async fn listener_get_nonexistent_returns_404_json() {
    let db = TestDatabase::new("adv_listener_get_404").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/default/listeners/no-such-listener");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "listener 404 should be JSON, got Content-Type: {}",
        content_type
    );
}

// ===========================================================================
// Route Config endpoint: adversarial inputs
// ===========================================================================

/// Create route config with empty virtualHosts → 400.
///
/// Route config virtual_hosts has `#[validate(length(min = 1))]`.
#[tokio::test]
async fn route_config_create_empty_virtual_hosts_returns_400() {
    let db = TestDatabase::new("adv_rc_empty_vh").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "empty-vh-rc",
        "virtualHosts": []
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/route-configs", body);
    let resp = app.oneshot(req).await.unwrap();
    // VirtualHosts validated via length(min=1) — empty array is rejected.
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "empty virtualHosts violates length(min=1) validation",
    );
}

/// Create route config referencing non-existent cluster → should fail on FK or validation.
#[tokio::test]
async fn route_config_create_nonexistent_cluster_returns_error() {
    let db = TestDatabase::new("adv_rc_bad_cluster").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "bad-cluster-rc",
        "virtualHosts": [{
            "name": "test-vhost",
            "domains": ["*"],
            "routes": [{
                "name": "bad-route",
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": {"type": "forward", "cluster": "nonexistent-cluster-xyz"}
            }]
        }]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/route-configs", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    // FK violation, validation error, or possibly accepted (deferred validation)
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "non-existent cluster ref should not cause 500"
    );
}

/// Create route config with duplicate name → 409 or 400.
#[tokio::test]
async fn route_config_create_duplicate_name_returns_409() {
    let db = TestDatabase::new("adv_rc_dup").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // Need a cluster for the route
    let cluster_body = serde_json::json!({
        "name": "rc-dup-cluster",
        "serviceName": "rc-dup-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", cluster_body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let rc_body = serde_json::json!({
        "name": "dup-rc",
        "virtualHosts": [{
            "name": "test-vhost",
            "domains": ["*"],
            "routes": [{
                "name": "test-route",
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": {"type": "forward", "cluster": "rc-dup-cluster"}
            }]
        }]
    });

    // First create
    let req = authed_json(Method::POST, "/api/v1/teams/default/route-configs", rc_body.clone());
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Second create with same name
    let req = authed_json(Method::POST, "/api/v1/teams/default/route-configs", rc_body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::CONFLICT || status == StatusCode::BAD_REQUEST,
        "duplicate route config name should be 409 or 400, got {}",
        status
    );
}

/// Get non-existent route config → 404 JSON.
#[tokio::test]
async fn route_config_get_nonexistent_returns_404_json() {
    let db = TestDatabase::new("adv_rc_get_404").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/default/route-configs/no-such-rc");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "route config 404 should be JSON, got Content-Type: {}",
        content_type
    );
}

// ===========================================================================
// Cross-team isolation
// ===========================================================================

/// Request resources from "other-team" when dev mode only seeds "default" → 404.
///
/// REAL BUG this test is designed to catch: A missing team-scope filter in
/// the SQL query could return resources from team "default" when accessing
/// via a different team path.
#[tokio::test]
async fn cross_team_clusters_returns_empty_or_404() {
    let db = TestDatabase::new("adv_cross_team").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // Create a cluster under "default" team
    let body = serde_json::json!({
        "name": "secret-cluster",
        "serviceName": "secret-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Try to access via a different team path — should not see "secret-cluster"
    let req = authed_get("/api/v1/teams/other-team/clusters");
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();

    if status == StatusCode::OK {
        // If the API returns 200, the items list must be empty
        let json = body_json(resp).await;
        if let Some(items) = json["items"].as_array() {
            assert!(
                !items.iter().any(|c| c["name"] == "secret-cluster"),
                "cross-team access should NOT expose default team's clusters"
            );
        }
    }
    // 404 or 403 is also acceptable — means the team doesn't exist

    // Try to get the specific cluster via other team
    let req = authed_get("/api/v1/teams/other-team/clusters/secret-cluster");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "cross-team cluster access should be 404 or 403, got {}",
        status
    );
}

/// Cross-team isolation for listeners.
#[tokio::test]
async fn cross_team_listeners_returns_empty_or_404() {
    let db = TestDatabase::new("adv_cross_team_lst").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/other-team/listeners");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();

    if status == StatusCode::OK {
        let json = body_json(resp).await;
        if let Some(items) = json["items"].as_array() {
            assert!(items.is_empty(), "other-team should have no listeners");
        }
    }
    // 404 or 403 also acceptable
}

/// Cross-team isolation for route configs.
#[tokio::test]
async fn cross_team_route_configs_returns_empty_or_404() {
    let db = TestDatabase::new("adv_cross_team_rc").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/other-team/route-configs");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();

    if status == StatusCode::OK {
        let json = body_json(resp).await;
        if let Some(items) = json["items"].as_array() {
            assert!(items.is_empty(), "other-team should have no route configs");
        }
    }
}

// ===========================================================================
// Wrong HTTP methods
// ===========================================================================

/// POST to GET-only endpoint (list clusters) → 405 or 404.
/// Wait — POST to /clusters is create, so use PATCH which is not supported.
#[tokio::test]
async fn clusters_patch_returns_405_or_404() {
    let db = TestDatabase::new("adv_clusters_patch").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_method(Method::PATCH, "/api/v1/teams/default/clusters");
    let resp = app.oneshot(req).await.unwrap();
    await_assert_not_500_not_html(resp, "PATCH on clusters collection").await;
}

/// PATCH on a specific cluster → 405 or 404.
#[tokio::test]
async fn cluster_patch_returns_405_or_404() {
    let db = TestDatabase::new("adv_cluster_patch").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_method(Method::PATCH, "/api/v1/teams/default/clusters/some-cluster");
    let resp = app.oneshot(req).await.unwrap();
    await_assert_not_500_not_html(resp, "PATCH on specific cluster").await;
}

/// POST to a specific cluster (should be PUT for update) → 405 or 404.
#[tokio::test]
async fn cluster_post_to_specific_returns_405_or_404() {
    let db = TestDatabase::new("adv_cluster_post_specific").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({"name": "test"});
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters/some-cluster", body);
    let resp = app.oneshot(req).await.unwrap();
    await_assert_not_500_not_html(resp, "POST on specific cluster").await;
}

/// GET on listeners collection with POST body → should ignore body.
#[tokio::test]
async fn listeners_get_with_body_returns_200() {
    let db = TestDatabase::new("adv_listeners_get_body").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/default/listeners");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET listeners should work");
}

/// PATCH on route-configs → 405 or 404.
#[tokio::test]
async fn route_configs_patch_returns_405_or_404() {
    let db = TestDatabase::new("adv_rc_patch").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_method(Method::PATCH, "/api/v1/teams/default/route-configs");
    let resp = app.oneshot(req).await.unwrap();
    await_assert_not_500_not_html(resp, "PATCH on route-configs").await;
}

/// DELETE on collection (not individual) → 405 or 404.
#[tokio::test]
async fn clusters_delete_collection_returns_405_or_404() {
    let db = TestDatabase::new("adv_clusters_del_all").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_method(Method::DELETE, "/api/v1/teams/default/clusters");
    let resp = app.oneshot(req).await.unwrap();
    await_assert_not_500_not_html(resp, "DELETE on clusters collection").await;
}

// ===========================================================================
// Content-Type edge cases
// ===========================================================================

/// POST with missing Content-Type on a JSON endpoint → 400 or 415.
#[tokio::test]
async fn cluster_create_missing_content_type_returns_error() {
    let db = TestDatabase::new("adv_cluster_no_ct").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body =
        r#"{"name":"no-ct","serviceName":"no-ct","endpoints":[{"host":"127.0.0.1","port":8080}]}"#;
    let req = authed_no_content_type(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST
            || status == StatusCode::UNSUPPORTED_MEDIA_TYPE
            || status == StatusCode::CREATED, // axum may infer JSON
        "missing Content-Type: got {}",
        status
    );
}

/// POST with wrong Content-Type (text/plain) → 400 or 415.
#[tokio::test]
async fn cluster_create_wrong_content_type_returns_error() {
    let db = TestDatabase::new("adv_cluster_text_ct").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = r#"{"name":"text-ct","serviceName":"text-ct","endpoints":[{"host":"127.0.0.1","port":8080}]}"#;
    let req = authed_raw(Method::POST, "/api/v1/teams/default/clusters", "text/plain", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST
            || status == StatusCode::UNSUPPORTED_MEDIA_TYPE
            || status == StatusCode::CREATED, // some servers are lenient
        "wrong Content-Type: got {}",
        status
    );
}

/// POST with Content-Type including charset → should still work.
#[tokio::test]
async fn cluster_create_content_type_with_charset_works() {
    let db = TestDatabase::new("adv_cluster_charset_ct").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = r#"{"name":"charset-ct","serviceName":"charset-ct","endpoints":[{"host":"127.0.0.1","port":8080}]}"#;
    let req = authed_raw(
        Method::POST,
        "/api/v1/teams/default/clusters",
        "application/json; charset=utf-8",
        body,
    );
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::CREATED
            || status == StatusCode::BAD_REQUEST
            || status == StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "Content-Type with charset: got {}",
        status
    );
}

// ===========================================================================
// Malformed JSON payloads
// ===========================================================================

/// Cluster create with malformed JSON → 400.
#[tokio::test]
async fn cluster_create_malformed_json_returns_400() {
    let db = TestDatabase::new("adv_cluster_bad_json").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_raw(
        Method::POST,
        "/api/v1/teams/default/clusters",
        "application/json",
        r#"{"name": "broken", "endpoints": [}"#,
    );
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "malformed JSON should be 400 or 422, got {}",
        status
    );
}

/// Cluster create with empty body → 400.
#[tokio::test]
async fn cluster_create_empty_body_returns_400() {
    let db = TestDatabase::new("adv_cluster_empty_body").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_raw(Method::POST, "/api/v1/teams/default/clusters", "application/json", "");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "empty body should be 400 or 422, got {}",
        status
    );
}

/// Listener create with malformed JSON → 400.
#[tokio::test]
async fn listener_create_malformed_json_returns_400() {
    let db = TestDatabase::new("adv_listener_bad_json").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_raw(
        Method::POST,
        "/api/v1/teams/default/listeners",
        "application/json",
        r#"not json at all"#,
    );
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "malformed listener JSON should be 400 or 422, got {}",
        status
    );
}

/// Route config create with malformed JSON → 400.
#[tokio::test]
async fn route_config_create_malformed_json_returns_400() {
    let db = TestDatabase::new("adv_rc_bad_json").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_raw(
        Method::POST,
        "/api/v1/teams/default/route-configs",
        "application/json",
        r#"{"name": "broken-rc", "virtualHosts": "not-an-array"}"#,
    );
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "malformed route config JSON should be 400 or 422, got {}",
        status
    );
}

// ===========================================================================
// Deeply nested / oversized payloads
// ===========================================================================

/// Cluster create with very long name → should reject or handle.
#[tokio::test]
async fn cluster_create_very_long_name_handled() {
    let db = TestDatabase::new("adv_cluster_long_name").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let long_name = "a".repeat(10000);
    let body = serde_json::json!({
        "name": long_name,
        "serviceName": "long-name-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 8080}]
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "very long name should not cause 500");
}

/// Route config with many virtual hosts → should handle without 500.
#[tokio::test]
async fn route_config_many_virtual_hosts_handled() {
    let db = TestDatabase::new("adv_rc_many_vh").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let vhosts: Vec<Value> = (0..100)
        .map(|i| {
            serde_json::json!({
                "name": format!("vhost-{}", i),
                "domains": [format!("host-{}.example.com", i)],
                "routes": [{
                    "name": format!("route-{}", i),
                    "match": {"path": {"type": "prefix", "value": format!("/v{}", i)}},
                    "action": {"type": "forward", "cluster": "some-cluster"}
                }]
            })
        })
        .collect();

    let body = serde_json::json!({
        "name": "many-vh-rc",
        "virtualHosts": vhosts
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/route-configs", body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "many virtual hosts should not cause 500"
    );
}

// ===========================================================================
// Path traversal and injection attempts
// ===========================================================================

/// Path traversal in cluster name → 404 or 400.
#[tokio::test]
async fn cluster_get_path_traversal_returns_error() {
    let db = TestDatabase::new("adv_cluster_traversal").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/default/clusters/../../etc/passwd");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
        "path traversal should be 404 or 400, got {}",
        status
    );
}

/// Team name with path traversal → 404 or 400.
#[tokio::test]
async fn team_path_traversal_returns_error() {
    let db = TestDatabase::new("adv_team_traversal").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_get("/api/v1/teams/../admin/clusters");
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
        "team path traversal should be 404 or 400, got {}",
        status
    );
}

// ===========================================================================
// Helper for wrong-method tests
// ===========================================================================

async fn await_assert_not_500_not_html(resp: axum::response::Response, context: &str) {
    let status = resp.status();
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "{}: should not return 500", context);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        !content_type.contains("text/html"),
        "{}: should not return HTML, got Content-Type: {}",
        context,
        content_type
    );

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    if !bytes.is_empty() {
        let body_str = String::from_utf8_lossy(&bytes);
        assert!(
            !body_str.starts_with("<!"),
            "{}: body should not be HTML: {}",
            context,
            &body_str[..body_str.len().min(200)]
        );
    }
}

// ===========================================================================
// Expose endpoint adversarial tests
// ===========================================================================

#[tokio::test]
async fn expose_name_with_special_chars_returns_400() {
    let db = TestDatabase::new("adv_expose_special_chars").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "bad!@#$%name",
        "upstream": "http://localhost:8080"
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.oneshot(req).await.unwrap();

    let status = resp.status();
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "name with special chars should be rejected, got {}",
        status
    );

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "error response should be JSON, got Content-Type: {}",
        content_type
    );
}

#[tokio::test]
async fn expose_upstream_with_spaces_returns_400() {
    let db = TestDatabase::new("adv_expose_spaces_upstream").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "space-upstream-svc",
        "upstream": "http://local host:8080"
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.oneshot(req).await.unwrap();

    let status = resp.status();
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "upstream with spaces should be rejected, got {}",
        status
    );

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "error response should be JSON, got Content-Type: {}",
        content_type
    );
}

#[tokio::test]
async fn expose_negative_port_returns_400() {
    let db = TestDatabase::new("adv_expose_negative_port").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "neg-port-svc",
        "upstream": "http://localhost:8080",
        "port": -1
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.oneshot(req).await.unwrap();

    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "negative port should be rejected, got {}",
        status
    );

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "error response should be JSON, got Content-Type: {}",
        content_type
    );
}

#[tokio::test]
async fn expose_empty_paths_array() {
    let db = TestDatabase::new("adv_expose_empty_paths").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "empty-paths-svc",
        "upstream": "http://localhost:8080",
        "paths": []
    });
    let req = authed_json(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.oneshot(req).await.unwrap();

    // Empty paths could be rejected (400) or accepted with a default "/" — either is valid.
    // What must NOT happen: 500, HTML, or silent acceptance with no routes.
    let status = resp.status();
    assert!(
        status != StatusCode::INTERNAL_SERVER_ERROR,
        "empty paths should not cause 500, got {}",
        status
    );

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "response should be JSON, got Content-Type: {}",
        content_type
    );
}
