//! E2E tests for Platform API listenerIsolation feature.
//! Tests both isolated listener mode (true) and shared listener mode (false).

use std::net::SocketAddr;
use tempfile::tempdir;

#[path = "e2e/support/mod.rs"]
mod support;
use support::api::{create_pat, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

/// Helper to create Platform API definition via HTTP
#[allow(clippy::too_many_arguments)]
async fn post_create_platform_api_isolated(
    api_addr: SocketAddr,
    bearer: &str,
    team: &str,
    domain: &str,
    listener_port: u16,
    prefix: &str,
    cluster_name: &str,
    endpoint: &str,
) -> anyhow::Result<serde_json::Value> {
    use http_body_util::BodyExt;
    use hyper::client::conn::http1;
    use hyper::{Method, Request, Uri};
    use hyper_util::rt::TokioIo;
    use serde_json::json;
    use tokio::net::TcpStream;

    let stream = TcpStream::connect(api_addr).await?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = http1::handshake(io).await?;
    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            eprintln!("Connection error: {:?}", err);
        }
    });

    let body_json = json!({
        "team": team,
        "domain": domain,
        "listenerIsolation": true,
        "listener": {
            "bindAddress": "127.0.0.1",
            "port": listener_port,
            "protocol": "HTTP"
        },
        "routes": [{
            "match": {"prefix": prefix},
            "cluster": {
                "name": cluster_name,
                "endpoint": endpoint
            }
        }]
    });

    let uri: Uri = format!("http://{}/api/v1/api-definitions", api_addr).parse()?;
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("authorization", format!("Bearer {}", bearer))
        .header("content-type", "application/json")
        .body(body_json.to_string())?;

    let response = sender.send_request(req).await?;
    let status = response.status();
    let body_bytes = response.into_body().collect().await?.to_bytes();
    let body = String::from_utf8_lossy(&body_bytes).to_string();

    if !status.is_success() {
        anyhow::bail!("POST failed {}: {}", status, body);
    }

    Ok(serde_json::from_str(&body)?)
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn platform_api_isolated_listener_mode() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e platform api isolated (set RUN_E2E=1 to enable)");
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
    let namer = UniqueNamer::for_test("platform_api_isolated");
    let domain = namer.domain();
    let route_path = namer.path("isolated");

    // Reserve ports
    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let isolated_listener = ports.reserve_labeled("isolated-listener");
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

    // Create token
    let token = create_pat(vec!["routes:write", "routes:read", "listeners:read", "clusters:read"])
        .await
        .expect("pat");

    // Create Platform API definition with isolated listener
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());
    let resp = post_create_platform_api_isolated(
        api_addr,
        &token,
        "e2e",
        &domain,
        isolated_listener,
        &route_path,
        &namer.test_id(),
        &endpoint,
    )
    .await
    .expect("create isolated api");

    let api_id = resp["id"].as_str().expect("api id");
    eprintln!(
        "Created Platform API definition {} with isolated listener on port {}",
        api_id, isolated_listener
    );

    // Verify route works on the ISOLATED listener port (not default gateway)
    let body = envoy
        .wait_for_route_on_port(isolated_listener, &domain, &route_path, 200)
        .await
        .expect("isolated route did not converge");
    assert!(body.starts_with("echo:"), "unexpected echo response");

    // Verify config_dump shows the isolated listener
    let dump = envoy.get_config_dump().await.expect("config_dump");
    assert!(dump.contains(&isolated_listener.to_string()), "isolated listener port in config_dump");
    assert!(dump.contains(&domain), "domain present in config_dump");

    eprintln!("✓ Isolated listener mode test passed on port {}", isolated_listener);

    // Cleanup
    echo.stop().await;
    guard.finish(true);
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn platform_api_shared_listener_mode() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e platform api shared (set RUN_E2E=1 to enable)");
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
    let namer = UniqueNamer::for_test("platform_api_shared");
    let domain = namer.domain();
    let route_path = namer.path("shared");

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

    // Create token
    let token = create_pat(vec!["routes:write", "routes:read", "listeners:read", "clusters:read"])
        .await
        .expect("pat");

    // Create Platform API definition with shared listener (listenerIsolation: false)
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());
    let resp = support::api::post_create_api(
        api_addr,
        &token,
        "e2e",
        &domain,
        &route_path,
        &namer.test_id(),
        &endpoint,
    )
    .await
    .expect("create shared api");

    let api_id = resp["id"].as_str().expect("api id");
    eprintln!("Created Platform API definition {} with shared listener (default gateway)", api_id);

    // Verify route works on the DEFAULT GATEWAY listener (port 10000)
    let body = envoy
        .wait_for_route(&domain, &route_path, 200)
        .await
        .expect("shared route did not converge");
    assert!(body.starts_with("echo:"), "unexpected echo response");

    // Verify config_dump shows routes in default gateway listener
    let dump = envoy.get_config_dump().await.expect("config_dump");
    assert!(dump.contains(&domain), "domain present in config_dump");
    assert!(dump.contains("default-gateway"), "default gateway present in config_dump");

    eprintln!("✓ Shared listener mode test passed on default gateway port 10000");

    // Cleanup
    echo.stop().await;
    guard.finish(true);
}
