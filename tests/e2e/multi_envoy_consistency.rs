//! Multi-instance: boot two Envoys; both receive consistent config and route.

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
#[ignore = "requires two Envoy processes"]
async fn multi_envoy_consistency() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping multi envoy (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("multi_envoy_consistency");
    let domain = namer.domain();
    let route_path = namer.path("echo");

    let mut ports = PortAllocator::new();
    let admin1 = ports.reserve_labeled("envoy-admin-1");
    let admin2 = ports.reserve_labeled("envoy-admin-2");
    let echo_upstream = ports.reserve_labeled("echo-upstream");

    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
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

    let envoy1 = EnvoyHandle::start(admin1, xds_addr.port()).expect("start envoy1");
    envoy1.wait_admin_ready().await;
    let envoy2 = EnvoyHandle::start(admin2, xds_addr.port()).expect("start envoy2");
    envoy2.wait_admin_ready().await;

    let token = create_pat(vec![
        "api-definitions:write",
        "api-definitions:read",
        "routes:read",
        "listeners:read",
        "clusters:read",
    ])
    .await
    .expect("pat");
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());
    let _resp =
        post_create_api(api_addr, &token, "e2e", &domain, &route_path, &namer.test_id(), &endpoint)
            .await
            .expect("create api");

    // Verify both Envoys can route
    let body1 = envoy1
        .wait_for_route(&domain, &route_path, 200)
        .await
        .expect("envoy1 should route successfully");
    assert!(body1.starts_with("echo:"), "unexpected echo response from envoy1");

    let body2 = envoy2
        .wait_for_route(&domain, &route_path, 200)
        .await
        .expect("envoy2 should route successfully");
    assert!(body2.starts_with("echo:"), "unexpected echo response from envoy2");

    echo.stop().await;
    guard.finish(true);
}
