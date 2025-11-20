//! Team Isolation xDS Integration Tests
//!
//! Validates that team-scoped resource filtering works correctly with real Envoy proxies.
//! Tests multiple Envoy instances with different team metadata to ensure proper isolation.

use std::net::SocketAddr;
use tempfile::tempdir;

mod support;
use support::api::{create_pat, post_create_api, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn team_isolation_clusters_routes_listeners() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping team isolation test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    // Setup test naming
    let namer_a = UniqueNamer::for_test("team_iso_a");
    let namer_b = UniqueNamer::for_test("team_iso_b");

    let team_a = "team-alpha";
    let team_b = "team-beta";

    let domain_a = namer_a.domain();
    let domain_b = namer_b.domain();

    let route_path_a = namer_a.path("alpha");
    let route_path_b = namer_b.path("beta");

    // Port allocation
    let mut ports = PortAllocator::new();
    let admin_team_a = ports.reserve_labeled("envoy-admin-team-a");
    let admin_team_b = ports.reserve_labeled("envoy-admin-team-b");
    let admin_all = ports.reserve_labeled("envoy-admin-all");
    let echo_upstream_a = ports.reserve_labeled("echo-upstream-a");
    let echo_upstream_b = ports.reserve_labeled("echo-upstream-b");

    // Database setup
    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-team-iso.sqlite");
    guard.track_path(&db_path);

    // Echo upstream servers
    let echo_addr_a: SocketAddr = format!("127.0.0.1:{}", echo_upstream_a).parse().unwrap();
    let echo_addr_b: SocketAddr = format!("127.0.0.1:{}", echo_upstream_b).parse().unwrap();
    let mut echo_a = EchoServerHandle::start(echo_addr_a).await;
    let mut echo_b = EchoServerHandle::start(echo_addr_b).await;

    // Start Control Plane
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    // Create PAT token
    let token = create_pat(vec![
        "openapi-import:write",
        "openapi-import:read",
        "routes:read",
        "listeners:read",
        "clusters:read",
    ])
    .await
    .expect("create pat");

    // Create OpenAPI import for Team A
    let endpoint_a = format!("127.0.0.1:{}", echo_addr_a.port());
    let _resp_a = post_create_api(
        api_addr,
        &token,
        team_a,
        &domain_a,
        &route_path_a,
        &namer_a.test_id(),
        &endpoint_a,
    )
    .await
    .expect("create api for team a");

    // Create OpenAPI import for Team B
    let endpoint_b = format!("127.0.0.1:{}", echo_addr_b.port());
    let _resp_b = post_create_api(
        api_addr,
        &token,
        team_b,
        &domain_b,
        &route_path_b,
        &namer_b.test_id(),
        &endpoint_b,
    )
    .await
    .expect("create api for team b");

    // Test Case 1: Envoy with Team A metadata sees only Team A resources
    let envoy_team_a = EnvoyHandle::start_with_metadata(
        admin_team_a,
        xds_addr.port(),
        serde_json::json!({ "team": team_a, "include_default": true }),
    )
    .expect("start envoy team a");
    envoy_team_a.wait_admin_ready().await;

    // Verify Team A config dump contains Team A cluster
    let dump_a = envoy_team_a.get_config_dump().await.expect("get team a config dump");
    assert!(dump_a.contains(&namer_a.test_id()), "Team A config should contain Team A cluster");
    assert!(
        !dump_a.contains(&namer_b.test_id()),
        "Team A config should NOT contain Team B cluster"
    );

    // Verify Team A can route to their service
    let body_a = envoy_team_a
        .wait_for_route(&domain_a, &route_path_a, 200)
        .await
        .expect("team a route should work");
    assert!(body_a.starts_with("echo:"), "Team A should receive echo response");

    // Test Case 2: Envoy with Team B metadata sees only Team B resources
    let envoy_team_b = EnvoyHandle::start_with_metadata(
        admin_team_b,
        xds_addr.port(),
        serde_json::json!({ "team": team_b, "include_default": true }),
    )
    .expect("start envoy team b");
    envoy_team_b.wait_admin_ready().await;

    // Verify Team B config dump contains Team B cluster
    let dump_b = envoy_team_b.get_config_dump().await.expect("get team b config dump");
    assert!(dump_b.contains(&namer_b.test_id()), "Team B config should contain Team B cluster");
    assert!(
        !dump_b.contains(&namer_a.test_id()),
        "Team B config should NOT contain Team A cluster"
    );

    // Verify Team B can route to their service
    let body_b = envoy_team_b
        .wait_for_route(&domain_b, &route_path_b, 200)
        .await
        .expect("team b route should work");
    assert!(body_b.starts_with("echo:"), "Team B should receive echo response");

    // Test Case 3: Envoy with no team metadata (Scope::All) sees all resources
    let envoy_all = EnvoyHandle::start_with_metadata(
        admin_all,
        xds_addr.port(),
        serde_json::json!({}), // No team metadata = Scope::All
    )
    .expect("start envoy all");
    envoy_all.wait_admin_ready().await;

    // Verify admin config dump contains both teams' clusters
    let dump_all = envoy_all.get_config_dump().await.expect("get all config dump");
    assert!(dump_all.contains(&namer_a.test_id()), "Admin config should contain Team A cluster");
    assert!(dump_all.contains(&namer_b.test_id()), "Admin config should contain Team B cluster");

    // Cleanup
    echo_a.stop().await;
    echo_b.stop().await;
    guard.finish(true);
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn team_isolation_route_filtering() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping team route filtering test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer_team1 = UniqueNamer::for_test("route_iso_team1");
    let namer_team2 = UniqueNamer::for_test("route_iso_team2");

    let team1 = "routing-team-1";
    let team2 = "routing-team-2";

    let domain = "shared-gateway.local"; // Same domain, different routes per team
    let route_path_1 = namer_team1.path("service1");
    let route_path_2 = namer_team2.path("service2");

    let mut ports = PortAllocator::new();
    let admin_1 = ports.reserve_labeled("envoy-admin-1");
    let admin_2 = ports.reserve_labeled("envoy-admin-2");
    let echo_upstream_1 = ports.reserve_labeled("echo-upstream-1");
    let echo_upstream_2 = ports.reserve_labeled("echo-upstream-2");

    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-route-iso.sqlite");
    guard.track_path(&db_path);

    let echo_addr_1: SocketAddr = format!("127.0.0.1:{}", echo_upstream_1).parse().unwrap();
    let echo_addr_2: SocketAddr = format!("127.0.0.1:{}", echo_upstream_2).parse().unwrap();
    let mut echo_1 = EchoServerHandle::start(echo_addr_1).await;
    let mut echo_2 = EchoServerHandle::start(echo_addr_2).await;

    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    let token = create_pat(vec![
        "openapi-import:write",
        "openapi-import:read",
        "routes:read",
        "listeners:read",
        "clusters:read",
    ])
    .await
    .expect("create pat");

    // Create route for Team 1
    let endpoint_1 = format!("127.0.0.1:{}", echo_addr_1.port());
    let _resp_1 = post_create_api(
        api_addr,
        &token,
        team1,
        domain,
        &route_path_1,
        &namer_team1.test_id(),
        &endpoint_1,
    )
    .await
    .expect("create api for team 1");

    // Create route for Team 2
    let endpoint_2 = format!("127.0.0.1:{}", echo_addr_2.port());
    let _resp_2 = post_create_api(
        api_addr,
        &token,
        team2,
        domain,
        &route_path_2,
        &namer_team2.test_id(),
        &endpoint_2,
    )
    .await
    .expect("create api for team 2");

    // Team 1 Envoy should only see Team 1 routes
    let envoy_1 = EnvoyHandle::start_with_metadata(
        admin_1,
        xds_addr.port(),
        serde_json::json!({ "team": team1, "include_default": true }),
    )
    .expect("start envoy 1");
    envoy_1.wait_admin_ready().await;

    let dump_1 = envoy_1.get_config_dump().await.expect("dump 1");
    assert!(dump_1.contains(&route_path_1), "Team 1 config should contain Team 1 route path");
    assert!(!dump_1.contains(&route_path_2), "Team 1 config should NOT contain Team 2 route path");

    // Team 2 Envoy should only see Team 2 routes
    let envoy_2 = EnvoyHandle::start_with_metadata(
        admin_2,
        xds_addr.port(),
        serde_json::json!({ "team": team2, "include_default": true }),
    )
    .expect("start envoy 2");
    envoy_2.wait_admin_ready().await;

    let dump_2 = envoy_2.get_config_dump().await.expect("dump 2");
    assert!(dump_2.contains(&route_path_2), "Team 2 config should contain Team 2 route path");
    assert!(!dump_2.contains(&route_path_1), "Team 2 config should NOT contain Team 1 route path");

    echo_1.stop().await;
    echo_2.stop().await;
    guard.finish(true);
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn team_isolation_listener_filtering() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping team listener filtering test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("listener_iso");
    let team_x = "team-x";
    let team_y = "team-y";

    let domain_x = namer.domain();
    let route_path = namer.path("test");

    let mut ports = PortAllocator::new();
    let admin_x = ports.reserve_labeled("envoy-admin-x");
    let admin_y = ports.reserve_labeled("envoy-admin-y");
    let echo_upstream = ports.reserve_labeled("echo-upstream");

    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-listener-iso.sqlite");
    guard.track_path(&db_path);

    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    let token = create_pat(vec![
        "openapi-import:write",
        "openapi-import:read",
        "routes:read",
        "listeners:read",
        "clusters:read",
    ])
    .await
    .expect("create pat");

    // Create isolated listener for Team X
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    // Note: This test uses the deprecated native API approach
    // It should be updated to use the new OpenAPI import endpoint
    let uri: hyper::http::Uri = format!("http://{}/api/v1/routes", api_addr).parse().unwrap();
    let body_x = serde_json::json!({
        "team": team_x,
        "domain": domain_x,
        "routes": [{
            "match": {"prefix": route_path},
            "cluster": {
                "name": format!("cluster-x-{}", namer.test_id()),
                "endpoint": format!("127.0.0.1:{}", echo_addr.port())
            },
            "timeoutSeconds": 3
        }]
    });

    let req_x = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri.clone())
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(body_x.to_string()))
        .unwrap();
    let res_x = client.request(req_x).await.unwrap();
    assert!(res_x.status().is_success(), "create listener X failed");

    // Create listener for Team Y
    let body_y = serde_json::json!({
        "team": team_y,
        "domain": domain_x,
        "routes": [{
            "match": {"prefix": route_path},
            "cluster": {
                "name": format!("cluster-y-{}", namer.test_id()),
                "endpoint": format!("127.0.0.1:{}", echo_addr.port())
            },
            "timeoutSeconds": 3
        }]
    });

    let req_y = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(body_y.to_string()))
        .unwrap();
    let res_y = client.request(req_y).await.unwrap();
    assert!(res_y.status().is_success(), "create listener Y failed");

    // Team X Envoy should see only their resources
    let envoy_x = EnvoyHandle::start_with_metadata(
        admin_x,
        xds_addr.port(),
        serde_json::json!({ "team": team_x, "include_default": false }),
    )
    .expect("start envoy x");
    envoy_x.wait_admin_ready().await;

    let dump_x = envoy_x.get_config_dump().await.expect("dump x");
    assert!(
        dump_x.contains(&format!("cluster-x-{}", namer.test_id())),
        "Team X should see their cluster"
    );
    assert!(
        !dump_x.contains(&format!("cluster-y-{}", namer.test_id())),
        "Team X should NOT see Team Y cluster"
    );

    // Team Y Envoy should see only their resources
    let envoy_y = EnvoyHandle::start_with_metadata(
        admin_y,
        xds_addr.port(),
        serde_json::json!({ "team": team_y, "include_default": false }),
    )
    .expect("start envoy y");
    envoy_y.wait_admin_ready().await;

    let dump_y = envoy_y.get_config_dump().await.expect("dump y");
    assert!(
        dump_y.contains(&format!("cluster-y-{}", namer.test_id())),
        "Team Y should see their cluster"
    );
    assert!(
        !dump_y.contains(&format!("cluster-x-{}", namer.test_id())),
        "Team Y should NOT see Team X cluster"
    );

    echo.stop().await;
    guard.finish(true);
}
