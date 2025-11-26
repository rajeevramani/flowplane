//! Platform API filter overrides: validate typed_per_filter_config presence in config_dump
//! and end-to-end routing still succeeds.

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

    // Ensure the e2e team exists before creating API definitions
    ensure_team_exists("e2e").await.expect("create e2e team");

    let token = create_pat(vec![
        "team:e2e:openapi-import:write",
        "team:e2e:openapi-import:read",
        "team:e2e:routes:read",
        "team:e2e:listeners:read",
        "team:e2e:clusters:read",
    ])
    .await
    .expect("pat");
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());

    // Craft a payload with filter overrides: CORS and JWT authn per-route settings.
    // Create API definition using OpenAPI import with x-flowplane filter extensions
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);
    let uri: hyper::http::Uri = format!(
        "http://{}/api/v1/openapi/import?team=e2e&listener_mode=new&new_listener_name={}-listener",
        api_addr,
        namer.test_id()
    )
    .parse()
    .unwrap();
    let body = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "E2E Filters Test API",
            "version": "1.0.0",
            "x-flowplane-domain": domain
        },
        "servers": [
            {
                "url": format!("http://{}", endpoint)
            }
        ],
        "paths": {
            route_path.clone(): {
                "get": {
                    "operationId": namer.test_id(),
                    "x-flowplane-filters": {
                        "cors": "allow-authenticated",
                        "authn": "disabled"
                    },
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
    assert!(res.status().is_success(), "create api failed: {}", res.status());

    // Verify routing still works
    let body = envoy
        .wait_for_route(&domain, &route_path, 200)
        .await
        .expect("route did not converge with filters");
    assert!(body.starts_with("echo:"), "unexpected echo response");

    // Validate config_dump contains the per-route typed_per_filter_config entries
    let dump = envoy.get_config_dump().await.expect("config_dump");
    assert!(dump.contains("envoy.filters.http.cors"), "CORS filter override present");
    assert!(dump.contains("envoy.filters.http.jwt_authn"), "JWT auth filter override present");

    echo.stop().await;
    guard.finish(true);
}
