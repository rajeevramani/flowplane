//! Ops diagnostic CLI + REST E2E tests — dev mode
//!
//! Tests the ops diagnostic REST endpoints and CLI commands: trace, topology,
//! validate, xds status, audit. All tests use the dev-mode harness (bearer
//! token, no Zitadel) and the CliRunner subprocess driver.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_ops -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use serde_json::json;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;

// ============================================================================
// REST endpoint tests (via harness.authed_get / authed_post)
// ============================================================================

/// GET /ops/trace?path=/test returns valid JSON with a path field.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_trace_returns_json() {
    let harness = dev_harness("dev_ops_trace").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let team = &harness.team;

    // Setup: expose a service so there's something to trace
    let cli = CliRunner::from_harness(&harness).unwrap();
    cli.run(&["expose", "http://127.0.0.1:9999", "--name", "ops-trace-svc"])
        .unwrap()
        .assert_success();

    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/ops/trace?path=/test"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status().as_u16(), 200, "trace endpoint should return 200");

    let body: serde_json::Value = resp.json().await.expect("response should be valid JSON");
    assert!(body.is_object(), "trace response should be a JSON object");
    assert!(body.get("path").is_some(), "trace response should contain a path field");
}

/// GET /ops/trace without path param should return 400 (missing required param).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_trace_missing_path_returns_400() {
    let harness = dev_harness("dev_ops_trace_400").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let team = &harness.team;

    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/ops/trace"))
        .await
        .expect("authed_get should succeed");

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "trace without path should return 400 or 422, got {status}"
    );

    // Error response should be JSON, not HTML
    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        content_type.contains("json"),
        "error response should be JSON, got content-type: {content_type}"
    );
}

/// GET /ops/topology returns valid JSON with structural fields.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_topology_returns_json() {
    let harness = dev_harness("dev_ops_topo").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let team = &harness.team;

    // Setup: expose a service so topology has something to report
    let cli = CliRunner::from_harness(&harness).unwrap();
    cli.run(&["expose", "http://127.0.0.1:9999", "--name", "ops-topo-svc"])
        .unwrap()
        .assert_success();

    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/ops/topology"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status().as_u16(), 200, "topology endpoint should return 200");

    let body: serde_json::Value = resp.json().await.expect("response should be valid JSON");
    assert!(
        body.is_object() || body.is_array(),
        "topology response should be JSON object or array"
    );

    // Should contain some structural data about listeners, clusters, or routes
    let body_str = body.to_string().to_lowercase();
    assert!(
        body_str.contains("listener") || body_str.contains("cluster") || body_str.contains("route"),
        "topology response should reference listeners, clusters, or routes: {body}"
    );
}

/// GET /ops/validate returns JSON with an issues array.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_validate_returns_json() {
    let harness = dev_harness("dev_ops_validate").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let team = &harness.team;

    // Setup: expose a service so validation has config to check
    let cli = CliRunner::from_harness(&harness).unwrap();
    cli.run(&["expose", "http://127.0.0.1:9999", "--name", "ops-validate-svc"])
        .unwrap()
        .assert_success();

    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/ops/validate"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status().as_u16(), 200, "validate endpoint should return 200");

    let body: serde_json::Value = resp.json().await.expect("response should be valid JSON");
    assert!(body.is_object(), "validate response should be a JSON object");

    // Should contain an issues array (may be empty for valid config)
    assert!(
        body.get("issues").is_some(),
        "validate response should contain an 'issues' field: {body}"
    );
    assert!(body["issues"].is_array(), "issues field should be an array: {}", body["issues"]);
}

/// GET /ops/xds/status returns valid JSON.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_xds_status_returns_json() {
    let harness = dev_harness("dev_ops_xds").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let team = &harness.team;

    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/ops/xds/status"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status().as_u16(), 200, "xds status endpoint should return 200");

    let body: serde_json::Value = resp.json().await.expect("response should be valid JSON");
    assert!(
        body.is_object() || body.is_array(),
        "xds status response should be JSON object or array"
    );
}

/// GET /ops/audit returns JSON array after creating resources.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_audit_returns_json() {
    let harness = dev_harness("dev_ops_audit").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let team = &harness.team;

    // Setup: expose a service to generate audit events
    let cli = CliRunner::from_harness(&harness).unwrap();
    cli.run(&["expose", "http://127.0.0.1:9999", "--name", "ops-audit-svc"])
        .unwrap()
        .assert_success();

    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/ops/audit"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status().as_u16(), 200, "audit endpoint should return 200");

    let body: serde_json::Value = resp.json().await.expect("response should be valid JSON");
    assert!(body.is_array(), "audit response should be a JSON array, got: {body}");

    // After exposing a service, there should be at least one audit event
    let events = body.as_array().unwrap();
    assert!(!events.is_empty(), "audit log should contain events after exposing a service");
}

// ============================================================================
// CLI tests (via cli.run)
// ============================================================================

/// Expose a service, trace its path, verify trace finds a match.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_trace_with_exposed_service() {
    let harness = dev_harness("dev_cli_trace").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Setup: expose a service
    cli.run(&["expose", "http://127.0.0.1:9999", "--name", "trace-test-svc"])
        .unwrap()
        .assert_success();

    // Trace its path — should succeed and show matching info
    let output = cli.run(&["trace", "/"]).unwrap();
    output.assert_success();

    // Output should contain some routing information
    let combined = format!("{}{}", output.stdout, output.stderr).to_lowercase();
    assert!(
        combined.contains("trace")
            || combined.contains("match")
            || combined.contains("route")
            || combined.contains("listener")
            || combined.contains("cluster")
            || combined.contains("trace-test-svc"),
        "trace output should contain routing information, got:\nstdout: {}\nstderr: {}",
        output.stdout,
        output.stderr
    );
}

/// Trace a path that doesn't match any exposed service — should succeed but
/// indicate no match.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_trace_nonexistent_path() {
    let harness = dev_harness("dev_cli_trace_nopath").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Trace a path with no matching service
    let output = cli.run(&["trace", "/nonexistent/path/that/should/not/match"]).unwrap();

    // Should either succeed with "no match" output or fail gracefully
    assert!(
        output.exit_code == 0 || output.exit_code == 1,
        "trace of nonexistent path should exit 0 or 1, got {}",
        output.exit_code
    );
    // Should not produce an empty result without explanation
    assert!(
        !output.stdout.trim().is_empty() || !output.stderr.trim().is_empty(),
        "trace of nonexistent path should produce some output"
    );
}

/// Expose a service, run topology, verify resource names in output.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_topology_shows_resources() {
    let harness = dev_harness("dev_cli_topo").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Setup: expose a service
    cli.run(&["expose", "http://127.0.0.1:9999", "--name", "topo-test-svc"])
        .unwrap()
        .assert_success();

    let output = cli.run(&["topology"]).unwrap();
    output.assert_success();

    // Topology should mention the service or its derived resources
    let combined = format!("{}{}", output.stdout, output.stderr).to_lowercase();
    assert!(
        combined.contains("topo-test-svc") || combined.contains("listener")
            || combined.contains("cluster") || combined.contains("route"),
        "topology output should reference the exposed service or its resources, got:\nstdout: {}\nstderr: {}",
        output.stdout,
        output.stderr
    );
}

/// Expose a valid service, validate config — should exit 0 (no issues).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_validate_clean_config() {
    let harness = dev_harness("dev_cli_validate").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Setup: expose a valid service
    cli.run(&["expose", "http://127.0.0.1:9999", "--name", "validate-test-svc"])
        .unwrap()
        .assert_success();

    let output = cli.run(&["validate"]).unwrap();
    output.assert_success();
}

/// Expose a service, run audit, verify create events appear.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_audit_after_changes() {
    let harness = dev_harness("dev_cli_audit").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Setup: expose a service to generate audit events
    cli.run(&["expose", "http://127.0.0.1:9999", "--name", "audit-test-svc"])
        .unwrap()
        .assert_success();

    let output = cli.run(&["audit"]).unwrap();
    output.assert_success();

    // Audit output should mention create events or the service name
    let combined = format!("{}{}", output.stdout, output.stderr).to_lowercase();
    assert!(
        combined.contains("create")
            || combined.contains("audit")
            || combined.contains("event")
            || combined.contains("audit-test-svc")
            || combined.contains("cluster"),
        "audit output should show create events, got:\nstdout: {}\nstderr: {}",
        output.stdout,
        output.stderr
    );
}

/// `flowplane xds status` should exit 0 and produce output.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_xds_status_runs() {
    let harness = dev_harness("dev_cli_xds_status").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["xds", "status"]).unwrap();
    output.assert_success();

    assert!(
        !output.stdout.trim().is_empty() || !output.stderr.trim().is_empty(),
        "xds status should produce some output"
    );
}

/// `flowplane xds nacks` should exit 0 (may be empty if no NACKs).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_xds_nacks_empty() {
    let harness = dev_harness("dev_cli_xds_nacks").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["xds", "nacks"]).unwrap();
    output.assert_success();
}

// ============================================================================
// Negative / adversarial tests
// ============================================================================

/// `flowplane trace ""` (empty path) should fail or indicate an error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_trace_empty_path() {
    let harness = dev_harness("dev_cli_trace_empty").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["trace", ""]).unwrap();

    // Empty path should either fail (non-zero exit) or produce an error/warning message
    let indicated_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("error")
        || output.stderr.to_lowercase().contains("invalid")
        || output.stderr.to_lowercase().contains("required")
        || output.stdout.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("no match")
        || output.stdout.to_lowercase().contains("invalid");
    assert!(
        indicated_error,
        "trace with empty path should fail or indicate an error.\n\
         exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// POST to /ops/trace should return 405 Method Not Allowed (it's a GET endpoint).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_wrong_method_returns_405() {
    let harness = dev_harness("dev_ops_405").await.expect("harness should start");
    if !harness.is_dev_mode() {
        return;
    }
    let team = &harness.team;

    let resp = harness
        .authed_post(&format!("/api/v1/teams/{team}/ops/trace?path=/test"), &json!({}))
        .await
        .expect("authed_post should succeed");

    let status = resp.status().as_u16();
    assert!(
        status == 405 || status == 404,
        "POST to GET-only /ops/trace should return 405 or 404, got {status}"
    );
}
