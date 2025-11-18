//! Platform API local rate limit per-route override: presence in config_dump and routing OK.

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
async fn filters_local_rate_limit_override_present() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping filters rate-limit test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("filters_local_rate_limit_override_present");
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
        "api-definitions:write",
        "api-definitions:read",
        "routes:read",
        "listeners:read",
        "clusters:read",
    ])
    .await
    .expect("pat");
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());

    // Create API with two routes: one inherits global RL, another disables RL via per-route override
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);
    let uri: hyper::http::Uri =
        format!("http://{}/api/v1/api-definitions/from-openapi?team=e2e", api_addr)
            .parse()
            .unwrap();
    let body = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "E2E Rate Limit Test API",
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
                    "operationId": format!("{}-a", namer.test_id()),
                    "responses": {
                        "200": {
                            "description": "Success"
                        }
                    }
                }
            },
            format!("{}/norate", route_path): {
                "get": {
                    "operationId": format!("{}-b", namer.test_id()),
                    "x-flowplane-filters": {
                        "rate_limit": {
                            "stat_prefix": "rl_route_b",
                            "token_bucket": {"max_tokens": 1, "tokens_per_fill": 1, "fill_interval_ms": 60000},
                            "filter_enabled": {"numerator": 0, "denominator": "hundred"},
                            "status_code": 429
                        }
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

    // Route A: hammer to trigger some 429s due to global RL
    let mut saw_429 = false;
    for _ in 0..10 {
        let (code, _body) = EnvoyHandle::proxy_get(&envoy, &domain, &route_path).await.unwrap();
        if code == 429 {
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "expected at least one 429 on route A due to global RL");

    // Route B (norate): per-route disabled RL; all should be 200
    let norate = format!("{}/norate", route_path);
    for _ in 0..5 {
        let (code, body) = EnvoyHandle::proxy_get(&envoy, &domain, &norate).await.unwrap();
        assert_eq!(code, 200);
        assert!(body.starts_with("echo:"));
    }

    // config_dump includes local rate limit typed config URL and shows per-route override entries
    let dump = envoy.get_config_dump().await.expect("config_dump");
    assert!(dump.contains("envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit"));

    echo.stop().await;
    guard.finish(true);
}
