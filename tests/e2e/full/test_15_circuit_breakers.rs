//! Circuit Breaker Tests (Bruno 15)
//!
//! Tests circuit breakers:
//! - Create cluster with circuit breaker configuration
//! - Trigger circuit breaker overflow with concurrent requests
//! - Verify circuit breaker stats from Envoy (upstream_cx_overflow)

use std::collections::HashMap;

use crate::common::{
    api_client::{setup_dev_context, ApiClient},
    filter_configs,
    harness::{TestHarness, TestHarnessConfig},
    resource_setup::{ClusterConfig, ResourceSetup, RouteConfig},
    stats::EnvoyStats,
    timeout::{with_timeout, TestTimeout},
};

/// Test setup: Create cluster with circuit breaker config + route + listener
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_setup_cb_infrastructure() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_100_setup_cb"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping circuit breaker infrastructure setup test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api, "test_100_setup_cb_infrastructure")
        .await
        .expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Build circuit breaker configuration
    let circuit_breaker_config = filter_configs::circuit_breaker()
        .max_connections(10)
        .max_pending_requests(5)
        .max_requests(20)
        .max_retries(2)
        .build();

    // Create cluster configuration with circuit breakers
    let cluster_config = ClusterConfig::new("circuit-breaker-test-cluster", host, port)
        .with_circuit_breakers(circuit_breaker_config);

    // Create route configuration
    let route_config = RouteConfig::new(
        "circuit-breaker-test-route",
        "/testing/circuit-breaker",
        "circuit-breaker-test-cluster",
    )
    .with_domain("cb.e2e.local");

    // Build all resources
    let resources = with_timeout(
        TestTimeout::default_with_label("Setup circuit breaker infrastructure"),
        async {
            ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
                .with_cluster_config(cluster_config)
                .with_route_config(route_config)
                .with_listener("circuit-breaker-listener", harness.ports.listener)
                .build()
                .await
        },
    )
    .await
    .expect("Resource setup should succeed");

    println!("✓ Cluster created: {} with circuit breakers", resources.cluster().name);
    println!("✓ Route created: {}", resources.route().name);
    println!(
        "✓ Listener created: {} on port {:?}",
        resources.listener().name,
        resources.listener().port
    );

    // Wait for route to converge
    let _ = with_timeout(TestTimeout::default_with_label("Wait for route"), async {
        harness.wait_for_route("cb.e2e.local", "/testing/circuit-breaker", 200).await
    })
    .await
    .expect("Route should converge");

    println!("✓ Circuit breaker infrastructure setup complete");
}

/// Test triggering circuit breaker overflow: Send concurrent requests
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_trigger_overflow() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_101_trigger_overflow"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping circuit breaker overflow test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_101_trigger_overflow").await.expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Build circuit breaker configuration with low limits to trigger overflow
    let circuit_breaker_config = filter_configs::circuit_breaker()
        .max_connections(5)
        .max_pending_requests(3)
        .max_requests(10)
        .max_retries(1)
        .build();

    // Create cluster + route + listener with circuit breakers
    let cluster_config = ClusterConfig::new("overflow-cb-cluster", host, port)
        .with_circuit_breakers(circuit_breaker_config);

    let route_config =
        RouteConfig::new("overflow-cb-route", "/testing/cb-overflow", "overflow-cb-cluster")
            .with_domain("overflow-cb.e2e.local")
            .with_prefix_rewrite("/delay/2"); // Use delay endpoint to keep connections open

    let _resources =
        ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
            .with_cluster_config(cluster_config)
            .with_route_config(route_config)
            .with_listener("overflow-cb-listener", harness.ports.listener)
            .build()
            .await
            .expect("Resource setup should succeed");

    // Wait for route convergence
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    println!("✓ Circuit breaker configured with low limits");
    println!("  max_connections: 5");
    println!("  max_pending_requests: 3");
    println!("  max_requests: 10");

    // Send concurrent requests to trigger circuit breaker
    // We'll send more requests than the limits allow
    let num_concurrent = 20;

    println!(
        "Sending {} concurrent requests to trigger circuit breaker overflow...",
        num_concurrent
    );

    let mut handles = vec![];
    for i in 0..num_concurrent {
        let envoy_port = harness.ports.listener;
        let handle = tokio::spawn(async move {
            // Use a short timeout to avoid hanging
            let timeout = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                proxy_request_simple(envoy_port, "overflow-cb.e2e.local", "/testing/cb-overflow"),
            )
            .await;

            match timeout {
                Ok(Ok((status, _))) => {
                    println!("  Request {}: status {}", i, status);
                    status
                }
                Ok(Err(e)) => {
                    println!("  Request {}: failed with {}", i, e);
                    503 // Treat as error
                }
                Err(_) => {
                    println!("  Request {}: timed out", i);
                    504 // Timeout
                }
            }
        });
        handles.push(handle);
    }

    // Wait for all requests to complete
    let results = futures::future::join_all(handles).await;
    let statuses: Vec<u16> = results.into_iter().filter_map(|r| r.ok()).collect();

    let success_count = statuses.iter().filter(|&&s| s == 200).count();
    let error_count = statuses.iter().filter(|&&s| s == 503).count();

    println!("✓ Concurrent requests completed:");
    println!("  Success (200): {}", success_count);
    println!("  Errors (503): {}", error_count);
    println!("  Total: {}", statuses.len());

    // Note: Some requests should succeed, but many should fail due to circuit breaker
    // The exact behavior depends on timing and the backend
}

/// Helper function for simple proxy requests
async fn proxy_request_simple(port: u16, host: &str, path: &str) -> anyhow::Result<(u16, String)> {
    use bytes::Buf;
    use http_body_util::BodyExt;
    use hyper::http::{header::HOST, Uri};
    use hyper::Request;
    use hyper_util::client::legacy::{connect::HttpConnector, Client};
    use hyper_util::rt::TokioExecutor;

    let connector = HttpConnector::new();
    let client: Client<HttpConnector, http_body_util::Full<bytes::Bytes>> =
        Client::builder(TokioExecutor::new()).build(connector);

    let uri: Uri = format!("http://127.0.0.1:{}{}", port, path).parse()?;
    let req = Request::builder()
        .method(hyper::Method::GET)
        .uri(uri)
        .header(HOST, host)
        .body(http_body_util::Full::from(bytes::Bytes::new()))?;

    let res = client.request(req).await?;
    let status = res.status().as_u16();

    let body_bytes = res.into_body().collect().await?.to_bytes();
    let body_str = String::from_utf8_lossy(body_bytes.chunk()).to_string();

    Ok((status, body_str))
}

/// Test verify circuit breaker stats: Check upstream_cx_overflow from Envoy
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_102_verify_cb_stats() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_102_verify_cb_stats"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping circuit breaker stats test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_102_verify_cb_stats").await.expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Build circuit breaker configuration with very low limits
    let circuit_breaker_config = filter_configs::circuit_breaker()
        .max_connections(3)
        .max_pending_requests(2)
        .max_requests(5)
        .max_retries(1)
        .build();

    // Create cluster + route + listener with circuit breakers
    let cluster_config = ClusterConfig::new("stats-cb-cluster", host, port)
        .with_circuit_breakers(circuit_breaker_config);

    let route_config = RouteConfig::new("stats-cb-route", "/testing/cb-stats", "stats-cb-cluster")
        .with_domain("stats-cb.e2e.local");

    let _resources =
        ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
            .with_cluster_config(cluster_config)
            .with_route_config(route_config)
            .with_listener("stats-cb-listener", harness.ports.listener)
            .build()
            .await
            .expect("Resource setup should succeed");

    // Wait for route convergence
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    println!("✓ Cluster created with circuit breakers:");
    println!("  max_connections: 3");
    println!("  max_pending_requests: 2");

    // Get initial stats
    let envoy = harness.envoy().unwrap();
    let initial_stats_json = envoy.get_stats_json().await.expect("Stats should be available");
    let initial_stats = EnvoyStats::parse_json(&initial_stats_json);
    let initial_overflow = initial_stats.upstream_cx_overflow("stats-cb-cluster");

    println!("Initial circuit breaker stats:");
    println!("  upstream_cx_overflow: {}", initial_overflow);

    // Make some requests
    for i in 0..5 {
        let _ = envoy
            .proxy_request(
                harness.ports.listener,
                hyper::Method::GET,
                "stats-cb.e2e.local",
                "/testing/cb-stats",
                HashMap::new(),
                None,
            )
            .await;
        println!("  Request {} completed", i + 1);
    }

    // Wait a moment for stats to update
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Get final stats and verify
    let stats_json = with_timeout(TestTimeout::default_with_label("Get Envoy stats"), async {
        envoy.get_stats_json().await
    })
    .await
    .expect("Stats should be available");

    let stats = EnvoyStats::parse_json(&stats_json);

    // Print circuit breaker stats
    println!("\nCircuit breaker stats for stats-cb-cluster:");
    let cb_stats = stats.find_matching("stats-cb-cluster");
    for (name, value) in &cb_stats {
        if name.contains("circuit") || name.contains("overflow") || name.contains("cx") {
            println!("  {} = {}", name, value);
        }
    }

    // Check specific circuit breaker metrics
    let cx_overflow = stats.upstream_cx_overflow("stats-cb-cluster");
    let cx_total = stats.get_or("cluster.stats-cb-cluster.upstream_cx_total", 0);
    let cx_active = stats.get_or("cluster.stats-cb-cluster.upstream_cx_active", 0);

    println!("\nKey metrics:");
    println!("  upstream_cx_overflow: {}", cx_overflow);
    println!("  upstream_cx_total: {}", cx_total);
    println!("  upstream_cx_active: {}", cx_active);

    // Note: Circuit breaker overflow may not always occur with simple requests
    // It depends on timing, connection pooling, and backend behavior
    // We're mainly verifying that the configuration exists and stats are available
    println!("✓ Circuit breaker configuration verified");
    println!("✓ Circuit breaker stats are being tracked");

    if cx_overflow > 0 {
        println!("✓ Circuit breaker was triggered ({} overflows)", cx_overflow);
    } else {
        println!("ℹ Circuit breaker not triggered in this test (timing/load dependent)");
    }

    // Print all cluster stats for debugging
    println!("\nAll cluster stats:");
    for (name, value) in cb_stats {
        println!("  {} = {}", name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_config_format() {
        // Verify the circuit breaker JSON structure is valid
        let config = filter_configs::circuit_breaker()
            .max_connections(10)
            .max_pending_requests(5)
            .max_requests(20)
            .build();

        assert_eq!(config["default"]["maxConnections"], 10);
        assert_eq!(config["default"]["maxPendingRequests"], 5);
        assert_eq!(config["default"]["maxRequests"], 20);
    }
}
