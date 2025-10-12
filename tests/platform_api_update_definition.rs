//! E2E test: PATCH /api/v1/api-definitions/{id} propagates changes to Envoy
//!
//! This test verifies that updating an API definition via PATCH:
//! 1. Updates the database correctly
//! 2. Triggers xDS cache refresh
//! 3. Propagates changes to Envoy proxies
//! 4. Results in correct routing behavior

use http_body_util::BodyExt;
use std::net::SocketAddr;
use tempfile::tempdir;

#[path = "e2e/support/mod.rs"]
mod support;
use support::api::wait_http_ready;
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn platform_api_patch_updates_propagate_to_envoy() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e platform_api_patch_updates (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("platform_api_patch");
    let initial_domain = format!("{}.flowplane.dev", namer.test_id());
    let updated_domain = format!("{}-updated.flowplane.dev", namer.test_id());

    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let envoy_listener = ports.reserve_labeled("envoy-listener");
    let echo_port = ports.reserve_labeled("echo");

    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Start echo server
    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_port).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    // Boot CP and Envoy
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;
    let envoy = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("start envoy");
    envoy.wait_admin_ready().await;

    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(hyper_util::client::legacy::connect::HttpConnector::new());

    // Step 1: Create Platform API definition with initial domain
    let create_payload = serde_json::json!({
        "team": "platform-team",
        "domain": initial_domain,
        "listenerIsolation": true,
        "listener": {
            "bindAddress": "0.0.0.0",
            "port": envoy_listener,
            "protocol": "http"
        },
        "routes": [
            {
                "match": { "prefix": "/api/v1/" },
                "cluster": {
                    "name": "backend-v1",
                    "endpoint": format!("127.0.0.1:{}", echo_port)
                },
                "timeoutSeconds": 5
            },
            {
                "match": { "path": "/health" },
                "cluster": {
                    "name": "health-check",
                    "endpoint": format!("127.0.0.1:{}", echo_port)
                },
                "timeoutSeconds": 2
            }
        ]
    });

    let create_uri: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions", api_addr).parse().unwrap();
    let create_req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(create_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(create_payload.to_string()))
        .unwrap();
    let res = client.request(create_req).await.unwrap();
    assert_eq!(res.status(), 201, "create api definition failed");

    let body = res.into_body().collect().await.unwrap().to_bytes();
    let create_response: serde_json::Value = serde_json::from_slice(body.as_ref()).unwrap();
    let definition_id = create_response["id"].as_str().expect("missing definition id");

    // Step 2: Verify initial routing works with original domain
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let body = envoy
        .wait_for_route(&initial_domain, "/api/v1/test", 200)
        .await
        .expect("initial routing failed");
    assert!(body.contains("echo:"), "unexpected response from initial domain");

    // Step 3: PATCH to update domain
    let patch_payload = serde_json::json!({
        "domain": updated_domain
    });

    let patch_uri: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions/{}", api_addr, definition_id).parse().unwrap();
    let patch_req = hyper::Request::builder()
        .method(hyper::http::Method::PATCH)
        .uri(patch_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(patch_payload.to_string()))
        .unwrap();
    let res = client.request(patch_req).await.unwrap();
    assert_eq!(res.status(), 200, "patch api definition failed: {}", res.status());

    let body = res.into_body().collect().await.unwrap().to_bytes();
    let patch_response: serde_json::Value = serde_json::from_slice(body.as_ref()).unwrap();
    assert_eq!(
        patch_response["domain"].as_str().unwrap(),
        updated_domain,
        "domain not updated in response"
    );

    // Step 4: Verify old domain no longer routes (should get 404 or error)
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let old_domain_result = envoy.proxy_get(&initial_domain, "/api/v1/test").await;
    assert!(
        old_domain_result.is_err() || old_domain_result.unwrap().0 != 200,
        "old domain still routing after PATCH update"
    );

    // Step 5: Verify new domain routes correctly
    let body = envoy
        .wait_for_route(&updated_domain, "/api/v1/test", 200)
        .await
        .expect("updated domain routing failed");
    assert!(body.contains("echo:"), "unexpected response from updated domain");

    // Step 6: Verify database was updated
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect(&format!("sqlite://{}", db_path.display()))
        .await
        .expect("connect to db");
    let stored_domain: String =
        sqlx::query_scalar("SELECT domain FROM api_definitions WHERE id = ?")
            .bind(definition_id)
            .fetch_one(&pool)
            .await
            .expect("fetch domain");
    assert_eq!(stored_domain, updated_domain, "domain not persisted in database");

    // Step 7: Verify version was incremented
    let version: i64 = sqlx::query_scalar("SELECT version FROM api_definitions WHERE id = ?")
        .bind(definition_id)
        .fetch_one(&pool)
        .await
        .expect("fetch version");
    assert!(version > 1, "version should be incremented after PATCH (got {})", version);

    // Cleanup
    echo.stop().await;
    guard.finish(true);
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn platform_api_patch_target_listeners_propagates_to_envoy() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e platform_api_patch_target_listeners (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("platform_api_patch_listeners");
    let domain = format!("{}.flowplane.dev", namer.test_id());

    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let echo_port = ports.reserve_labeled("echo");

    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Start echo server
    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_port).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    // Boot CP and Envoy
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;
    let envoy = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("start envoy");
    envoy.wait_admin_ready().await;

    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(hyper_util::client::legacy::connect::HttpConnector::new());

    // Step 1: Create Platform API definition with shared listener mode
    let create_payload = serde_json::json!({
        "team": "platform-team",
        "domain": domain,
        "listenerIsolation": false,
        "targetListeners": ["default-gateway-listener"],
        "routes": [
            {
                "match": { "prefix": "/api/" },
                "cluster": {
                    "name": "backend",
                    "endpoint": format!("127.0.0.1:{}", echo_port)
                },
                "timeoutSeconds": 5
            }
        ]
    });

    let create_uri: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions", api_addr).parse().unwrap();
    let create_req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(create_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(create_payload.to_string()))
        .unwrap();
    let res = client.request(create_req).await.unwrap();
    assert_eq!(res.status(), 201, "create api definition failed");

    let body = res.into_body().collect().await.unwrap().to_bytes();
    let create_response: serde_json::Value = serde_json::from_slice(body.as_ref()).unwrap();
    let definition_id = create_response["id"].as_str().expect("missing definition id");

    // Step 2: Verify initial routing works
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let body =
        envoy.wait_for_route(&domain, "/api/test", 200).await.expect("initial routing failed");
    assert!(body.contains("echo:"), "unexpected response");

    // Step 3: PATCH to update targetListeners (verify xDS refresh propagates)
    // Note: We're not actually changing the listeners, just verifying the update path works
    let patch_payload = serde_json::json!({
        "targetListeners": ["default-gateway-listener"]
    });

    let patch_uri: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions/{}", api_addr, definition_id).parse().unwrap();
    let patch_req = hyper::Request::builder()
        .method(hyper::http::Method::PATCH)
        .uri(patch_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(patch_payload.to_string()))
        .unwrap();
    let res = client.request(patch_req).await.unwrap();
    assert_eq!(res.status(), 200, "patch api definition failed: {}", res.status());

    // Step 4: Verify routing still works after PATCH (xDS refresh succeeded)
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let body =
        envoy.wait_for_route(&domain, "/api/test", 200).await.expect("routing failed after PATCH");
    assert!(body.contains("echo:"), "unexpected response after PATCH");

    // Cleanup
    echo.stop().await;
    guard.finish(true);
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn platform_api_patch_route_cascade_updates_propagate_to_envoy() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e platform_api_patch_route_cascade (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("platform_api_patch_routes");
    let domain = format!("{}.flowplane.dev", namer.test_id());

    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let envoy_listener = ports.reserve_labeled("envoy-listener");
    let echo_port = ports.reserve_labeled("echo");
    let echo_port_2 = ports.reserve_labeled("echo-2");

    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Start echo servers (two backends for different routes)
    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_port).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    let echo_addr_2: SocketAddr = format!("127.0.0.1:{}", echo_port_2).parse().unwrap();
    let mut echo_2 = EchoServerHandle::start(echo_addr_2).await;

    // Boot CP and Envoy
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;
    let envoy = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("start envoy");
    envoy.wait_admin_ready().await;

    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(hyper_util::client::legacy::connect::HttpConnector::new());

    // Step 1: Create Platform API definition with initial routes
    let create_payload = serde_json::json!({
        "team": "platform-team",
        "domain": domain,
        "listenerIsolation": true,
        "listener": {
            "bindAddress": "0.0.0.0",
            "port": envoy_listener,
            "protocol": "http"
        },
        "routes": [
            {
                "match": { "prefix": "/api/v1/" },
                "cluster": {
                    "name": "backend-v1",
                    "endpoint": format!("127.0.0.1:{}", echo_port)
                },
                "timeoutSeconds": 5
            },
            {
                "match": { "path": "/health" },
                "cluster": {
                    "name": "health-check",
                    "endpoint": format!("127.0.0.1:{}", echo_port)
                },
                "timeoutSeconds": 2
            }
        ]
    });

    let create_uri: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions", api_addr).parse().unwrap();
    let create_req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(create_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(create_payload.to_string()))
        .unwrap();
    let res = client.request(create_req).await.unwrap();
    assert_eq!(res.status(), 201, "create api definition failed");

    let body = res.into_body().collect().await.unwrap().to_bytes();
    let create_response: serde_json::Value = serde_json::from_slice(body.as_ref()).unwrap();
    let definition_id = create_response["id"].as_str().expect("missing definition id");

    // Step 2: Verify initial routing works
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let body = envoy
        .wait_for_route(&domain, "/api/v1/test", 200)
        .await
        .expect("initial v1 routing failed");
    assert!(body.contains("echo:"), "unexpected response from v1 route");

    let body =
        envoy.wait_for_route(&domain, "/health", 200).await.expect("initial health routing failed");
    assert!(body.contains("echo:"), "unexpected response from health route");

    // Step 3: PATCH to update routes (different paths and backend)
    let patch_payload = serde_json::json!({
        "routes": [
            {
                "match": { "prefix": "/api/v2/" },
                "cluster": {
                    "name": "backend-v2",
                    "endpoint": format!("127.0.0.1:{}", echo_port_2)
                },
                "timeoutSeconds": 10
            },
            {
                "match": { "prefix": "/api/v1/" },
                "cluster": {
                    "name": "backend-v1-updated",
                    "endpoint": format!("127.0.0.1:{}", echo_port)
                },
                "timeoutSeconds": 3
            }
        ]
    });

    let patch_uri: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions/{}", api_addr, definition_id).parse().unwrap();
    let patch_req = hyper::Request::builder()
        .method(hyper::http::Method::PATCH)
        .uri(patch_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(patch_payload.to_string()))
        .unwrap();
    let res = client.request(patch_req).await.unwrap();
    assert_eq!(res.status(), 200, "patch api definition failed: {}", res.status());

    // Step 4: Verify old /health route no longer exists (cascade delete worked)
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let old_health_result = envoy.proxy_get(&domain, "/health").await;
    assert!(
        old_health_result.is_err() || old_health_result.unwrap().0 != 200,
        "old /health route still active after cascade update"
    );

    // Step 5: Verify new /api/v2/ route works (routed to echo_port_2)
    let body = envoy
        .wait_for_route(&domain, "/api/v2/test", 200)
        .await
        .expect("v2 routing failed after PATCH");
    assert!(body.contains("echo:"), "unexpected response from v2 route");

    // Step 6: Verify updated /api/v1/ route still works (should use echo_port)
    let body = envoy
        .wait_for_route(&domain, "/api/v1/test", 200)
        .await
        .expect("updated v1 routing failed after PATCH");
    assert!(body.contains("echo:"), "unexpected response from updated v1 route");

    // Step 7: Verify database route count is correct (2 routes)
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect(&format!("sqlite://{}", db_path.display()))
        .await
        .expect("connect to db");
    let route_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM api_routes WHERE api_definition_id = ?")
            .bind(definition_id)
            .fetch_one(&pool)
            .await
            .expect("fetch route count");
    assert_eq!(route_count, 2, "expected 2 routes after cascade update, got {}", route_count);

    // Step 8: Verify routes have correct match values in database
    let routes: Vec<(String, String)> = sqlx::query_as(
        "SELECT match_type, match_value FROM api_routes WHERE api_definition_id = ? ORDER BY route_order",
    )
    .bind(definition_id)
    .fetch_all(&pool)
    .await
    .expect("fetch routes");

    assert_eq!(routes.len(), 2, "expected 2 routes in database");
    assert_eq!(routes[0].0, "prefix", "first route should be prefix match");
    assert_eq!(routes[0].1, "/api/v2/", "first route should match /api/v2/");
    assert_eq!(routes[1].0, "prefix", "second route should be prefix match");
    assert_eq!(routes[1].1, "/api/v1/", "second route should match /api/v1/");

    // Step 9: Verify version was incremented
    let version: i64 = sqlx::query_scalar("SELECT version FROM api_definitions WHERE id = ?")
        .bind(definition_id)
        .fetch_one(&pool)
        .await
        .expect("fetch version");
    assert!(version > 1, "version should be incremented after route update (got {})", version);

    // Cleanup
    echo.stop().await;
    echo_2.stop().await;
    guard.finish(true);
}
