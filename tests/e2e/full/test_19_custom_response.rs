//! Custom Response Filter Tests (Bruno 19)
//!
//! Tests the custom response filter:
//! - Create filter with custom response for 5xx errors
//! - Install filter on listener
//! - Configure filter at route-config level
//! - Trigger 500 error and verify custom response body
//! - Route-specific override for different status codes

use std::collections::HashMap;

use crate::common::{
    api_client::{setup_dev_context, ApiClient},
    filter_configs,
    harness::{TestHarness, TestHarnessConfig},
    resource_setup::ResourceSetup,
    timeout::{with_timeout, TestTimeout},
};

/// Test creating a custom response filter
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_create_filter() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_100_create_custom_response").without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_100_create_filter").await.expect("Setup should succeed");

    // Create custom response filter for 5xx errors
    let filter_config = filter_configs::custom_response()
        .add_matcher(
            500,
            r#"{"error":"service_unavailable","message":"Service temporarily unavailable"}"#,
            "application/json",
            None,
        )
        .build();

    let filter =
        with_timeout(TestTimeout::default_with_label("Create custom response filter"), async {
            api.create_filter(
                &ctx.admin_token,
                &ctx.team_a_name,
                "custom-error-response",
                "custom_response",
                filter_config,
            )
            .await
        })
        .await
        .expect("Filter creation should succeed");

    assert_eq!(filter.name, "custom-error-response");
    assert_eq!(filter.filter_type, "custom_response");
    println!("✓ Custom response filter created: {} (id={})", filter.name, filter.id);
}

/// Test full custom response flow: filter + listener + verify custom body
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_verify_custom_response() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_101_verify_custom_response"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping custom response test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api, "test_101_verify_custom_response")
        .await
        .expect("Setup should succeed");

    // Extract echo server endpoint - use the smart mock which returns status based on path
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create custom response filter for 500 errors
    let custom_response_body =
        r#"{"error":"custom_error","message":"This is a custom error response from the proxy"}"#;

    let filter_config = filter_configs::custom_response()
        .add_matcher(500, custom_response_body, "application/json", None)
        .build();

    // Setup resources with filter
    let resources = ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name)
        .with_cluster("custom-response-cluster", host, port)
        .with_route("custom-response-route", "/testing/custom")
        .with_listener("custom-response-listener", harness.ports.listener)
        .with_filter("custom-error-filter", "custom_response", filter_config)
        .build()
        .await
        .expect("Resource setup should succeed");

    let route = resources.route();
    let filter = resources.filter();

    println!("✓ Setup complete: filter={}, route={}", filter.name, route.name);

    // Configure filter at route-config level (required for filter to be active)
    let _config_result =
        with_timeout(TestTimeout::default_with_label("Configure filter at route-config"), async {
            api.configure_filter_at_route_config(&ctx.admin_token, &filter.id, &route.name).await
        })
        .await
        .expect("Configure filter at route-config should succeed");

    println!("✓ Filter configured at route-config level");

    // Wait for route to converge (uses /success path which returns 200)
    let _ = with_timeout(TestTimeout::default_with_label("Wait for route convergence"), async {
        harness
            .wait_for_route(&format!("{}.e2e.local", route.name), "/testing/custom/success", 200)
            .await
    })
    .await
    .expect("Route should converge");

    println!("✓ Route converged successfully");

    let envoy = harness.envoy().unwrap();

    // Test 1: Trigger 500 error and verify custom response
    // Smart mock returns 500 for paths ending in /fail or /error
    let (status, headers, body) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            &format!("{}.e2e.local", route.name),
            "/testing/custom/fail",
            HashMap::new(),
            None,
        )
        .await
        .expect("Request should complete");

    println!("Response status: {}", status);
    println!("Response headers: {:?}", headers);
    println!("Response body: {}", body);

    assert_eq!(status, 500, "Status should still be 500");

    // Parse the response body
    let body_json: serde_json::Value =
        serde_json::from_str(&body).expect("Response should be valid JSON");

    // Verify it's the custom response, not the original backend error
    assert_eq!(body_json["error"], "custom_error", "Should contain custom error field");
    assert_eq!(
        body_json["message"], "This is a custom error response from the proxy",
        "Should contain custom message"
    );

    // Verify content-type header
    let content_type = headers.get("content-type");
    assert!(
        content_type.map(|s| s.contains("application/json")).unwrap_or(false),
        "Content-Type should be application/json"
    );

    println!("✓ Custom response body verified");

    // Test 2: Verify successful requests are not affected
    let (status_ok, _, body_ok) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            &format!("{}.e2e.local", route.name),
            "/testing/custom/success",
            HashMap::new(),
            None,
        )
        .await
        .expect("Successful request should complete");

    assert_eq!(status_ok, 200, "Successful requests should still work");
    let body_ok_json: serde_json::Value =
        serde_json::from_str(&body_ok).expect("Success response should be valid JSON");
    // Mock returns {"status": 200} for successful requests (not {"status": "ok"})
    assert_eq!(body_ok_json["status"], 200, "Successful response should be unmodified");

    println!("✓ Custom response filter test completed");
}

/// Test route-specific custom response override
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_102_route_override() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_102_route_override"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping route override test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_102_route_override").await.expect("Setup should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create base filter with generic error message
    let base_filter_config = filter_configs::custom_response()
        .add_matcher(
            503,
            r#"{"error":"base_error","message":"Generic service error"}"#,
            "application/json",
            None,
        )
        .build();

    // Setup resources with filter
    // Note: Use "cr-" prefix (custom response) to avoid collision with test_12's "override-route"
    let resources = ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name)
        .with_cluster("cr-override-cluster", host, port)
        .with_route("cr-override-route", "/testing/custom-override")
        .with_listener("cr-override-listener", harness.ports.listener)
        .with_filter("cr-base-custom-filter", "custom_response", base_filter_config)
        .build()
        .await
        .expect("Resource setup should succeed");

    let route = resources.route();
    let filter = resources.filter();

    println!("✓ Base setup complete: filter={}, route={}", filter.name, route.name);

    // Configure filter at route-config level first
    let _config_result =
        with_timeout(TestTimeout::default_with_label("Configure filter at route-config"), async {
            api.configure_filter_at_route_config(&ctx.admin_token, &filter.id, &route.name).await
        })
        .await
        .expect("Configure filter at route-config should succeed");

    println!("✓ Filter configured at route-config level");

    // Add route-specific override with different message
    // scope_id format: "{route-config-name}/{vhost-name}/{route-name}"
    // From resource_setup.rs: vhost = "{name}-vh", route = "{name}-route"
    let scope_id = format!("{}/{}-vh/{}-route", route.name, route.name, route.name);
    let override_config = filter_configs::custom_response()
        .add_matcher(
            503,
            r#"{"error":"route_override","message":"Route-specific error message","route":"override"}"#,
            "application/json",
            None,
        )
        .build();

    let override_result =
        with_timeout(TestTimeout::default_with_label("Add route override"), async {
            api.add_route_filter_override(&ctx.admin_token, &filter.id, &scope_id, override_config)
                .await
        })
        .await
        .expect("Route override should succeed");

    println!("✓ Route override added: {:?}", override_result);

    // Wait for xDS propagation (3 sec delay per design principles)
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Wait for route to converge
    let _ = with_timeout(TestTimeout::default_with_label("Wait for route convergence"), async {
        harness
            .wait_for_route(
                &format!("{}.e2e.local", route.name),
                "/testing/custom-override/success",
                200,
            )
            .await
    })
    .await
    .expect("Route should converge");

    // Make request and verify override is applied
    // Smart mock: path containing /503 returns 503
    let envoy = harness.envoy().unwrap();
    let (status, _, body) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            &format!("{}.e2e.local", route.name),
            "/testing/custom-override/503",
            HashMap::new(),
            None,
        )
        .await
        .expect("Request should complete");

    println!("Response status: {}", status);
    println!("Response body: {}", body);

    assert_eq!(status, 503, "Status should be 503");

    // Parse response body
    let body_json: serde_json::Value =
        serde_json::from_str(&body).expect("Response should be valid JSON");

    // Verify the route-specific override is applied
    assert_eq!(body_json["error"], "route_override", "Should contain route override error");
    assert_eq!(
        body_json["message"], "Route-specific error message",
        "Should contain route-specific message"
    );
    assert_eq!(body_json["route"], "override", "Should contain route identifier");

    println!("✓ Route override verified");
    println!("✓ Custom response route override test completed");
}

/// Test custom response with status code override
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_103_status_override() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_103_status_override"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping status override test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_103_status_override").await.expect("Setup should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create filter that changes 500 to 503
    let filter_config = filter_configs::custom_response()
        .add_matcher(
            500,
            r#"{"error":"service_maintenance","message":"Service under maintenance"}"#,
            "application/json",
            Some(503), // Override status code
        )
        .build();

    let resources = ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name)
        .with_cluster("status-override-cluster", host, port)
        .with_route("status-override-route", "/testing/status")
        .with_listener("status-override-listener", harness.ports.listener)
        .with_filter("status-override-filter", "custom_response", filter_config)
        .build()
        .await
        .expect("Resource setup should succeed");

    let route = resources.route();
    let filter = resources.filter();

    println!("✓ Setup complete: filter={}, route={}", filter.name, route.name);

    // Configure filter at route-config level
    let _config_result =
        with_timeout(TestTimeout::default_with_label("Configure filter at route-config"), async {
            api.configure_filter_at_route_config(&ctx.admin_token, &filter.id, &route.name).await
        })
        .await
        .expect("Configure filter at route-config should succeed");

    println!("✓ Filter configured at route-config level");

    // Wait for route to converge
    let _ = with_timeout(TestTimeout::default_with_label("Wait for route convergence"), async {
        harness
            .wait_for_route(&format!("{}.e2e.local", route.name), "/testing/status/success", 200)
            .await
    })
    .await
    .expect("Route should converge");

    let envoy = harness.envoy().unwrap();

    // Smart mock: path ending in /fail returns 500
    let (status, _, body) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            &format!("{}.e2e.local", route.name),
            "/testing/status/fail",
            HashMap::new(),
            None,
        )
        .await
        .expect("Request should complete");

    println!("Response status: {}", status);
    println!("Response body: {}", body);

    // Verify status code was overridden from 500 to 503
    assert_eq!(status, 503, "Status should be overridden to 503");

    let body_json: serde_json::Value =
        serde_json::from_str(&body).expect("Response should be valid JSON");
    assert_eq!(body_json["error"], "service_maintenance", "Should contain custom error");

    println!("✓ Status code override verified (500 → 503)");
    println!("✓ Custom response status override test completed");
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_custom_response_config_format() {
        // Config matches backend's CustomResponseConfig format
        let config = serde_json::json!({
            "matchers": [
                {
                    "status_code": { "type": "exact", "code": 500 },
                    "response": {
                        "body": "{\"error\":\"custom\"}",
                        "headers": { "content-type": "application/json" }
                    }
                }
            ]
        });

        assert!(config["matchers"].is_array());
        let matcher = &config["matchers"][0];
        assert_eq!(matcher["status_code"]["type"], "exact");
        assert_eq!(matcher["status_code"]["code"], 500);
    }
}
