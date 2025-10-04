//! Resilience: restart Control Plane; Envoy should serve last-good config and resync on recovery.

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
async fn resilience_restart_cp() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping resilience test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    // Artifacts dir and teardown guard
    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    // Unique naming
    let namer = UniqueNamer::for_test("resilience_restart_cp");
    let domain = namer.domain();
    let route_path = namer.path("echo");

    // Reserve ports
    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let echo_upstream = ports.reserve_labeled("echo-upstream");

    // DB path persists across CP restart
    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Boot echo upstream
    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    // Fixed API/XDS addresses to reuse on restart
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();

    // Start CP and Envoy
    let cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;
    let envoy = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("start envoy");
    envoy.wait_admin_ready().await;

    // Create token and API
    let token = create_pat(vec!["routes:write", "routes:read", "listeners:read", "clusters:read"])
        .await
        .expect("pat");
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());
    let _resp = post_create_api(
        api_addr,
        &token,
        "e2e",
        &domain,
        &namer.base_path(),
        &namer.test_id(),
        &endpoint,
    )
    .await
    .expect("create api");

    // Verify routing
    let body = envoy
        .wait_for_route(&domain, &route_path, 200)
        .await
        .expect("initial routing did not converge");
    assert!(body.starts_with("echo:"), "unexpected echo response");

    // Drop CP (simulates outage)
    drop(cp);

    // Verify Envoy still serves last-good config (no ADS updates expected during outage)
    let (code, body) =
        envoy.proxy_get(&domain, &route_path).await.expect("proxy get during outage");
    assert_eq!(code, 200);
    assert!(body.starts_with("echo:"));

    // Restart CP with same DB and addresses
    let _cp2 =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("restart cp");
    wait_http_ready(api_addr).await;

    // Verify routing continues to work after resync
    let (code2, body2) =
        envoy.proxy_get(&domain, &route_path).await.expect("proxy get after cp restart");
    assert_eq!(code2, 200);
    assert!(body2.starts_with("echo:"));

    echo.stop().await;
    drop(envoy);
    guard.finish(true);
}
