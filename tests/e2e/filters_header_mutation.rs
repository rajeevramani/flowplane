//! E2E: Header Mutation filter adds/removes request and response headers.

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
async fn filters_header_mutation_request_and_response() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping header mutation test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("filters_header_mutation");
    let domain = namer.domain();

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

    let token = create_pat(vec![
        "listeners:write",
        "listeners:read",
        "routes:write",
        "routes:read",
        "clusters:write",
        "clusters:read",
    ])
    .await
    .expect("pat");

    // Create listener with header mutation filter using Native API
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    // First create a cluster for routing
    let cluster_uri: hyper::http::Uri =
        format!("http://{}/api/v1/clusters", api_addr).parse().unwrap();
    let cluster_body = serde_json::json!({
        "name": namer.test_id(),
        "endpoints": [{"host": "127.0.0.1", "port": echo_addr.port()}]
    });
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(cluster_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(cluster_body.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success(), "create cluster failed: {}", res.status());

    // Create route config
    let route_name = format!("{}-routes", namer.test_id());
    let route_uri: hyper::http::Uri = format!("http://{}/api/v1/routes", api_addr).parse().unwrap();
    let route_body = serde_json::json!({
        "name": route_name,
        "virtualHosts": [{
            "name": format!("{}-vh", namer.test_id()),
            "domains": [domain],
            "routes": [{
                "name": namer.test_id(),
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": {"type": "forward", "cluster": namer.test_id(), "timeoutSeconds": 3}
            }]
        }]
    });
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(route_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(route_body.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success(), "create route failed: {}", res.status());

    // Create listener with header mutation filter
    let listener_port = ports.reserve_labeled("listener");
    let listener_uri: hyper::http::Uri =
        format!("http://{}/api/v1/listeners", api_addr).parse().unwrap();
    let listener_body = serde_json::json!({
        "name": format!("{}-listener", namer.test_id()),
        "address": "127.0.0.1",
        "port": listener_port,
        "protocol": "HTTP",
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": route_name,
                "httpFilters": [
                    {
                        "filter": {
                            "type": "header_mutation",
                            "request_headers_to_add": [
                                {"key": "x-custom-request", "value": "test-value", "append": false}
                            ],
                            "request_headers_to_remove": ["user-agent"],
                            "response_headers_to_add": [
                                {"key": "x-custom-response", "value": "response-value", "append": true}
                            ],
                            "response_headers_to_remove": ["server"]
                        }
                    }
                ]
            }]
        }]
    });
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(listener_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(listener_body.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success(), "create listener failed: {}", res.status());

    // Wait for listener to be ready and test routing
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Verify routing works through the header mutation filter
    let traffic_uri: hyper::http::Uri =
        format!("http://127.0.0.1:{}/test", listener_port).parse().unwrap();
    let traffic_req = hyper::Request::builder()
        .uri(traffic_uri)
        .header(hyper::http::header::HOST, &domain)
        .body(http_body_util::Full::<bytes::Bytes>::default())
        .unwrap();
    let traffic_res = client.request(traffic_req).await.unwrap();
    assert_eq!(traffic_res.status(), 200, "Traffic should route through header mutation filter");

    // Validate config_dump contains header mutation typed config
    let dump = envoy.get_config_dump().await.expect("config_dump");
    assert!(
        dump.contains("envoy.extensions.filters.http.header_mutation"),
        "Header mutation filter should be in config_dump"
    );
    assert!(dump.contains("x-custom-request"), "Request header mutation present");
    assert!(dump.contains("x-custom-response"), "Response header mutation present");

    echo.stop().await;
    guard.finish(true);
}
