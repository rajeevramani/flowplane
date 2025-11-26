//! Listener scoping: ensure Envoy DP sees only allowed listeners per metadata (team/allowlist).

use http_body_util::BodyExt;
use std::net::SocketAddr;
use tempfile::tempdir;

mod support;
use support::api::{create_pat, ensure_team_exists, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn listener_scoping_team_and_allowlist() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping listener scoping (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("listener_scoping");
    let team = "e2e";
    let domain = namer.domain();
    let route_path = namer.path("echo");
    let listener_name = format!("scoped-{}", namer.test_id());

    let mut ports = PortAllocator::new();
    let admin1 = ports.reserve_labeled("envoy-admin-1");
    let admin2 = ports.reserve_labeled("envoy-admin-2");
    let admin3 = ports.reserve_labeled("envoy-admin-3");
    let iso_port = ports.reserve_labeled("iso-listener");
    let echo_upstream = ports.reserve_labeled("echo-upstream");

    // DB path
    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Echo upstream
    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    // Start CP
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    // Ensure the e2e team exists before creating API definitions
    ensure_team_exists("e2e").await.expect("create e2e team");

    // Create PAT
    let token = create_pat(vec![
        "team:e2e:openapi-import:write",
        "team:e2e:openapi-import:read",
        "team:e2e:routes:read",
        "team:e2e:listeners:read",
        "team:e2e:clusters:read",
    ])
    .await
    .expect("pat");

    // Create isolated listener with OpenAPI import
    // In the new system, we create a listener via OpenAPI import with listener_mode=new
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);
    let uri: hyper::http::Uri = format!(
        "http://{}/api/v1/openapi/import?team={}&listener_mode=new&new_listener_name={}&new_listener_address=127.0.0.1&new_listener_port={}",
        api_addr, team, listener_name, iso_port
    )
    .parse()
    .unwrap();
    let body = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "E2E Listener Scoping Test API",
            "version": "1.0.0",
            "x-flowplane-domain": domain
        },
        "servers": [
            {
                "url": format!("http://127.0.0.1:{}", echo_addr.port())
            }
        ],
        "paths": {
            route_path.clone(): {
                "get": {
                    "operationId": namer.test_id(),
                    "responses": {
                        "200": {
                            "description": "Success"
                        }
                    }
                }
            }
        }
    });
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(body.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success(), "create isolated api failed: {}", res.status());

    // Case A: team scope only (no default)
    let envoy_a = EnvoyHandle::start_with_metadata(
        admin1,
        xds_addr.port(),
        serde_json::json!({ "team": team, "include_default": false }),
    )
    .expect("start envoy a");
    envoy_a.wait_admin_ready().await;
    // Listener set should include only the isolated listener
    let dump_a = envoy_a.get_config_dump().await.expect("dump a");
    assert!(dump_a.contains(&listener_name));
    assert!(!dump_a.contains(flowplane::openapi::defaults::DEFAULT_GATEWAY_LISTENER));
    // Route via isolated listener port
    let (code_a, body_a) = {
        // direct HTTP to iso_port
        let connector = hyper_util::client::legacy::connect::HttpConnector::new();
        let client: hyper_util::client::legacy::Client<_, _> =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(connector);
        let uri: hyper::http::Uri =
            format!("http://127.0.0.1:{}{}", iso_port, route_path).parse().unwrap();
        let req = hyper::Request::builder()
            .uri(uri)
            .header(hyper::http::header::HOST, &domain)
            .body(http_body_util::Full::<bytes::Bytes>::default())
            .unwrap();
        let res = client.request(req).await.unwrap();
        let code = res.status().as_u16();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (code, String::from_utf8_lossy(bytes.as_ref()).to_string())
    };
    assert_eq!(code_a, 200);
    assert!(body_a.starts_with("echo:"));

    // Case B: team scope with include_default
    let envoy_b = EnvoyHandle::start_with_metadata(
        admin2,
        xds_addr.port(),
        serde_json::json!({ "team": team, "include_default": true }),
    )
    .expect("start envoy b");
    envoy_b.wait_admin_ready().await;
    let dump_b = envoy_b.get_config_dump().await.expect("dump b");
    assert!(dump_b.contains(&listener_name));
    assert!(dump_b.contains(flowplane::openapi::defaults::DEFAULT_GATEWAY_LISTENER));

    // Case C: allowlist only that named listener
    let envoy_c = EnvoyHandle::start_with_metadata(
        admin3,
        xds_addr.port(),
        serde_json::json!({ "listener_allowlist": [ listener_name ] }),
    )
    .expect("start envoy c");
    envoy_c.wait_admin_ready().await;
    let dump_c = envoy_c.get_config_dump().await.expect("dump c");
    assert!(dump_c.contains(&listener_name));
    assert!(!dump_c.contains(flowplane::openapi::defaults::DEFAULT_GATEWAY_LISTENER));

    echo.stop().await;
    guard.finish(true);
}
