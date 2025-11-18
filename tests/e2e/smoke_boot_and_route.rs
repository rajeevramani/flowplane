//! Smoke test scaffold: boots minimal harness pieces and demonstrates
//! unique naming, port reservations, and deterministic teardown.
//!
//! NOTE: This test is currently a scaffold for the full E2E (Envoy + CP + echo)
//! and is marked ignored by default. Enable with `RUN_E2E=1` or remove the
//! `ignore` once the environment and harness are wired.

use std::net::SocketAddr;
use tempfile::tempdir;

mod support;
use support::api::{create_pat, ensure_team_exists, post_create_api, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

#[tokio::test]
#[ignore = "requires Docker/Envoy + CP runtime; scaffold only"]
async fn smoke_boot_and_route() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e smoke (set RUN_E2E=1 to enable)");
        return;
    }

    // Artifacts dir and teardown guard
    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    // Unique naming for this test
    let namer = UniqueNamer::for_test("smoke_boot_and_route");
    let domain = namer.domain();
    let base_path = namer.base_path();
    let route_path = namer.path("echo");

    // Reserve ports deterministically for admin/listener/echo
    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let envoy_listener = ports.reserve_labeled("envoy-listener");
    let echo_upstream = ports.reserve_labeled("echo-upstream");

    // Create per-test DB path and track for cleanup
    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Boot echo upstream
    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    // Boot control plane (in-process servers)
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    // Ensure the e2e team exists before creating API definitions
    ensure_team_exists("e2e").await.expect("create e2e team");

    // Optionally boot Envoy if binary exists
    let maybe_envoy = if EnvoyHandle::is_available() {
        let e = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("start envoy");
        e.wait_admin_ready().await;
        Some(e)
    } else {
        eprintln!("envoy binary not found; skipping full proxy validation");
        None
    };

    // Create a PAT with api-definitions:write scope (required in v0.0.2+)
    let token = create_pat(vec![
        "api-definitions:write",
        "api-definitions:read",
        "routes:read",
        "listeners:read",
        "clusters:read",
    ])
    .await
    .expect("pat");

    // Create API via Platform API using unique domain/path
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());
    let _resp =
        post_create_api(api_addr, &token, "e2e", &domain, &base_path, &namer.test_id(), &endpoint)
            .await
            .expect("create api");

    // If Envoy is available, we can now verify routing and config_dump in follow-ups
    if let Some(envoy) = maybe_envoy {
        // Probe routing through Envoy until it converges
        let body = envoy
            .wait_for_route(&domain, &route_path, 200)
            .await
            .expect("envoy did not route to echo within timeout");
        assert!(body.starts_with("echo:"), "unexpected echo response");

        // Validate config_dump contains our domain and upstream endpoint
        let dump = envoy.get_config_dump().await.expect("config_dump");
        assert!(dump.contains(&domain), "vhost domain present in config_dump");
        assert!(
            dump.contains(&echo_addr.port().to_string()),
            "upstream port present in config_dump"
        );
        assert!(dump.contains("platform-api"), "platform route resources present in config_dump");
    }

    // For now, assert that our uniqueness helpers produce distinct, well-formed values
    assert!(domain.ends_with(".e2e.local"), "domain shape");
    assert!(base_path.starts_with("/e2e/") && base_path.len() > 6, "base path shape");
    assert!(route_path.starts_with(&base_path), "route under base path");
    assert_ne!(envoy_admin, envoy_listener);
    assert_ne!(envoy_listener, echo_upstream);

    // If we reach here, consider the scaffold successful and allow teardown to delete artifacts
    // Stop echo server explicitly to free the port
    echo.stop().await;

    guard.finish(true);
}
