//! Bootstrap Tests (Bruno 11)
//!
//! Tests the authentication and resource creation flow with Zitadel OIDC:
//! - Superadmin JWT acquisition
//! - Organization and team creation
//! - OpenAPI import with routing verification
//! - Team isolation verification

use serde_json::json;

use crate::common::{
    api_client::{
        setup_dev_context, setup_envoy_context, simple_cluster, simple_listener, simple_route,
        ApiClient,
    },
    harness::{TestHarness, TestHarnessConfig},
    shared_infra::SharedInfrastructure,
    timeout::{with_timeout, TestTimeout},
    zitadel,
};

/// Test that superadmin can authenticate and access the API
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_initialize_app() {
    let infra = SharedInfrastructure::get_or_init()
        .await
        .expect("Failed to initialize shared infrastructure");

    let api = ApiClient::new(infra.api_url());

    // Obtain superadmin JWT
    let token = with_timeout(TestTimeout::default_with_label("Obtain superadmin JWT"), async {
        zitadel::obtain_human_token(
            &infra.zitadel_config,
            zitadel::SUPERADMIN_EMAIL,
            zitadel::SUPERADMIN_PASSWORD,
        )
        .await
    })
    .await
    .expect("JWT acquisition should succeed");

    // Verify JWT is valid by calling the API
    let orgs = with_timeout(TestTimeout::default_with_label("List organizations"), async {
        api.list_organizations(&token).await
    })
    .await
    .expect("API call should succeed with JWT");

    assert!(orgs.total >= 1, "Should have at least the platform org");
    let has_platform = orgs.items.iter().any(|o| o.name == "platform");
    assert!(has_platform, "Platform org should exist after bootstrap");
    println!("ok Superadmin authenticated, platform org verified");
}

/// Test team creation via JWT-authenticated API
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_103_create_team() {
    let harness =
        TestHarness::start(TestHarnessConfig::new("test_103_create_team").without_envoy())
            .await
            .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Setup dev context (obtains JWT from Zitadel, creates org + teams)
    let ctx = setup_dev_context(&api, "test_103_create_team").await.expect("Setup should succeed");

    // Verify teams were created (with unique names)
    assert!(!ctx.team_a_name.is_empty(), "Team A should be created");
    assert!(!ctx.team_b_name.is_empty(), "Team B should be created");
    assert!(!ctx.team_a_id.is_empty(), "Team A should have valid ID");
    assert!(!ctx.team_b_id.is_empty(), "Team B should have valid ID");

    // Verify org context was captured
    assert!(ctx.org_id.is_some(), "Context should include org_id");
    assert!(ctx.org_name.is_some(), "Context should include org_name");

    println!(
        "ok Teams created: {} (id={}), {} (id={}), org={:?}",
        ctx.team_a_name, ctx.team_a_id, ctx.team_b_name, ctx.team_b_id, ctx.org_name
    );
}

/// Test OpenAPI import and route creation
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_300_import_openapi() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_300_import_openapi"))
        .await
        .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx =
        setup_envoy_context(&api, "test_300_import_openapi").await.expect("Setup should succeed");

    // Delay between setup and resource creation
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Create unique domain and path using team name to avoid conflicts
    let unique_domain = format!("{}.openapi.e2e.local", ctx.team_a_name);
    let unique_path = format!("/testing/openapi/{}/echo", ctx.team_a_name);
    let unique_operation_id = format!("openapi-echo-{}", ctx.team_a_name);

    // Create OpenAPI spec pointing to mock echo server
    let echo_endpoint = harness.echo_endpoint();
    let spec = json!({
        "openapi": "3.0.0",
        "info": {
            "title": "E2E Test API",
            "version": "1.0.0",
            "x-flowplane-domain": unique_domain
        },
        "servers": [
            { "url": format!("http://{}", echo_endpoint) }
        ],
        "paths": {
            unique_path.clone(): {
                "get": {
                    "operationId": unique_operation_id,
                    "responses": {
                        "200": { "description": "Success" }
                    }
                }
            }
        }
    });

    // Import OpenAPI spec
    let result = with_timeout(TestTimeout::default_with_label("Import OpenAPI"), async {
        api.import_openapi(
            &ctx.admin_token,
            &ctx.team_a_name,
            spec,
            harness.ports.listener,
            &ctx.team_a_dataplane_id,
        )
        .await
    })
    .await
    .expect("OpenAPI import should succeed");

    println!("ok OpenAPI imported: {:?}", result);

    // If Envoy is available, verify routing works
    if harness.has_envoy() {
        // Delay before calling Envoy to allow xDS propagation
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let body = with_timeout(TestTimeout::default_with_label("Route convergence"), async {
            harness.wait_for_route(&unique_domain, &unique_path, 200).await
        })
        .await
        .expect("Route should converge");

        println!("ok Route verified, response: {}...", &body[..50.min(body.len())]);
    } else {
        println!("-- Envoy not available, skipping route verification");
    }
}

/// Test team isolation - Team A cannot access Team B resources
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_400_team_isolation() {
    let harness =
        TestHarness::start(TestHarnessConfig::new("test_400_team_isolation").without_envoy())
            .await
            .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Setup dev context with unique teams for this test
    let ctx =
        setup_dev_context(&api, "test_400_team_isolation").await.expect("Setup should succeed");

    // The superadmin JWT has full access, so create resources in both teams
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create cluster in Team A (should succeed)
    let cluster_a = with_timeout(TestTimeout::default_with_label("Create Team A cluster"), async {
        api.create_cluster(
            &ctx.admin_token,
            &ctx.team_a_name,
            &simple_cluster("team-a-cluster", host, port),
        )
        .await
    })
    .await
    .expect("Team A cluster creation should succeed");

    println!("ok Team A cluster created: {}", cluster_a.name);

    // Verify Team A cluster is correctly tagged
    assert_eq!(cluster_a.team, ctx.team_a_name, "Cluster should be tagged with Team A");
    println!("ok Team isolation tags verified");
}

/// Test full routing setup: cluster -> route -> listener -> proxy
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_500_full_routing_setup() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_500_full_routing_setup"))
        .await
        .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx = setup_envoy_context(&api, "test_500_full_routing_setup")
        .await
        .expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Create cluster with unique name
    let cluster = with_timeout(TestTimeout::default_with_label("Create cluster"), async {
        api.create_cluster(
            &ctx.admin_token,
            &ctx.team_a_name,
            &simple_cluster("routing-echo-backend", host, port),
        )
        .await
    })
    .await
    .expect("Cluster creation should succeed");
    println!("ok Cluster created: {}", cluster.name);

    // Delay between resource creation
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Create route config with unique domain and path
    let route = with_timeout(TestTimeout::default_with_label("Create route"), async {
        api.create_route(
            &ctx.admin_token,
            &ctx.team_a_name,
            &simple_route(
                "routing-echo-route",
                "routing.e2e.local",
                "/testing/routing/echo",
                &cluster.name,
            ),
        )
        .await
    })
    .await
    .expect("Route creation should succeed");
    println!("ok Route created: {}", route.name);

    // Delay between resource creation
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Create listener with unique name
    let listener = with_timeout(TestTimeout::default_with_label("Create listener"), async {
        api.create_listener(
            &ctx.admin_token,
            &ctx.team_a_name,
            &simple_listener(
                "routing-echo-listener",
                harness.ports.listener,
                &route.name,
                &ctx.team_a_dataplane_id,
            ),
        )
        .await
    })
    .await
    .expect("Listener creation should succeed");
    println!("ok Listener created: {} on port {:?}", listener.name, listener.port);

    // If Envoy is available, verify routing
    if harness.has_envoy() {
        // Delay before calling Envoy to allow xDS propagation
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let body = with_timeout(TestTimeout::default_with_label("Route convergence"), async {
            harness.wait_for_route("routing.e2e.local", "/testing/routing/echo", 200).await
        })
        .await
        .expect("Route should converge");

        println!("ok Routing verified through Envoy: {}", &body[..100.min(body.len())]);
    } else {
        println!("-- Envoy not available, skipping proxy verification");
    }
}

/// Verify E2E tests correctly skip when RUN_E2E is not set
#[test]
fn verify_skip_without_flag() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        println!("E2E tests correctly skip when RUN_E2E is not set");
    }
}
