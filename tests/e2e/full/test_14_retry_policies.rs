//! Retry Policies Tests (Bruno 14)
//!
//! Tests retry policies:
//! - Create cluster + route with retry policy + listener
//! - Trigger retries by requesting backend that returns 503
//! - Verify retry stats from Envoy (upstream_rq_retry count)

use std::collections::HashMap;

use crate::common::{
    api_client::{setup_dev_context, ApiClient},
    filter_configs,
    harness::{TestHarness, TestHarnessConfig},
    resource_setup::{ClusterConfig, ResourceSetup, RouteConfig},
    stats::{EnvoyStats, StatAssertions},
    timeout::{with_timeout, TestTimeout},
};

/// Test setup: Create cluster + route with retry policy + listener
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_setup_retry_infrastructure() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_100_setup_retry_infrastructure"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping retry infrastructure setup test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api, "test_100_setup_retry_infrastructure")
        .await
        .expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Build retry policy configuration
    let retry_policy = filter_configs::retry_policy()
        .max_retries(3)
        .retry_on(vec!["5xx", "reset", "connect-failure", "retriable-4xx"])
        .per_try_timeout_seconds(10)
        .backoff(100, 1000)
        .build();

    // Create cluster configuration
    let cluster_config = ClusterConfig::new("retry-test-cluster", host, port);

    // Create route with retry policy using prefix rewrite to /status/503
    let route_config =
        RouteConfig::new("retry-test-route", "/testing/retry-basic", "retry-test-cluster")
            .with_domain("retry.e2e.local")
            .with_retry_policy(retry_policy)
            .with_prefix_rewrite("/status/503");

    // Build all resources
    let resources =
        with_timeout(TestTimeout::default_with_label("Setup retry infrastructure"), async {
            ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
                .with_cluster_config(cluster_config)
                .with_route_config(route_config)
                .with_listener("retry-listener", harness.ports.listener)
                .build()
                .await
        })
        .await
        .expect("Resource setup should succeed");

    println!("✓ Cluster created: {}", resources.cluster().name);
    println!("✓ Route created: {} with retry policy", resources.route().name);
    println!(
        "✓ Listener created: {} on port {:?}",
        resources.listener().name,
        resources.listener().port
    );

    // Wait for route to converge
    let _ = with_timeout(TestTimeout::default_with_label("Wait for route"), async {
        harness.wait_for_route("retry.e2e.local", "/testing/retry-basic", 503).await
    })
    .await
    .expect("Route should converge");

    println!("✓ Retry infrastructure setup complete");
}

/// Test triggering retries: Request backend that returns 503
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_trigger_retries() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_101_trigger_retries"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping trigger retries test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_101_trigger_retries").await.expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Build retry policy configuration
    let retry_policy = filter_configs::retry_policy()
        .max_retries(3)
        .retry_on(vec!["5xx", "reset", "connect-failure", "retriable-4xx"])
        .per_try_timeout_seconds(10)
        .backoff(100, 1000)
        .build();

    // Create cluster + route + listener with retry policy
    let cluster_config = ClusterConfig::new("trigger-retry-cluster", host, port);
    let route_config =
        RouteConfig::new("trigger-retry-route", "/testing/retry-trigger", "trigger-retry-cluster")
            .with_domain("trigger-retry.e2e.local")
            .with_retry_policy(retry_policy)
            .with_prefix_rewrite("/status/503");

    let _resources =
        ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
            .with_cluster_config(cluster_config)
            .with_route_config(route_config)
            .with_listener("trigger-retry-listener", harness.ports.listener)
            .build()
            .await
            .expect("Resource setup should succeed");

    // Wait for route to converge (xDS propagation)
    let _ = with_timeout(TestTimeout::default_with_label("Wait for retry route"), async {
        harness.wait_for_route("trigger-retry.e2e.local", "/testing/retry-trigger", 503).await
    })
    .await
    .expect("Route should converge");

    // Make request that will trigger retries (echo server /status/503 returns 503)
    let envoy = harness.envoy().unwrap();
    let (status, _headers, body) =
        with_timeout(TestTimeout::default_with_label("Request that triggers retries"), async {
            envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::GET,
                    "trigger-retry.e2e.local",
                    "/testing/retry-trigger",
                    HashMap::new(),
                    None,
                )
                .await
        })
        .await
        .expect("Request should complete (even if it fails with 503)");

    // Echo server should return 503 after retries exhausted
    assert_eq!(status, 503, "Expected 503 from backend after retries");
    println!("✓ Request triggered retries, final status: {}", status);
    println!("✓ Response body: {}", body);
}

/// Test verify retry stats: Check upstream_rq_retry count from Envoy
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_102_verify_retry_stats() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_102_verify_retry_stats"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping retry stats test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_102_verify_retry_stats").await.expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Build retry policy configuration
    let retry_policy = filter_configs::retry_policy()
        .max_retries(3)
        .retry_on(vec!["5xx", "reset", "connect-failure", "retriable-4xx"])
        .per_try_timeout_seconds(10)
        .backoff(100, 1000)
        .build();

    // Create cluster + route + listener with retry policy
    let cluster_config = ClusterConfig::new("stats-retry-cluster", host, port);
    let route_config =
        RouteConfig::new("stats-retry-route", "/testing/retry-stats", "stats-retry-cluster")
            .with_domain("stats-retry.e2e.local")
            .with_retry_policy(retry_policy)
            .with_prefix_rewrite("/status/503");

    let _resources =
        ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
            .with_cluster_config(cluster_config)
            .with_route_config(route_config)
            .with_listener("stats-retry-listener", harness.ports.listener)
            .build()
            .await
            .expect("Resource setup should succeed");

    // Wait for route to converge (xDS propagation)
    let _ = with_timeout(TestTimeout::default_with_label("Wait for retry route"), async {
        harness.wait_for_route("stats-retry.e2e.local", "/testing/retry-stats", 503).await
    })
    .await
    .expect("Route should converge");

    // Make request that will trigger retries
    let envoy = harness.envoy().unwrap();
    let (status, _, _) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "stats-retry.e2e.local",
            "/testing/retry-stats",
            HashMap::new(),
            None,
        )
        .await
        .expect("Request should complete");

    assert_eq!(status, 503, "Expected 503 from backend");
    println!("✓ Request completed with status {}", status);

    // Wait a moment for stats to be updated
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Get Envoy stats and verify retry count
    let stats_json = with_timeout(TestTimeout::default_with_label("Get Envoy stats"), async {
        envoy.get_stats_json().await
    })
    .await
    .expect("Stats should be available");

    let stats = EnvoyStats::parse_json(&stats_json);

    // Verify retry count
    let retry_count = stats.upstream_rq_retry("stats-retry-cluster");
    println!("Retry stats for stats-retry-cluster:");
    println!("  upstream_rq_retry: {}", retry_count);
    println!("  upstream_rq_total: {}", stats.upstream_rq_total("stats-retry-cluster"));

    // Assert that retries occurred (should be >= 1, typically 3)
    stats.assert_retries("stats-retry-cluster", 1);

    println!("✓ Retry policy verified - {} retries completed", retry_count);

    // Print all retry-related stats for debugging
    let retry_stats = stats.find_matching("retry");
    if !retry_stats.is_empty() {
        println!("\nAll retry-related stats:");
        for (name, value) in retry_stats {
            println!("  {} = {}", name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy_config_format() {
        // Verify the retry policy JSON structure is valid
        let config = filter_configs::retry_policy()
            .max_retries(3)
            .retry_on(vec!["5xx"])
            .per_try_timeout_seconds(5)
            .backoff(100, 1000)
            .build();

        assert_eq!(config["maxRetries"], 3);
        assert_eq!(config["perTryTimeoutSeconds"], 5);
        assert!(config["backoff"].is_object());
        assert!(config["retryOn"].is_array());
    }
}
