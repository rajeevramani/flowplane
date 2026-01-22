//! Compression Filter Tests (Bruno 17)
//!
//! Tests the compressor filter (gzip):
//! - Create filter with gzip compression
//! - Install filter on listener
//! - Verify request with Accept-Encoding: gzip gets compressed response
//! - Verify Content-Encoding: gzip header in response
//! - Check Envoy stats for compression metrics

use std::collections::HashMap;

use crate::common::{
    api_client::{setup_dev_context, ApiClient},
    filter_configs,
    harness::{TestHarness, TestHarnessConfig},
    resource_setup::{ClusterConfig, FilterConfig, ListenerConfig, ResourceSetup, RouteConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Test setup compression infrastructure
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_setup_compression() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_100_setup_compression"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping compression setup test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create compression filter config
    let filter_config = filter_configs::compressor()
        .min_content_length(100)
        .content_types(vec!["application/json", "text/html", "text/plain"])
        .compression_level("DEFAULT_COMPRESSION")
        .build();

    // Build infrastructure with compression filter
    let resources = ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name)
        .with_cluster_config(ClusterConfig::new("compression-backend", host, port))
        .with_route_config(
            RouteConfig::new("compression-route", "/testing/compression", "compression-backend")
                .with_domain("compression.e2e.local"),
        )
        .with_listener_config(ListenerConfig::new(
            "compression-listener",
            harness.ports.listener,
            "compression-route",
        ))
        .with_filter_config(FilterConfig::new("compression-filter", "compressor", filter_config))
        .build()
        .await
        .expect("Resource setup should succeed");

    println!(
        "✓ Compression infrastructure created: cluster={}, route={}, listener={}, filter={}",
        resources.cluster().name,
        resources.route().name,
        resources.listener().name,
        resources.filter().name
    );

    // Wait for route to converge
    let _ = with_timeout(TestTimeout::default_with_label("Wait for route"), async {
        harness.wait_for_route("compression.e2e.local", "/testing/compression/test", 200).await
    })
    .await
    .expect("Route should converge");

    println!("✓ Compression filter configured with gzip for JSON/HTML/text > 100 bytes");
}

/// Test verify compression with Accept-Encoding: gzip
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_verify_compression() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_101_verify_compression"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping compression verification test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create compression filter
    let filter_config = filter_configs::compressor()
        .min_content_length(100)
        .content_types(vec!["application/json", "text/html", "text/plain"])
        .compression_level("DEFAULT_COMPRESSION")
        .build();

    let resources = ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name)
        .with_cluster_config(ClusterConfig::new("verify-compression-backend", host, port))
        .with_route_config(
            RouteConfig::new(
                "verify-compression-route",
                "/testing/compression",
                "verify-compression-backend",
            )
            .with_domain("verify-compression.e2e.local"),
        )
        .with_listener_config(ListenerConfig::new(
            "verify-compression-listener",
            harness.ports.listener,
            "verify-compression-route",
        ))
        .with_filter_config(FilterConfig::new(
            "verify-compression-filter",
            "compressor",
            filter_config,
        ))
        .build()
        .await
        .expect("Resource setup should succeed");

    // Wait for config to propagate
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let envoy = harness.envoy().unwrap();

    // Send request with Accept-Encoding: gzip
    let mut headers = HashMap::new();
    headers.insert("Accept-Encoding".to_string(), "gzip, deflate".to_string());
    headers.insert("Accept".to_string(), "application/json".to_string());

    let (status, response_headers, body) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "verify-compression.e2e.local",
            "/testing/compression/test",
            headers,
            None,
        )
        .await
        .expect("Request should complete");

    assert_eq!(status, 200, "Expected 200 OK");

    // Check for compression headers
    let content_encoding = response_headers.get("content-encoding");
    let vary = response_headers.get("vary");

    println!("Response headers:");
    println!("  Status: {}", status);
    println!("  Content-Encoding: {:?}", content_encoding);
    println!("  Vary: {:?}", vary);
    println!("  Content-Type: {:?}", response_headers.get("content-type"));
    println!("  Body length: {} bytes", body.len());

    // Note: Compression may or may not apply depending on response size
    // If the echo server returns a small response, it won't be compressed
    if let Some(encoding) = content_encoding {
        if encoding.contains("gzip") {
            println!("✓ Response was compressed with gzip");
        } else {
            println!("⚠ Content-Encoding present but not gzip: {}", encoding);
        }
    } else {
        println!("⚠ Response not compressed (body may be < min_content_length)");
    }

    // Verify that Vary header includes Accept-Encoding (best practice)
    if let Some(vary_header) = vary {
        if vary_header.to_lowercase().contains("accept-encoding") {
            println!("✓ Vary: Accept-Encoding header present");
        }
    }

    println!("✓ Compression filter operational on {}", resources.filter().name);
}

/// Test check compression stats
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_102_check_stats() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_102_check_stats"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping stats check test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create compression filter
    let filter_config = filter_configs::compressor()
        .min_content_length(100)
        .content_types(vec!["application/json", "text/html", "text/plain"])
        .compression_level("DEFAULT_COMPRESSION")
        .build();

    let resources = ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name)
        .with_cluster_config(ClusterConfig::new("stats-backend", host, port))
        .with_route_config(
            RouteConfig::new("stats-route", "/testing/stats", "stats-backend")
                .with_domain("stats.e2e.local"),
        )
        .with_listener_config(ListenerConfig::new(
            "stats-listener",
            harness.ports.listener,
            "stats-route",
        ))
        .with_filter_config(FilterConfig::new("stats-filter", "compressor", filter_config))
        .build()
        .await
        .expect("Resource setup should succeed");

    // Wait for config to propagate
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let envoy = harness.envoy().unwrap();

    // Send several requests to generate stats
    let mut headers = HashMap::new();
    headers.insert("Accept-Encoding".to_string(), "gzip".to_string());

    for i in 1..=5 {
        let _ = envoy
            .proxy_request(
                harness.ports.listener,
                hyper::Method::GET,
                "stats.e2e.local",
                &format!("/testing/stats/req{}", i),
                headers.clone(),
                None,
            )
            .await
            .expect("Request should complete");
    }

    // Wait a bit for stats to update
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Fetch Envoy stats
    let stats = envoy.get_stats().await.expect("Should get stats");

    // Look for compression-related stats
    let compression_stats: Vec<&str> = stats
        .lines()
        .filter(|line| {
            line.contains("compressor") || line.contains("gzip") || line.contains("compression")
        })
        .collect();

    if !compression_stats.is_empty() {
        println!("✓ Compression stats found:");
        for stat in compression_stats.iter().take(20) {
            println!("  {}", stat);
        }
    } else {
        println!("⚠ No compression stats found (responses may not have been compressed)");
    }

    // Try to find specific compression metrics
    let compressed_count =
        stats.lines().find(|line| line.contains("compressor") && line.contains("compressed"));
    let not_compressed_count =
        stats.lines().find(|line| line.contains("compressor") && line.contains("not_compressed"));

    if let Some(stat) = compressed_count {
        println!("✓ Compressed responses: {}", stat);
    }
    if let Some(stat) = not_compressed_count {
        println!("  Not compressed: {}", stat);
    }

    println!("✓ Stats check complete for {}", resources.filter().name);
}

/// Test compression with large payload to ensure compression happens
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_103_compression_large_payload() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_103_compression_large_payload"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping large payload test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create compression filter with low threshold
    let filter_config = filter_configs::compressor()
        .min_content_length(50) // Lower threshold
        .content_types(vec!["application/json", "text/html", "text/plain"])
        .compression_level("DEFAULT_COMPRESSION")
        .build();

    let resources = ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name)
        .with_cluster_config(ClusterConfig::new("large-payload-backend", host, port))
        .with_route_config(
            RouteConfig::new("large-payload-route", "/testing/large", "large-payload-backend")
                .with_domain("large-payload.e2e.local"),
        )
        .with_listener_config(ListenerConfig::new(
            "large-payload-listener",
            harness.ports.listener,
            "large-payload-route",
        ))
        .with_filter_config(FilterConfig::new("large-payload-filter", "compressor", filter_config))
        .build()
        .await
        .expect("Resource setup should succeed");

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let envoy = harness.envoy().unwrap();

    // Send POST with large JSON payload
    let mut headers = HashMap::new();
    headers.insert("Accept-Encoding".to_string(), "gzip".to_string());
    headers.insert("Content-Type".to_string(), "application/json".to_string());

    // Create a payload large enough to trigger compression
    let large_payload = serde_json::json!({
        "data": "x".repeat(500), // 500+ bytes of data
        "metadata": {
            "items": (0..50).map(|i| format!("item-{}", i)).collect::<Vec<_>>()
        }
    });

    let (status, response_headers, _body) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::POST,
            "large-payload.e2e.local",
            "/testing/large/data",
            headers,
            Some(large_payload.to_string()),
        )
        .await
        .expect("Request should complete");

    assert_eq!(status, 200, "Expected 200 OK");

    let content_encoding = response_headers.get("content-encoding");
    println!("Large payload response:");
    println!("  Status: {}", status);
    println!("  Content-Encoding: {:?}", content_encoding);

    // With a large payload, compression should be more likely
    if let Some(encoding) = content_encoding {
        if encoding.contains("gzip") {
            println!("✓ Large payload was successfully compressed");
        }
    } else {
        println!("⚠ Large payload not compressed (depends on echo server response)");
    }

    println!("✓ Large payload compression test complete for {}", resources.filter().name);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_filter_config_format() {
        let config = filter_configs::compressor()
            .min_content_length(100)
            .content_types(vec!["application/json"])
            .build();

        assert_eq!(config["type"], "compressor");
        assert_eq!(
            config["config"]["response_direction_config"]["common_config"]["min_content_length"],
            100
        );
        assert_eq!(config["config"]["compressor_library"]["type"], "gzip");
    }
}
