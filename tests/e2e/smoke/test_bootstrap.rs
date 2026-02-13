//! Smoke test: Bootstrap and Authentication
//!
//! Quick validation of the auth flow:
//! - Bootstrap → login → PAT creation
//!
//! Expected time: ~10 seconds

use crate::common::{
    api_client::{ApiClient, TEST_EMAIL, TEST_NAME, TEST_PASSWORD},
    harness::{TestHarness, TestHarnessConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Smoke test for auth flow: bootstrap → login → create PAT
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn smoke_test_auth_flow() {
    // Start minimal harness (no Envoy needed for auth tests)
    let harness =
        TestHarness::start(TestHarnessConfig::new("smoke_test_auth_flow").without_envoy())
            .await
            .expect("Failed to start test harness");

    let api = ApiClient::new(harness.api_url());

    // Check if bootstrap is needed (idempotent for shared infrastructure)
    let needs_bootstrap = with_timeout(TestTimeout::quick("Check bootstrap status"), async {
        api.needs_bootstrap().await
    })
    .await
    .unwrap_or(true);

    // 1. Bootstrap (only if needed - uses standard test credentials)
    if needs_bootstrap {
        let bootstrap = with_timeout(TestTimeout::quick("Bootstrap"), async {
            api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME).await
        })
        .await
        .expect("Bootstrap should succeed");

        assert!(
            bootstrap.setup_token.starts_with("fp_setup_"),
            "Setup token should have correct prefix"
        );
    }
    println!("✓ Bootstrap complete");

    // 2. Login (uses standard test credentials)
    let (session, login_resp) = with_timeout(TestTimeout::quick("Login"), async {
        api.login_full(TEST_EMAIL, TEST_PASSWORD).await
    })
    .await
    .expect("Login should succeed");

    assert!(!session.csrf_token.is_empty(), "CSRF token should be present");

    // Verify org context from login (bootstrap creates platform org)
    assert!(login_resp.org_id.is_some(), "Login should include org_id after bootstrap");
    assert!(login_resp.org_name.is_some(), "Login should include org_name after bootstrap");
    println!("✓ Login complete");

    // 3. Create PAT
    let token = with_timeout(TestTimeout::quick("Create PAT"), async {
        api.create_token(&session, "smoke-token", vec!["admin:all".to_string()]).await
    })
    .await
    .expect("Token creation should succeed");

    assert!(token.token.starts_with("fp_pat_"), "PAT should have correct prefix");
    println!("✓ PAT created");

    println!("🚀 Auth smoke test PASSED");
}
