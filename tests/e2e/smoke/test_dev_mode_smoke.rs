//! Dev-mode E2E smoke tests
//!
//! Exercises the dual-mode harness in dev auth mode (bearer token, no Zitadel).
//! These tests run with `FLOWPLANE_E2E_AUTH_MODE=dev`.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e test_dev_mode -- --ignored --nocapture
//! ```

use crate::common::harness::{dev_harness, envoy_harness};

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

/// Make an authenticated POST with a JSON body and custom token.
async fn post_with_token(
    base_url: &str,
    path: &str,
    token: &str,
    body: &serde_json::Value,
) -> reqwest::Response {
    let url = format!("{}{}", base_url, path);
    reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(body)
        .send()
        .await
        .expect("HTTP POST should not fail at transport level")
}

// ===========================================================================
// Lifecycle: expose → status → unexpose (happy path)
// ===========================================================================

/// Full expose/unexpose lifecycle through real HTTP in dev mode.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_expose_lifecycle() {
    let harness = dev_harness("dev_expose_lifecycle").await.expect("harness should start");

    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let base = harness.api_url();
    let token = &harness.auth_token;
    let team = &harness.team;

    // Expose a service
    let body = serde_json::json!({
        "name": "e2e-dev-svc",
        "upstream": "http://127.0.0.1:9999"
    });
    let resp =
        post_with_token(&base, &format!("/api/v1/teams/{}/expose", team), token, &body).await;
    assert!(
        resp.status() == reqwest::StatusCode::CREATED || resp.status() == reqwest::StatusCode::OK,
        "expose should return 201 or 200 (idempotent), got {}",
        resp.status()
    );
    let json: serde_json::Value = resp.json().await.expect("response should be JSON");
    assert_eq!(json["name"], "e2e-dev-svc");
    assert!(json["port"].as_u64().is_some(), "response should include a port");

    // Verify the cluster was created (GET)
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{}/clusters/e2e-dev-svc", team))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "cluster should exist after expose");

    // Verify the listener was created
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{}/listeners/e2e-dev-svc-listener", team))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "listener should exist after expose");

    // Unexpose
    let resp = reqwest::Client::new()
        .delete(format!("{}/api/v1/teams/{}/expose/e2e-dev-svc", base, team))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("delete should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT, "unexpose should return 204");

    // Verify cleanup — cluster should be gone
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{}/clusters/e2e-dev-svc", team))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "cluster should be gone after unexpose"
    );
}

// ===========================================================================
// Full E2E: expose → Envoy routes traffic → unexpose
// ===========================================================================

/// True end-to-end test: expose API → xDS → Envoy routes traffic → upstream.
///
/// Uses the expose API (the actual user-facing endpoint) to create all resources
/// in one call, then verifies Envoy routes traffic to the echo server.
///
/// Requires Envoy binary on PATH. Skips gracefully if Envoy is not available.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_expose_routes_traffic_through_envoy() {
    use std::time::Duration;

    let harness = envoy_harness("dev_envoy_routing").await.expect("harness should start");

    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let base = harness.api_url();
    let token = &harness.auth_token;
    let team = &harness.team;

    // Get the echo server endpoint (started by the harness mock services)
    let echo_endpoint = harness.echo_endpoint();
    let upstream = format!("http://{}", echo_endpoint);

    // Expose the echo server through the gateway using the expose API
    let body = serde_json::json!({
        "name": "e2e-envoy-svc",
        "upstream": upstream
    });
    let resp =
        post_with_token(&base, &format!("/api/v1/teams/{}/expose", team), token, &body).await;
    assert!(
        resp.status() == reqwest::StatusCode::CREATED || resp.status() == reqwest::StatusCode::OK,
        "expose should return 201 or 200, got {}",
        resp.status()
    );
    let json: serde_json::Value = resp.json().await.expect("response should be JSON");
    let port = json["port"].as_u64().expect("expose response should include port") as u16;
    println!("Exposed e2e-envoy-svc on port {} → upstream {}", port, upstream);

    // Wait for xDS to deliver all resources to Envoy.
    // The expose API creates cluster + route_config + listener in rapid succession.
    // Envoy needs time to process the xDS snapshot and bind the new listener port.
    println!("Waiting 5s for xDS convergence...");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Try to reach Envoy on the auto-allocated port.
    // The expose route config uses domains: ["*"] and path prefix: "/".
    let envoy_result = harness.wait_for_route_on_port(port, "localhost", "/", 200).await;

    match envoy_result {
        Ok(body) => {
            assert!(!body.is_empty(), "Proxied response body should not be empty");
            println!(
                "PASS: Envoy routed traffic to echo server: {}...",
                &body[..50.min(body.len())]
            );
        }
        Err(e) => {
            // Dump Envoy config for diagnostics before panicking
            eprintln!("--- Envoy config_dump for diagnostics ---");
            match harness.get_config_dump().await {
                Ok(dump) => {
                    // Print listener and route sections only (truncated)
                    for line in dump.lines() {
                        if line.contains("e2e-envoy")
                            || line.contains("route_config_name")
                            || line.contains("\"port_value\"")
                            || line.contains("10001")
                        {
                            eprintln!("  {}", line);
                        }
                    }
                }
                Err(dump_err) => eprintln!("  Failed to get config_dump: {}", dump_err),
            }
            eprintln!("--- end config_dump ---");
            panic!("Envoy did not converge route for e2e-envoy-svc on port {}: {}", port, e);
        }
    }

    // Unexpose and verify cleanup
    let resp = reqwest::Client::new()
        .delete(format!("{}/api/v1/teams/{}/expose/e2e-envoy-svc", base, team))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("delete should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT, "unexpose should return 204");

    println!("Full Envoy routing E2E test PASSED");
}

// ===========================================================================
// Auth negative cases
// ===========================================================================

/// No Authorization header → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_no_auth_header_returns_401() {
    let harness = dev_harness("dev_no_auth_401").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
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

/// Wrong token → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_wrong_token_returns_401() {
    let harness = dev_harness("dev_wrong_token_401").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some("Bearer totally-wrong-token-value"),
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED, "wrong token → 401");

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// Empty string token → 401
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_empty_token_returns_401() {
    let harness = dev_harness("dev_empty_token_401").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some("Bearer "),
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED, "empty token → 401");

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
/// would accept "Bearer Bearer <real-token>" as "Bearer <real-token>" — effectively
/// treating the doubled prefix as valid. This test catches that.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_doubled_bearer_prefix_returns_401() {
    let harness = dev_harness("dev_doubled_bearer_401").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    // "Bearer Bearer <real_token>" — the extracted token would be "Bearer <real_token>"
    // which should NOT match the actual dev token.
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

/// Token with trailing newline — rejected at transport level.
///
/// HTTP headers cannot contain newlines (RFC 7230). reqwest/hyper reject
/// `\n` in header values with InvalidHeaderValue before the request is sent.
/// This is correct behavior — the malformed token never reaches the server.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_token_with_trailing_newline_rejected_at_transport() {
    let harness = dev_harness("dev_trailing_newline_401").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let bad_header = format!("Bearer {}\n", harness.auth_token);
    let url = format!("{}/api/v1/teams/{}/clusters", harness.api_url(), harness.team);
    let result = reqwest::Client::new().get(&url).header("Authorization", &bad_header).send().await;

    // reqwest rejects newlines in header values — the request never leaves the client.
    // This is the correct security behavior: tokens with embedded newlines are malformed.
    assert!(result.is_err(), "newline in header value should be rejected at transport level");
    let err = result.unwrap_err();
    assert!(err.is_builder(), "error should be a builder/header error, got: {}", err);
}

/// Token with trailing whitespace — accepted (hyper strips OWS per RFC 7230).
///
/// RFC 7230 §3.2.6: optional whitespace (OWS) around header field values is
/// stripped by compliant HTTP implementations. hyper (used by axum) does this,
/// so `"Bearer token "` becomes `"Bearer token"` before reaching the middleware.
/// This is correct HTTP behavior, not a bug.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_token_with_trailing_space_accepted_per_rfc7230() {
    let harness = dev_harness("dev_trailing_space_ok").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    // Trailing space in header value is stripped by hyper per RFC 7230 OWS rules,
    // so this is equivalent to sending the correct token.
    let header = format!("Bearer {} ", harness.auth_token);
    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some(&header),
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "trailing space is stripped by HTTP layer per RFC 7230 — token should be accepted"
    );
}

/// Authorization header with wrong scheme (Basic instead of Bearer) → 401.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_basic_auth_scheme_returns_401() {
    let harness = dev_harness("dev_basic_scheme_401").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    // Send a valid-looking Basic auth header — server should reject non-Bearer
    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some("Basic dGVzdDp0ZXN0"), // base64("test:test")
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "Basic auth scheme should be rejected"
    );

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

/// A JWT-format token in dev mode — should still be treated as a bearer string
/// and rejected because it doesn't match the actual dev token.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_jwt_format_token_returns_401() {
    let harness = dev_harness("dev_jwt_format_401").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    // A fake JWT (3 dot-separated base64 parts) — should NOT match the dev token
    let fake_jwt = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.fake_signature";
    let resp = raw_request(
        &harness.api_url(),
        &format!("/api/v1/teams/{}/clusters", harness.team),
        Some(&format!("Bearer {}", fake_jwt)),
    )
    .await;
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "JWT-format token should be rejected in dev mode (doesn't match dev token)"
    );

    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "401 should return JSON, got: {}",
        content_type
    );
}

// ===========================================================================
// Public endpoints should work without auth even in dev mode
// ===========================================================================

/// Health endpoint requires no auth.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_health_endpoint_no_auth() {
    let harness = dev_harness("dev_health_no_auth").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let resp = raw_request(&harness.api_url(), "/health", None).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "health should work without auth");
}

/// Auth mode endpoint should report "dev".
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_auth_mode_endpoint_reports_dev() {
    let harness = dev_harness("dev_auth_mode_check").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let resp = raw_request(&harness.api_url(), "/api/v1/auth/mode", None).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.expect("should be JSON");
    assert_eq!(json["auth_mode"], "dev", "auth mode should be 'dev'");
}
