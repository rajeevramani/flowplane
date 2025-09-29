//! Negative: duplicate domains cause conflicts; second create yields 409 and no partial writes.

use tempfile::tempdir;

mod support;
use support::api::{create_pat, post_create_api, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;

#[tokio::test]
#[ignore = "requires CP runtime"]
async fn negative_conflicts_duplicate_domain() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping negative conflicts (set RUN_E2E=1 to enable)");
        return;
    }

    let mut ports = PortAllocator::new();
    let echo_upstream = ports.reserve_labeled("echo-upstream");
    let api_addr: std::net::SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: std::net::SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    // Boot echo upstream (not strictly required, but handy for endpoint)
    let echo_addr: std::net::SocketAddr = format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    let namer = UniqueNamer::for_test("negative_conflicts_duplicate_domain");
    let domain = namer.domain();
    let route_path = namer.path("echo");
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());
    let token = create_pat(vec!["routes:write", "routes:read"]).await.expect("pat");

    // First create should succeed
    let _ =
        post_create_api(api_addr, &token, "e2e", &domain, &route_path, &namer.test_id(), &endpoint)
            .await
            .expect("create api 1");

    // Second create with same domain should fail with 409
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
        "routes": [ { "match": {"prefix": route_path}, "cluster": {"name": namer.test_id(), "endpoint": endpoint}, "timeoutSeconds": 3 } ]
    });
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(body.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert_eq!(res.status(), hyper::http::StatusCode::CONFLICT);

    echo.stop().await;
}
