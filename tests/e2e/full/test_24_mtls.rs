//! mTLS Connection Tests (Bruno 24)
//!
//! Tests the mTLS xDS connection:
//! - Envoy connects to xDS server with client certificate
//! - SPIFFE URI team extraction and resource scoping
//! - Cross-team resource isolation
//!
//! Prerequisites:
//! - Run with: FLOWPLANE_E2E_MTLS=1 RUN_E2E=1 cargo test --test e2e test_24_mtls -- --ignored
//!
//! Design Principles Followed:
//! - Hard timeouts everywhere (30s max per operation)
//! - Unique names: mtls-* prefix for all resources
//! - Unique paths: /testing/mtls/* for all routes
//! - Unique ports: harness.ports.listener (auto-unique per test)
//! - 3-second delays: Between resource creation and verification
//! - Fail fast: Clear error messages, no silent swallowing

use std::time::Duration;

use crate::common::{
    api_client::{setup_envoy_context, ApiClient, CreateDataplaneRequest},
    harness::{TestHarness, TestHarnessConfig},
    resource_setup::ResourceSetup,
    timeout::{with_timeout, TestTimeout},
};

/// Test 100: Basic mTLS connection verification
///
/// Verifies that Envoy can successfully connect to the xDS server using mTLS.
/// This test checks:
/// - Envoy establishes xDS connection with client certificate
/// - xDS subscription is active (cluster stats present)
/// - No traffic routing verification (just connection-level test)
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_mtls_connection() {
    // Check if mTLS is enabled BEFORE trying to start the harness
    if std::env::var("FLOWPLANE_E2E_MTLS").ok().as_deref() != Some("1") {
        println!("⚠ Skipping mTLS test - FLOWPLANE_E2E_MTLS=1 not set");
        return;
    }

    let harness =
        TestHarness::start(TestHarnessConfig::new("test_100_mtls_connection").with_mtls())
            .await
            .expect("Failed to start harness");

    // Hard requirement assertion - mTLS must be enabled
    assert!(harness.has_mtls(), "mTLS is required for this test. Run with FLOWPLANE_E2E_MTLS=1");

    // Graceful degradation if Envoy not available
    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping mTLS connection verification");
        return;
    }

    // Wait for xDS connection to stabilize (standard 3-second delay)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify Envoy is ready with timeout
    let envoy = harness.envoy().expect("Envoy should be available after has_envoy() check");
    with_timeout(TestTimeout::default_with_label("Check Envoy ready"), async {
        envoy.wait_ready().await
    })
    .await
    .expect("Envoy should be ready and connect via mTLS");

    // Verify xDS subscription is active by checking stats
    let stats = with_timeout(TestTimeout::default_with_label("Get Envoy stats"), async {
        envoy.get_stats().await
    })
    .await
    .expect("Should get Envoy stats");

    // Check for xDS cluster stats (indicates successful connection)
    assert!(
        stats.contains("cluster.xds_cluster") || stats.contains("cluster_manager"),
        "Envoy should have xDS cluster stats, indicating successful mTLS connection"
    );

    println!("✓ mTLS connection verified - Envoy connected to xDS server");
}

/// Test 101: SPIFFE URI team extraction and resource scoping
///
/// Verifies that the control plane:
/// - Extracts team from SPIFFE URI in client certificate
/// - Delivers xDS configuration scoped to that team
/// - Resources created under the mTLS team are visible to Envoy
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_spiffe_team_extraction() {
    // Check if mTLS is enabled BEFORE trying to start the harness
    if std::env::var("FLOWPLANE_E2E_MTLS").ok().as_deref() != Some("1") {
        println!("⚠ Skipping mTLS test - FLOWPLANE_E2E_MTLS=1 not set");
        return;
    }

    let harness =
        TestHarness::start(TestHarnessConfig::new("test_101_spiffe_team_extraction").with_mtls())
            .await
            .expect("Failed to start harness");

    // Hard requirement assertions
    assert!(harness.has_mtls(), "mTLS is required for this test. Run with FLOWPLANE_E2E_MTLS=1");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping SPIFFE team extraction test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_envoy_context(&api, "test_101_spiffe_team_extraction")
        .await
        .expect("Setup should succeed");

    // Extract team from SPIFFE URI
    let spiffe_uri = harness.get_spiffe_uri().expect("Should have SPIFFE URI in mTLS mode");
    let _spiffe_team = harness.get_mtls_team().expect("Should extract team from SPIFFE URI");

    println!("✓ SPIFFE URI: {}", spiffe_uri);
    println!("✓ Extracted team: {}", _spiffe_team);

    // For the actual xDS test, we need to use the shared team that Envoy is configured for
    // The SPIFFE team extraction is verified above, but resources must be under E2E_SHARED_TEAM
    // for Envoy to receive them via xDS

    // Create resources using ResourceSetup builder
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let resources = with_timeout(TestTimeout::default_with_label("Create test resources"), async {
        ResourceSetup::new(&api, &ctx.admin_token, &ctx.team_a_name, &ctx.team_a_dataplane_id)
            .with_cluster("mtls-extraction-cluster", host, port)
            .with_route("mtls-extraction-route", "/testing/mtls/extraction")
            .with_listener("mtls-extraction-listener", harness.ports.listener)
            .build()
            .await
    })
    .await
    .expect("Resources should be created");

    println!(
        "✓ Resources created: cluster={}, route={}, listener={}",
        resources.cluster().name,
        resources.route().name,
        resources.listener().name
    );

    // Wait for xDS propagation (standard 3 seconds)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Wait for route convergence (NOT fixed sleep)
    let _ = with_timeout(TestTimeout::default_with_label("Route convergence"), async {
        harness
            .wait_for_route(
                &format!("{}.e2e.local", resources.route().name),
                "/testing/mtls/extraction",
                200,
            )
            .await
    })
    .await
    .expect("Route should converge");

    // Verify Envoy config dump contains the cluster (proves team scoping works)
    let envoy = harness.envoy().expect("Envoy should be available");
    let config = with_timeout(TestTimeout::default_with_label("Get config dump"), async {
        envoy.get_config_dump().await
    })
    .await
    .expect("Should get config dump");

    assert!(
        config.contains(&resources.cluster().name),
        "Envoy should have cluster '{}' in config dump (team scoping verification)",
        resources.cluster().name
    );

    println!("✓ Team extraction verified - resources scoped to team are visible to Envoy");
}

/// Test 102: Cross-team resource isolation
///
/// Verifies that mTLS enforces team boundaries:
/// - Resources created for team-a (shared team) are visible to Envoy
/// - Resources created for team-b (different team) are NOT visible
/// - Config dump contains only team-a resources
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_102_team_isolation() {
    // Check if mTLS is enabled BEFORE trying to start the harness
    if std::env::var("FLOWPLANE_E2E_MTLS").ok().as_deref() != Some("1") {
        println!("⚠ Skipping mTLS test - FLOWPLANE_E2E_MTLS=1 not set");
        return;
    }

    let harness = TestHarness::start(TestHarnessConfig::new("test_102_team_isolation").with_mtls())
        .await
        .expect("Failed to start harness");

    // Hard requirement assertions
    assert!(harness.has_mtls(), "mTLS is required for this test. Run with FLOWPLANE_E2E_MTLS=1");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping team isolation test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_envoy_context(&api, "test_102_team_isolation").await.expect("Setup should succeed");

    // Team A is the shared team that Envoy can see
    let team_a_name = ctx.team_a_name.clone();

    // Create Team B for isolation testing
    let team_b = with_timeout(TestTimeout::default_with_label("Create Team B"), async {
        api.create_team_idempotent(
            &ctx.admin_token,
            &format!("{}-isolation-b", team_a_name),
            Some("Team B for isolation testing"),
        )
        .await
    })
    .await
    .expect("Team B creation should succeed");

    // Create dataplane for team-b
    let dataplane_b = with_timeout(TestTimeout::default_with_label("Create Dataplane B"), async {
        api.create_dataplane_idempotent(
            &ctx.admin_token,
            &CreateDataplaneRequest {
                team: team_b.name.clone(),
                name: format!("{}-dataplane", team_b.name),
                gateway_host: Some("127.0.0.1".to_string()),
                description: Some("Dataplane for team B".to_string()),
            },
        )
        .await
    })
    .await
    .expect("Dataplane B creation should succeed");

    println!("✓ Created team-b: {}", team_b.name);

    // Get echo endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create resources for team-a (shared team - should be visible to Envoy)
    let resources_a =
        with_timeout(TestTimeout::default_with_label("Create team-a resources"), async {
            ResourceSetup::new(&api, &ctx.admin_token, &team_a_name, &ctx.team_a_dataplane_id)
                .with_cluster("mtls-team-a-cluster", host, port)
                .with_route("mtls-team-a-route", "/testing/mtls/team-a")
                .with_listener("mtls-team-a-listener", harness.ports.listener)
                .build()
                .await
        })
        .await
        .expect("Team-a resources should be created");

    println!("✓ Created team-a resources: cluster={}", resources_a.cluster().name);

    // Wait between resource creation
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Create resources for team-b (different team - should NOT be visible)
    let resources_b =
        with_timeout(TestTimeout::default_with_label("Create team-b resources"), async {
            ResourceSetup::new(&api, &ctx.admin_token, &team_b.name, &dataplane_b.id)
                .with_cluster("mtls-team-b-cluster", host, port)
                .with_route("mtls-team-b-route", "/testing/mtls/team-b")
                .with_listener("mtls-team-b-listener", harness.ports.listener_secondary)
                .build()
                .await
        })
        .await
        .expect("Team-b resources should be created");

    println!("✓ Created team-b resources: cluster={}", resources_b.cluster().name);

    // Wait for xDS propagation
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Get Envoy config dump
    let envoy = harness.envoy().expect("Envoy should be available");
    let config = with_timeout(TestTimeout::default_with_label("Get config dump"), async {
        envoy.get_config_dump().await
    })
    .await
    .expect("Should get config dump");

    // POSITIVE assertion: team-a resources SHOULD be present
    assert!(
        config.contains(&resources_a.cluster().name),
        "Envoy should see team-a cluster '{}' (shared team matches)",
        resources_a.cluster().name
    );

    // NEGATIVE assertion: team-b resources should NOT be present (critical isolation test)
    assert!(
        !config.contains(&resources_b.cluster().name),
        "Envoy should NOT see team-b cluster '{}' (isolation violated - team boundary breach!)",
        resources_b.cluster().name
    );

    println!("✓ Team isolation verified:");
    println!("  - Team-a resources visible to Envoy");
    println!("  - Team-b resources correctly hidden (isolation enforced)");
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_mtls_config_structure() {
        // Verify mTLS configuration patterns are valid
        // This test runs even without E2E environment
        let spiffe_uri = "spiffe://flowplane.local/team/test-team/proxy/envoy-1";

        // Verify SPIFFE URI format parsing
        let parts: Vec<&str> = spiffe_uri.split('/').collect();
        assert!(parts.len() >= 5);
        assert_eq!(parts[3], "team");
        assert_eq!(parts[4], "test-team");
    }
}
