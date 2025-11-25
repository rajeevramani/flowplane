//! Negative: invalid API payload rejected; no Platform API resources materialized.

use http_body_util::BodyExt;
use tempfile::tempdir;

mod support;
use support::api::{create_pat, ensure_team_exists, wait_http_ready};
use support::env::ControlPlaneHandle;
use support::ports::PortAllocator;

#[tokio::test]
#[ignore = "requires CP runtime"]
async fn negative_invalid_payload_rejected() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping negative invalid payload (set RUN_E2E=1 to enable)");
        return;
    }

    let mut ports = PortAllocator::new();
    let api_addr: std::net::SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: std::net::SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    // Ensure the e2e team exists before creating API definitions
    ensure_team_exists("e2e").await.expect("create e2e team");

    let token = create_pat(vec!["team:e2e:api-definitions:write", "team:e2e:api-definitions:read"])
        .await
        .expect("pat");

    // Invalid: malformed OpenAPI spec (missing openapi version)
    let body = serde_json::json!({
        "info": {
            "title": "Invalid API",
            "version": "1.0.0"
        },
        "paths": {}
    });

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);
    let uri: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions/from-openapi?team=e2e", api_addr)
            .parse()
            .unwrap();
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(body.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert_eq!(res.status(), hyper::http::StatusCode::BAD_REQUEST);

    // Ensure list API definitions returns empty
    let uri_list: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions", api_addr).parse().unwrap();
    let req_list = hyper::Request::builder()
        .method(hyper::http::Method::GET)
        .uri(uri_list)
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::default())
        .unwrap();
    let res_list = client.request(req_list).await.unwrap();
    assert!(res_list.status().is_success());
    let bytes = res_list.into_body().collect().await.unwrap().to_bytes();
    let arr: serde_json::Value = serde_json::from_slice(bytes.as_ref()).unwrap();
    assert!(arr.as_array().unwrap().is_empty(), "no definitions should be materialized");
}
