//! Smoke test: Basic Routing
//!
//! Quick validation of routing:
//! - Create cluster → route → listener
//! - Verify proxy works through Envoy
//!
//! Expected time: ~15 seconds

use crate::common::{
    api_client::{setup_dev_context, simple_cluster, simple_listener, simple_route, ApiClient},
    harness::{TestHarness, TestHarnessConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Smoke test for basic routing: cluster → route → listener → proxy
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

    println!("🚀 Routing smoke test PASSED");
}
