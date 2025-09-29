//! Platform API filter overrides: validate typed_per_filter_config presence in config_dump
//! and end-to-end routing still succeeds.

use std::net::SocketAddr;
use tempfile::tempdir;

mod support;
use support::api::{create_pat, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn filters_cors_and_jwt_overrides() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping filters test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("filters_cors_and_jwt_overrides");
    let domain = namer.domain();
    let route_path = namer.path("echo");

    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
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

    let envoy = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("start envoy");
    envoy.wait_admin_ready().await;

    let token = create_pat(vec!["routes:write", "routes:read", "listeners:read", "clusters:read"])
        .await
        .expect("pat");
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());

    // Craft a payload with filter overrides: CORS and JWT authn per-route settings.
    // The test helper builds a default route; we'll inline the POST manually here for filters.
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);
    let uri: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions", api_addr).parse().unwrap();
    let body = serde_json::json!({
        "team": "e2e",
        "domain": domain,
        "listenerIsolation": false,
        "routes": [
            {
                "match": {"prefix": route_path},
                "cluster": {"name": namer.test_id(), "endpoint": endpoint},
                "timeoutSeconds": 3,
                "filters": {
                    "cors": "allow-authenticated",
                    "authn": "disabled"
                }
            }
        ]
    });
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(body.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success(), "create api failed: {}", res.status());

    // Verify routing still works
    let mut ok = false;
    for _ in 0..60 {
        match EnvoyHandle::proxy_get(&envoy, &domain, &route_path).await {
            Ok((200, body)) if body.starts_with("echo:") => {
                ok = true;
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    }
    assert!(ok, "route did not converge with filters");

    // Validate config_dump contains the per-route typed_per_filter_config entries
    let dump = envoy.get_config_dump().await.expect("config_dump");
    assert!(dump.contains("envoy.filters.http.cors"), "CORS filter override present");
    assert!(dump.contains("envoy.filters.http.jwt_authn"), "JWT auth filter override present");

    echo.stop().await;
    guard.finish(true);
}
