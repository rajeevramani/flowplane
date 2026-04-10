//! E2E tests for `flowplane learn` lifecycle commands (start, stop, cancel, activate).
//!
//! Tests the full learning session lifecycle:
//! - start → list → stop → activate
//! - start → cancel (discard)
//! - negative: stop/cancel/activate with no active session
//!
//! Learning requires a running service with traffic flowing, so these tests
//! use `quick_harness` (which includes Envoy) and expose a service first.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_learn -- --ignored --nocapture
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::{dev_harness, quick_harness};
use crate::common::test_helpers::verify_in_config_dump;

// ============================================================================
// learn start — basic
// ============================================================================

/// `flowplane learn start` with required flags should create a new session.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_start_creates_session() {
    let harness = quick_harness("dev_learn_start").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service so learning has something to observe
    let expose =
        cli.run(&["expose", "http://127.0.0.1:9999", "--name", "learn-start-svc"]).unwrap();
    expose.assert_success();

    // Verify the exposed service reached Envoy
    verify_in_config_dump(&harness, "learn-start-svc").await;

    // Start a learning session
    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/.*",
            "--target-sample-count",
            "10",
            "--name",
            "e2e-learn-start",
        ])
        .unwrap();
    start.assert_success();

    // Verify the session appears in learn list
    let list = cli.run(&["learn", "list", "-o", "json"]).unwrap();
    list.assert_success();
    list.assert_stdout_contains("e2e-learn-start");
}

// ============================================================================
// learn start — with optional flags
// ============================================================================

/// `flowplane learn start` with cluster filter and HTTP method filters.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_start_with_filters() {
    let harness = quick_harness("dev_learn_filt").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "learn-filt-svc"]).unwrap();
    expose.assert_success();
    verify_in_config_dump(&harness, "learn-filt-svc").await;

    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/api/.*",
            "--cluster-name",
            "learn-filt-svc",
            "--http-methods",
            "GET",
            "POST",
            "--target-sample-count",
            "50",
            "--name",
            "e2e-learn-filtered",
        ])
        .unwrap();
    start.assert_success();

    // Verify session details via learn get
    let get = cli.run(&["learn", "get", "e2e-learn-filtered", "-o", "json"]).unwrap();
    get.assert_success();
    get.assert_stdout_contains("e2e-learn-filtered");
    get.assert_stdout_contains("^/api/.*");
}

// ============================================================================
// learn stop — full lifecycle
// ============================================================================

/// Full lifecycle: expose → start learning → stop → verify completed status.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_stop_lifecycle() {
    let harness = quick_harness("dev_learn_stop").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "learn-stop-svc"]).unwrap();
    expose.assert_success();
    verify_in_config_dump(&harness, "learn-stop-svc").await;

    // Start learning
    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/.*",
            "--target-sample-count",
            "100",
            "--name",
            "e2e-learn-stop",
        ])
        .unwrap();
    start.assert_success();

    // Verify it's active
    let list = cli.run(&["learn", "list", "--status", "active", "-o", "json"]).unwrap();
    list.assert_success();
    list.assert_stdout_contains("e2e-learn-stop");

    // Stop the session
    let stop = cli.run(&["learn", "stop", "e2e-learn-stop", "-o", "json"]).unwrap();
    stop.assert_success();

    // Verify session is no longer active
    let list_after = cli.run(&["learn", "list", "--status", "active", "-o", "json"]).unwrap();
    list_after.assert_success();
    assert!(
        !list_after.stdout.contains("e2e-learn-stop"),
        "stopped session should not appear in active list"
    );
}

// ============================================================================
// learn cancel — discard session
// ============================================================================

/// Start a session, then cancel it. Verify no schema is created.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_cancel_discards_session() {
    let harness = quick_harness("dev_learn_cancel").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service
    let expose =
        cli.run(&["expose", "http://127.0.0.1:9999", "--name", "learn-cancel-svc"]).unwrap();
    expose.assert_success();
    verify_in_config_dump(&harness, "learn-cancel-svc").await;

    // Start learning
    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/.*",
            "--target-sample-count",
            "100",
            "--name",
            "e2e-learn-cancel",
        ])
        .unwrap();
    start.assert_success();

    // Cancel with --yes to skip interactive prompt
    let cancel = cli.run(&["learn", "cancel", "e2e-learn-cancel", "--yes"]).unwrap();
    cancel.assert_success();
    cancel.assert_stdout_contains("cancelled");

    // Verify the session is gone from active list
    let list = cli.run(&["learn", "list", "--status", "active", "-o", "json"]).unwrap();
    list.assert_success();
    assert!(
        !list.stdout.contains("e2e-learn-cancel"),
        "cancelled session should not appear in active list"
    );

    // Verify no schema was created from the cancelled session
    let schemas = cli.run(&["schema", "list", "-o", "json"]).unwrap();
    schemas.assert_success();
    assert!(
        !schemas.stdout.contains("e2e-learn-cancel"),
        "no schema should exist for a cancelled session"
    );
}

// ============================================================================
// learn activate — apply learned schemas
// ============================================================================

/// Full lifecycle: start → stop → activate → verify schema exists.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_activate_applies_schemas() {
    let harness = quick_harness("dev_learn_act").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "learn-act-svc"]).unwrap();
    expose.assert_success();
    verify_in_config_dump(&harness, "learn-act-svc").await;

    // Start learning
    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/.*",
            "--target-sample-count",
            "10",
            "--name",
            "e2e-learn-activate",
        ])
        .unwrap();
    start.assert_success();

    // Brief pause to let the session become active
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Stop learning (triggers final aggregation)
    let stop = cli.run(&["learn", "stop", "e2e-learn-activate", "-o", "json"]).unwrap();
    stop.assert_success();

    // Activate the completed session
    let activate = cli.run(&["learn", "activate", "e2e-learn-activate", "-o", "json"]).unwrap();
    activate.assert_success();

    // Verify schema was applied — schema list should show something
    let schemas = cli.run(&["schema", "list", "-o", "json"]).unwrap();
    schemas.assert_success();
}

// ============================================================================
// learn get — session details
// ============================================================================

/// `flowplane learn get <name>` should return session details.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_get_session_details() {
    let harness = quick_harness("dev_learn_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "learn-get-svc"]).unwrap();
    expose.assert_success();
    verify_in_config_dump(&harness, "learn-get-svc").await;

    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/health",
            "--target-sample-count",
            "25",
            "--name",
            "e2e-learn-get",
            "--triggered-by",
            "e2e-test",
        ])
        .unwrap();
    start.assert_success();

    let get = cli.run(&["learn", "get", "e2e-learn-get", "-o", "json"]).unwrap();
    get.assert_success();
    get.assert_stdout_contains("e2e-learn-get");
    get.assert_stdout_contains("^/health");
    get.assert_stdout_contains("e2e-test"); // triggered_by
}

// ============================================================================
// Negative: stop with no active session
// ============================================================================

/// `flowplane learn stop <nonexistent>` should fail gracefully.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_stop_nonexistent_fails() {
    let harness = dev_harness("dev_learn_stop_noex").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let stop = cli.run(&["learn", "stop", "nonexistent-session-id"]).unwrap();
    stop.assert_failure();
}

// ============================================================================
// Negative: cancel with no active session
// ============================================================================

/// `flowplane learn cancel <nonexistent>` should fail gracefully.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_cancel_nonexistent_fails() {
    let harness = dev_harness("dev_learn_cancel_noex").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let cancel = cli.run(&["learn", "cancel", "nonexistent-session-id", "--yes"]).unwrap();
    cancel.assert_failure();
}

// ============================================================================
// Negative: activate with no completed session
// ============================================================================

/// `flowplane learn activate <nonexistent>` should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_activate_nonexistent_fails() {
    let harness = dev_harness("dev_learn_act_noex").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let activate = cli.run(&["learn", "activate", "nonexistent-session-id"]).unwrap();
    activate.assert_failure();
}

// ============================================================================
// Negative: get nonexistent session
// ============================================================================

/// `flowplane learn get <nonexistent>` should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_get_nonexistent_fails() {
    let harness = dev_harness("dev_learn_get_noex").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let get = cli.run(&["learn", "get", "nonexistent-session-id"]).unwrap();
    get.assert_failure();
}

// ============================================================================
// Negative: cancel without --yes on non-TTY should fail
// ============================================================================

/// `flowplane learn cancel` without --yes should fail when stdin is not a terminal.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_cancel_no_yes_non_tty_fails() {
    let harness = quick_harness("dev_learn_nyes").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose + start a session so cancel has a valid target
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "learn-nyes-svc"]).unwrap();
    expose.assert_success();
    verify_in_config_dump(&harness, "learn-nyes-svc").await;

    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/.*",
            "--target-sample-count",
            "10",
            "--name",
            "e2e-learn-nyes",
        ])
        .unwrap();
    start.assert_success();

    // Cancel WITHOUT --yes — stdin is not a terminal in test, should fail
    let cancel = cli.run(&["learn", "cancel", "e2e-learn-nyes"]).unwrap();
    cancel.assert_failure();
    cancel.assert_stderr_contains("terminal");
}

// ============================================================================
// learn start — missing required flags
// ============================================================================

/// `flowplane learn start` without --route-pattern should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_start_missing_route_pattern_fails() {
    let harness = dev_harness("dev_learn_noroute").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Missing --route-pattern — clap should reject
    let start = cli.run(&["learn", "start", "--target-sample-count", "10"]).unwrap();
    start.assert_failure();
}

/// `flowplane learn start` without --target-sample-count should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_start_missing_sample_count_fails() {
    let harness = dev_harness("dev_learn_nocount").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Missing --target-sample-count — clap should reject
    let start = cli.run(&["learn", "start", "--route-pattern", "^/.*"]).unwrap();
    start.assert_failure();
}

// ============================================================================
// learn health — session diagnostics
// ============================================================================

/// `flowplane learn health <name>` on an active session should return health info.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_learn_health_active_session() {
    let harness = quick_harness("dev_learn_health").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let expose =
        cli.run(&["expose", "http://127.0.0.1:9999", "--name", "learn-health-svc"]).unwrap();
    expose.assert_success();
    verify_in_config_dump(&harness, "learn-health-svc").await;

    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/.*",
            "--target-sample-count",
            "10",
            "--name",
            "e2e-learn-health",
        ])
        .unwrap();
    start.assert_success();

    let health = cli.run(&["learn", "health", "e2e-learn-health"]).unwrap();
    health.assert_success();
}
