//! Adversarial smoke tests for fp-4n5 (unified dev/prod auth via dev OIDC mock).
//!
//! * Test a — a JWT signed by an unrelated mock OIDC instance must be rejected
//!   by the control plane, even though it has the same `sub`. Catches regressions
//!   where the CP would accept any signature from any mock, or where the dev OIDC
//!   mock injection on the CP gets dropped silently.
//!
//! * Test b — the harness-minted token validates against the CP's JWKS AND its
//!   `iss` claim points to a local mock URL. Together that proves fp-4n5's
//!   single-mock wiring: the harness's mock and the CP's mock are one instance.
//!
//! Test for subprocess credentials-handoff race is tracked as fp-5ho7 and
//! intentionally excluded here — needs a real `flowplane init` subprocess,
//! not the in-process `dev_harness`.

use crate::common::harness::dev_harness;
use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use flowplane::auth::dev_token::DEV_USER_SUB;
use flowplane::dev::oidc_server::{MockOidcConfig, MockOidcServer};

/// Decode a JWT payload segment without verifying the signature.
fn decode_jwt_payload(token: &str) -> Result<serde_json::Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(anyhow!("token does not have three segments: got {}", parts.len()));
    }
    let payload_bytes =
        URL_SAFE_NO_PAD.decode(parts[1]).context("payload segment was not valid base64url")?;
    let claims: serde_json::Value =
        serde_json::from_slice(&payload_bytes).context("payload was not valid JSON")?;
    Ok(claims)
}

async fn raw_get(base_url: &str, path: &str, bearer: &str) -> reqwest::Response {
    let url = format!("{}{}", base_url, path);
    reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {}", bearer))
        .send()
        .await
        .expect("HTTP request must not fail at transport level")
}

// ===========================================================================
// Test a — drift detection: foreign mock JWT must be rejected
// ===========================================================================

#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_fp4n5_drift_detection_rejects_foreign_mock_token() {
    let harness = dev_harness("fp4n5_drift_detection").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let base = harness.api_url();
    let team = &harness.team;
    let authed_path = format!("/api/v1/teams/{}/clusters", team);

    // Sanity: harness's own token is accepted on an authed endpoint.
    let ok = raw_get(&base, &authed_path, &harness.auth_token).await;
    assert_eq!(
        ok.status(),
        reqwest::StatusCode::OK,
        "harness-issued token must be accepted by CP on authed endpoint"
    );

    // Spin up a totally independent mock OIDC instance. Same code path the
    // CP/harness uses internally — different process state, different keys.
    let foreign_mock = MockOidcServer::start(MockOidcConfig::default())
        .await
        .expect("second mock OIDC server should start");

    let foreign_token = foreign_mock
        .issue_token_for_sub(DEV_USER_SUB)
        .await
        .expect("foreign mock should mint a token");

    assert_ne!(
        foreign_token, harness.auth_token,
        "foreign-mock token must not coincidentally equal harness token"
    );

    let resp = raw_get(&base, &authed_path, &foreign_token).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "JWT signed by an unrelated mock OIDC must be rejected (got {})",
        resp.status()
    );

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 response must be JSON, got Content-Type: {}",
        content_type
    );
}

// ===========================================================================
// Test b — harness token validates against local mock JWKS
// ===========================================================================

/// The harness-minted token validates against the CP's JWKS AND the `iss`
/// claim points to a local mock URL (`http://127.0.0.1:*` or
/// `http://localhost:*`) — together proves fp-4n5's single-mock wiring.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_fp4n5_harness_token_validates_against_local_mock_jwks() {
    let harness = dev_harness("fp4n5_single_mock").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let claims = decode_jwt_payload(&harness.auth_token).expect("harness token must decode");
    let iss = claims["iss"].as_str().expect("harness token must carry an iss claim");

    assert!(
        iss.starts_with("http://127.0.0.1:") || iss.starts_with("http://localhost:"),
        "issuer should be a local mock URL, got: {}",
        iss
    );

    let base = harness.api_url();
    let path = format!("/api/v1/teams/{}/clusters", harness.team);

    let resp = raw_get(&base, &path, &harness.auth_token).await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "harness token must validate against CP JWKS (got {})",
        resp.status()
    );
}
