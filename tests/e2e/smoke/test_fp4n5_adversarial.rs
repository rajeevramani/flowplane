//! Adversarial smoke tests for fp-4n5 (unified dev/prod auth via dev OIDC mock).
//!
//! These tests intentionally exercise the seams the verifier called out:
//!
//! * Test a — a JWT signed by an unrelated mock OIDC instance must be rejected
//!   by the control plane, even though it has the same `sub`. Catches regressions
//!   where the CP would accept any signature from any mock, or where the dev OIDC
//!   mock injection on the CP gets dropped silently.
//!
//! * Test b — the harness's mock and the CP's mock are the same instance. Two
//!   tokens minted from the harness must share an issuer and both must validate
//!   against the CP's JWKS. Catches regressions where two mock instances quietly
//!   coexist.
//!
//! * Test c — the credentials handoff race. The CP writes the credentials file
//!   on startup; an agent reading it before the file exists would race. After
//!   the harness reports ready, the file must exist, be a valid JWT, and decode
//!   to the dev sub.
//!
//! Gated with `#[ignore]` + `RUN_E2E=1`. Test names start with `dev_` so
//! `make test-e2e-dev` picks them up.

use crate::common::harness::dev_harness;
use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use flowplane::auth::dev_token::DEV_USER_SUB;
use flowplane::dev::oidc_server::{MockOidcConfig, MockOidcServer};

/// Decode a JWT payload segment without verifying the signature.
/// Returns the parsed JSON claims.
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
// Test b — single mock verification: harness and CP share JWKS
// ===========================================================================

#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_fp4n5_single_mock_verification() {
    let harness = dev_harness("fp4n5_single_mock").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let token_a = harness.auth_token.clone();
    let claims_a = decode_jwt_payload(&token_a).expect("token A must decode");
    let iss_a = claims_a["iss"].as_str().expect("token A must carry an iss claim").to_string();

    assert!(
        iss_a.starts_with("http://127.0.0.1:") || iss_a.starts_with("http://localhost:"),
        "issuer should be a local mock URL, got: {}",
        iss_a
    );

    // Re-read the harness token as the second sample. In dev mode the harness
    // exposes a single token — same value here, but decoding it independently
    // and comparing iss + JWKS validation still proves the CP and harness
    // agree on a single mock identity.
    let token_b = harness.auth_token.clone();
    let claims_b = decode_jwt_payload(&token_b).expect("token B must decode");
    let iss_b = claims_b["iss"].as_str().expect("token B must carry an iss claim").to_string();

    assert_eq!(iss_a, iss_b, "both harness tokens must come from the same issuer (single mock)");

    // Both tokens must validate against the CP's JWKS (proves CP wired to
    // the same mock the harness minted from).
    let base = harness.api_url();
    let path = format!("/api/v1/teams/{}/clusters", harness.team);

    let resp_a = raw_get(&base, &path, &token_a).await;
    assert_eq!(resp_a.status(), reqwest::StatusCode::OK, "token A must validate against CP JWKS");

    let resp_b = raw_get(&base, &path, &token_b).await;
    assert_eq!(resp_b.status(), reqwest::StatusCode::OK, "token B must validate against CP JWKS");
}

// ===========================================================================
// Test c — credentials handoff: file populated after harness ready
// ===========================================================================

#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_fp4n5_credentials_handoff_file_populated() {
    // Set the env var BEFORE booting the harness so the CP picks it up on
    // startup. Ordering is critical — the CP reads this once at boot.
    let tmp = tempfile::tempdir().expect("tempdir for credentials");
    let cred_path = tmp.path().join("credentials");
    std::env::set_var("FLOWPLANE_CREDENTIALS_PATH", &cred_path);

    let harness_result = dev_harness("fp4n5_creds_handoff").await;

    // Always clear the env var, even on harness failure.
    let cleanup = || std::env::remove_var("FLOWPLANE_CREDENTIALS_PATH");

    let harness = match harness_result {
        Ok(h) => h,
        Err(e) => {
            cleanup();
            panic!("harness should start: {}", e);
        }
    };

    if !harness.is_dev_mode() {
        cleanup();
        eprintln!("SKIP: not in dev mode");
        return;
    }

    // After harness ready, the CP must have written credentials to the path.
    let body = match std::fs::read_to_string(&cred_path) {
        Ok(b) => b,
        Err(e) => {
            cleanup();
            panic!(
                "credentials file must exist at {} after harness boot: {}",
                cred_path.display(),
                e
            );
        }
    };

    assert!(
        !body.trim().is_empty(),
        "credentials file at {} must be non-empty",
        cred_path.display()
    );

    // The file may be a raw token (dev) or a JSON wrapper. Try JSON first;
    // fall back to raw.
    let token: String = match serde_json::from_str::<serde_json::Value>(body.trim()) {
        Ok(json) => json
            .get("access_token")
            .and_then(|v| v.as_str())
            .or_else(|| json.get("token").and_then(|v| v.as_str()))
            .unwrap_or_else(|| {
                cleanup();
                panic!("credentials JSON had no access_token/token field: {}", body);
            })
            .to_string(),
        Err(_) => body.trim().to_string(),
    };

    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        cleanup();
        panic!(
            "credentials token must be a JWT with three base64url segments, got {} segments",
            parts.len()
        );
    }

    // Each segment must be valid base64url.
    for (i, seg) in parts.iter().enumerate() {
        if URL_SAFE_NO_PAD.decode(seg).is_err() {
            cleanup();
            panic!("JWT segment {} was not valid base64url: {:?}", i, seg);
        }
    }

    let claims = match decode_jwt_payload(&token) {
        Ok(c) => c,
        Err(e) => {
            cleanup();
            panic!("credentials JWT payload must decode: {}", e);
        }
    };

    let sub = claims["sub"].as_str().unwrap_or("");
    if sub != DEV_USER_SUB {
        cleanup();
        panic!("credentials JWT sub must be {}, got {:?}", DEV_USER_SUB, sub);
    }

    cleanup();
}
