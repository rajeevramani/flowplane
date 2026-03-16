//! Mock OIDC prod-mode E2E smoke tests
//!
//! Exercises the dual-mode harness in prod auth mode with a mock OIDC provider
//! (no Zitadel container). These tests run with `FLOWPLANE_E2E_AUTH_MODE=prod-mock`.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=prod-mock RUN_E2E=1 cargo test --test e2e test_mock_oidc -- --ignored --nocapture
//! ```

use crate::common::harness::dev_harness;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Make a raw HTTP request with a custom Authorization header value.
async fn raw_request(base_url: &str, path: &str, auth_header: Option<&str>) -> reqwest::Response {
    let url = format!("{}{}", base_url, path);
    let mut builder = reqwest::Client::new().get(&url);
    if let Some(auth) = auth_header {
        builder = builder.header("Authorization", auth);
    }
    builder.send().await.expect("HTTP request should not fail at transport level")
}

/// Make an authenticated DELETE request.
async fn authed_delete(base_url: &str, path: &str, token: &str) -> reqwest::Response {
    let url = format!("{}{}", base_url, path);
    reqwest::Client::new()
        .delete(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("HTTP DELETE should not fail at transport level")
}

// ===========================================================================
// Lifecycle: expose → verify resources → unexpose → verify cleanup
// ===========================================================================

/// Full expose/unexpose lifecycle through real HTTP with mock OIDC JWT.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=prod-mock"]
async fn mock_oidc_expose_lifecycle() {
    let harness = dev_harness("mock_oidc_expose_lifecycle").await.expect("harness should start");

    if harness.is_dev_mode() {
        eprintln!("SKIP: test requires prod mode (FLOWPLANE_E2E_AUTH_MODE=prod-mock)");
        return;
    }

    let base = harness.api_url();
    let token = &harness.auth_token;
    let team = &harness.team;

    // Expose a service
    let body = serde_json::json!({
        "name": "e2e-mock-oidc-svc",
        "upstream": "http://127.0.0.1:9999"
    });
    let resp = reqwest::Client::new()
        .post(format!("{}/api/v1/teams/{}/expose", base, team))
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await
        .expect("POST should succeed at transport level");

    assert!(
        resp.status() == reqwest::StatusCode::CREATED || resp.status() == reqwest::StatusCode::OK,
        "expose should return 201 or 200 (idempotent), got {}",
        resp.status()
    );
    let json: serde_json::Value = resp.json().await.expect("response should be JSON");
    assert_eq!(json["name"], "e2e-mock-oidc-svc");
    assert!(json["port"].as_u64().is_some(), "response should include a port");

    // Verify the cluster was created
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{}/clusters/e2e-mock-oidc-svc", team))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "cluster should exist after expose");

    // Verify the route-config was created
    let resp = harness
        .authed_get(&format!(
            "/api/v1/teams/{}/route-configs/e2e-mock-oidc-svc-routes",
            team
        ))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "route-config should exist after expose"
    );

    // Verify the listener was created
    let resp = harness
        .authed_get(&format!(
            "/api/v1/teams/{}/listeners/e2e-mock-oidc-svc-listener",
            team
        ))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "listener should exist after expose"
    );

    // Unexpose
    let resp = authed_delete(
        &base,
        &format!("/api/v1/teams/{}/expose/e2e-mock-oidc-svc", team),
        token,
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NO_CONTENT,
        "unexpose should return 204"
    );

    // Verify cleanup — cluster should be gone
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{}/clusters/e2e-mock-oidc-svc", team))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "cluster should be gone after unexpose"
    );

    // Verify cleanup — route-config should be gone
    let resp = harness
        .authed_get(&format!(
            "/api/v1/teams/{}/route-configs/e2e-mock-oidc-svc-routes",
            team
        ))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "route-config should be gone after unexpose"
    );

    // Verify cleanup — listener should be gone
    let resp = harness
        .authed_get(&format!(
            "/api/v1/teams/{}/listeners/e2e-mock-oidc-svc-listener",
            team
        ))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "listener should be gone after unexpose"
    );
}

// ===========================================================================
// Auth negative cases (mock OIDC prod mode)
// ===========================================================================

/// No Authorization header → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=prod-mock"]
async fn mock_oidc_no_auth_header_returns_401() {
    let harness = dev_harness("mock_oidc_no_auth_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let resp =
        raw_request(&harness.api_url(), &format!("/api/v1/teams/{}/clusters", harness.team), None)
            .await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED, "no auth → 401");

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// Random bearer token (not a valid JWT) → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=prod-mock"]
async fn mock_oidc_random_bearer_returns_401() {
    let harness = dev_harness("mock_oidc_random_bearer_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some("Bearer this-is-not-a-jwt-at-all"),
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "random bearer token should be rejected"
    );

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// Malformed JWT (wrong number of parts — only 2 dot-separated segments) → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=prod-mock"]
async fn mock_oidc_malformed_jwt_returns_401() {
    let harness = dev_harness("mock_oidc_malformed_jwt_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    // Only 2 dot-separated parts instead of 3
    let malformed = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0";
    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some(&format!("Bearer {}", malformed)),
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "malformed JWT (2 parts) should be rejected"
    );

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// JWT with valid 3-part structure but signed with wrong key → 401
///
/// BUG CANDIDATE: A server that only decodes without verifying the signature
/// would accept this token. This test ensures signature validation is enforced.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=prod-mock"]
async fn mock_oidc_jwt_wrong_signature_returns_401() {
    let harness = dev_harness("mock_oidc_wrong_sig_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    // Well-formed JWT with a completely fabricated signature (not from our mock OIDC key)
    let forged = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.\
                  eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0.\
                  SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some(&format!("Bearer {}", forged)),
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "JWT signed with wrong key should be rejected"
    );

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

// ===========================================================================
// Mode verification
// ===========================================================================

/// Auth mode endpoint should report "prod" in mock-OIDC mode.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=prod-mock"]
async fn mock_oidc_auth_mode_reports_prod() {
    let harness = dev_harness("mock_oidc_auth_mode_check").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let resp = raw_request(&harness.api_url(), "/api/v1/auth/mode", None).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.expect("should be JSON");
    assert_eq!(json["auth_mode"], "prod", "auth mode should be 'prod' in mock-OIDC mode");
}

/// Health endpoint works without auth even in prod mock-OIDC mode.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=prod-mock"]
async fn mock_oidc_health_no_auth() {
    let harness = dev_harness("mock_oidc_health_no_auth").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let resp = raw_request(&harness.api_url(), "/health", None).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "health should work without auth");
}
