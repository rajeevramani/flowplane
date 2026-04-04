//! Phase 3 CLI E2E tests — dev mode
//!
//! Tests the CLI commands added in Phase 3: expose, unexpose, list, status,
//! filter, learn, doctor. All tests use the dev-mode harness (bearer token,
//! no Zitadel) and the CliRunner subprocess driver.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_phase3 -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use serde_json::json;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;

// ============================================================================
// expose — with API verification
// ============================================================================

/// Expose a service via CLI, then verify the backend created cluster, listener,
/// and route-config resources by querying the API directly.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_expose_creates_backend_resources() {
    let harness = dev_harness("dev_cli_expose_resources").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();
    let team = &harness.team;

    let output = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p3-res"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("Exposed");

    // Verify cluster exists
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/clusters/e2e-p3-res"))
        .await
        .expect("authed_get cluster should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "cluster should exist after CLI expose");

    // Verify listener exists
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/listeners/e2e-p3-res-listener"))
        .await
        .expect("authed_get listener should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "listener should exist after CLI expose");

    // Verify route config exists
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/route-configs/e2e-p3-res-routes"))
        .await
        .expect("authed_get route-config should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "route-config should exist after CLI expose"
    );
}

/// Exposing the same name+upstream twice should succeed both times (idempotent).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_expose_idempotent() {
    let harness = dev_harness("dev_cli_expose_idem").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let first = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p3-idem"]).unwrap();
    first.assert_success();
    first.assert_stdout_contains("Exposed");

    let second = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p3-idem"]).unwrap();
    second.assert_success();
}

/// Expose the same name but a different upstream — should fail or indicate a
/// conflict rather than silently overwriting.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_expose_conflict_different_upstream() {
    let harness = dev_harness("dev_cli_expose_conflict").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // First expose with one upstream
    let first = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p3-conflict"]).unwrap();
    first.assert_success();

    // Second expose: same name, different upstream
    let second =
        cli.run(&["expose", "http://127.0.0.1:8888", "--name", "e2e-p3-conflict"]).unwrap();

    // Should either fail (non-zero exit) or print a conflict/warning message.
    // It must NOT silently succeed with no indication of the mismatch.
    let indicated_conflict = second.exit_code != 0
        || second.stdout.to_lowercase().contains("conflict")
        || second.stdout.to_lowercase().contains("already")
        || second.stdout.to_lowercase().contains("update")
        || second.stderr.to_lowercase().contains("conflict")
        || second.stderr.to_lowercase().contains("already");
    assert!(
        indicated_conflict,
        "Expected conflict indication when exposing same name with different upstream.\n\
         exit_code={}, stdout={}, stderr={}",
        second.exit_code, second.stdout, second.stderr
    );
}

/// Expose with a --path argument should succeed.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_expose_with_path() {
    let harness = dev_harness("dev_cli_expose_path").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli
        .run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p3-path", "--path", "/users"])
        .unwrap();
    output.assert_success();
    output.assert_stdout_contains("Exposed");
}

/// Expose without --name: the CLI should auto-generate a name from the upstream.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_expose_auto_name() {
    let harness = dev_harness("dev_cli_expose_auto").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["expose", "http://127.0.0.1:9999"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("Exposed");
}

// ============================================================================
// unexpose — with API verification
// ============================================================================

/// Expose a service, then unexpose it. Verify that cluster, listener, and
/// route-config are removed by checking that the API returns 404 for each.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_unexpose_cleans_resources() {
    let harness = dev_harness("dev_cli_unexpose_clean").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();
    let team = &harness.team;

    // Expose first
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p3-clean"]).unwrap();
    expose.assert_success();

    // Sanity check: cluster exists before unexpose
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/clusters/e2e-p3-clean"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "cluster should exist before unexpose");

    // Unexpose
    let unexpose = cli.run(&["unexpose", "e2e-p3-clean"]).unwrap();
    unexpose.assert_success();

    // Verify cluster is gone
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/clusters/e2e-p3-clean"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "cluster should be deleted after unexpose"
    );

    // Verify listener is gone
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/listeners/e2e-p3-clean-listener"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "listener should be deleted after unexpose"
    );

    // Verify route-config is gone
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/route-configs/e2e-p3-clean-routes"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "route-config should be deleted after unexpose"
    );
}

/// Unexpose a nonexistent service: should not crash.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_unexpose_nonexistent() {
    let harness = dev_harness("dev_cli_unexpose_noexist").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["unexpose", "does-not-exist-at-all"]).unwrap();
    // Should handle gracefully — either exit 0 with message or non-zero without panic
    assert!(
        !output.stdout.trim().is_empty() || !output.stderr.trim().is_empty(),
        "Expected non-empty output when unexposing nonexistent service"
    );
}

/// Unexpose twice in a row: the second invocation should not crash or error
/// unexpectedly (idempotent teardown).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_unexpose_idempotent() {
    let harness = dev_harness("dev_cli_unexpose_idem").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose first
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p3-unidem"]).unwrap();
    expose.assert_success();

    // Unexpose first time
    let first = cli.run(&["unexpose", "e2e-p3-unidem"]).unwrap();
    first.assert_success();

    // Unexpose second time — should not panic or produce an unexpected error
    let second = cli.run(&["unexpose", "e2e-p3-unidem"]).unwrap();
    assert!(
        !second.stdout.trim().is_empty() || !second.stderr.trim().is_empty(),
        "Expected non-empty output on second unexpose"
    );
}

// ============================================================================
// list / status
// ============================================================================

/// `flowplane list` with no exposed services: should succeed.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_phase3_list() {
    let harness = dev_harness("dev_cli_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["list"]).unwrap();
    output.assert_success();
}

/// `flowplane status` system overview: should succeed and show resource info.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_phase3_status() {
    let harness = dev_harness("dev_cli_status").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["status"]).unwrap();
    output.assert_success();
}

// ============================================================================
// filter / learn subcommands
// ============================================================================

/// `flowplane filter list`: should succeed (may return empty list).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_phase3_filter_list() {
    let harness = dev_harness("dev_cli_filter_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["filter", "list"]).unwrap();
    output.assert_success();
}

/// `flowplane learn list`: should succeed (may return empty list).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_phase3_learn_list() {
    let harness = dev_harness("dev_cli_learn_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["learn", "list"]).unwrap();
    output.assert_success();
}

// ============================================================================
// doctor
// ============================================================================

/// `flowplane doctor`: should succeed and report CP health.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_phase3_doctor() {
    let harness = dev_harness("dev_cli_doctor").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["doctor"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("Control plane");
}

// ============================================================================
// Integration: expose → list → unexpose
// ============================================================================

/// Full lifecycle: expose → list (verify name appears) → unexpose → list
/// (verify name gone).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_expose_list_unexpose_lifecycle() {
    let harness = dev_harness("dev_cli_lifecycle").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose
    let expose_output =
        cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p3-lifecycle"]).unwrap();
    expose_output.assert_success();
    expose_output.assert_stdout_contains("Exposed");

    // List should include the service
    let list_output = cli.run(&["list"]).unwrap();
    list_output.assert_success();
    list_output.assert_stdout_contains("e2e-p3-lifecycle");

    // Unexpose
    let unexpose_output = cli.run(&["unexpose", "e2e-p3-lifecycle"]).unwrap();
    unexpose_output.assert_success();

    // List should no longer include the service
    let list_after = cli.run(&["list"]).unwrap();
    list_after.assert_success();
    assert!(
        !list_after.stdout.contains("e2e-p3-lifecycle"),
        "service should not appear in list after unexpose"
    );
}

// ============================================================================
// list / status — deeper tests
// ============================================================================

/// Expose a service, then run `list` — verify output contains the service name
/// and does NOT contain the "-listener" suffix.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_list_after_expose_shows_service() {
    let harness = dev_harness("dev_cli_list_after_expose").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service first
    let expose = cli.run(&["expose", "http://127.0.0.1:8877", "--name", "e2e-list-svc"]).unwrap();
    expose.assert_success();

    // List should show the service name
    let list = cli.run(&["list"]).unwrap();
    list.assert_success();
    list.assert_stdout_contains("e2e-list-svc");

    // The raw listener suffix should be stripped in the output
    assert!(
        !list.stdout.contains("e2e-list-svc-listener"),
        "Expected list output to strip '-listener' suffix, but found it:\n{}",
        list.stdout
    );
}

/// `flowplane status` with no argument shows system overview with resource info.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_status_system_overview() {
    let harness = dev_harness("dev_cli_status_overview").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["status"]).unwrap();
    output.assert_success();

    // Should contain some resource overview text
    let stdout_lower = output.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("listener")
            || stdout_lower.contains("service")
            || stdout_lower.contains("cluster")
            || stdout_lower.contains("route"),
        "Expected status output to contain resource info, got:\n{}",
        output.stdout
    );
}

/// Expose a service, then query status for the specific listener.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_status_specific_listener() {
    let harness = dev_harness("dev_cli_status_specific").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose so there is something to query
    let expose = cli.run(&["expose", "http://127.0.0.1:8878", "--name", "e2e-stat-svc"]).unwrap();
    expose.assert_success();

    // Query status for that listener
    let status = cli.run(&["status", "e2e-stat-svc-listener"]).unwrap();
    status.assert_success();
    // Output should mention the listener name
    assert!(
        status.stdout.contains("e2e-stat-svc") || status.stderr.contains("e2e-stat-svc"),
        "Expected status output to reference the listener, got:\nstdout: {}\nstderr: {}",
        status.stdout,
        status.stderr
    );
}

// ============================================================================
// filter / learn
// ============================================================================

/// `flowplane filter list` should exit 0 (validates command parsing + API).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_list_succeeds() {
    let harness = dev_harness("dev_cli_filter_list_ok").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["filter", "list"]).unwrap();
    output.assert_success();
}

/// `flowplane learn list` should exit 0 (validates command parsing + API).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_list_succeeds() {
    let harness = dev_harness("dev_cli_learn_list_ok").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["learn", "list"]).unwrap();
    output.assert_success();
}

// ============================================================================
// doctor
// ============================================================================

/// `flowplane doctor` should report control plane health.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_doctor_reports_health() {
    let harness = dev_harness("dev_cli_doctor_health").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["doctor"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("Control plane");

    // Should show some kind of health status
    let stdout_lower = output.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("healthy")
            || stdout_lower.contains("ok")
            || stdout_lower.contains("reachable")
            || stdout_lower.contains("✓")
            || stdout_lower.contains("pass"),
        "Expected doctor output to report health status, got:\n{}",
        output.stdout
    );
}

// ============================================================================
// import openapi
// ============================================================================

/// Import a minimal OpenAPI spec from a temp file.
/// Uses --port 10019 to avoid collision with other test listeners.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_import_openapi_from_file() {
    let harness = dev_harness("dev_cli_import_openapi").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();

    // Ensure a dataplane exists for the team (import command requires one)
    let dp_body = json!({
        "name": "e2e-import-dataplane",
        "gateway_host": "127.0.0.1",
        "description": "Dataplane for import test"
    });
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{}/dataplanes", harness.team), &dp_body)
        .await
        .expect("dataplane creation request should succeed");
    // Accept 200/201 (created) or 409 (already exists)
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 201 || status == 409,
        "dataplane creation returned unexpected status: {status}"
    );

    // Write a minimal OpenAPI 3.0 spec to a temp file
    let spec = r#"openapi: "3.0.0"
info:
  title: E2E Test API
  version: "1.0.0"
servers:
  - url: http://127.0.0.1:9999
paths:
  /pets:
    get:
      operationId: listPets
      summary: List pets
      responses:
        "200":
          description: OK
"#;
    let spec_dir = tempfile::tempdir().expect("create temp dir for spec");
    let spec_path = spec_dir.path().join("petstore.yaml");
    std::fs::write(&spec_path, spec).expect("write spec file");

    let output = cli
        .run(&[
            "import",
            "openapi",
            spec_path.to_str().expect("valid utf-8 path"),
            "--name",
            "e2e-petstore",
            "--port",
            "10019",
        ])
        .unwrap();
    output.assert_success();
}

// ============================================================================
// auth commands
// ============================================================================

/// `flowplane auth token` should print the current auth token.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_auth_token_matches_credentials() {
    let harness = dev_harness("dev_cli_auth_token").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["auth", "token"]).unwrap();
    output.assert_success();

    let printed_token = output.stdout.trim();
    assert_eq!(
        printed_token, harness.auth_token,
        "auth token output should match harness credentials"
    );
}

/// `flowplane auth whoami` should succeed and show user info.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_auth_whoami_shows_user() {
    let harness = dev_harness("dev_cli_auth_whoami").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["auth", "whoami"]).unwrap();
    output.assert_success();

    // Should print something meaningful about the user/team
    assert!(
        !output.stdout.trim().is_empty(),
        "Expected whoami to print user info, got empty stdout"
    );
}
