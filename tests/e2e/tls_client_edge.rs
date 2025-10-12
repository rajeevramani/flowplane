//! TLS client↔Envoy edge tests. Requires fixtures under tests/e2e/fixtures/tls.

mod support;
use support::api::{create_pat, post_create_api, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};
use support::tls::TlsFixtures;

#[tokio::test]
#[ignore = "requires TLS fixtures and envoy"]
async fn tls_client_edge_matrix() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping tls edge (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let Some(fx) = TlsFixtures::load() else {
        eprintln!("TLS fixtures not found; generate per README to enable");
        return;
    };

    // Artifacts dir and teardown guard
    let artifacts = tempfile::tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    // Unique naming for this test
    let namer = UniqueNamer::for_test("tls_client_edge_matrix");
    let domain = namer.domain();
    let base_path = namer.base_path();
    let route_path = namer.path("echo");

    // Reserve ports
    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let tls_port = ports.reserve_labeled("envoy-tls");
    let echo_upstream = ports.reserve_labeled("echo-upstream");

    // Create per-test DB path
    let db_dir = tempfile::tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Boot echo upstream
    let echo_addr: std::net::SocketAddr = format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    // Boot control plane
    let api_addr: std::net::SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: std::net::SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    // Start Envoy with TLS listener
    let envoy = EnvoyHandle::start_tls(
        envoy_admin,
        xds_addr.port(),
        tls_port,
        &fx.server_cert,
        &fx.server_key,
    )
    .expect("start envoy tls");
    envoy.wait_admin_ready().await;

    // Create API via Platform API using unique domain/path
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
        post_create_api(api_addr, &token, "e2e", &domain, &base_path, &namer.test_id(), &endpoint)
            .await
            .expect("create api");

    // Case 1: Valid CA trust, HTTPS succeeds
    let mut ok = false;
    for _ in 0..60 {
        match envoy.proxy_get_tls(tls_port, &domain, &route_path, Some(&fx.ca)).await {
            Ok((200, body)) if body.starts_with("echo:") => {
                ok = true;
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    }
    assert!(ok, "TLS edge did not route to echo within timeout");

    // Case 2: No custom CA — handshake should fail for self-signed cert
    let res = envoy.proxy_get_tls(tls_port, &domain, &route_path, None).await;
    assert!(res.is_err(), "expected TLS handshake failure without CA trust");

    // Stop echo to free port
    echo.stop().await;
    guard.finish(true);
}
