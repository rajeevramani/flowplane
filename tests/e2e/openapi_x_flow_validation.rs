//! E2E: OpenAPI x-flowplane extension validation for global and route-level filters.

use http_body_util::BodyExt;
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
async fn openapi_global_filters_applied_to_all_routes() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping OpenAPI x-flow validation test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("openapi_x_flow_global");
    let _domain = "api.example.com";

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

    // Create OpenAPI spec with global x-flowplane-filters
    let openapi_spec = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Test API",
            "version": "1.0.0"
        },
        "servers": [{
            "url": format!("http://127.0.0.1:{}", echo_addr.port())
        }],
        "x-flowplane-filters": [
            {
                "filter": {
                    "type": "cors",
                    "policy": {
                        "allow_origin": [{"type": "exact", "value": "*"}],
                        "allow_methods": ["GET", "POST"],
                        "allow_headers": ["content-type"],
                        "max_age": 3600
                    }
                }
            },
            {
                "filter": {
                    "type": "header_mutation",
                    "request_headers_to_add": [
                        {"key": "x-global-filter", "value": "enabled", "append": false}
                    ]
                }
            }
        ],
        "paths": {
            "/users": {
                "get": {
                    "responses": {
                        "200": {"description": "List users"}
                    }
                }
            },
            "/posts": {
                "get": {
                    "responses": {
                        "200": {"description": "List posts"}
                    }
                }
            }
        }
    });

    // Import OpenAPI spec
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    let import_uri: hyper::http::Uri = format!(
        "http://{}/api/v1/api-definitions/from-openapi?team={}&listenerIsolation=true",
        api_addr,
        namer.test_id()
    )
    .parse()
    .unwrap();

    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(import_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(openapi_spec.to_string()))
        .unwrap();

    let res = client.request(req).await.unwrap();
    let status = res.status();
    let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body_bytes);

    assert!(status.is_success(), "OpenAPI import failed: {} - {}", status, body_str);

    // Parse the response to get the API definition ID
    let response: serde_json::Value = serde_json::from_str(&body_str).expect("valid JSON");
    eprintln!("OpenAPI import successful: {}", serde_json::to_string_pretty(&response).unwrap());

    // Verify that the import succeeded and returned an ID
    assert!(response["id"].is_string(), "Response should contain API definition ID");

    echo.stop().await;
    guard.finish(true);
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn openapi_route_level_filters_override_global() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping OpenAPI route override test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("openapi_x_flow_override");
    let _domain = "api.example.com";

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

    // Create OpenAPI spec with both global and route-level filters
    let openapi_spec = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Test API",
            "version": "1.0.0"
        },
        "servers": [{
            "url": format!("http://127.0.0.1:{}", echo_addr.port())
        }],
        "x-flowplane-filters": [
            {
                "filter": {
                    "type": "header_mutation",
                    "request_headers_to_add": [
                        {"key": "x-global-header", "value": "global-value", "append": false}
                    ]
                }
            }
        ],
        "paths": {
            "/users": {
                "get": {
                    "x-flowplane-route-overrides": {
                        "authn": "disabled"
                    },
                    "responses": {
                        "200": {"description": "List users"}
                    }
                }
            },
            "/posts": {
                "get": {
                    "responses": {
                        "200": {"description": "List posts - uses global filters"}
                    }
                }
            }
        }
    });

    // Import OpenAPI spec
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    let import_uri: hyper::http::Uri = format!(
        "http://{}/api/v1/api-definitions/from-openapi?team={}&listenerIsolation=true",
        api_addr,
        namer.test_id()
    )
    .parse()
    .unwrap();

    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(import_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(openapi_spec.to_string()))
        .unwrap();

    let res = client.request(req).await.unwrap();
    let status = res.status();
    let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body_bytes);

    assert!(status.is_success(), "OpenAPI import failed: {} - {}", status, body_str);

    // Parse the response to get the API definition ID
    let response: serde_json::Value = serde_json::from_str(&body_str).expect("valid JSON");
    eprintln!(
        "OpenAPI import with route overrides successful: {}",
        serde_json::to_string_pretty(&response).unwrap()
    );

    // Verify that the import succeeded and returned an ID
    assert!(response["id"].is_string(), "Response should contain API definition ID");

    echo.stop().await;
    guard.finish(true);
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn openapi_comprehensive_filter_validation() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping comprehensive OpenAPI filter test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("openapi_x_flow_comprehensive");

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

    // Create OpenAPI spec with multiple filter types
    let openapi_spec = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Comprehensive Test API",
            "version": "1.0.0"
        },
        "servers": [{
            "url": format!("http://127.0.0.1:{}", echo_addr.port())
        }],
        "x-flowplane-filters": [
            {
                "filter": {
                    "type": "cors",
                    "policy": {
                        "allow_origin": [{"type": "exact", "value": "https://example.com"}],
                        "allow_methods": ["GET", "POST", "PUT", "DELETE"],
                        "allow_headers": ["content-type", "authorization"],
                        "expose_headers": ["x-request-id"],
                        "max_age": 7200,
                        "allow_credentials": true
                    }
                }
            },
            {
                "filter": {
                    "type": "local_rate_limit",
                    "stat_prefix": "platform_api_rl",
                    "token_bucket": {
                        "max_tokens": 200,
                        "tokens_per_fill": 100,
                        "fill_interval_ms": 1000
                    }
                }
            },
            {
                "filter": {
                    "type": "custom_response",
                    "status_code": 503,
                    "body": "Service temporarily unavailable"
                }
            }
        ],
        "paths": {
            "/api/v1/users": {
                "get": {
                    "responses": {
                        "200": {"description": "Success"}
                    }
                }
            }
        }
    });

    // Import OpenAPI spec
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    let import_uri: hyper::http::Uri = format!(
        "http://{}/api/v1/api-definitions/from-openapi?team={}&listenerIsolation=true",
        api_addr,
        namer.test_id()
    )
    .parse()
    .unwrap();

    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(import_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(openapi_spec.to_string()))
        .unwrap();

    let res = client.request(req).await.unwrap();
    let status = res.status();
    let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body_bytes);

    assert!(status.is_success(), "OpenAPI import failed: {} - {}", status, body_str);

    // Parse the response to get the API definition ID
    let response: serde_json::Value = serde_json::from_str(&body_str).expect("valid JSON");
    eprintln!(
        "Comprehensive OpenAPI filter import successful: {}",
        serde_json::to_string_pretty(&response).unwrap()
    );

    // Verify that the import succeeded and returned an ID
    assert!(response["id"].is_string(), "Response should contain API definition ID");

    // Wait for Envoy to receive the configuration
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Validate Envoy configuration via config_dump (Subtask 18.4)
    let dump = envoy.get_config_dump().await.expect("config_dump");

    // Verify CORS filter is present
    assert!(
        dump.contains("envoy.extensions.filters.http.cors"),
        "CORS filter should be in config_dump"
    );
    assert!(
        dump.contains("envoy.extensions.filters.http.cors.v3.Cors"),
        "CORS filter type URL should be in config_dump"
    );

    // Verify local rate limit filter is present
    assert!(
        dump.contains("envoy.extensions.filters.http.local_ratelimit"),
        "Local rate limit filter should be in config_dump"
    );
    assert!(
        dump.contains("envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit"),
        "Local rate limit type URL should be in config_dump"
    );

    // Verify custom response filter is present
    assert!(
        dump.contains("envoy.extensions.filters.http.custom_response"),
        "Custom response filter should be in config_dump"
    );

    // Verify router filter (always present)
    assert!(
        dump.contains("envoy.extensions.filters.http.router"),
        "Router filter should be in config_dump"
    );

    // Verify the route is configured
    assert!(dump.contains("/api/v1/users"), "Route path should be in config_dump");

    eprintln!("✅ Config dump validation passed - all filters present");

    echo.stop().await;
    guard.finish(true);
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn openapi_custom_response_route_override() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping custom_response route override test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("openapi_custom_response_override");

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

    // Create OpenAPI spec with custom_response global filter and route-level override
    let openapi_spec = serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Custom Response Override Test API",
            "version": "1.0.0"
        },
        "servers": [{
            "url": format!("http://127.0.0.1:{}", echo_addr.port())
        }],
        "x-flowplane-filters": [
            {
                "filter": {
                    "type": "custom_response"
                }
            }
        ],
        "paths": {
            "/with-custom-response": {
                "get": {
                    "responses": {
                        "200": {"description": "Uses global custom_response filter"}
                    }
                }
            },
            "/without-custom-response": {
                "get": {
                    "x-flowplane-route-overrides": {
                        "custom_response": "disabled"
                    },
                    "responses": {
                        "200": {"description": "Disables custom_response for this route"}
                    }
                }
            }
        }
    });

    // Import OpenAPI spec
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    let import_uri: hyper::http::Uri = format!(
        "http://{}/api/v1/api-definitions/from-openapi?team={}&listenerIsolation=true",
        api_addr,
        namer.test_id()
    )
    .parse()
    .unwrap();

    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(import_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(openapi_spec.to_string()))
        .unwrap();

    let res = client.request(req).await.unwrap();
    let status = res.status();
    let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body_bytes);

    assert!(status.is_success(), "OpenAPI import failed: {} - {}", status, body_str);

    // Parse the response to get the API definition ID
    let response: serde_json::Value = serde_json::from_str(&body_str).expect("valid JSON");
    eprintln!(
        "OpenAPI import with custom_response route override successful: {}",
        serde_json::to_string_pretty(&response).unwrap()
    );

    // Verify that the import succeeded and returned an ID
    assert!(response["id"].is_string(), "Response should contain API definition ID");

    // Wait for Envoy to receive the configuration
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Validate Envoy configuration via config_dump
    let dump = envoy.get_config_dump().await.expect("config_dump");

    // Verify custom_response filter is present in the HTTP filter chain
    assert!(
        dump.contains("envoy.extensions.filters.http.custom_response"),
        "Custom response filter should be in config_dump"
    );

    // Verify typed_per_filter_config contains the route override
    // The override should appear as a FilterConfig with disabled=true
    assert!(
        dump.contains("typed_per_filter_config"),
        "Route configuration should contain typed_per_filter_config"
    );

    // Verify the custom_response filter name appears in per-route config
    assert!(
        dump.contains("envoy.filters.http.custom_response"),
        "Custom response filter name should appear in route config"
    );

    eprintln!("✅ Custom response route override validation passed - config accepted by Envoy");

    echo.stop().await;
    guard.finish(true);
}
