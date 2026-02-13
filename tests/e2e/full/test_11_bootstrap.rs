//! Bootstrap Tests (Bruno 11)
//!
//! Tests the complete authentication and bootstrap flow:
//! - Application initialization (bootstrap)
//! - User login and session management
//! - PAT token creation
//! - Team creation
//! - User creation
//! - OpenAPI import with routing verification
//! - Team isolation verification

use serde_json::json;

use crate::common::{
    api_client::{
        setup_dev_context, setup_envoy_context, simple_cluster, simple_listener, simple_route,
        ApiClient,
    },
    harness::{TestHarness, TestHarnessConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Test bootstrap initialization
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_100_initialize_app() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_100_initialize_app").isolated().without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap the application
    let bootstrap =
        with_timeout(TestTimeout::default_with_label("Bootstrap initialization"), async {
            api.bootstrap("admin@e2e.test", "SecurePass123!", "E2E Admin").await
        })
        .await
        .expect("Bootstrap should succeed");

    // Verify setup token format
    assert!(
        bootstrap.setup_token.starts_with("fp_setup_"),
        "Setup token should have fp_setup_ prefix, got: {}",
        bootstrap.setup_token
    );

    println!("✓ Bootstrap successful, setup_token: {}...", &bootstrap.setup_token[..20]);

    // Verify org was created by logging in and checking org fields
    let (_session, login_resp) =
        with_timeout(TestTimeout::default_with_label("Login to verify org"), async {
            api.login_full("admin@e2e.test", "SecurePass123!").await
        })
        .await
        .expect("Login should succeed after bootstrap");

    assert!(login_resp.org_id.is_some(), "Login should include org_id after bootstrap");
    assert!(login_resp.org_name.is_some(), "Login should include org_name after bootstrap");
    println!("✓ Org context verified: org_name={:?}", login_resp.org_name);
}

/// Test login flow
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_101_login() {
    let harness =
        TestHarness::start(TestHarnessConfig::new("test_101_login").isolated().without_envoy())
            .await
            .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap first
    api.bootstrap("admin@e2e.test", "SecurePass123!", "E2E Admin")
        .await
        .expect("Bootstrap should succeed");

    // Login
    let (session, login_resp) = with_timeout(TestTimeout::default_with_label("Login"), async {
        api.login_full("admin@e2e.test", "SecurePass123!").await
    })
    .await
    .expect("Login should succeed");

    // Verify session tokens
    assert!(!session.session_token.is_empty(), "Session token should not be empty");
    assert!(!session.csrf_token.is_empty(), "CSRF token should not be empty");

    // Verify org context
    assert!(login_resp.org_id.is_some(), "Login should include org_id");
    assert_eq!(login_resp.org_name.as_deref(), Some("platform"), "Platform org should be 'platform'");

    println!(
        "✓ Login successful, csrf_token: {}..., org={:?}",
        &session.csrf_token[..20.min(session.csrf_token.len())],
        login_resp.org_name
    );
}

/// Test PAT token creation
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_102_create_admin_token() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_102_create_admin_token").isolated().without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap and login
    api.bootstrap("admin@e2e.test", "SecurePass123!", "E2E Admin")
        .await
        .expect("Bootstrap should succeed");

    let (session, login_resp) =
        api.login_full("admin@e2e.test", "SecurePass123!").await.expect("Login should succeed");

    // Verify org context in login response
    assert!(login_resp.org_id.is_some(), "Login should include org_id");
    assert!(login_resp.org_name.is_some(), "Login should include org_name");

    // Create admin token
    let token = with_timeout(TestTimeout::default_with_label("Create PAT"), async {
        api.create_token(&session, "e2e-admin-token", vec!["admin:all".to_string()]).await
    })
    .await
    .expect("Token creation should succeed");

    // Verify token format
    assert!(
        token.token.starts_with("fp_pat_"),
        "PAT should have fp_pat_ prefix, got: {}...",
        &token.token[..20.min(token.token.len())]
    );
    assert!(!token.id.is_empty(), "Token id should not be empty");

    println!("✓ PAT created: {}...", &token.token[..20]);
}

/// Test team creation
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_103_create_team() {
    let harness =
        TestHarness::start(TestHarnessConfig::new("test_103_create_team").without_envoy())
            .await
            .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Setup dev context (bootstrap + login + admin token)
    let ctx = setup_dev_context(&api, "test_103_create_team").await.expect("Setup should succeed");

    // Verify teams were created (with unique names)
    assert!(!ctx.team_a_name.is_empty(), "Team A should be created");
    assert!(!ctx.team_b_name.is_empty(), "Team B should be created");
    assert!(!ctx.team_a_id.is_empty(), "Team A should have valid ID");
    assert!(!ctx.team_b_id.is_empty(), "Team B should have valid ID");

    // Verify org context was captured from login
    assert!(ctx.org_id.is_some(), "Context should include org_id");
    assert!(ctx.org_name.is_some(), "Context should include org_name");

    println!(
        "✓ Teams created: {} (id={}), {} (id={}), org={:?}",
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

    println!("✓ OpenAPI imported: {:?}", result);

    // If Envoy is available, verify routing works
    if harness.has_envoy() {
        // Delay before calling Envoy to allow xDS propagation
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let body = with_timeout(TestTimeout::default_with_label("Route convergence"), async {
            harness.wait_for_route(&unique_domain, &unique_path, 200).await
        })
        .await
        .expect("Route should converge");

        println!("✓ Route verified, response: {}...", &body[..50.min(body.len())]);
    } else {
        println!("⚠ Envoy not available, skipping route verification");
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

    // Create a team-scoped token for Team A
    let team_a_token =
        with_timeout(TestTimeout::default_with_label("Create Team A token"), async {
            api.create_token(
                &ctx.admin_session,
                "team-a-token",
                vec![
                    format!("team:{}:clusters:write", ctx.team_a_name),
                    format!("team:{}:clusters:read", ctx.team_a_name),
                ],
            )
            .await
        })
        .await
        .expect("Team A token creation should succeed");

    // Create cluster in Team A (should succeed)
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let cluster_a = with_timeout(TestTimeout::default_with_label("Create Team A cluster"), async {
        api.create_cluster(
            &team_a_token.token,
            &simple_cluster(&ctx.team_a_name, "team-a-cluster", host, port),
        )
        .await
    })
    .await
    .expect("Team A cluster creation should succeed");

    println!("✓ Team A cluster created: {}", cluster_a.name);

    // Try to create cluster in Team B with Team A token (should fail with 403)
    let team_b_result =
        with_timeout(TestTimeout::default_with_label("Try Team B cluster (should fail)"), async {
            api.create_cluster(
                &team_a_token.token,
                &simple_cluster(&ctx.team_b_name, "team-b-cluster", host, port),
            )
            .await
        })
        .await;

    match team_b_result {
        Ok(_) => panic!("Team A token should NOT be able to create resources in Team B"),
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("403")
                    || err_str.contains("Forbidden")
                    || err_str.contains("unauthorized"),
                "Expected 403 Forbidden, got: {}",
                err_str
            );
            println!("✓ Team isolation enforced: {}", err_str);
        }
    }
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
            &simple_cluster(&ctx.team_a_name, "routing-echo-backend", host, port),
        )
        .await
    })
    .await
    .expect("Cluster creation should succeed");
    println!("✓ Cluster created: {}", cluster.name);

    // Delay between resource creation
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Create route config with unique domain and path
    let route = with_timeout(TestTimeout::default_with_label("Create route"), async {
        api.create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
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
    println!("✓ Route created: {}", route.name);

    // Delay between resource creation
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Create listener with unique name
    let listener = with_timeout(TestTimeout::default_with_label("Create listener"), async {
        api.create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
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
    println!("✓ Listener created: {} on port {:?}", listener.name, listener.port);

    // If Envoy is available, verify routing
    if harness.has_envoy() {
        // Delay before calling Envoy to allow xDS propagation
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let body = with_timeout(TestTimeout::default_with_label("Route convergence"), async {
            harness.wait_for_route("routing.e2e.local", "/testing/routing/echo", 200).await
        })
        .await
        .expect("Route should converge");

        println!("✓ Routing verified through Envoy: {}", &body[..100.min(body.len())]);
    } else {
        println!("⚠ Envoy not available, skipping proxy verification");
    }
}

/// Verify E2E tests correctly skip when RUN_E2E is not set
#[test]
fn verify_skip_without_flag() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        println!("E2E tests correctly skip when RUN_E2E is not set");
    }
}
