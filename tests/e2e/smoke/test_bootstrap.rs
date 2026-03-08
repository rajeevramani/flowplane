//! Smoke test: Zitadel OIDC Authentication
//!
//! Quick validation of the Zitadel auth flow:
//! - Superadmin JWT acquisition via Session API
//! - API access with JWT Bearer token
//!
//! Expected time: ~10 seconds (after shared infra warm-up)

use crate::common::{
    api_client::ApiClient,
    shared_infra::SharedInfrastructure,
    timeout::{with_timeout, TestTimeout},
    zitadel,
};

/// Smoke test for Zitadel auth flow: obtain JWT → call API
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn smoke_test_auth_flow() {
    // Get shared infrastructure (starts PG + Zitadel + CP if not already running)
    let infra = SharedInfrastructure::get_or_init()
        .await
        .expect("Failed to initialize shared infrastructure");

    let api = ApiClient::new(infra.api_url());

    // 1. Obtain superadmin JWT from Zitadel
    let token = with_timeout(TestTimeout::quick("Obtain superadmin JWT"), async {
        zitadel::obtain_human_token(
            &infra.zitadel_config,
            zitadel::SUPERADMIN_EMAIL,
            zitadel::SUPERADMIN_PASSWORD,
        )
        .await
    })
    .await
    .expect("JWT token acquisition should succeed");

    assert!(!token.is_empty(), "JWT token should not be empty");
    // JWT tokens have 3 dot-separated parts
    assert_eq!(token.split('.').count(), 3, "Token should be a valid JWT (3 parts)");
    println!("ok JWT token obtained ({} chars)", token.len());

    // 2. Verify API access with JWT token - list organizations
    let orgs = with_timeout(TestTimeout::quick("List organizations"), async {
        api.list_organizations(&token).await
    })
    .await
    .expect("API call with JWT should succeed");

    assert!(orgs.total >= 1, "Should have at least 1 org (platform)");
    println!("ok API access verified: {} organizations", orgs.total);

    println!("Auth smoke test PASSED");
}
