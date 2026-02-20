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
    api_client::{setup_envoy_context, simple_cluster, simple_listener, simple_route, ApiClient},
    filter_configs,
    harness::{TestHarness, TestHarnessConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Debug test: Step-by-step verification of ext_authz setup
/// 1. Create cluster/route/listener and verify backend works
/// 2. Test ext_authz mock directly
/// 3. Verify backend cluster was propagated to Envoy
/// 4. Add ext_authz filter and verify authz cluster propagation
/// 5. Test with ext_authz filter active
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_098_debug_ext_authz_step_by_step() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_098_debug_ext_authz_step_by_step").with_ext_authz_mock(),
    )
    .await
    .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx = setup_envoy_context(&api, "test_098_debug_ext_authz_step_by_step")
        .await
        .expect("Setup should succeed");

    // Get endpoints
    let echo_endpoint = harness.echo_endpoint();
    let echo_parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (echo_host, echo_port) = (echo_parts[0], echo_parts[1].parse::<u16>().unwrap_or(8080));

    let authz_endpoint =
        harness.mocks().ext_authz_endpoint().expect("ext_authz mock should be running");

    println!("\n========================================");
    println!("STEP 0: Endpoints");
    println!("========================================");
    println!("Echo backend: {}", echo_endpoint);
    println!("ext_authz mock: {}", authz_endpoint);
    println!("Envoy listener port: {}", harness.ports.listener);

    // ========================================
    // STEP 1: Create cluster/route/listener and verify backend
    // ========================================
    println!("\n========================================");
    println!("STEP 1: Create infrastructure (no filter)");
    println!("========================================");

    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "debug-backend", echo_host, echo_port),
        )
        .await
        .expect("Cluster creation should succeed");
    println!("✓ Cluster created: {}", cluster.name);

    println!("  ... waiting 3s for xDS propagation ...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "debug-route",
                "debug.e2e.local",
                "/testing/debug",
                &cluster.name,
            ),
        )
        .await
        .expect("Route creation should succeed");
    println!("✓ Route created: {} (domain: debug.e2e.local, path: /testing/debug)", route.name);

    println!("  ... waiting 3s for xDS propagation ...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "debug-listener",
                harness.ports.listener,
                &route.name,
                &ctx.team_a_dataplane_id,
            ),
        )
        .await
        .expect("Listener creation should succeed");
    println!("✓ Listener created: {} on port {}", listener.name, harness.ports.listener);

    println!("  ... waiting 3s for xDS propagation ...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Wait for route and test backend
    println!("\n--- Testing backend WITHOUT ext_authz filter ---");
    let body = harness
        .wait_for_route("debug.e2e.local", "/testing/debug/hello", 200)
        .await
        .expect("Route should work");
    println!("✓ Backend responds: {}", &body[..body.len().min(100)]);

    // ========================================
    // STEP 2: Test ext_authz mock directly
    // ========================================
    println!("\n========================================");
    println!("STEP 2: Test ext_authz mock directly");
    println!("========================================");

    let client = reqwest::Client::new();

    // Test paths that should match ^/auth.*
    let test_paths = vec!["/auth", "/auth/", "/auth/check", "/auth/testing/debug/hello"];

    for path in &test_paths {
        let url = format!("http://{}{}", authz_endpoint, path);

        // Without allow header -> should get 403
        let resp = client.post(&url).send().await;
        match resp {
            Ok(r) => println!("  POST {} (no header) -> {}", path, r.status()),
            Err(e) => println!("  POST {} (no header) -> ERROR: {}", path, e),
        }

        // With allow header -> should get 200
        let resp = client.post(&url).header("x-ext-authz-allow", "true").send().await;
        match resp {
            Ok(r) => println!("  POST {} (with header) -> {}", path, r.status()),
            Err(e) => println!("  POST {} (with header) -> ERROR: {}", path, e),
        }
    }

    // Test path that should NOT match (no /auth prefix)
    let bad_path = "/testing/debug/hello";
    let url = format!("http://{}{}", authz_endpoint, bad_path);
    let resp = client.post(&url).send().await;
    match resp {
        Ok(r) => println!("  POST {} (no /auth prefix) -> {} (expect 404)", bad_path, r.status()),
        Err(e) => println!("  POST {} (no /auth prefix) -> ERROR: {}", bad_path, e),
    }

    // ========================================
    // STEP 3: Verify backend cluster propagated to Envoy
    // ========================================
    println!("\n========================================");
    println!("STEP 3: Check if backend cluster propagated to Envoy");
    println!("========================================");

    let envoy = harness.envoy().unwrap();

    // Check Envoy config dump for our backend cluster
    match envoy.get_config_dump().await {
        Ok(config_dump) => {
            // Look for our backend cluster in the config dump
            if config_dump.contains("debug-backend") {
                println!("✓ Backend cluster 'debug-backend' found in Envoy config dump");

                // Try to extract more details about the cluster
                let lines: Vec<&str> =
                    config_dump.lines().filter(|l| l.contains("debug-backend")).take(5).collect();
                for line in &lines {
                    println!("  Config line: {}", line.trim());
                }
            } else {
                println!("✗ Backend cluster 'debug-backend' NOT found in Envoy config dump");
                println!("  This could indicate xDS propagation issues");
            }
        }
        Err(e) => println!("⚠ Failed to get Envoy config dump: {}", e),
    }

    // Check cluster stats
    match envoy.get_stats().await {
        Ok(stats) => {
            let cluster_stats: Vec<&str> = stats
                .lines()
                .filter(|l| l.contains("debug-backend") && l.contains("membership"))
                .take(10)
                .collect();

            if cluster_stats.is_empty() {
                println!("⚠ No membership stats found for debug-backend cluster");
            } else {
                println!("✓ Backend cluster stats:");
                for stat in &cluster_stats {
                    println!("  {}", stat);
                }
            }
        }
        Err(e) => println!("⚠ Failed to get Envoy stats: {}", e),
    }

    // ========================================
    // STEP 4: Add ext_authz filter
    // ========================================
    println!("\n========================================");
    println!("STEP 4: Add ext_authz filter");
    println!("========================================");

    // Create authz cluster
    let authz_parts: Vec<&str> = authz_endpoint.split(':').collect();
    let (authz_host, authz_port) = (authz_parts[0], authz_parts[1].parse::<u16>().unwrap_or(8080));

    let authz_cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "debug-authz-cluster", authz_host, authz_port),
        )
        .await
        .expect("Authz cluster creation should succeed");
    println!("✓ Authz cluster created: {}", authz_cluster.name);

    println!("  ... waiting 3s for xDS propagation ...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Verify authz cluster is now in Envoy
    println!("\n--- Checking if authz cluster propagated to Envoy ---");
    match envoy.get_config_dump().await {
        Ok(config_dump) => {
            if config_dump.contains("debug-authz-cluster") {
                println!("✓ Authz cluster 'debug-authz-cluster' found in Envoy config dump");
            } else {
                println!("✗ Authz cluster 'debug-authz-cluster' NOT found in Envoy config dump!");
                println!("  This is likely the root cause of ext_authz failures");
            }
        }
        Err(e) => println!("⚠ Failed to get Envoy config dump: {}", e),
    }

    match envoy.get_stats().await {
        Ok(stats) => {
            let authz_cluster_stats: Vec<&str> = stats
                .lines()
                .filter(|l| l.contains("debug-authz-cluster") && l.contains("membership"))
                .take(5)
                .collect();

            if authz_cluster_stats.is_empty() {
                println!("⚠ No membership stats found for debug-authz-cluster");
            } else {
                println!("✓ Authz cluster stats:");
                for stat in &authz_cluster_stats {
                    println!("  {}", stat);
                }
            }
        }
        Err(e) => println!("⚠ Failed to get Envoy stats: {}", e),
    }

    // Create ext_authz filter
    let filter_config = filter_configs::ext_authz(&authz_cluster.name)
        .timeout_seconds(5)
        .path_prefix("/auth")
        .build();

    println!("✓ Filter config: {}", serde_json::to_string_pretty(&filter_config).unwrap());

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "debug-authz-filter",
            "ext_authz",
            filter_config,
        )
        .await
        .expect("Filter creation should succeed");
    println!("✓ Filter created: {} (id={})", filter.name, filter.id);

    println!("  ... waiting 3s for xDS propagation ...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Install filter on listener
    api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(100))
        .await
        .expect("Filter installation should succeed");
    println!("✓ Filter installed on listener: {}", listener.name);

    println!("  ... waiting 3s for xDS propagation ...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Configure at route-config level
    api.configure_filter_at_route_config(&ctx.admin_token, &filter.id, &route.name)
        .await
        .expect("Filter route-config configuration should succeed");
    println!("✓ Filter configured at route-config level: {}", route.name);

    // Wait for xDS propagation
    println!("\n--- Waiting 3s for xDS propagation ---");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Check ext_authz filter in Envoy config dump
    println!("\n--- Checking ext_authz filter in Envoy config ---");
    match envoy.get_config_dump().await {
        Ok(config_dump) => {
            // Look for http_service in ext_authz config (this is the actual filter config)
            if config_dump.contains("http_service") {
                println!("✓ http_service found in config dump");
                let lines: Vec<&str> = config_dump.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if line.contains("http_service") {
                        println!("  Found http_service at line {}", i);
                        // Print 15 lines of context
                        for (j, context_line) in
                            lines.iter().enumerate().skip(i).take(15.min(lines.len() - i))
                        {
                            println!("    {}: {}", j, context_line.trim());
                        }
                        break;
                    }
                }
            } else {
                println!("✗ http_service NOT found - ext_authz filter may not be configured!");
            }
            // Also check for the authz cluster name specifically
            if config_dump.contains("debug-authz-cluster") {
                println!("✓ 'debug-authz-cluster' reference found in config dump");
            } else {
                println!("✗ 'debug-authz-cluster' NOT found in config dump!");
            }
        }
        Err(e) => println!("⚠ Failed to get config dump: {}", e),
    }

    // ========================================
    // STEP 5: Test with ext_authz filter active
    // ========================================
    println!("\n========================================");
    println!("STEP 5: Test with ext_authz filter active");
    println!("========================================");

    // Test WITHOUT the allow header (should get 403 from ext_authz)
    println!("\n--- Request WITHOUT x-ext-authz-allow header ---");
    let headers_empty: HashMap<String, String> = HashMap::new();
    let result = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "debug.e2e.local",
            "/testing/debug/protected",
            headers_empty,
            None,
        )
        .await;

    match result {
        Ok((status, headers, body)) => {
            println!("  Status: {}", status);
            println!("  Headers: {:?}", headers);
            println!("  Body: {}", &body[..body.len().min(200)]);
            if status == 403 {
                println!("  ✓ Got expected 403 (ext_authz denied)");
            } else if status == 404 {
                println!("  ✗ Got 404 - route or ext_authz path issue!");
            } else {
                println!("  ? Unexpected status");
            }
        }
        Err(e) => println!("  ERROR: {}", e),
    }

    // Test WITH the allow header (should get 200)
    println!("\n--- Request WITH x-ext-authz-allow header ---");
    let mut headers_allow: HashMap<String, String> = HashMap::new();
    headers_allow.insert("x-ext-authz-allow".to_string(), "true".to_string());
    let result = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "debug.e2e.local",
            "/testing/debug/protected",
            headers_allow,
            None,
        )
        .await;

    match result {
        Ok((status, headers, body)) => {
            println!("  Status: {}", status);
            println!("  Headers: {:?}", headers);
            println!("  Body: {}", &body[..body.len().min(200)]);
            if status == 200 {
                println!("  ✓ Got expected 200 (ext_authz allowed)");
            } else {
                println!("  ✗ Unexpected status {}", status);
            }
        }
        Err(e) => println!("  ERROR: {}", e),
    }

    // Check Envoy stats for ext_authz
    println!("\n--- Envoy ext_authz stats ---");
    if let Ok(stats) = envoy.get_stats().await {
        let authz_stats: Vec<&str> =
            stats.lines().filter(|l| l.contains("ext_authz")).take(20).collect();
        for stat in authz_stats {
            println!("  {}", stat);
        }

        // Check authz cluster connection stats
        println!("\n--- debug-authz-cluster connection stats ---");
        let cluster_stats: Vec<&str> =
            stats.lines().filter(|l| l.contains("debug-authz-cluster")).take(30).collect();
        for stat in cluster_stats {
            println!("  {}", stat);
        }
    }

    println!("\n========================================");
    println!("DEBUG TEST COMPLETE");
    println!("========================================");
}

/// Test setup: Create authz infrastructure with ext_authz filter
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_setup_ext_authz() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_100_setup_ext_authz").with_ext_authz_mock(),
    )
    .await
    .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping ext_authz setup test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx =
        setup_envoy_context(&api, "test_100_setup_ext_authz").await.expect("Setup should succeed");

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
                "/testing/authz-setup",
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
                &ctx.team_a_dataplane_id,
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

    // Configure filter at route-config level (required for ext_authz to be active)
    api.configure_filter_at_route_config(&ctx.admin_token, &filter.id, &route.name)
        .await
        .expect("Filter route-config configuration should succeed");

    println!("✓ Filter configured at route-config level: {}", route.name);

    // Wait for configuration to propagate
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    println!("✓ ext_authz setup complete");
}

/// Test ext_authz allows requests with valid authorization header
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_authz_allow() {
    let harness =
        TestHarness::start(TestHarnessConfig::new("test_101_authz_allow").with_ext_authz_mock())
            .await
            .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping authz allow test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx =
        setup_envoy_context(&api, "test_101_authz_allow").await.expect("Setup should succeed");

    // Setup infrastructure (same as test_100)
    let authz_endpoint =
        harness.mocks().ext_authz_endpoint().expect("ext_authz mock should be running");
    println!("✓ ext_authz mock endpoint: {}", authz_endpoint);
    let parts: Vec<&str> = authz_endpoint.split(':').collect();
    let (authz_host, authz_port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));
    println!("✓ Creating authz cluster at {}:{}", authz_host, authz_port);

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
                "/testing/authz-allow",
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
                &ctx.team_a_dataplane_id,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    println!("✓ Listener created: {} on port {}", listener.name, harness.ports.listener);
    println!("✓ Route domain: allow.e2e.local, path: /testing/authz-allow");

    // Create and install ext_authz filter
    let filter_config = filter_configs::ext_authz(&authz_cluster.name).timeout_seconds(5).build();
    println!(
        "✓ Filter config: {}",
        serde_json::to_string_pretty(&filter_config).unwrap_or_default()
    );

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

    // Configure filter at route-config level (required for ext_authz to be active)
    api.configure_filter_at_route_config(&ctx.admin_token, &filter.id, &route.name)
        .await
        .expect("Filter route-config configuration should succeed");

    println!("✓ Filter configured at route-config level: {}", route.name);

    // Wait for xDS to propagate all resources including the authz cluster
    // The authz cluster needs to be fully available before ext_authz will work
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    println!("✓ Waited 3s for xDS propagation");

    // Verify we can reach the authz mock directly
    let client = reqwest::Client::new();
    let authz_url = format!("http://{}/auth/test", authz_endpoint);
    match client.post(&authz_url).send().await {
        Ok(resp) => println!("✓ Direct authz mock check: status {}", resp.status()),
        Err(e) => println!("⚠ Cannot reach authz mock directly: {}", e),
    }

    // Check what clusters and listeners Envoy has
    let envoy = harness.envoy().unwrap();
    match envoy.get_stats().await {
        Ok(stats) => {
            let authz_cluster_stats: Vec<&str> =
                stats.lines().filter(|line| line.contains("allow-authz-cluster")).take(5).collect();
            if authz_cluster_stats.is_empty() {
                println!(
                    "⚠ No stats found for allow-authz-cluster - cluster may not be configured"
                );
            } else {
                println!("✓ Found authz cluster stats: {:?}", authz_cluster_stats);
            }

            // Check listener stats
            let listener_stats: Vec<&str> = stats
                .lines()
                .filter(|line| line.contains("listener.") && line.contains("downstream_cx"))
                .take(10)
                .collect();
            println!("✓ Listener stats: {:?}", listener_stats);
        }
        Err(e) => println!("⚠ Cannot get Envoy stats: {}", e),
    }

    // Check what port we're actually trying to connect to
    println!("✓ Will connect to Envoy on port {}", harness.ports.listener);

    // Wait for route to be ready - we need 200 to confirm ext_authz is working correctly
    let envoy = harness.envoy().unwrap();
    with_timeout(TestTimeout::default_with_label("Wait for route"), async {
        // Retry until we get 200 (route ready AND ext_authz working)
        for i in 0..30 {
            let mut check_headers = HashMap::new();
            check_headers.insert("x-ext-authz-allow".to_string(), "true".to_string());
            let result = envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::GET,
                    "allow.e2e.local",
                    "/testing/authz-allow/test",
                    check_headers,
                    None,
                )
                .await;
            if let Ok((status, _, _)) = result {
                println!("  Wait loop iteration {}: got status {}", i, status);
                if status == 200 {
                    println!("✓ Route ready (got 200 - ext_authz allowing correctly)");
                    return Ok::<(), anyhow::Error>(());
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        anyhow::bail!("Route did not become ready with 200")
    })
    .await
    .expect("Route should become ready");

    // Make request with authorization header that ext_authz will allow
    let mut headers = HashMap::new();
    headers.insert("x-ext-authz-allow".to_string(), "true".to_string());

    let (status, response_headers, body) =
        with_timeout(TestTimeout::default_with_label("Request with valid authz"), async {
            envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::GET,
                    "allow.e2e.local",
                    "/testing/authz-allow/protected",
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
    let harness =
        TestHarness::start(TestHarnessConfig::new("test_102_authz_deny").with_ext_authz_mock())
            .await
            .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping authz deny test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx = setup_envoy_context(&api, "test_102_authz_deny").await.expect("Setup should succeed");

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
            &simple_route(
                &ctx.team_a_name,
                "deny-route",
                "deny.e2e.local",
                "/testing/authz-deny",
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
                "deny-listener",
                harness.ports.listener,
                &route.name,
                &ctx.team_a_dataplane_id,
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

    // Configure filter at route-config level (required for ext_authz to be active)
    api.configure_filter_at_route_config(&ctx.admin_token, &filter.id, &route.name)
        .await
        .expect("Filter route-config configuration should succeed");

    println!("✓ Filter configured at route-config level: {}", route.name);

    // Wait for route to be ready by making a request WITH the allow header
    // This confirms xDS propagation and that ext_authz is working
    let envoy = harness.envoy().unwrap();

    println!(
        "✓ Waiting for route convergence on port {} with host deny.e2e.local",
        harness.ports.listener
    );
    println!("✓ Authz cluster: {} at {}:{}", authz_cluster.name, authz_host, authz_port);

    with_timeout(TestTimeout::default_with_label("Wait for route"), async {
        // Retry until we get 200 (route ready AND ext_authz working correctly)
        for i in 0..30 {
            let mut check_headers = HashMap::new();
            check_headers.insert("x-ext-authz-allow".to_string(), "true".to_string());
            let result = envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::GET,
                    "deny.e2e.local",
                    "/testing/authz-deny/test",
                    check_headers,
                    None,
                )
                .await;
            match &result {
                Ok((status, _, body)) => {
                    println!(
                        "  Wait loop iteration {}: status={}, body={}",
                        i,
                        status,
                        &body[..body.len().min(100)]
                    );
                    if *status == 200 {
                        println!("✓ Route ready (got 200 - ext_authz allowing correctly)");
                        return Ok::<(), anyhow::Error>(());
                    }
                }
                Err(e) => {
                    println!("  Wait loop iteration {}: error={}", i, e);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        anyhow::bail!("Route did not become ready with 200")
    })
    .await
    .expect("Route should become ready");

    // Make request WITHOUT the authorization header - should be denied
    let headers = HashMap::new(); // No x-ext-authz-allow header

    let (status, _, body) =
        with_timeout(TestTimeout::default_with_label("Request without authz"), async {
            envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::GET,
                    "deny.e2e.local",
                    "/testing/authz-deny/protected",
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

        // Config is inner config only (no type wrapper - API client adds it)
        // API expects: { service: { type: "http", server_uri: {...}, ... }, ... }
        assert_eq!(config["service"]["type"], "http");
        assert_eq!(config["service"]["server_uri"]["cluster"], "test-cluster");
        assert_eq!(config["service"]["server_uri"]["timeout_ms"], 10000);
        assert_eq!(config["service"]["path_prefix"], "/authz");
    }

    #[test]
    fn test_ext_authz_with_request_body() {
        let config =
            filter_configs::ext_authz("test-cluster").with_request_body(1024, false).build();

        // Config is inner config only (no type wrapper)
        assert!(config["with_request_body"].is_object());
        assert_eq!(config["with_request_body"]["max_request_bytes"], 1024);
        assert_eq!(config["with_request_body"]["allow_partial_message"], false);
    }
}
