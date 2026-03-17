//! Integration tests for dev-mode authentication, team-scoped endpoints, and expose/unexpose API.
//!
//! These tests start a real PostgreSQL via testcontainers, seed dev resources,
//! build the axum router with dev auth middleware, and exercise HTTP endpoints
//! via `tower::ServiceExt::oneshot`.

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
// Helpers
// ---------------------------------------------------------------------------

/// Build the full axum router in dev auth mode, seeded with dev resources.
/// Returns the router and an EnvGuard that restores env vars on drop.
async fn dev_router(
    db: &TestDatabase,
) -> (axum::Router, EnvGuard, std::sync::MutexGuard<'static, ()>) {
    let lock = common::env_guard::ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let mut env = EnvGuard::new();
    env.set("FLOWPLANE_AUTH_MODE", "dev");
    env.set("FLOWPLANE_DEV_TOKEN", "test-dev-token-integration");
    env.set("FLOWPLANE_COOKIE_SECURE", "false");
    env.set("FLOWPLANE_BASE_URL", "http://localhost:8080");

    // Seed dev resources (org, team, user, dataplane)
    seed_dev_resources(&db.pool).await.expect("seed dev resources");

    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), db.pool.clone()));
    (flowplane::api::routes::build_router(state), env, lock)
}

/// Make a JSON request with the dev bearer token.
fn authed_request(method: Method, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer test-dev-token-integration")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap()
}

/// Make a JSON request with a specific bearer token.
fn request_with_token(method: Method, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap()
}

/// Make a JSON request with a body and dev bearer token.
fn authed_request_with_body(method: Method, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer test-dev-token-integration")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Extract response body as JSON.
async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

// ===========================================================================
// Dev Auth Tests
// ===========================================================================

#[tokio::test]
async fn dev_auth_valid_token_returns_200() {
    let db = TestDatabase::new("dev_auth_valid").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_request(Method::GET, "/api/v1/teams/default/clusters");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn dev_auth_wrong_token_returns_401() {
    let db = TestDatabase::new("dev_auth_wrong").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = request_with_token(Method::GET, "/api/v1/teams/default/clusters", "wrong-token");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 response should be JSON, got Content-Type: {}",
        content_type
    );
}

#[tokio::test]
async fn dev_auth_missing_header_returns_401() {
    let db = TestDatabase::new("dev_auth_missing").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/teams/default/clusters")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 response should be JSON, got Content-Type: {}",
        content_type
    );
}

#[tokio::test]
async fn dev_auth_options_passthrough() {
    let db = TestDatabase::new("dev_auth_options").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/v1/teams/default/clusters")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // OPTIONS should not be rejected by auth (CORS preflight)
    assert!(
        matches!(
            resp.status(),
            StatusCode::OK | StatusCode::NO_CONTENT | StatusCode::METHOD_NOT_ALLOWED
        ),
        "OPTIONS should return 200, 204, or 405, got {}",
        resp.status()
    );
}

// ===========================================================================
// Team-Scoped Cluster CRUD
// ===========================================================================

#[tokio::test]
async fn cluster_crud_via_team_scoped_api() {
    let db = TestDatabase::new("cluster_crud").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // Create cluster (camelCase body matching CreateClusterBody serde)
    let body = serde_json::json!({
        "name": "test-svc",
        "serviceName": "test-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 9090}]
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/clusters", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "create cluster");

    // List clusters — should contain the new one
    let req = authed_request(Method::GET, "/api/v1/teams/default/clusters");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let items = json["items"].as_array().expect("items array");
    assert!(items.iter().any(|c| c["name"] == "test-svc"), "cluster should appear in list");

    // Get cluster by name
    let req = authed_request(Method::GET, "/api/v1/teams/default/clusters/test-svc");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["name"], "test-svc");

    // Update cluster
    let update_body = serde_json::json!({
        "name": "test-svc",
        "serviceName": "test-svc-updated",
        "endpoints": [{"host": "10.0.0.1", "port": 8080}]
    });
    let req = authed_request_with_body(
        Method::PUT,
        "/api/v1/teams/default/clusters/test-svc",
        update_body,
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "update cluster");

    // Delete cluster
    let req = authed_request(Method::DELETE, "/api/v1/teams/default/clusters/test-svc");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete cluster");

    // Verify deleted
    let req = authed_request(Method::GET, "/api/v1/teams/default/clusters/test-svc");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "cluster should be gone");
}

// ===========================================================================
// Team-Scoped Listener CRUD
// ===========================================================================

#[tokio::test]
async fn listener_crud_via_team_scoped_api() {
    let db = TestDatabase::new("listener_crud").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // Need a cluster + route config first for the listener to reference
    let cluster_body = serde_json::json!({
        "name": "listener-test-svc",
        "serviceName": "listener-test-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 9090}]
    });
    let req =
        authed_request_with_body(Method::POST, "/api/v1/teams/default/clusters", cluster_body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "create backing cluster");

    // Create route config
    let rc_body = serde_json::json!({
        "name": "listener-test-routes",
        "virtualHosts": [{
            "name": "default-vhost",
            "domains": ["*"],
            "routes": [{
                "name": "default-route",
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": {"type": "forward", "cluster": "listener-test-svc"}
            }]
        }]
    });
    let req =
        authed_request_with_body(Method::POST, "/api/v1/teams/default/route-configs", rc_body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "create route config");

    // Create listener (camelCase body matching CreateListenerBody serde)
    let listener_body = serde_json::json!({
        "name": "test-listener",
        "address": "0.0.0.0",
        "port": 15001,
        "protocol": "HTTP",
        "dataplaneId": "dev-dataplane-id",
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": "listener-test-routes"
            }]
        }]
    });
    let req =
        authed_request_with_body(Method::POST, "/api/v1/teams/default/listeners", listener_body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "create listener");

    // List listeners
    let req = authed_request(Method::GET, "/api/v1/teams/default/listeners");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let items = json["items"].as_array().expect("items array");
    assert!(items.iter().any(|l| l["name"] == "test-listener"), "listener in list");

    // Get listener
    let req = authed_request(Method::GET, "/api/v1/teams/default/listeners/test-listener");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Delete listener
    let req = authed_request(Method::DELETE, "/api/v1/teams/default/listeners/test-listener");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete listener");
}

// ===========================================================================
// Team-Scoped Route Config CRUD
// ===========================================================================

#[tokio::test]
async fn route_config_crud_via_team_scoped_api() {
    let db = TestDatabase::new("route_config_crud").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // Need a cluster for the route to point at
    let cluster_body = serde_json::json!({
        "name": "rc-test-svc",
        "serviceName": "rc-test-svc",
        "endpoints": [{"host": "127.0.0.1", "port": 9090}]
    });
    let req =
        authed_request_with_body(Method::POST, "/api/v1/teams/default/clusters", cluster_body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "create backing cluster");

    // Create route config
    let rc_body = serde_json::json!({
        "name": "test-rc",
        "virtualHosts": [{
            "name": "test-vhost",
            "domains": ["*"],
            "routes": [{
                "name": "test-route",
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": {"type": "forward", "cluster": "rc-test-svc"}
            }]
        }]
    });
    let req =
        authed_request_with_body(Method::POST, "/api/v1/teams/default/route-configs", rc_body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "create route config");

    // List
    let req = authed_request(Method::GET, "/api/v1/teams/default/route-configs");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let items = json["items"].as_array().expect("items array");
    assert!(items.iter().any(|r| r["name"] == "test-rc"), "route config in list");

    // Get
    let req = authed_request(Method::GET, "/api/v1/teams/default/route-configs/test-rc");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Delete
    let req = authed_request(Method::DELETE, "/api/v1/teams/default/route-configs/test-rc");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete route config");

    // Verify deleted
    let req = authed_request(Method::GET, "/api/v1/teams/default/route-configs/test-rc");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ===========================================================================
// Old non-team-scoped paths should 404
// ===========================================================================

#[tokio::test]
async fn old_non_team_scoped_paths_return_404() {
    let db = TestDatabase::new("old_paths_404").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // These old paths (without /teams/{team}/) should not be routed
    let old_paths = ["/api/v1/clusters", "/api/v1/listeners", "/api/v1/route-configs"];

    for path in &old_paths {
        let req = authed_request(Method::GET, path);
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "old path {} should return 404", path);
    }
}

// ===========================================================================
// Expose / Unexpose API
// ===========================================================================

#[tokio::test]
async fn expose_creates_cluster_route_listener_atomically() {
    let db = TestDatabase::new("expose_create").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "my-svc",
        "upstream": "http://localhost:8080"
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let json = body_json(resp).await;
    assert_eq!(json["name"], "my-svc");
    // Don't assert exact upstream — server may normalize (strip scheme)
    assert_eq!(json["cluster"], "my-svc");
    assert_eq!(json["route_config"], "my-svc-routes");
    assert_eq!(json["listener"], "my-svc-listener");
    // Port should be in the pool range
    let port = json["port"].as_u64().expect("port");
    assert!((10001..=10020).contains(&port), "port {} outside pool range", port);

    // Verify sub-resources exist
    let req = authed_request(Method::GET, "/api/v1/teams/default/clusters/my-svc");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "cluster should exist after expose");

    // Verify the cluster's stored endpoint matches the upstream host/port
    let cluster_json = body_json(resp).await;
    let endpoints = cluster_json["endpoints"].as_array().expect("endpoints array");
    assert!(!endpoints.is_empty(), "cluster should have at least one endpoint");
    assert_eq!(endpoints[0]["host"], "localhost", "endpoint host should match upstream");
    assert_eq!(endpoints[0]["port"], 8080, "endpoint port should match upstream");

    let req = authed_request(Method::GET, "/api/v1/teams/default/route-configs/my-svc-routes");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "route config should exist after expose");

    let req = authed_request(Method::GET, "/api/v1/teams/default/listeners/my-svc-listener");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "listener should exist after expose");
}

#[tokio::test]
async fn unexpose_deletes_all_three_resources() {
    let db = TestDatabase::new("unexpose_delete").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // First expose
    let body = serde_json::json!({
        "name": "del-svc",
        "upstream": "http://localhost:9090"
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Unexpose
    let req = authed_request(Method::DELETE, "/api/v1/teams/default/expose/del-svc");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify all three resources are gone
    let req = authed_request(Method::GET, "/api/v1/teams/default/clusters/del-svc");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "cluster should be gone after unexpose");

    let req = authed_request(Method::GET, "/api/v1/teams/default/route-configs/del-svc-routes");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "route config should be gone after unexpose");

    let req = authed_request(Method::GET, "/api/v1/teams/default/listeners/del-svc-listener");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "listener should be gone after unexpose");
}

#[tokio::test]
async fn expose_idempotent_same_upstream() {
    let db = TestDatabase::new("expose_idempotent").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "idem-svc",
        "upstream": "http://localhost:7070"
    });

    // First expose
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body.clone());
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let first = body_json(resp).await;
    let first_port = first["port"].as_u64().expect("port");

    // Second expose with same name and upstream — should be idempotent (200 OK)
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "idempotent expose should return 200");
    let second = body_json(resp).await;
    assert_eq!(second["port"].as_u64().expect("port"), first_port, "port should be stable");
}

#[tokio::test]
async fn expose_rejects_empty_name() {
    let db = TestDatabase::new("expose_empty_name").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "",
        "upstream": "http://localhost:8080"
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json.get("error").is_some(), "400 response must include an 'error' field");
}

#[tokio::test]
async fn expose_rejects_invalid_upstream() {
    let db = TestDatabase::new("expose_bad_upstream").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "bad-upstream",
        "upstream": "no-port-here"
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json.get("error").is_some(), "400 response must include an 'error' field");
}

#[tokio::test]
async fn expose_rejects_port_outside_pool() {
    let db = TestDatabase::new("expose_bad_port").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "bad-port-svc",
        "upstream": "http://localhost:8080",
        "port": 9999
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert!(json.get("error").is_some(), "400 response must include an 'error' field");
}

// ===========================================================================
// Public endpoints don't require auth
// ===========================================================================

#[tokio::test]
async fn health_endpoint_no_auth_required() {
    let db = TestDatabase::new("health_no_auth").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = Request::builder().method(Method::GET).uri("/health").body(Body::empty()).unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_mode_endpoint_returns_dev() {
    let db = TestDatabase::new("auth_mode_dev").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/auth/mode")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["auth_mode"], "dev");
}

// ===========================================================================
// Unexpose on non-existent service is idempotent (204)
// ===========================================================================

#[tokio::test]
async fn unexpose_nonexistent_returns_204() {
    let db = TestDatabase::new("unexpose_nonexistent").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_request(Method::DELETE, "/api/v1/teams/default/expose/no-such-svc");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "unexpose of missing service should be 204");
}

// ===========================================================================
// Expose with explicit valid port
// ===========================================================================

#[tokio::test]
async fn expose_with_explicit_port() {
    let db = TestDatabase::new("expose_explicit_port").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "explicit-port-svc",
        "upstream": "http://localhost:8080",
        "port": 10005
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp).await;
    assert_eq!(json["port"].as_u64().unwrap(), 10005);
}

// ===========================================================================
// Expose with custom paths
// ===========================================================================

#[tokio::test]
async fn expose_with_custom_paths() {
    let db = TestDatabase::new("expose_custom_paths").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "multi-path-svc",
        "upstream": "http://localhost:8080",
        "paths": ["/api", "/health"]
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp).await;
    let paths = json["paths"].as_array().expect("paths array");
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0], "/api");
    assert_eq!(paths[1], "/health");
}

// ===========================================================================
// Expose with HTTPS scheme and path in upstream
// ===========================================================================

#[tokio::test]
async fn expose_with_https_and_path_upstream() {
    let db = TestDatabase::new("expose_https_path").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "https-path-svc",
        "upstream": "https://api.example.com:443/v2"
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let json = body_json(resp).await;
    assert_eq!(json["name"], "https-path-svc");
    assert_eq!(json["cluster"], "https-path-svc");
    assert_eq!(json["route_config"], "https-path-svc-routes");
    assert_eq!(json["listener"], "https-path-svc-listener");
    assert!(json["port"].as_u64().is_some(), "port should be present");
}

// ===========================================================================
// Expose route_config has valid cluster FK
// ===========================================================================

#[tokio::test]
async fn expose_route_config_has_valid_cluster_fk() {
    let db = TestDatabase::new("expose_fk_check").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let body = serde_json::json!({
        "name": "fk-test-svc",
        "upstream": "http://localhost:8080"
    });
    let req = authed_request_with_body(Method::POST, "/api/v1/teams/default/expose", body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Query the DB directly for the route_config's cluster_name.
    // The team column stores team IDs (not names); dev seed uses "dev-default-team-id".
    let dev_team_id = "dev-default-team-id";
    let row = sqlx::query_as::<_, (String,)>(
        "SELECT cluster_name FROM route_configs WHERE name = $1 AND team = $2",
    )
    .bind("fk-test-svc-routes")
    .bind(dev_team_id)
    .fetch_one(&db.pool)
    .await
    .expect("route_config should exist in DB");

    let cluster_name = &row.0;
    // cluster_name should be a plain string, not a JSON object
    assert!(
        !cluster_name.starts_with('{'),
        "cluster_name should be a plain string, not JSON: {}",
        cluster_name
    );
    assert_eq!(cluster_name, "fk-test-svc", "cluster_name should match the expose name");

    // Verify the cluster actually exists in the clusters table
    let cluster_exists =
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM clusters WHERE name = $1 AND team = $2")
            .bind("fk-test-svc")
            .bind(dev_team_id)
            .fetch_one(&db.pool)
            .await
            .expect("cluster query should succeed");
    assert_eq!(cluster_exists.0, 1, "cluster referenced by route_config should exist");
}

// ===========================================================================
// Adversarial API path tests — API should never return HTML
// ===========================================================================

#[tokio::test]
async fn misspelled_api_path_returns_json_not_html() {
    let db = TestDatabase::new("misspelled_path").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // "liteners" is a deliberate misspelling of "listeners"
    let req = authed_request(Method::GET, "/api/v1/teams/default/liteners");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "misspelled API path should return JSON, got Content-Type: {}",
        content_type
    );

    let json = body_json(resp).await;
    assert!(json.get("error").is_some(), "response should contain an error field");
}

#[tokio::test]
async fn nonexistent_api_path_returns_json_not_html() {
    let db = TestDatabase::new("nonexistent_path").await;
    let (app, _env, _lock) = dev_router(&db).await;

    let req = authed_request(Method::GET, "/api/v1/teams/default/nonexistent");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "nonexistent API path should return JSON, got Content-Type: {}",
        content_type
    );

    let json = body_json(resp).await;
    assert!(json.get("error").is_some(), "response should contain an error field");
}

#[tokio::test]
async fn expose_wrong_http_method_returns_error_not_html() {
    let db = TestDatabase::new("expose_wrong_method").await;
    let (app, _env, _lock) = dev_router(&db).await;

    // GET on expose endpoint — should be POST only
    let req = authed_request(Method::GET, "/api/v1/teams/default/expose");
    let resp = app.oneshot(req).await.unwrap();

    let status = resp.status();
    assert!(
        status == StatusCode::METHOD_NOT_ALLOWED || status == StatusCode::NOT_FOUND,
        "wrong method should return 405 or 404, got {}",
        status
    );

    let content_type =
        resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert!(
        !content_type.contains("text/html"),
        "API should never return HTML, got Content-Type: {}",
        content_type
    );

    // Body may be empty (axum default 405) or JSON — either is fine, just not HTML
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    if !bytes.is_empty() {
        let body_str = String::from_utf8_lossy(&bytes);
        assert!(
            !body_str.starts_with("<!"),
            "response body should not be HTML, got: {}",
            &body_str[..body_str.len().min(200)]
        );
    }
}
