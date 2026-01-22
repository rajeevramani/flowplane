//! Custom Response Filter Tests (Bruno 19)
//!
//! Tests the custom response filter:
//! - Create filter with custom response for 5xx errors
//! - Install filter on listener
//! - Trigger 500 error and verify custom response body
//! - Route-specific override for different status codes

use serde_json::json;
use std::collections::HashMap;
use wiremock::{matchers::path, Mock, ResponseTemplate};

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
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

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
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Setup mock endpoints that return errors
    let mocks = harness.mocks();
    Mock::given(path("/error"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": "Original backend error"
        })))
        .mount(&mocks.echo)
        .await;

    Mock::given(path("/success"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "ok"
        })))
        .mount(&mocks.echo)
        .await;

    // Extract echo server endpoint
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

    // Wait for route to converge
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
    let (status, headers, body) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            &format!("{}.e2e.local", route.name),
            "/testing/custom/error",
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
    assert_eq!(body_ok_json["status"], "ok", "Successful response should be unmodified");

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
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Setup mock endpoints
    let mocks = harness.mocks();
    Mock::given(path("/error"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "error": "Service unavailable"
        })))
        .mount(&mocks.echo)
        .await;

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
    let resources = ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name)
        .with_cluster("override-cluster", host, port)
        .with_route("override-route", "/testing/override")
        .with_listener("override-listener", harness.ports.listener)
        .with_filter("base-custom-filter", "custom_response", base_filter_config)
        .build()
        .await
        .expect("Resource setup should succeed");

    let route = resources.route();
    let filter = resources.filter();

    println!("✓ Base setup complete: filter={}, route={}", filter.name, route.name);

    // Add route-specific override with different message
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
            api.add_route_filter_override(
                &ctx.admin_token,
                &route.name,
                &filter.id,
                override_config,
            )
            .await
        })
        .await
        .expect("Route override should succeed");

    println!("✓ Route override added: {:?}", override_result);

    // Wait for config to propagate
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Make request and verify override is applied
    let envoy = harness.envoy().unwrap();
    let (status, _, body) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            &format!("{}.e2e.local", route.name),
            "/testing/override/error",
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
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Setup mock that returns 500
    let mocks = harness.mocks();
    Mock::given(path("/backend-error"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": "Internal error"
        })))
        .mount(&mocks.echo)
        .await;

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

    // Wait for config
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let envoy = harness.envoy().unwrap();
    let (status, _, body) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            &format!("{}.e2e.local", route.name),
            "/testing/status/backend-error",
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
        let config = serde_json::json!({
            "type": "custom_response",
            "config": {
                "custom_response_matchers": [
                    {
                        "matcher": {
                            "status_code_matcher": {
                                "match_type": "exact",
                                "value": 500
                            }
                        },
                        "response": {
                            "body": {
                                "inline_string": "{\"error\":\"custom\"}"
                            },
                            "content_type": "application/json"
                        }
                    }
                ]
            }
        });

        assert_eq!(config["type"], "custom_response");
        assert!(config["config"]["custom_response_matchers"].is_array());
    }
}
