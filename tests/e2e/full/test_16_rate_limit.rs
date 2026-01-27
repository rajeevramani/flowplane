//! Local Rate Limit Filter Tests (Bruno 16)
//!
//! Tests the local_rate_limit filter:
//! - Create filter with base token limit (10/min)
//! - Install filter on listener
//! - Verify first N requests pass
//! - Configure route-specific override (5/min)
//! - Verify 6th request returns 429 Too Many Requests
//! - Verify base limit still applies to other routes

use serde_json::json;
use std::collections::HashMap;

use crate::common::{
    api_client::{setup_dev_context, ApiClient},
    filter_configs,
    harness::{TestHarness, TestHarnessConfig},
    resource_setup::{ClusterConfig, FilterConfig, ListenerConfig, ResourceSetup, RouteConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Test setup rate limit infrastructure
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_setup_rate_limit() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_100_setup_rate_limit"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping rate limit setup test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_100_setup_rate_limit").await.expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create rate limit filter config with 5 tokens/min for easy testing
    let filter_config =
        filter_configs::rate_limit().max_tokens(5).fill_interval_ms(60000).status_code(429).build();

    // Build infrastructure with rate limit filter
    let resources =
        ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
            .with_cluster_config(ClusterConfig::new("rl-setup-backend", host, port))
            .with_route_config(
                RouteConfig::new("rl-setup-route", "/testing/rl/setup", "rl-setup-backend")
                    .with_domain("rl-setup.e2e.local"),
            )
            .with_listener_config(ListenerConfig::new(
                "rl-setup-listener",
                harness.ports.listener,
                "rl-setup-route",
            ))
            .with_filter_config(FilterConfig::new(
                "rl-setup-filter",
                "local_rate_limit",
                filter_config,
            ))
            .build()
            .await
            .expect("Resource setup should succeed");

    println!(
        "✓ Rate limit infrastructure created: cluster={}, route={}, listener={}, filter={}",
        resources.cluster().name,
        resources.route().name,
        resources.listener().name,
        resources.filter().name
    );

    // Wait for xDS propagation (3 sec delay per design principles)
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    println!("✓ Rate limit filter configured with 5 tokens/min");
}

/// Test verify base limit allows N requests
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_verify_base_limit() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_101_verify_base_limit"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping rate limit verification test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_101_verify_base_limit").await.expect("Setup should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create rate limit filter with 6 tokens (5 for test + 1 for wait_for_route health check)
    let filter_config =
        filter_configs::rate_limit().max_tokens(6).fill_interval_ms(60000).status_code(429).build();

    let resources =
        ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
            .with_cluster_config(ClusterConfig::new("rl-verify-backend", host, port))
            .with_route_config(
                RouteConfig::new("rl-verify-route", "/testing/rl/verify", "rl-verify-backend")
                    .with_domain("rl-verify.e2e.local"),
            )
            .with_listener_config(ListenerConfig::new(
                "rl-verify-listener",
                harness.ports.listener,
                "rl-verify-route",
            ))
            .with_filter_config(FilterConfig::new(
                "rl-verify-filter",
                "local_rate_limit",
                filter_config,
            ))
            .build()
            .await
            .expect("Resource setup should succeed");

    // Wait for route to converge (3 sec delay per design principles)
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Wait for route to be ready in Envoy
    let _ = with_timeout(TestTimeout::default_with_label("Wait for route"), async {
        harness.wait_for_route("rl-verify.e2e.local", "/testing/rl/verify/health", 200).await
    })
    .await
    .expect("Route should converge");

    let envoy = harness.envoy().unwrap();

    // Send 5 requests - all should succeed
    for i in 1..=5 {
        let (status, _, _) = envoy
            .proxy_request(
                harness.ports.listener,
                hyper::Method::GET,
                "rl-verify.e2e.local",
                &format!("/testing/rl/verify/req{}", i),
                HashMap::new(),
                None,
            )
            .await
            .expect("Request should complete");

        assert_eq!(status, 200, "Request {} should succeed with 200 OK", i);
        println!("✓ Request {}/5 passed (200 OK)", i);
    }

    // 6th request should be rate limited
    let (status, _, _) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "rl-verify.e2e.local",
            "/testing/rl/verify/req6",
            HashMap::new(),
            None,
        )
        .await
        .expect("Request should complete");

    assert_eq!(status, 429, "6th request should be rate limited with 429");
    println!("✓ Request 6/5 rate limited (429 Too Many Requests)");

    println!("✓ Rate limit verification complete: {} has 6 tokens/min", resources.filter().name);
}

/// Test configure route-specific token limit override
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_102_configure_route_override() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_102_configure_route_override"))
        .await
        .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api, "test_102_configure_route_override")
        .await
        .expect("Setup should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create base filter with 10 tokens
    let base_filter_config = filter_configs::rate_limit()
        .max_tokens(10)
        .fill_interval_ms(60000)
        .status_code(429)
        .build();

    let resources =
        ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
            .with_cluster_config(ClusterConfig::new("rl-override-backend", host, port))
            .with_route_config(
                RouteConfig::new(
                    "rl-override-route",
                    "/testing/rl/override",
                    "rl-override-backend",
                )
                .with_domain("rl-override.e2e.local"),
            )
            .with_listener_config(ListenerConfig::new(
                "rl-override-listener",
                harness.ports.listener,
                "rl-override-route",
            ))
            .with_filter_config(FilterConfig::new(
                "rl-override-filter",
                "local_rate_limit",
                base_filter_config,
            ))
            .build()
            .await
            .expect("Resource setup should succeed");

    // Wait for resources to propagate (3 sec delay per design principles)
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    println!("✓ Base filter created: {} with 10 tokens/min", resources.filter().name);

    // Configure route-specific override with 3 tokens
    // scope_id format: {route-config-name}/{vhost-name}/{route-name}
    // ResourceSetup creates: vhost = "{name}-vh", route = "{name}-route"
    let route_config_name = &resources.route().name;
    let scope_id =
        format!("{}/{}-vh/{}-route", route_config_name, route_config_name, route_config_name);

    let override_config = json!({
        "stat_prefix": "low_limit_override",
        "token_bucket": {
            "max_tokens": 3,
            "tokens_per_fill": 3,
            "fill_interval_ms": 60000
        },
        "status_code": 429,
        "filter_enabled": {
            "numerator": 100,
            "denominator": "hundred"
        },
        "filter_enforced": {
            "numerator": 100,
            "denominator": "hundred"
        }
    });

    let override_result =
        with_timeout(TestTimeout::default_with_label("Configure route override"), async {
            api.add_route_filter_override(
                &ctx.admin_token,
                &resources.filter().id,
                &scope_id,
                override_config,
            )
            .await
        })
        .await
        .expect("Route override should succeed");

    println!("✓ Route override configured: {:?}", override_result);
    println!("  Base: 10 tokens/min (inherited by all routes)");
    println!("  Override: 3 tokens/min for /testing/rl/override");
}

/// Test verify override applies - 4th request should fail
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_103_verify_override() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_103_verify_override"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping override verification test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_103_verify_override").await.expect("Setup should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create base filter with 10 tokens
    let base_filter_config = filter_configs::rate_limit()
        .max_tokens(10)
        .fill_interval_ms(60000)
        .status_code(429)
        .build();

    let resources =
        ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
            .with_cluster_config(ClusterConfig::new("rl-verify-ovr-backend", host, port))
            .with_route_config(
                RouteConfig::new(
                    "rl-verify-ovr-route",
                    "/testing/rl/verify-override",
                    "rl-verify-ovr-backend",
                )
                .with_domain("rl-verify-ovr.e2e.local"),
            )
            .with_listener_config(ListenerConfig::new(
                "rl-verify-ovr-listener",
                harness.ports.listener,
                "rl-verify-ovr-route",
            ))
            .with_filter_config(FilterConfig::new(
                "rl-verify-ovr-filter",
                "local_rate_limit",
                base_filter_config,
            ))
            .build()
            .await
            .expect("Resource setup should succeed");

    // Wait for resources to propagate (3 sec delay per design principles)
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Configure route override with 3 tokens
    // scope_id format: {route-config-name}/{vhost-name}/{route-name}
    // ResourceSetup creates: vhost = "{name}-vh", route = "{name}-route"
    let route_config_name = &resources.route().name;
    let scope_id =
        format!("{}/{}-vh/{}-route", route_config_name, route_config_name, route_config_name);

    let override_config = json!({
        "stat_prefix": "verify_override",
        "token_bucket": {
            "max_tokens": 3,
            "tokens_per_fill": 3,
            "fill_interval_ms": 60000
        },
        "status_code": 429,
        "filter_enabled": {
            "numerator": 100,
            "denominator": "hundred"
        },
        "filter_enforced": {
            "numerator": 100,
            "denominator": "hundred"
        }
    });

    api.add_route_filter_override(
        &ctx.admin_token,
        &resources.filter().id,
        &scope_id,
        override_config,
    )
    .await
    .expect("Route override should succeed");

    // Wait for config to propagate (3 sec delay per design principles)
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let envoy = harness.envoy().unwrap();

    // Send 3 requests - all should succeed
    for i in 1..=3 {
        let (status, _, _) = envoy
            .proxy_request(
                harness.ports.listener,
                hyper::Method::GET,
                "rl-verify-ovr.e2e.local",
                &format!("/testing/rl/verify-override/req{}", i),
                HashMap::new(),
                None,
            )
            .await
            .expect("Request should complete");

        assert_eq!(status, 200, "Request {} should succeed (override limit is 3)", i);
        println!("✓ Request {}/3 passed (200 OK)", i);
    }

    // 4th request should be rate limited due to override
    let (status, _, _) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "rl-verify-ovr.e2e.local",
            "/testing/rl/verify-override/req4",
            HashMap::new(),
            None,
        )
        .await
        .expect("Request should complete");

    assert_eq!(status, 429, "4th request should be rate limited (override is 3 tokens/min)");
    println!("✓ Request 4/3 rate limited (429 Too Many Requests)");

    println!("✓ Route override verification complete: base=10, override=3 tokens/min");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_filter_config_format() {
        // build() returns inner config only (without type wrapper)
        // The API client adds the wrapper when creating filters
        let config = filter_configs::rate_limit()
            .max_tokens(5)
            .fill_interval_ms(60000)
            .status_code(429)
            .build();

        assert_eq!(config["token_bucket"]["max_tokens"], 5);
        assert_eq!(config["token_bucket"]["fill_interval_ms"], 60000);
        assert_eq!(config["status_code"], 429);
    }
}
