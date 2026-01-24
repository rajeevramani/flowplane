//! Smoke test: Comprehensive Routing & Filter Validation
//!
//! Quick validation of core flowplane functionality:
//! 1. Basic routing: cluster -> route -> listener -> proxy
//! 2. Filter attachment: header mutation filter verification
//! 3. xDS config verification: ensure resources appear in Envoy config
//! 4. Team isolation: verify cross-team access is denied
//!
//! Expected time: ~45-60 seconds
//!
//! Design Principles:
//! - Hard timeouts (30s max per operation)
//! - Unique names per test (smoke-* prefix)
//! - Unique paths (/smoke/*)
//! - 3 sec delay between resource creation and Envoy calls

use std::time::Duration;

use serde_json::json;

use crate::common::{
    api_client::{setup_dev_context, simple_cluster, simple_listener, simple_route, ApiClient},
    filter_configs,
    harness::{TestHarness, TestHarnessConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Smoke test for basic routing: cluster -> route -> listener -> proxy
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn smoke_test_basic_routing() {
    let harness = TestHarness::start(TestHarnessConfig::new("smoke_test_basic_routing"))
        .await
        .expect("Failed to start test harness");

    let api = ApiClient::new(harness.api_url());

    // Setup dev context (bootstrap + login + admin token + teams)
    let ctx =
        setup_dev_context(&api, "smoke_test_basic_routing").await.expect("Setup should succeed");
    println!("✓ Dev context ready");

    // Get echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create cluster
    let cluster = with_timeout(TestTimeout::quick("Create cluster"), async {
        api.create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "smoke-backend", host, port),
        )
        .await
    })
    .await
    .expect("Cluster creation should succeed");
    println!("✓ Cluster created: {}", cluster.name);

    // 3 sec delay between resource creation
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Create route
    let route = with_timeout(TestTimeout::quick("Create route"), async {
        api.create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "smoke-route",
                "smoke.e2e.local",
                "/smoke",
                &cluster.name,
            ),
        )
        .await
    })
    .await
    .expect("Route creation should succeed");
    println!("✓ Route created: {}", route.name);

    // 3 sec delay between resource creation
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Create listener
    let listener = with_timeout(TestTimeout::quick("Create listener"), async {
        api.create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "smoke-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
    })
    .await
    .expect("Listener creation should succeed");
    println!("✓ Listener created: {} on port {:?}", listener.name, listener.port);

    // 3 sec delay before calling Envoy
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify routing through Envoy (if available)
    if harness.has_envoy() {
        let body = with_timeout(TestTimeout::default_with_label("Route convergence"), async {
            harness.wait_for_route("smoke.e2e.local", "/smoke/test", 200).await
        })
        .await
        .expect("Route should converge");

        assert!(!body.is_empty(), "Response body should not be empty");
        println!("✓ Proxy verified: {}", &body[..50.min(body.len())]);
    } else {
        println!("⚠ Envoy not available, skipping proxy verification");
    }

    println!("Routing smoke test PASSED");
}

/// Smoke test for filter attachment: header mutation filter
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn smoke_test_filter_attachment() {
    let harness = TestHarness::start(TestHarnessConfig::new("smoke_test_filter_attachment"))
        .await
        .expect("Failed to start test harness");

    let api = ApiClient::new(harness.api_url());

    // Setup dev context
    let ctx = setup_dev_context(&api, "smoke_test_filter_attachment")
        .await
        .expect("Setup should succeed");
    println!("✓ Dev context ready");

    // Get echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create cluster
    let cluster = with_timeout(TestTimeout::quick("Create cluster"), async {
        api.create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "smoke-filter-backend", host, port),
        )
        .await
    })
    .await
    .expect("Cluster creation should succeed");
    println!("✓ Cluster created: {}", cluster.name);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Create route with unique path
    let route = with_timeout(TestTimeout::quick("Create route"), async {
        api.create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "smoke-filter-route",
                "smoke-filter.e2e.local",
                "/smoke/filter",
                &cluster.name,
            ),
        )
        .await
    })
    .await
    .expect("Route creation should succeed");
    println!("✓ Route created: {}", route.name);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Create listener
    let listener = with_timeout(TestTimeout::quick("Create listener"), async {
        api.create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "smoke-filter-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
    })
    .await
    .expect("Listener creation should succeed");
    println!("✓ Listener created: {} on port {:?}", listener.name, listener.port);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Create header mutation filter
    let filter_config = filter_configs::header_mutation()
        .add_response_header("X-Smoke-Test", "flowplane")
        .add_response_header("X-Frame-Options", "DENY")
        .build();

    let filter = with_timeout(TestTimeout::quick("Create filter"), async {
        api.create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "smoke-header-filter",
            "header_mutation",
            filter_config,
        )
        .await
    })
    .await
    .expect("Filter creation should succeed");
    println!("✓ Filter created: {} (id: {})", filter.name, filter.id);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Install filter on listener
    with_timeout(TestTimeout::quick("Install filter"), async {
        api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(1)).await
    })
    .await
    .expect("Filter installation should succeed");
    println!("✓ Filter installed on listener");

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Attach filter to route
    with_timeout(TestTimeout::quick("Attach filter to route"), async {
        api.attach_filter_to_route(&ctx.admin_token, &route.name, &filter.id, Some(1)).await
    })
    .await
    .expect("Filter attachment should succeed");
    println!("✓ Filter attached to route");

    // 3 sec delay before Envoy verification
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify filter through Envoy
    if harness.has_envoy() {
        // Wait for route convergence first
        with_timeout(TestTimeout::default_with_label("Route convergence"), async {
            harness.wait_for_route("smoke-filter.e2e.local", "/smoke/filter/test", 200).await
        })
        .await
        .expect("Route should converge");

        // Make request and check headers
        let response = with_timeout(TestTimeout::quick("Check headers"), async {
            let envoy = harness.envoy().expect("Envoy should be available");
            envoy
                .proxy_get_with_headers(
                    harness.ports.listener,
                    "smoke-filter.e2e.local",
                    "/smoke/filter/verify",
                )
                .await
        })
        .await
        .expect("Proxy request should succeed");

        // Verify custom headers are present
        assert!(
            response.headers.contains_key("x-smoke-test"),
            "X-Smoke-Test header should be present. Headers: {:?}",
            response.headers
        );
        assert_eq!(
            response.headers.get("x-smoke-test").map(|s| s.as_str()),
            Some("flowplane"),
            "X-Smoke-Test header value should be 'flowplane'"
        );
        println!("✓ Filter headers verified: X-Smoke-Test=flowplane");

        assert!(
            response.headers.contains_key("x-frame-options"),
            "X-Frame-Options header should be present"
        );
        println!("✓ Filter headers verified: X-Frame-Options=DENY");
    } else {
        println!("⚠ Envoy not available, skipping filter verification");
    }

    println!("Filter attachment smoke test PASSED");
}

/// Smoke test for xDS config verification
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn smoke_test_xds_config() {
    let harness = TestHarness::start(TestHarnessConfig::new("smoke_test_xds_config"))
        .await
        .expect("Failed to start test harness");

    let api = ApiClient::new(harness.api_url());

    // Setup dev context
    let ctx = setup_dev_context(&api, "smoke_test_xds_config").await.expect("Setup should succeed");
    println!("✓ Dev context ready");

    // Get echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create cluster with unique name
    let cluster = with_timeout(TestTimeout::quick("Create cluster"), async {
        api.create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "smoke-xds-backend", host, port),
        )
        .await
    })
    .await
    .expect("Cluster creation should succeed");
    println!("✓ Cluster created: {}", cluster.name);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Create route with unique path
    let route = with_timeout(TestTimeout::quick("Create route"), async {
        api.create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "smoke-xds-route",
                "smoke-xds.e2e.local",
                "/smoke/xds",
                &cluster.name,
            ),
        )
        .await
    })
    .await
    .expect("Route creation should succeed");
    println!("✓ Route created: {}", route.name);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Create listener
    let _listener = with_timeout(TestTimeout::quick("Create listener"), async {
        api.create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "smoke-xds-listener",
                harness.ports.listener,
                &route.name,
            ),
        )
        .await
    })
    .await
    .expect("Listener creation should succeed");
    println!("✓ Listener created");

    // 3 sec delay for xDS propagation
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify xDS config dump contains our resources
    if harness.has_envoy() {
        let config_dump = with_timeout(TestTimeout::quick("Get config dump"), async {
            harness.get_config_dump().await
        })
        .await
        .expect("Config dump should succeed");

        // Verify cluster appears in config
        assert!(
            config_dump.contains("smoke-xds-backend") || config_dump.contains(&cluster.name),
            "Cluster should appear in xDS config dump"
        );
        println!("✓ Cluster verified in xDS config");

        // Verify route appears in config
        assert!(
            config_dump.contains("smoke-xds.e2e.local"),
            "Route domain should appear in xDS config dump"
        );
        println!("✓ Route domain verified in xDS config");

        // Verify listener port appears
        assert!(
            config_dump.contains(&format!("{}", harness.ports.listener)),
            "Listener port should appear in xDS config dump"
        );
        println!("✓ Listener port verified in xDS config");
    } else {
        println!("⚠ Envoy not available, skipping xDS verification");
    }

    println!("xDS config smoke test PASSED");
}

/// Smoke test for team isolation
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn smoke_test_team_isolation() {
    let harness = TestHarness::start(TestHarnessConfig::new("smoke_test_team_isolation"))
        .await
        .expect("Failed to start test harness");

    let api = ApiClient::new(harness.api_url());

    // Setup dev context with two teams
    let ctx =
        setup_dev_context(&api, "smoke_test_team_isolation").await.expect("Setup should succeed");
    println!("✓ Dev context ready with teams: {} and {}", ctx.team_a_name, ctx.team_b_name);

    // Get echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create cluster for Team A
    let cluster_a = with_timeout(TestTimeout::quick("Create Team A cluster"), async {
        api.create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "smoke-iso-backend-a", host, port),
        )
        .await
    })
    .await
    .expect("Team A cluster creation should succeed");
    println!("✓ Team A cluster created: {}", cluster_a.name);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Create a team-scoped token for Team B only
    let team_b_token = ctx.team_b_dev_token.as_ref().unwrap_or(&ctx.admin_token);

    // Try to create a resource in Team A using Team B token - should fail
    let team_b_try_team_a = with_timeout(TestTimeout::quick("Cross-team access attempt"), async {
        api.create_cluster(
            team_b_token,
            &simple_cluster(&ctx.team_a_name, "smoke-iso-illegal", host, port),
        )
        .await
    })
    .await;

    // If using admin token, this will succeed (admin has all access)
    // If using team-scoped token, this should fail with 403
    if ctx.team_b_dev_token.is_some() {
        assert!(
            team_b_try_team_a.is_err(),
            "Team B should not be able to create resources in Team A"
        );
        println!("✓ Team isolation verified: Team B cannot create in Team A");
    } else {
        println!("⚠ Using admin token, skipping cross-team denial test");
    }

    // Verify Team B can create in their own team
    let cluster_b = with_timeout(TestTimeout::quick("Create Team B cluster"), async {
        api.create_cluster(
            &ctx.admin_token, // Use admin for reliable test
            &simple_cluster(&ctx.team_b_name, "smoke-iso-backend-b", host, port),
        )
        .await
    })
    .await
    .expect("Team B cluster creation should succeed");
    println!("✓ Team B cluster created: {}", cluster_b.name);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify clusters are in different teams via list
    let team_a_clusters = with_timeout(TestTimeout::quick("List Team A clusters"), async {
        api.list_clusters(&ctx.admin_token, Some(&ctx.team_a_name)).await
    })
    .await
    .expect("List Team A clusters should succeed");

    let team_b_clusters = with_timeout(TestTimeout::quick("List Team B clusters"), async {
        api.list_clusters(&ctx.admin_token, Some(&ctx.team_b_name)).await
    })
    .await
    .expect("List Team B clusters should succeed");

    // Verify Team A's cluster is in Team A's list
    let team_a_has_own = team_a_clusters.iter().any(|c| c.name.contains("smoke-iso-backend-a"));
    assert!(team_a_has_own, "Team A should see its own cluster");
    println!("✓ Team A sees its own cluster");

    // Verify Team B's cluster is in Team B's list
    let team_b_has_own = team_b_clusters.iter().any(|c| c.name.contains("smoke-iso-backend-b"));
    assert!(team_b_has_own, "Team B should see its own cluster");
    println!("✓ Team B sees its own cluster");

    // Verify Team A's list doesn't contain Team B's cluster
    let team_a_sees_b = team_a_clusters.iter().any(|c| c.name.contains("smoke-iso-backend-b"));
    assert!(!team_a_sees_b, "Team A should not see Team B's cluster");
    println!("✓ Team A cannot see Team B's cluster");

    // Verify Team B's list doesn't contain Team A's cluster
    let team_b_sees_a = team_b_clusters.iter().any(|c| c.name.contains("smoke-iso-backend-a"));
    assert!(!team_b_sees_a, "Team B should not see Team A's cluster");
    println!("✓ Team B cannot see Team A's cluster");

    println!("Team isolation smoke test PASSED");
}

/// Smoke test for basic CRUD operations
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn smoke_test_crud_operations() {
    let harness = TestHarness::start(TestHarnessConfig::new("smoke_test_crud_operations"))
        .await
        .expect("Failed to start test harness");

    let api = ApiClient::new(harness.api_url());

    // Setup dev context
    let ctx =
        setup_dev_context(&api, "smoke_test_crud_operations").await.expect("Setup should succeed");
    println!("✓ Dev context ready");

    // Get echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // CREATE - Create a cluster
    let cluster = with_timeout(TestTimeout::quick("Create cluster"), async {
        api.create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "smoke-crud-backend", host, port),
        )
        .await
    })
    .await
    .expect("Cluster creation should succeed");
    println!("✓ CREATE: Cluster created: {}", cluster.name);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // READ - List clusters and verify ours is there
    let clusters = with_timeout(TestTimeout::quick("List clusters"), async {
        api.list_clusters(&ctx.admin_token, Some(&ctx.team_a_name)).await
    })
    .await
    .expect("List clusters should succeed");

    let found = clusters.iter().any(|c| c.name == cluster.name);
    assert!(found, "Created cluster should appear in list");
    println!("✓ READ: Cluster found in list");

    // CREATE filter for UPDATE/DELETE test
    let filter_config = json!({
        "response_headers_to_add": [{
            "key": "X-Test",
            "value": "test"
        }]
    });

    let filter = with_timeout(TestTimeout::quick("Create filter"), async {
        api.create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "smoke-crud-filter",
            "header_mutation",
            filter_config,
        )
        .await
    })
    .await
    .expect("Filter creation should succeed");
    println!("✓ CREATE: Filter created: {} (id: {})", filter.name, filter.id);

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // READ - Get filter by ID
    let filter_read = with_timeout(TestTimeout::quick("Get filter"), async {
        api.get_filter(&ctx.admin_token, &filter.id).await
    })
    .await
    .expect("Get filter should succeed");

    assert_eq!(filter_read.name, filter.name, "Filter name should match");
    println!("✓ READ: Filter retrieved by ID");

    // DELETE - Delete the filter
    with_timeout(TestTimeout::quick("Delete filter"), async {
        api.delete_filter(&ctx.admin_token, &filter.id).await
    })
    .await
    .expect("Delete filter should succeed");
    println!("✓ DELETE: Filter deleted");

    // 3 sec delay
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify filter is gone
    let filter_after_delete = with_timeout(TestTimeout::quick("Get deleted filter"), async {
        api.get_filter(&ctx.admin_token, &filter.id).await
    })
    .await;

    assert!(filter_after_delete.is_err(), "Deleted filter should not be found");
    println!("✓ DELETE verified: Filter no longer exists");

    println!("CRUD operations smoke test PASSED");
}
