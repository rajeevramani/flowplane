//! Shared E2E test helpers for Envoy verification and resource chain setup.
//!
//! These helpers are used across multiple CLI E2E test files to verify that
//! resources reach Envoy via xDS and that traffic flows through the proxy.

use std::io::Write;
use std::time::Duration;

use crate::common::harness::TestHarness;

/// Write content to a named temp file with the given extension.
///
/// The temp file is kept alive by the caller holding the returned handle.
/// Dropping the handle deletes the file.
pub fn write_temp_file(content: &str, extension: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new().suffix(extension).tempfile().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

/// Poll Envoy config_dump until the given resource name appears, or timeout.
///
/// If Envoy is not available (API-only harness), prints a skip message and
/// returns without failing.
pub async fn verify_in_config_dump(harness: &TestHarness, resource_name: &str) {
    if !harness.has_envoy() {
        eprintln!("SKIP: Envoy not available — cannot verify config_dump");
        return;
    }
    let envoy = harness.envoy().expect("Envoy should be available");
    envoy.wait_for_config_content(resource_name).await.unwrap_or_else(|e| {
        panic!("Resource '{}' not found in Envoy config_dump: {}", resource_name, e)
    });
}

/// Verify traffic flows through Envoy for a given domain and path.
///
/// Polls Envoy proxy until the route returns HTTP 200 with a non-empty body,
/// or panics on timeout. If Envoy is not available, prints a skip message.
pub async fn verify_traffic(harness: &TestHarness, domain: &str, path: &str) {
    if !harness.has_envoy() {
        eprintln!("SKIP: Envoy not available — cannot verify traffic");
        return;
    }
    let body = harness
        .wait_for_route(domain, path, 200)
        .await
        .unwrap_or_else(|e| panic!("Traffic verification failed for {}:{} — {}", domain, path, e));
    assert!(!body.is_empty(), "Proxied response body should not be empty");
}

/// Create a route config + listener via the API to complete the Envoy chain
/// for a cluster that was already created via CLI.
///
/// Returns `(route_name, domain)` for subsequent traffic verification.
pub async fn create_chain_for_cluster(
    harness: &TestHarness,
    cluster_name: &str,
    test_prefix: &str,
) -> (String, String) {
    let team = &harness.team;
    let route_name = format!("{}-rc", test_prefix);
    let domain = format!("{}.e2e.local", test_prefix);

    // Create route config pointing to the cluster
    let route_body = serde_json::json!({
        "name": route_name,
        "virtualHosts": [{
            "name": format!("{}-vh", test_prefix),
            "domains": [&domain],
            "routes": [{
                "name": format!("{}-rt", test_prefix),
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": {"type": "forward", "cluster": cluster_name, "timeoutSeconds": 30}
            }]
        }]
    });
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{}/route-configs", team), &route_body)
        .await
        .expect("route create should succeed");
    assert!(
        resp.status().is_success(),
        "route create returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Create listener pointing to the route config
    let listener_body = serde_json::json!({
        "name": format!("{}-ls", test_prefix),
        "address": "0.0.0.0",
        "port": harness.ports.listener,
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": route_name
            }]
        }],
        "dataplaneId": "dev-dataplane-id"
    });
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{}/listeners", team), &listener_body)
        .await
        .expect("listener create should succeed");
    assert!(
        resp.status().is_success(),
        "listener create returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    tokio::time::sleep(Duration::from_secs(2)).await;
    (route_name, domain)
}

/// Create a listener via the API to complete the Envoy chain for a route config
/// that was already created via CLI (with its prerequisite cluster already in place).
///
/// Returns the domain for traffic verification.
pub async fn create_chain_for_route(
    harness: &TestHarness,
    route_name: &str,
    test_prefix: &str,
    domain: &str,
) -> String {
    let team = &harness.team;

    // Create listener pointing to the route config
    let listener_body = serde_json::json!({
        "name": format!("{}-ls", test_prefix),
        "address": "0.0.0.0",
        "port": harness.ports.listener,
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": route_name
            }]
        }],
        "dataplaneId": "dev-dataplane-id"
    });
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{}/listeners", team), &listener_body)
        .await
        .expect("listener create should succeed");
    assert!(
        resp.status().is_success(),
        "listener create returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    tokio::time::sleep(Duration::from_secs(2)).await;
    domain.to_string()
}
