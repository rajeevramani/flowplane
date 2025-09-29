//! Config update: change upstream target for a cluster and assert traffic shifts.

use http_body_util::BodyExt;
use std::net::SocketAddr;
use tempfile::tempdir;

mod support;
use support::api::wait_http_ready;
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn config_update_change_upstream() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e change_upstream (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("config_update_change_upstream");
    let route_path = namer.path("swap");
    let cluster_name = format!("{}-swap", namer.test_id());

    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let echo_a = ports.reserve_labeled("echo-a");
    let echo_b = ports.reserve_labeled("echo-b");

    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Start echo A only initially
    let echo_a_addr: SocketAddr = format!("127.0.0.1:{}", echo_a).parse().unwrap();
    let mut echo1 = EchoServerHandle::start(echo_a_addr).await;

    // Boot CP and Envoy
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;
    let envoy = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("start envoy");
    envoy.wait_admin_ready().await;

    // Create cluster pointing to echo A
    let create_cluster = serde_json::json!({
        "name": cluster_name,
        "endpoints": [ {"host": "127.0.0.1", "port": echo_a_addr.port()} ],
        "connectTimeoutSeconds": 2,
        "useTls": false
    });
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(hyper_util::client::legacy::connect::HttpConnector::new());
    let uri: hyper::http::Uri = format!("http://{}/api/v1/clusters", api_addr).parse().unwrap();
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(create_cluster.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success(), "create cluster failed: {}", res.status());

    // Update DEFAULT_GATEWAY_ROUTES to add our specific path → cluster rule
    let get_routes_uri: hyper::http::Uri =
        format!("http://{}/api/v1/routes/default-gateway-routes", api_addr).parse().unwrap();
    let res = client.get(get_routes_uri.clone()).await.unwrap();
    assert!(res.status().is_success());
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let mut route_def: serde_json::Value = serde_json::from_slice(body.as_ref()).unwrap();
    // Construct our route rule
    let new_rule = serde_json::json!({
        "name": namer.test_id(),
        "match": {"path": {"type": "prefix", "value": route_path}},
        "action": {"type": "forward", "cluster": cluster_name, "timeoutSeconds": 3}
    });
    // Prepend into first vhost routes to take precedence
    let routes = route_def["config"]["virtualHosts"][0]["routes"].as_array_mut().unwrap();
    routes.insert(0, new_rule);
    // PUT updated route config
    let put_req = hyper::Request::builder()
        .method(hyper::http::Method::PUT)
        .uri(get_routes_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(route_def["config"].to_string()))
        .unwrap();
    let res = client.request(put_req).await.unwrap();
    assert!(res.status().is_success(), "update routes failed: {}", res.status());

    // Probe via Envoy; should be 200 with echo body
    let mut ok = false;
    for _ in 0..60 {
        match envoy.proxy_get("example.local", &route_path).await {
            Ok((200, body)) if body.starts_with("echo:") => {
                ok = true;
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    }
    assert!(ok, "initial routing to echo A failed");

    // Start echo B and update cluster to point to echo B
    let echo_b_addr: SocketAddr = format!("127.0.0.1:{}", echo_b).parse().unwrap();
    let mut echo2 = EchoServerHandle::start(echo_b_addr).await;
    let update_cluster = serde_json::json!({
        "name": cluster_name,
        "serviceName": cluster_name,
        "endpoints": [ {"host": "127.0.0.1", "port": echo_b_addr.port()} ],
        "connectTimeoutSeconds": 2,
        "useTls": false
    });
    let put_cluster_uri: hyper::http::Uri =
        format!("http://{}/api/v1/clusters/{}", api_addr, cluster_name).parse().unwrap();
    let put_cluster = hyper::Request::builder()
        .method(hyper::http::Method::PUT)
        .uri(put_cluster_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::<bytes::Bytes>::from(update_cluster.to_string()))
        .unwrap();
    let res = client.request(put_cluster).await.unwrap();
    assert!(res.status().is_success(), "update cluster failed: {}", res.status());

    // Stop echo A to ensure traffic must shift
    echo1.stop().await;

    let mut ok2 = false;
    for _ in 0..60 {
        match envoy.proxy_get("example.local", &route_path).await {
            Ok((200, body)) if body.starts_with("echo:") => {
                ok2 = true;
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
        }
    }
    assert!(ok2, "routing did not recover to echo B after cluster update");

    // Cleanup
    echo2.stop().await;
    guard.finish(true);
}
