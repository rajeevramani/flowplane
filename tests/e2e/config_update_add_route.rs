//! Config update: append a new route and verify routing + config converges.

use std::net::SocketAddr;
use tempfile::tempdir;

mod support;
use support::api::{create_pat, post_append_route, post_create_api, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn config_update_add_route() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e config update (set RUN_E2E=1 to enable)");
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
    let namer = UniqueNamer::for_test("config_update_add_route");
    let domain = namer.domain();
    let _base_path = namer.base_path();
    let route1 = namer.path("echo1");
    let route2 = namer.path("echo2");

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

    // Create token and initial API with route1
    let token = create_pat(vec![
        "team:e2e:openapi-import:write",
        "team:e2e:openapi-import:read",
        "team:e2e:routes:write",
        "team:e2e:routes:read",
        "team:e2e:listeners:read",
        "team:e2e:clusters:read",
    ])
    .await
    .expect("pat");
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());
    let resp =
        post_create_api(api_addr, &token, "e2e", &domain, &route1, &namer.test_id(), &endpoint)
            .await
            .expect("create api");
    let api_id = resp["id"].as_str().expect("api id");

    // Verify route1 works on the default gateway
    let body =
        envoy.wait_for_route(&domain, &route1, 200).await.expect("initial route did not converge");
    assert!(body.starts_with("echo:"), "unexpected echo response");

    // Append route2
    let _ = post_append_route(
        api_addr,
        &token,
        api_id,
        &route2,
        &namer.test_id(),
        &endpoint,
        Some("add route2"),
    )
    .await
    .expect("append route");

    // Verify both route1 and route2 work
    let body =
        envoy.wait_for_route(&domain, &route2, 200).await.expect("appended route did not converge");
    assert!(body.starts_with("echo:"), "unexpected echo response");

    // Optional: config_dump contains both paths
    let dump = envoy.get_config_dump().await.expect("config_dump");
    assert!(dump.contains(&route1));
    assert!(dump.contains(&route2));

    // Cleanup
    echo.stop().await;
    guard.finish(true);
}
