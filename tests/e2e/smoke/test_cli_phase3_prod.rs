//! Phase 3 CLI E2E tests — prod mode (Zitadel OIDC)
//!
//! Mirror of test_cli_phase3.rs but running in prod mode with JWT auth.
//! Validates that the same CLI commands work with both dev bearer tokens
//! and prod JWT tokens from the Zitadel OIDC provider.
//!
//! ```bash
//! RUN_E2E=1 cargo test --test e2e prod_cli -- --ignored --nocapture
//! ```

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;

// ============================================================================
// expose / unexpose
// ============================================================================

/// Expose via CLI with JWT auth — verify backend resources via API.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_expose_creates_resources() {
    let harness = dev_harness("prod_cli_expose_res").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();
    let team = &harness.team;

    let output =
        cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-prod-cli-svc"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("Exposed");

    // Verify backend resources via API
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/clusters/e2e-prod-cli-svc"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "cluster should exist after CLI expose");

    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/listeners/e2e-prod-cli-svc-listener"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "listener should exist after CLI expose");

    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/route-configs/e2e-prod-cli-svc-routes"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "route-config should exist after CLI expose"
    );

    // Cleanup: release port
    let _ = cli.run(&["unexpose", "e2e-prod-cli-svc"]);
}

/// Unexpose via CLI with JWT auth — verify cleanup.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_unexpose_cleans_resources() {
    let harness = dev_harness("prod_cli_unexpose_clean").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();
    let team = &harness.team;

    // Expose
    let output =
        cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-prod-cli-unx"]).unwrap();
    output.assert_success();

    // Unexpose
    let output = cli.run(&["unexpose", "e2e-prod-cli-unx"]).unwrap();
    output.assert_success();

    // Verify resources are gone
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/clusters/e2e-prod-cli-unx"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "cluster should be gone after unexpose"
    );

    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/listeners/e2e-prod-cli-unx-listener"))
        .await
        .expect("authed_get should succeed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "listener should be gone after unexpose"
    );
}

/// Idempotent expose with JWT auth.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_expose_idempotent() {
    let harness = dev_harness("prod_cli_expose_idem").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();

    let output1 =
        cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-prod-cli-idem"]).unwrap();
    output1.assert_success();

    let output2 =
        cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-prod-cli-idem"]).unwrap();
    output2.assert_success();

    // Cleanup: release port
    let _ = cli.run(&["unexpose", "e2e-prod-cli-idem"]);
}

// ============================================================================
// list / status
// ============================================================================

/// List command with JWT auth.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_list() {
    let harness = dev_harness("prod_cli_list").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["list"]).unwrap();
    output.assert_success();
}

/// Status command with JWT auth.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_status() {
    let harness = dev_harness("prod_cli_status").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["status"]).unwrap();
    output.assert_success();
}

// ============================================================================
// filter / learn / doctor
// ============================================================================

/// Filter list with JWT auth.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_filter_list() {
    let harness = dev_harness("prod_cli_filter_list").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["filter", "list"]).unwrap();
    output.assert_success();
}

/// Learn list with JWT auth.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_learn_list() {
    let harness = dev_harness("prod_cli_learn_list").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["learn", "list"]).unwrap();
    output.assert_success();
}

/// Doctor with JWT auth.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_doctor() {
    let harness = dev_harness("prod_cli_doctor").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["doctor"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("Control plane");
}

// ============================================================================
// Full lifecycle: expose → list → unexpose
// ============================================================================

/// Full CLI lifecycle with JWT auth.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_expose_list_unexpose_lifecycle() {
    let harness = dev_harness("prod_cli_lifecycle").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }

    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose
    let output =
        cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-prod-lifecycle"]).unwrap();
    output.assert_success();

    // List should show the service
    let list = cli.run(&["list"]).unwrap();
    list.assert_success();
    list.assert_stdout_contains("e2e-prod-lifecycle");

    // Unexpose
    let output = cli.run(&["unexpose", "e2e-prod-lifecycle"]).unwrap();
    output.assert_success();
}
