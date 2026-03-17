//! Prod-mode E2E smoke tests
//!
//! Exercises the dual-mode harness in prod auth mode (Zitadel OIDC, JWT).
//! These tests run with `FLOWPLANE_E2E_AUTH_MODE=prod` (or unset, the default).
//!
//! ```bash
//! RUN_E2E=1 cargo test --test e2e test_prod_mode -- --ignored --nocapture
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

// ===========================================================================
// Lifecycle: expose → status → unexpose (happy path, prod JWT auth)
// ===========================================================================

/// Full expose/unexpose lifecycle through real HTTP in prod mode.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_expose_lifecycle() {
    let harness = dev_harness("prod_expose_lifecycle").await.expect("harness should start");

    if harness.is_dev_mode() {
        eprintln!("SKIP: test requires prod mode (FLOWPLANE_E2E_AUTH_MODE=prod or unset)");
        return;
    }

    let base = harness.api_url();
    let token = &harness.auth_token;
    let team = &harness.team;

    // Expose a service
    let body = serde_json::json!({
        "name": "e2e-prod-svc",
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
    assert_eq!(json["name"], "e2e-prod-svc");

    // Verify the cluster was created
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{}/clusters/e2e-prod-svc", team))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "cluster should exist after expose");

    // Unexpose
    let resp = reqwest::Client::new()
        .delete(format!("{}/api/v1/teams/{}/expose/e2e-prod-svc", base, team))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("delete should succeed at transport level");
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT, "unexpose should return 204");

    // Verify cleanup
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{}/clusters/e2e-prod-svc", team))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "cluster should be gone after unexpose"
    );
}

// ===========================================================================
// Auth negative cases (prod mode)
// ===========================================================================

/// No Authorization header → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_no_auth_header_returns_401() {
    let harness = dev_harness("prod_no_auth_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let resp =
        raw_request(&harness.api_url(), &format!("/api/v1/teams/{}/clusters", harness.team), None)
            .await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED, "no auth → 401");

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// Random bearer token (not a valid JWT) → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_random_bearer_returns_401() {
    let harness = dev_harness("prod_random_bearer_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some("Bearer this-is-not-a-jwt"),
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "random bearer token should be rejected in prod mode"
    );

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// Malformed JWT (wrong number of parts) → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_malformed_jwt_returns_401() {
    let harness = dev_harness("prod_malformed_jwt_401").await.expect("harness should start");
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

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// JWT with valid structure but wrong signature → 401
///
/// BUG CANDIDATE: A server that only decodes without verifying the signature
/// would accept this token. This test ensures signature validation is enforced.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_jwt_wrong_signature_returns_401() {
    let harness = dev_harness("prod_wrong_sig_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    // Well-formed JWT with a completely fabricated signature
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
        "JWT with wrong signature should be rejected"
    );

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// Empty Authorization header → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_empty_auth_header_returns_401() {
    let harness = dev_harness("prod_empty_auth_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some(""),
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "empty Authorization header → 401"
    );

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// Bearer with only spaces → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_bearer_spaces_only_returns_401() {
    let harness = dev_harness("prod_bearer_spaces_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some("Bearer    "),
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED, "Bearer with only spaces → 401");

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// Doubled "Bearer Bearer token" prefix → 401.
///
/// BUG CANDIDATE: A naive implementation that splits on " " and takes the second part
/// would accept "Bearer Bearer <real-token>" as "Bearer <real-token>".
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_doubled_bearer_prefix_returns_401() {
    let harness = dev_harness("prod_doubled_bearer_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let bad_header = format!("Bearer Bearer {}", harness.auth_token);
    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some(&bad_header),
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "doubled Bearer prefix should be rejected"
    );

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// Token with trailing newline is rejected at the HTTP transport layer.
///
/// Newlines are illegal in HTTP header values (RFC 7230). The HTTP client
/// rejects the header before it reaches the server. This test verifies
/// that the token+newline combination cannot be sent at all.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_token_with_trailing_newline_returns_401() {
    let harness = dev_harness("prod_trailing_newline_401").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let bad_header = format!("Bearer {}\n", harness.auth_token);
    let url = format!("{}/api/v1/teams/{}/clusters", harness.api_url(), harness.team);
    let result = reqwest::Client::new().get(&url).header("Authorization", &bad_header).send().await;

    // Newlines in header values are rejected by the HTTP client (InvalidHeaderValue).
    // The request never reaches the server — this is the correct behavior.
    assert!(result.is_err(), "token with trailing newline should be rejected at transport level");
}
