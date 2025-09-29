//! Config delete: create a dedicated route + listener, verify routing, then delete route and assert removal.

use std::net::SocketAddr;
use tempfile::tempdir;

mod support;
use http_body_util::BodyExt;
use support::api::wait_http_ready;
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn config_delete_cleanup() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e delete_cleanup (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("config_delete_cleanup");
    let route_name = format!("{}-rds", namer.test_id());
    let listener_name = format!("{}-listener", namer.test_id());
    let cluster_name = format!("{}-cluster", namer.test_id());
    let route_path = namer.path("gone");

    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let listener_port = ports.reserve_labeled("iso-port");
    let echo_port = ports.reserve_labeled("echo");

    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_port).parse().unwrap();
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

    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(hyper_util::client::legacy::connect::HttpConnector::new());

    // Create cluster for our listener route
    let create_cluster = serde_json::json!({
        "name": cluster_name,
        "endpoints": [ {"host": "127.0.0.1", "port": echo_addr.port()} ]
    });
    let uri_clusters: hyper::http::Uri =
        format!("http://{}/api/v1/clusters", api_addr).parse().unwrap();
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri_clusters)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(create_cluster.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success());

    // Create route configuration
    let create_route = serde_json::json!({
        "name": route_name,
        "virtualHosts": [
            {"name": format!("{}-vh", namer.test_id()), "domains": ["*"], "routes": [
                {"name": namer.test_id(), "match": {"path": {"type": "prefix", "value": route_path}}, "action": {"type": "forward", "cluster": cluster_name, "timeoutSeconds": 3}}
            ]}
        ]
    });
    let uri_routes: hyper::http::Uri =
        format!("http://{}/api/v1/routes", api_addr).parse().unwrap();
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri_routes)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(create_route.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success());

    // Create listener referencing this route config
    let create_listener = serde_json::json!({
        "name": listener_name,
        "address": "127.0.0.1",
        "port": listener_port,
        "protocol": "HTTP",
        "filterChains": [
            {"name": "default", "filters": [
                {"name": "envoy.filters.network.http_connection_manager", "type": "httpConnectionManager",
                 "routeConfigName": route_name,
                 "httpFilters": [ {"type": "router"} ]}
            ]}
        ]
    });
    let uri_listeners: hyper::http::Uri =
        format!("http://{}/api/v1/listeners", api_addr).parse().unwrap();
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri_listeners)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(create_listener.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success());

    // Probe route via dedicated listener
    let (code_ok, body_ok) = {
        let uri: hyper::http::Uri =
            format!("http://127.0.0.1:{}{}", listener_port, route_path).parse().unwrap();
        let req = hyper::Request::builder()
            .uri(uri)
            .header(hyper::http::header::HOST, "test.local")
            .body(http_body_util::Full::<bytes::Bytes>::default())
            .unwrap();
        let res = client.request(req).await.unwrap();
        let code = res.status().as_u16();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (code, String::from_utf8_lossy(body.as_ref()).to_string())
    };
    assert_eq!(code_ok, 200, "listener route should work");
    assert!(body_ok.starts_with("echo:"));

    // Delete route configuration
    let del_route_uri: hyper::http::Uri =
        format!("http://{}/api/v1/routes/{}", api_addr, route_name).parse().unwrap();
    let req = hyper::Request::builder()
        .method(hyper::http::Method::DELETE)
        .uri(del_route_uri)
        .body(http_body_util::Full::<bytes::Bytes>::default())
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert_eq!(res.status(), hyper::http::StatusCode::NO_CONTENT);

    // After deletion, request should not succeed (expect 404/503)
    let uri: hyper::http::Uri =
        format!("http://127.0.0.1:{}{}", listener_port, route_path).parse().unwrap();
    let req = hyper::Request::builder()
        .uri(uri)
        .header(hyper::http::header::HOST, "test.local")
        .body(http_body_util::Full::<bytes::Bytes>::default())
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status() != hyper::http::StatusCode::OK, "route should not be OK after deletion");

    // Config dump no longer contains our route name
    let dump = envoy.get_config_dump().await.expect("config_dump");
    assert!(!dump.contains(&route_name), "config_dump should not contain deleted route");

    echo.stop().await;
    guard.finish(true);
}
