//! CORS Filter Tests (Bruno 21)
//!
//! Tests the CORS filter:
//! - Create CORS filter with allowed origins
//! - Install filter on listener
//! - Verify OPTIONS preflight requests with allowed origin return CORS headers
//! - Verify GET requests with allowed origin receive CORS headers
//! - Verify requests with blocked origin don't receive CORS headers

use std::collections::HashMap;

use crate::common::{
    api_client::{setup_dev_context, simple_cluster, simple_listener, simple_route, ApiClient},
    filter_configs,
    harness::{TestHarness, TestHarnessConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Test setup: Create CORS filter infrastructure
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_setup_cors() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_100_setup_cors"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping CORS setup test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Get echo server endpoint for backend
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create backend cluster
    let cluster = with_timeout(TestTimeout::default_with_label("Create cluster"), async {
        api.create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "cors-backend", host, port),
        )
        .await
    })
    .await
    .expect("Backend cluster creation should succeed");

    println!("✓ Backend cluster created: {}", cluster.name);

    // Create route
    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "cors-route",
                "cors.e2e.local",
                "/testing/cors",
                &cluster.name,
            ),
        )
        .await
        .expect("Route creation should succeed");

    println!("✓ Route created: {}", route.name);

    // Create listener
    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "cors-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    println!("✓ Listener created: {} on port {:?}", listener.name, listener.port);

    // Create CORS filter using builder with exact and prefix origin matching
    let filter_config = filter_configs::cors()
        .allow_origin_exact("https://example.com")
        .allow_origin_prefix("https://app.")
        .allow_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"])
        .allow_headers(vec!["authorization", "content-type", "x-request-id"])
        .expose_headers(vec!["x-custom-header"])
        .max_age(86400)
        .allow_credentials(true)
        .build();

    let filter = with_timeout(TestTimeout::default_with_label("Create CORS filter"), async {
        api.create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "cors-test-filter",
            "cors",
            filter_config,
        )
        .await
    })
    .await
    .expect("Filter creation should succeed");

    assert_eq!(filter.name, "cors-test-filter");
    assert_eq!(filter.filter_type, "cors");
    println!("✓ CORS filter created: {} (id={})", filter.name, filter.id);

    // Install filter on listener
    let installation = with_timeout(TestTimeout::default_with_label("Install filter"), async {
        api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(100)).await
    })
    .await
    .expect("Filter installation should succeed");

    println!(
        "✓ Filter installed: filter_id={} on listener={}",
        installation.filter_id, installation.listener_name
    );

    // Wait for configuration to propagate
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    println!("✓ CORS setup complete");
}

/// Test CORS preflight request (OPTIONS) with allowed origin
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_preflight_allowed() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_101_preflight_allowed"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping CORS preflight test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Setup infrastructure
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "preflight-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "preflight-route",
                "preflight.e2e.local",
                "/testing/cors",
                &cluster.name,
            ),
        )
        .await
        .expect("Route creation should succeed");

    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "preflight-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    println!("✓ Listener created: {} on port {:?}", listener.name, listener.port);

    // Create and install CORS filter
    let filter_config = filter_configs::cors()
        .allow_origin_exact("https://example.com")
        .allow_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"])
        .allow_headers(vec!["authorization", "content-type", "x-request-id"])
        .expose_headers(vec!["x-custom-header"])
        .max_age(86400)
        .allow_credentials(true)
        .build();

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "preflight-cors-filter",
            "cors",
            filter_config,
        )
        .await
        .expect("Filter creation should succeed");

    api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(100))
        .await
        .expect("Filter installation should succeed");

    println!("✓ Filter installed on listener: {}", listener.name);

    // Wait for route to converge
    let _ = with_timeout(TestTimeout::default_with_label("Wait for route"), async {
        harness.wait_for_route("preflight.e2e.local", "/testing/cors", 200).await
    })
    .await;

    // Make OPTIONS preflight request with allowed origin
    let envoy = harness.envoy().unwrap();
    let mut headers = HashMap::new();
    headers.insert("Origin".to_string(), "https://example.com".to_string());
    headers.insert("Access-Control-Request-Method".to_string(), "POST".to_string());
    headers.insert(
        "Access-Control-Request-Headers".to_string(),
        "authorization,content-type".to_string(),
    );

    let (status, response_headers, body) =
        with_timeout(TestTimeout::default_with_label("CORS preflight request"), async {
            envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::OPTIONS,
                    "preflight.e2e.local",
                    "/testing/cors",
                    headers,
                    None,
                )
                .await
        })
        .await
        .expect("Preflight request should succeed");

    println!("CORS Preflight Test Results:");
    println!("  Status: {}", status);
    println!("  Response headers: {:?}", response_headers);
    println!("  Body: {}", body);

    // Preflight can return 200 or 204
    assert!(status == 200 || status == 204, "Expected 200 or 204 for preflight, got: {}", status);

    // Verify CORS headers
    let allow_origin = response_headers.get("access-control-allow-origin");
    assert_eq!(
        allow_origin.map(|s| s.as_str()),
        Some("https://example.com"),
        "Expected Access-Control-Allow-Origin header"
    );
    println!("✓ Access-Control-Allow-Origin: {:?}", allow_origin);

    let allow_credentials = response_headers.get("access-control-allow-credentials");
    assert_eq!(
        allow_credentials.map(|s| s.as_str()),
        Some("true"),
        "Expected Access-Control-Allow-Credentials header"
    );
    println!("✓ Access-Control-Allow-Credentials: {:?}", allow_credentials);

    // Verify methods and headers are allowed
    if let Some(methods) = response_headers.get("access-control-allow-methods") {
        println!("✓ Access-Control-Allow-Methods: {}", methods);
    }

    if let Some(headers) = response_headers.get("access-control-allow-headers") {
        println!("✓ Access-Control-Allow-Headers: {}", headers);
    }

    if let Some(max_age) = response_headers.get("access-control-max-age") {
        println!("✓ Access-Control-Max-Age: {}", max_age);
    }

    println!("✓ CORS preflight ALLOWED test passed");
}

/// Test CORS on actual request (GET) with allowed origin
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_102_request_with_origin() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_102_request_with_origin"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping CORS request test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Setup infrastructure
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "request-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "request-route",
                "request.e2e.local",
                "/testing/cors",
                &cluster.name,
            ),
        )
        .await
        .expect("Route creation should succeed");

    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "request-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    // Create and install CORS filter
    let filter_config = filter_configs::cors()
        .allow_origin_exact("https://example.com")
        .allow_methods(vec!["GET", "POST"])
        .allow_headers(vec!["authorization", "content-type"])
        .expose_headers(vec!["x-custom-header"])
        .allow_credentials(true)
        .build();

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "request-cors-filter",
            "cors",
            filter_config,
        )
        .await
        .expect("Filter creation should succeed");

    api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(100))
        .await
        .expect("Filter installation should succeed");

    println!("✓ Filter installed on listener: {}", listener.name);

    // Wait for configuration to propagate
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Make GET request with Origin header
    let envoy = harness.envoy().unwrap();
    let mut headers = HashMap::new();
    headers.insert("Origin".to_string(), "https://example.com".to_string());

    let (status, response_headers, body) =
        with_timeout(TestTimeout::default_with_label("CORS GET request"), async {
            envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::GET,
                    "request.e2e.local",
                    "/testing/cors",
                    headers,
                    None,
                )
                .await
        })
        .await
        .expect("Request should succeed");

    println!("CORS Simple Request Test:");
    println!("  Status: {}", status);
    println!("  Response headers: {:?}", response_headers);
    println!("  Body: {}", body);

    assert_eq!(status, 200, "Expected 200 OK for simple request, got: {}", status);

    // Verify CORS headers in response
    let allow_origin = response_headers.get("access-control-allow-origin");
    assert_eq!(
        allow_origin.map(|s| s.as_str()),
        Some("https://example.com"),
        "Expected Access-Control-Allow-Origin header"
    );
    println!("✓ Access-Control-Allow-Origin: {:?}", allow_origin);

    if let Some(expose_headers) = response_headers.get("access-control-expose-headers") {
        println!("✓ Access-Control-Expose-Headers: {}", expose_headers);
    }

    if let Some(allow_credentials) = response_headers.get("access-control-allow-credentials") {
        println!("✓ Access-Control-Allow-Credentials: {}", allow_credentials);
    }

    println!("✓ CORS request with origin test passed");
}

/// Test CORS blocks requests from disallowed origin
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_103_blocked_origin() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_103_blocked_origin"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping CORS blocked origin test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Setup infrastructure
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "blocked-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "blocked-route",
                "blocked.e2e.local",
                "/testing/cors",
                &cluster.name,
            ),
        )
        .await
        .expect("Route creation should succeed");

    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "blocked-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    // Create and install CORS filter - only allows https://example.com
    let filter_config = filter_configs::cors()
        .allow_origin_exact("https://example.com")
        .allow_methods(vec!["GET", "POST"])
        .allow_headers(vec!["authorization", "content-type"])
        .allow_credentials(true)
        .build();

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "blocked-cors-filter",
            "cors",
            filter_config,
        )
        .await
        .expect("Filter creation should succeed");

    api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(100))
        .await
        .expect("Filter installation should succeed");

    println!("✓ Filter installed on listener: {}", listener.name);

    // Wait for configuration to propagate
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Make OPTIONS request from blocked origin (https://evil.com)
    let envoy = harness.envoy().unwrap();
    let mut headers = HashMap::new();
    headers.insert("Origin".to_string(), "https://evil.com".to_string());
    headers.insert("Access-Control-Request-Method".to_string(), "POST".to_string());

    let (status, response_headers, _body) =
        with_timeout(TestTimeout::default_with_label("CORS blocked origin request"), async {
            envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::OPTIONS,
                    "blocked.e2e.local",
                    "/testing/cors",
                    headers,
                    None,
                )
                .await
        })
        .await
        .expect("Request should complete");

    println!("CORS Blocked Origin Test:");
    println!("  Status: {}", status);
    println!("  Response headers: {:?}", response_headers);

    // Verify that Access-Control-Allow-Origin is NOT present for blocked origin
    let allow_origin = response_headers.get("access-control-allow-origin");
    assert!(
        allow_origin.is_none(),
        "Expected NO Access-Control-Allow-Origin header for blocked origin, but got: {:?}",
        allow_origin
    );

    println!("✓ CORS blocked origin test passed - no CORS headers for disallowed origin");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cors_filter_config_format() {
        let config = filter_configs::cors()
            .allow_origin_exact("https://example.com")
            .allow_methods(vec!["GET", "POST"])
            .allow_credentials(true)
            .build();

        assert_eq!(config["type"], "cors");
        assert!(config["config"]["policy"]["allow_origin"].is_array());
        assert!(config["config"]["policy"]["allow_credentials"].as_bool().unwrap());
    }

    #[test]
    fn test_cors_multiple_origins() {
        let config = filter_configs::cors()
            .allow_origin_exact("https://example.com")
            .allow_origin_prefix("https://app.")
            .allow_origin_regex(r"^https://.*\.example\.com$")
            .build();

        let origins = config["config"]["policy"]["allow_origin"].as_array().unwrap();
        assert_eq!(origins.len(), 3);
        assert_eq!(origins[0]["type"], "exact");
        assert_eq!(origins[1]["type"], "prefix");
        assert_eq!(origins[2]["type"], "regex");
    }

    #[test]
    fn test_cors_with_max_age() {
        let config = filter_configs::cors()
            .allow_origin_exact("https://example.com")
            .allow_methods(vec!["GET"])
            .max_age(3600)
            .build();

        assert_eq!(config["config"]["policy"]["max_age"], 3600);
    }
}
