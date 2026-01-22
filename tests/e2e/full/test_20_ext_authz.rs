//! External Authorization Filter Tests (Bruno 20)
//!
//! Tests the ext_authz filter:
//! - Create authz cluster pointing to mock service
//! - Create ext_authz filter configuration
//! - Install filter on listener
//! - Verify requests with valid auth header are allowed (200)
//! - Verify requests without auth header are denied (403)

use std::collections::HashMap;

use crate::common::{
    api_client::{setup_dev_context, simple_cluster, simple_listener, simple_route, ApiClient},
    filter_configs,
    harness::{TestHarness, TestHarnessConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Test setup: Create authz infrastructure with ext_authz filter
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_setup_ext_authz() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_100_setup_ext_authz"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping ext_authz setup test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Get ext_authz mock server endpoint
    let authz_endpoint =
        harness.mocks().ext_authz_endpoint().expect("ext_authz mock should be running");
    let parts: Vec<&str> = authz_endpoint.split(':').collect();
    let (authz_host, authz_port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create authz cluster
    let authz_cluster =
        with_timeout(TestTimeout::default_with_label("Create authz cluster"), async {
            api.create_cluster(
                &ctx.admin_token,
                &simple_cluster(&ctx.team_a_name, "authz-cluster", authz_host, authz_port),
            )
            .await
        })
        .await
        .expect("Authz cluster creation should succeed");

    println!("✓ Authz cluster created: {}", authz_cluster.name);

    // Get echo server endpoint for backend
    let echo_endpoint = harness.echo_endpoint();
    let echo_parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (echo_parts[0], echo_parts[1].parse::<u16>().unwrap_or(8080));

    // Create backend cluster
    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "authz-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    println!("✓ Backend cluster created: {}", cluster.name);

    // Create route
    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "authz-route",
                "authz.e2e.local",
                "/api",
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
                "authz-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    println!("✓ Listener created: {} on port {:?}", listener.name, listener.port);

    // Create ext_authz filter using builder
    let filter_config = filter_configs::ext_authz(&authz_cluster.name)
        .timeout_seconds(5)
        .path_prefix("/auth")
        .failure_mode_allow(false)
        .build();

    let filter = with_timeout(TestTimeout::default_with_label("Create ext_authz filter"), async {
        api.create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "ext-authz-filter",
            "ext_authz",
            filter_config,
        )
        .await
    })
    .await
    .expect("Filter creation should succeed");

    assert_eq!(filter.name, "ext-authz-filter");
    assert_eq!(filter.filter_type, "ext_authz");
    println!("✓ ext_authz filter created: {} (id={})", filter.name, filter.id);

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

    println!("✓ ext_authz setup complete");
}

/// Test ext_authz allows requests with valid authorization header
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_authz_allow() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_101_authz_allow"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping authz allow test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Setup infrastructure (same as test_100)
    let authz_endpoint =
        harness.mocks().ext_authz_endpoint().expect("ext_authz mock should be running");
    let parts: Vec<&str> = authz_endpoint.split(':').collect();
    let (authz_host, authz_port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let authz_cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "allow-authz-cluster", authz_host, authz_port),
        )
        .await
        .expect("Authz cluster creation should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let echo_parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (echo_parts[0], echo_parts[1].parse::<u16>().unwrap_or(8080));

    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "allow-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "allow-route",
                "allow.e2e.local",
                "/api",
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
                "allow-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    // Create and install ext_authz filter
    let filter_config = filter_configs::ext_authz(&authz_cluster.name).timeout_seconds(5).build();

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "allow-authz-filter",
            "ext_authz",
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
        harness.wait_for_route("allow.e2e.local", "/api/test", 200).await
    })
    .await;

    // Make request with authorization header that ext_authz will allow
    let envoy = harness.envoy().unwrap();
    let mut headers = HashMap::new();
    headers.insert("x-ext-authz-allow".to_string(), "true".to_string());

    let (status, response_headers, body) =
        with_timeout(TestTimeout::default_with_label("Request with valid authz"), async {
            envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::GET,
                    "allow.e2e.local",
                    "/api/protected",
                    headers,
                    None,
                )
                .await
        })
        .await
        .expect("Request should succeed");

    println!("Response status: {}", status);
    println!("Response headers: {:?}", response_headers);
    println!("Response body: {}", body);

    assert_eq!(status, 200, "Expected 200 OK for authorized request, got: {}", status);

    // Verify ext_authz check was performed
    // The mock ext_authz service adds a header when authorization succeeds
    if let Some(check_header) = response_headers.get("x-ext-authz-check-received") {
        assert_eq!(check_header, "true", "Expected ext_authz check header to be present");
        println!("✓ ext_authz check header confirmed");
    }

    println!("✓ ext_authz ALLOW test passed - authorized request succeeded");
}

/// Test ext_authz denies requests without valid authorization
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_102_authz_deny() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_102_authz_deny"))
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping authz deny test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api).await.expect("Setup should succeed");

    // Setup infrastructure
    let authz_endpoint =
        harness.mocks().ext_authz_endpoint().expect("ext_authz mock should be running");
    let parts: Vec<&str> = authz_endpoint.split(':').collect();
    let (authz_host, authz_port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let authz_cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "deny-authz-cluster", authz_host, authz_port),
        )
        .await
        .expect("Authz cluster creation should succeed");

    let echo_endpoint = harness.echo_endpoint();
    let echo_parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (echo_parts[0], echo_parts[1].parse::<u16>().unwrap_or(8080));

    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "deny-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(&ctx.team_a_name, "deny-route", "deny.e2e.local", "/api", &cluster.name),
        )
        .await
        .expect("Route creation should succeed");

    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "deny-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    // Create and install ext_authz filter
    let filter_config = filter_configs::ext_authz(&authz_cluster.name).timeout_seconds(5).build();

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "deny-authz-filter",
            "ext_authz",
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

    // Make request WITHOUT the authorization header - should be denied
    let envoy = harness.envoy().unwrap();
    let headers = HashMap::new(); // No x-ext-authz-allow header

    let (status, _, body) =
        with_timeout(TestTimeout::default_with_label("Request without authz"), async {
            envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::GET,
                    "deny.e2e.local",
                    "/api/protected",
                    headers,
                    None,
                )
                .await
        })
        .await
        .expect("Request should complete");

    println!("Response status: {}", status);
    println!("Response body: {}", body);

    assert_eq!(status, 403, "Expected 403 Forbidden for unauthorized request, got: {}", status);
    println!("✓ ext_authz DENY test passed - unauthorized request blocked with 403");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ext_authz_filter_config_format() {
        let config = filter_configs::ext_authz("test-cluster")
            .timeout_seconds(10)
            .path_prefix("/authz")
            .build();

        assert_eq!(config["type"], "ext_authz");
        assert_eq!(config["config"]["http_service"]["server_uri"]["cluster"], "test-cluster");
        assert_eq!(config["config"]["http_service"]["server_uri"]["timeout_seconds"], 10);
        assert_eq!(config["config"]["http_service"]["path_prefix"], "/authz");
    }

    #[test]
    fn test_ext_authz_with_request_body() {
        let config =
            filter_configs::ext_authz("test-cluster").with_request_body(1024, false).build();

        assert!(config["config"]["with_request_body"].is_object());
        assert_eq!(config["config"]["with_request_body"]["max_request_bytes"], 1024);
        assert_eq!(config["config"]["with_request_body"]["allow_partial_message"], false);
    }
}
