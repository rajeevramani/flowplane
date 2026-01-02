//! E2E: WASM filter with Install/Configure pattern.
//!
//! This test validates:
//! 1. WASM filter schema is loaded as a built-in filter type
//! 2. WASM filter can be created via API
//! 3. Filter can be installed on a listener
//! 4. xDS config contains correct WASM configuration
//! 5. Traffic flows through the WASM filter (adds headers)

use std::net::SocketAddr;
use std::path::PathBuf;
use tempfile::tempdir;

mod support;
use support::api::{create_pat, ensure_team_exists, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

/// Get the path to the WASM filter binary
fn wasm_filter_path() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(manifest_dir)
        .join("examples/custom-filter/add-header/dist/add_header_filter.wasm")
}

#[tokio::test]
#[ignore = "requires Envoy + CP runtime with WASM support"]
async fn filters_wasm_add_header_e2e() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping WASM filter test (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    // Verify WASM binary exists
    let wasm_path = wasm_filter_path();
    if !wasm_path.exists() {
        eprintln!(
            "WASM filter not built. Run: cd examples/custom-filter/add-header && cargo build --target wasm32-unknown-unknown --release"
        );
        return;
    }

    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    let namer = UniqueNamer::for_test("filters_wasm");
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

    ensure_team_exists("e2e").await.expect("create e2e team");

    let token = create_pat(vec![
        "team:e2e:listeners:write",
        "team:e2e:listeners:read",
        "team:e2e:routes:write",
        "team:e2e:routes:read",
        "team:e2e:clusters:write",
        "team:e2e:clusters:read",
        "team:e2e:filters:write",
        "team:e2e:filters:read",
    ])
    .await
    .expect("pat");

    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client: hyper_util::client::legacy::Client<_, _> =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);

    // === Step 1: Create cluster ===
    let cluster_uri: hyper::http::Uri =
        format!("http://{}/api/v1/clusters", api_addr).parse().unwrap();
    let cluster_body = serde_json::json!({
        "team": "e2e",
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

    // === Step 2: Create route config ===
    let route_name = format!("{}-routes", namer.test_id());
    let route_uri: hyper::http::Uri =
        format!("http://{}/api/v1/route-configs", api_addr).parse().unwrap();
    let route_body = serde_json::json!({
        "team": "e2e",
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

    // === Step 3: Create listener (without filters initially) ===
    let listener_port = ports.reserve_labeled("listener");
    let listener_name = format!("{}-listener", namer.test_id());
    let listener_uri: hyper::http::Uri =
        format!("http://{}/api/v1/listeners", api_addr).parse().unwrap();
    let listener_body = serde_json::json!({
        "team": "e2e",
        "name": listener_name,
        "address": "127.0.0.1",
        "port": listener_port,
        "protocol": "HTTP",
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": route_name
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

    // === Step 4: Create WASM filter using Install/Configure pattern ===
    let filter_name = format!("{}-wasm-filter", namer.test_id());
    let filter_uri: hyper::http::Uri =
        format!("http://{}/api/v1/filters", api_addr).parse().unwrap();
    let filter_body = serde_json::json!({
        "team": "e2e",
        "name": filter_name,
        "filterType": "wasm",
        "config": {
            "config": {
                "name": "add_header",
                "vm_config": {
                    "runtime": "envoy.wasm.runtime.v8",
                    "code": {
                        "local": {
                            "filename": wasm_path.to_string_lossy()
                        }
                    }
                }
            }
        }
    });
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(filter_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(filter_body.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    let status = res.status();
    let body_bytes = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body_bytes);

    assert!(status.is_success(), "create WASM filter failed: {} - {}", status, body_str);

    let filter_response: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    let filter_id = filter_response["id"].as_str().unwrap();
    eprintln!("Created WASM filter with ID: {}", filter_id);

    // === Step 5: Install filter on listener ===
    let install_uri: hyper::http::Uri =
        format!("http://{}/api/v1/filters/{}/installations", api_addr, filter_id).parse().unwrap();
    let install_body = serde_json::json!({
        "listenerName": listener_name,
        "order": 1
    });
    let req = hyper::Request::builder()
        .method(hyper::http::Method::POST)
        .uri(install_uri)
        .header(hyper::http::header::CONTENT_TYPE, "application/json")
        .header(hyper::http::header::AUTHORIZATION, format!("Bearer {}", token))
        .body(http_body_util::Full::<bytes::Bytes>::from(install_body.to_string()))
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert!(res.status().is_success(), "install WASM filter failed: {}", res.status());
    eprintln!("Installed WASM filter on listener: {}", listener_name);

    // Wait for xDS update to propagate
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // === Step 6: Verify xDS config contains WASM filter ===
    let dump = envoy.get_config_dump().await.expect("config_dump");
    assert!(
        dump.contains("envoy.filters.http.wasm"),
        "WASM filter should be in config_dump. Dump: {}",
        &dump[..dump.len().min(2000)]
    );
    assert!(dump.contains("envoy.wasm.runtime.v8"), "WASM runtime should be configured");

    // === Step 7: Test traffic and verify headers ===
    let traffic_uri: hyper::http::Uri =
        format!("http://127.0.0.1:{}/test", listener_port).parse().unwrap();
    let traffic_req = hyper::Request::builder()
        .uri(traffic_uri)
        .header(hyper::http::header::HOST, &domain)
        .body(http_body_util::Full::<bytes::Bytes>::default())
        .unwrap();
    let traffic_res = client.request(traffic_req).await.unwrap();

    assert_eq!(traffic_res.status(), 200, "Traffic should route through WASM filter");

    // Check for WASM-added response header
    let wasm_header = traffic_res.headers().get("x-wasm-response");
    assert!(
        wasm_header.is_some(),
        "WASM filter should add x-wasm-response header. Headers: {:?}",
        traffic_res.headers()
    );
    assert_eq!(
        wasm_header.unwrap().to_str().unwrap(),
        "added",
        "x-wasm-response header should have value 'added'"
    );

    eprintln!("WASM filter E2E test passed!");

    echo.stop().await;
    guard.finish(true);
}

/// Test that validates WASM filter schema is loaded as a built-in filter
#[tokio::test]
async fn filters_wasm_schema_loaded() {
    use flowplane::domain::filter_schema::FilterSchemaRegistry;

    // WASM is now a built-in schema, no need to load from directory
    let registry = FilterSchemaRegistry::with_builtin_schemas();

    // Check that WASM schema is loaded as a built-in filter
    let wasm_schema = registry.get("wasm");
    assert!(wasm_schema.is_some(), "WASM filter schema should be loaded as a built-in filter");

    let schema = wasm_schema.unwrap();
    assert_eq!(schema.name, "wasm");
    assert_eq!(schema.envoy.http_filter_name, "envoy.filters.http.wasm");
    assert_eq!(
        schema.envoy.type_url,
        "type.googleapis.com/envoy.extensions.filters.http.wasm.v3.Wasm"
    );

    eprintln!("WASM schema validation passed:");
    eprintln!("  - name: {}", schema.name);
    eprintln!("  - filter: {}", schema.envoy.http_filter_name);
    eprintln!("  - type_url: {}", schema.envoy.type_url);
}
