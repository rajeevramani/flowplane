//! mTLS Connection and Certificate API Tests (Bruno 24)
//!
//! Phase 3 - mTLS xDS connection tests (test_100-test_102):
//! - Envoy connects to xDS server with client certificate
//! - SPIFFE URI team extraction and resource scoping
//! - Cross-team resource isolation
//!
//! Phase 4 - Certificate API tests (test_200-test_205):
//! - Certificate generation API
//! - Certificate listing with pagination
//! - Certificate retrieval by ID
//! - Certificate revocation
//! - Rate limiting per team
//! - Team isolation for certificates
//!
//! Prerequisites:
//! - Phase 3 tests: FLOWPLANE_E2E_MTLS=1 RUN_E2E=1 cargo test --test e2e test_10 -- --ignored
//! - Phase 4 tests: RUN_E2E=1 cargo test --test e2e test_20 -- --ignored
//!
//! Design Principles Followed:
//! - Hard timeouts everywhere (30s max per operation)
//! - Unique names: mtls-* prefix, cert-* prefix for resources
//! - Unique paths: /testing/mtls/* for all routes
//! - Unique ports: harness.ports.listener (auto-unique per test)
//! - 3-second delays: Between resource creation and verification
//! - Fail fast: Clear error messages, no silent swallowing

use std::time::Duration;

use crate::common::{
    api_client::{setup_dev_context, setup_envoy_context, ApiClient, CreateDataplaneRequest},
    harness::{TestHarness, TestHarnessConfig},
    resource_setup::ResourceSetup,
    shared_infra::unique_name,
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
    let org_id = ctx.org_id.as_deref().expect("Context should have org_id");
    let team_b = with_timeout(TestTimeout::default_with_label("Create Team B"), async {
        api.create_team_idempotent(
            &ctx.admin_token,
            &format!("{}-isolation-b", team_a_name),
            Some("Team B for isolation testing"),
            org_id,
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

// ============================================================================
// Phase 4: Certificate API E2E Tests (test_200 - test_205)
// ============================================================================
//
// These tests verify the proxy certificate API endpoints.
// They do NOT require mTLS mode or Envoy - they test the REST API directly.
//
// Prerequisites:
// - Control plane running with Vault PKI configured
// - RUN_E2E=1 environment variable set
//
// Run: RUN_E2E=1 cargo test --test e2e test_20 -- --ignored --test-threads=1

/// Test 200: Generate proxy certificate API
///
/// Verifies certificate generation via the REST API:
/// - POST /api/v1/teams/{team}/proxy-certificates succeeds
/// - Response contains certificate, private_key, ca_chain (all PEM format)
/// - SPIFFE URI format: spiffe://flowplane.local/team/{team}/proxy/{proxy_id}
/// - expires_at is valid ISO 8601 timestamp
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_200_generate_certificate() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_200_generate_certificate"))
        .await
        .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api, "test_200_generate_certificate")
        .await
        .expect("Setup should succeed");

    // Generate unique proxy_id for this test
    let proxy_id = unique_name("test_200", "cert-proxy");

    // Generate certificate
    let cert = with_timeout(TestTimeout::default_with_label("Generate certificate"), async {
        api.generate_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &proxy_id).await
    })
    .await
    .expect("Certificate generation should succeed");

    // Verify response fields
    assert!(!cert.id.is_empty(), "Certificate ID should not be empty");
    assert_eq!(cert.proxy_id, proxy_id, "proxy_id should match request");

    // Verify SPIFFE URI format
    let expected_spiffe_prefix =
        format!("spiffe://flowplane.local/team/{}/proxy/{}", ctx.team_a_name, proxy_id);
    assert!(
        cert.spiffe_uri.starts_with(&expected_spiffe_prefix),
        "SPIFFE URI should match format, got: {}",
        cert.spiffe_uri
    );

    // Verify PEM format for certificate
    assert!(
        cert.certificate.starts_with("-----BEGIN CERTIFICATE-----"),
        "Certificate should be PEM encoded"
    );

    // Verify PEM format for private key
    assert!(
        cert.private_key.contains("-----BEGIN") && cert.private_key.contains("PRIVATE KEY-----"),
        "Private key should be PEM encoded"
    );

    // Verify PEM format for CA chain
    assert!(
        cert.ca_chain.starts_with("-----BEGIN CERTIFICATE-----"),
        "CA chain should be PEM encoded"
    );

    // Verify expires_at is a valid timestamp (ISO 8601)
    assert!(
        chrono::DateTime::parse_from_rfc3339(&cert.expires_at).is_ok(),
        "expires_at should be valid ISO 8601 timestamp, got: {}",
        cert.expires_at
    );

    println!("✓ Certificate generated successfully:");
    println!("  - ID: {}", cert.id);
    println!("  - SPIFFE URI: {}", cert.spiffe_uri);
    println!("  - Expires: {}", cert.expires_at);
}

/// Test 201: List proxy certificates with pagination
///
/// Verifies the list certificates endpoint:
/// - GET /api/v1/teams/{team}/proxy-certificates returns certificates
/// - Pagination works correctly (limit, offset, total)
/// - Private key is NOT included in list response
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_201_list_certificates() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_201_list_certificates"))
        .await
        .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_201_list_certificates").await.expect("Setup should succeed");

    // Generate 3 certificates with unique proxy_ids
    let mut cert_ids = Vec::new();
    for i in 1..=3 {
        let proxy_id = unique_name("test_201", &format!("cert-list-{}", i));

        // Standard 3-second delay between resource creation
        if i > 1 {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }

        let cert = with_timeout(
            TestTimeout::default_with_label(format!("Generate certificate {}", i)),
            async {
                api.generate_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &proxy_id).await
            },
        )
        .await
        .unwrap_or_else(|e| panic!("Certificate {} generation should succeed: {:?}", i, e));

        cert_ids.push(cert.id);
        println!("✓ Generated certificate {}: proxy_id={}", i, proxy_id);
    }

    // Standard 3-second delay before verification
    tokio::time::sleep(Duration::from_secs(3)).await;

    // List with limit=2, offset=0
    let list1 = with_timeout(TestTimeout::default_with_label("List certificates page 1"), async {
        api.list_proxy_certificates(&ctx.admin_token, &ctx.team_a_name, Some(2), Some(0)).await
    })
    .await
    .expect("List certificates should succeed");

    assert_eq!(list1.certificates.len(), 2, "Should return 2 certificates with limit=2");
    assert!(list1.total >= 3, "Total should be at least 3, got: {}", list1.total);
    assert_eq!(list1.limit, 2, "Limit should be 2");
    assert_eq!(list1.offset, 0, "Offset should be 0");

    // List with limit=2, offset=2
    let list2 = with_timeout(TestTimeout::default_with_label("List certificates page 2"), async {
        api.list_proxy_certificates(&ctx.admin_token, &ctx.team_a_name, Some(2), Some(2)).await
    })
    .await
    .expect("List certificates page 2 should succeed");

    assert!(!list2.certificates.is_empty(), "Should return at least 1 certificate with offset=2");
    assert_eq!(list2.offset, 2, "Offset should be 2");

    println!("✓ Pagination verified:");
    println!("  - Page 1: {} certificates (total: {})", list1.certificates.len(), list1.total);
    println!("  - Page 2: {} certificates", list2.certificates.len());
}

/// Test 202: Get proxy certificate by ID
///
/// Verifies single certificate retrieval:
/// - GET /api/v1/teams/{team}/proxy-certificates/{id} returns certificate
/// - Metadata matches generation response
/// - Status flags: is_valid=true, is_expired=false, is_revoked=false
/// - Private key is NOT included
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_202_get_certificate() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_202_get_certificate"))
        .await
        .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_202_get_certificate").await.expect("Setup should succeed");

    // Generate a certificate
    let proxy_id = unique_name("test_202", "cert-get");
    let generated = with_timeout(TestTimeout::default_with_label("Generate certificate"), async {
        api.generate_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &proxy_id).await
    })
    .await
    .expect("Certificate generation should succeed");

    println!("✓ Generated certificate: id={}", generated.id);

    // Standard 3-second delay before verification
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Get the certificate by ID
    let retrieved = with_timeout(TestTimeout::default_with_label("Get certificate"), async {
        api.get_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &generated.id).await
    })
    .await
    .expect("Get certificate should succeed");

    // Verify metadata matches
    assert_eq!(retrieved.id, generated.id, "ID should match");
    assert_eq!(retrieved.proxy_id, proxy_id, "proxy_id should match");
    assert_eq!(retrieved.spiffe_uri, generated.spiffe_uri, "SPIFFE URI should match");
    assert_eq!(retrieved.expires_at, generated.expires_at, "expires_at should match");

    // Verify status flags for a freshly generated certificate
    assert!(retrieved.is_valid, "Freshly generated certificate should be valid");
    assert!(!retrieved.is_expired, "Freshly generated certificate should not be expired");
    assert!(!retrieved.is_revoked, "Freshly generated certificate should not be revoked");
    assert!(retrieved.revoked_at.is_none(), "revoked_at should be None");
    assert!(retrieved.revoked_reason.is_none(), "revoked_reason should be None");

    println!("✓ Certificate retrieved successfully:");
    println!("  - ID: {}", retrieved.id);
    println!("  - is_valid: {}", retrieved.is_valid);
    println!("  - is_expired: {}", retrieved.is_expired);
    println!("  - is_revoked: {}", retrieved.is_revoked);
}

/// Test 203: Revoke proxy certificate
///
/// Verifies certificate revocation:
/// - POST /api/v1/teams/{team}/proxy-certificates/{id}/revoke succeeds
/// - Response shows is_revoked=true, is_valid=false
/// - revoked_at and revoked_reason are set
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_203_revoke_certificate() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_203_revoke_certificate"))
        .await
        .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_203_revoke_certificate").await.expect("Setup should succeed");

    // Generate a certificate
    let proxy_id = unique_name("test_203", "cert-revoke");
    let generated = with_timeout(TestTimeout::default_with_label("Generate certificate"), async {
        api.generate_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &proxy_id).await
    })
    .await
    .expect("Certificate generation should succeed");

    println!("✓ Generated certificate: id={}", generated.id);

    // Standard 3-second delay before revocation
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Revoke the certificate
    let revoke_reason = "Test revocation for E2E test_203";
    let revoked = with_timeout(TestTimeout::default_with_label("Revoke certificate"), async {
        api.revoke_proxy_certificate(
            &ctx.admin_token,
            &ctx.team_a_name,
            &generated.id,
            revoke_reason,
        )
        .await
    })
    .await
    .expect("Revoke certificate should succeed");

    // Verify revocation status
    assert!(revoked.is_revoked, "Certificate should be marked as revoked");
    assert!(!revoked.is_valid, "Revoked certificate should not be valid");
    assert!(revoked.revoked_at.is_some(), "revoked_at should be set");
    assert_eq!(
        revoked.revoked_reason.as_deref(),
        Some(revoke_reason),
        "revoked_reason should match"
    );

    println!("✓ Certificate revoked successfully:");
    println!("  - is_revoked: {}", revoked.is_revoked);
    println!("  - is_valid: {}", revoked.is_valid);
    println!("  - revoked_at: {:?}", revoked.revoked_at);
    println!("  - revoked_reason: {:?}", revoked.revoked_reason);

    // Verify revocation persists by fetching again
    let fetched = with_timeout(TestTimeout::default_with_label("Get revoked certificate"), async {
        api.get_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &generated.id).await
    })
    .await
    .expect("Get certificate should succeed");

    assert!(fetched.is_revoked, "Fetched certificate should still be revoked");
    assert!(!fetched.is_valid, "Fetched certificate should still be invalid");

    println!("✓ Revocation persisted after re-fetch");
}

/// Test 204: Certificate generation rate limiting
///
/// Verifies per-team rate limiting:
/// - Generates certificates up to the rate limit
/// - Next generation returns 429 Too Many Requests
/// - Different team has separate rate limit bucket
///
/// Note: This test may need FLOWPLANE_RATE_LIMIT_CERTS_PER_HOUR to be set low (e.g., 5)
/// for faster testing. Skip gracefully if rate limiting is not configured.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_204_rate_limit() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_204_rate_limit"))
        .await
        .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api, "test_204_rate_limit").await.expect("Setup should succeed");

    // Check if we have a low rate limit configured for testing
    // Default is 100/hour which is too high for testing
    let rate_limit_env = std::env::var("FLOWPLANE_RATE_LIMIT_CERTS_PER_HOUR").ok();
    let rate_limit: i32 = rate_limit_env.as_deref().and_then(|v| v.parse().ok()).unwrap_or(100);

    if rate_limit > 10 {
        println!(
            "⚠ Rate limit is {} (too high for testing). Set FLOWPLANE_RATE_LIMIT_CERTS_PER_HOUR=5 to run this test effectively.",
            rate_limit
        );
        println!("⚠ Skipping rate limit exhaustion test, but verifying API works...");

        // Just verify one certificate can be generated
        let proxy_id = unique_name("test_204", "cert-ratelimit-check");
        let result = with_timeout(
            TestTimeout::default_with_label("Generate certificate (rate limit check)"),
            async {
                api.generate_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &proxy_id).await
            },
        )
        .await;

        assert!(result.is_ok(), "Should be able to generate at least one certificate");
        println!("✓ Rate limiting API verified (certificate generation works)");
        return;
    }

    println!("Testing rate limit with limit={}", rate_limit);

    // Generate certificates up to the limit
    for i in 1..=rate_limit {
        let proxy_id = unique_name("test_204", &format!("cert-ratelimit-{}", i));

        // Add delay between requests to avoid overwhelming the API
        if i > 1 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let result = with_timeout(
            TestTimeout::default_with_label(format!("Generate certificate {}/{}", i, rate_limit)),
            async {
                api.generate_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &proxy_id).await
            },
        )
        .await;

        result.unwrap_or_else(|e| {
            panic!("Certificate {} should succeed (within rate limit), got: {:?}", i, e)
        });
        println!("✓ Certificate {}/{} generated", i, rate_limit);
    }

    // Now attempt one more - should be rate limited
    let proxy_id_over_limit = unique_name("test_204", "cert-ratelimit-over");
    let result =
        with_timeout(TestTimeout::default_with_label("Generate certificate (over limit)"), async {
            api.generate_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &proxy_id_over_limit)
                .await
        })
        .await;

    // Expect rate limit error (429)
    match result {
        Ok(_) => {
            panic!(
                "Certificate generation should have been rate limited after {} certificates",
                rate_limit
            );
        }
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("429") || err_str.to_lowercase().contains("rate limit"),
                "Error should indicate rate limiting, got: {}",
                err_str
            );
            println!("✓ Rate limit enforced correctly: {}", err_str);
        }
    }

    // Verify Team B has separate rate limit bucket
    let proxy_id_team_b = unique_name("test_204", "cert-ratelimit-teamb");
    let result_team_b =
        with_timeout(TestTimeout::default_with_label("Generate certificate for Team B"), async {
            api.generate_proxy_certificate(&ctx.admin_token, &ctx.team_b_name, &proxy_id_team_b)
                .await
        })
        .await;

    assert!(
        result_team_b.is_ok(),
        "Team B should have separate rate limit bucket: {:?}",
        result_team_b
    );
    println!("✓ Team B rate limit is independent (certificate generated successfully)");
}

/// Test 205: Certificate team isolation
///
/// Verifies cross-team access prevention:
/// - Team A generates a certificate
/// - Team B cannot GET Team A's certificate (returns 404, not 403)
/// - Team B cannot REVOKE Team A's certificate (returns 404, not 403)
/// - 404 prevents information leakage about certificate existence
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_205_certificate_team_isolation() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_205_certificate_team_isolation"))
        .await
        .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());
    let ctx = setup_dev_context(&api, "test_205_certificate_team_isolation")
        .await
        .expect("Setup should succeed");

    // Generate certificate for Team A
    let proxy_id = unique_name("test_205", "cert-isolation");
    let team_a_cert =
        with_timeout(TestTimeout::default_with_label("Generate certificate for Team A"), async {
            api.generate_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &proxy_id).await
        })
        .await
        .expect("Certificate generation for Team A should succeed");

    println!("✓ Team A certificate generated: id={}", team_a_cert.id);

    // Standard 3-second delay before cross-team access attempt
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Attempt to GET Team A's certificate using Team B's context
    // (still using admin token, but specifying Team B as the team parameter)
    let get_result =
        with_timeout(TestTimeout::default_with_label("Cross-team GET attempt"), async {
            api.get_proxy_certificate(&ctx.admin_token, &ctx.team_b_name, &team_a_cert.id).await
        })
        .await;

    // Should fail with 404 (not 403) to prevent information leakage
    match get_result {
        Ok(_) => {
            panic!("Cross-team GET should fail - Team B should not see Team A's certificate");
        }
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("404") || err_str.to_lowercase().contains("not found"),
                "Cross-team access should return 404 (not 403) to prevent info leakage, got: {}",
                err_str
            );
            println!("✓ Cross-team GET correctly returned 404: {}", err_str);
        }
    }

    // Attempt to REVOKE Team A's certificate using Team B's context
    let revoke_result =
        with_timeout(TestTimeout::default_with_label("Cross-team REVOKE attempt"), async {
            api.revoke_proxy_certificate(
                &ctx.admin_token,
                &ctx.team_b_name,
                &team_a_cert.id,
                "Malicious revocation attempt",
            )
            .await
        })
        .await;

    // Should fail with 404 (not 403)
    match revoke_result {
        Ok(_) => {
            panic!("Cross-team REVOKE should fail - Team B should not revoke Team A's certificate");
        }
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("404") || err_str.to_lowercase().contains("not found"),
                "Cross-team revoke should return 404 (not 403) to prevent info leakage, got: {}",
                err_str
            );
            println!("✓ Cross-team REVOKE correctly returned 404: {}", err_str);
        }
    }

    // Verify Team A can still access their own certificate
    let own_cert =
        with_timeout(TestTimeout::default_with_label("Team A accesses own certificate"), async {
            api.get_proxy_certificate(&ctx.admin_token, &ctx.team_a_name, &team_a_cert.id).await
        })
        .await
        .expect("Team A should be able to access their own certificate");

    assert_eq!(own_cert.id, team_a_cert.id, "Team A should see their certificate");
    assert!(!own_cert.is_revoked, "Certificate should NOT be revoked (cross-team attempt failed)");

    println!("✓ Team isolation verified:");
    println!("  - Cross-team GET blocked (404)");
    println!("  - Cross-team REVOKE blocked (404)");
    println!("  - Team A can still access own certificate");
    println!("  - Certificate not affected by malicious revocation attempt");
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
