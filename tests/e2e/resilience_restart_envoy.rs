//! Resilience: restart Envoy, ensure last-good config serves and ADS resyncs.

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
async fn resilience_restart_envoy() {
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

    // Unique naming for this test
    let namer = UniqueNamer::for_test("resilience_restart_envoy");
    let domain = namer.domain();
    let route_path = namer.path("echo");

    // Reserve ports
    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let echo_upstream = ports.reserve_labeled("echo-upstream");

    // Create per-test DB path and track for cleanup
    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Boot echo upstream
    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    // Boot control plane (in-process)
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    // Start Envoy and wait admin ready
    let envoy = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("start envoy");
    envoy.wait_admin_ready().await;

    // Create token and initial API
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

    // Verify routing via Envoy
    let body = envoy
        .wait_for_route(&domain, &route_path, 200)
        .await
        .expect("initial routing did not converge");
    assert!(body.starts_with("echo:"), "unexpected echo response");

    // Restart Envoy (drop handle to kill, then start again)
    drop(envoy);
    let envoy2 = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("restart envoy");
    envoy2.wait_admin_ready().await;

    // Verify routing again after restart
    let body = envoy2
        .wait_for_route(&domain, &route_path, 200)
        .await
        .expect("routing did not recover after Envoy restart");
    assert!(body.starts_with("echo:"), "unexpected echo response");

    echo.stop().await;
    guard.finish(true);
}
