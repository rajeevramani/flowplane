//! E2E tests for CLI commands that mutate local state.
//!
//! Tests config init, config set, auth logout, and wasm download —
//! all commands that create/modify files in the isolated HOME directory.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_local -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;

// ============================================================================
// config init
// ============================================================================

/// `flowplane config init --force` should create a config file and exit 0.
/// We use --force because CliRunner already creates a config.toml.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_config_init_creates_file() {
    let harness = dev_harness("dev_cli_cfg_init").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Run config init --force (CliRunner already wrote a config, so --force is needed)
    let output = cli.run(&["config", "init", "--force"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("Configuration file created at");

    // Verify the config file is readable via `config path`
    let path_output = cli.run(&["config", "path"]).unwrap();
    path_output.assert_success();
    assert!(
        path_output.stdout.trim().ends_with("config.toml"),
        "config path should end with config.toml, got: {}",
        path_output.stdout.trim()
    );

    // Verify `config show` works on the newly initialized config
    let show_output = cli.run(&["config", "show"]).unwrap();
    show_output.assert_success();
}

/// `flowplane config init` without --force should fail when config already exists.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_config_init_refuses_overwrite_without_force() {
    let harness = dev_harness("dev_cli_cfg_init_noforce").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // CliRunner already wrote config.toml, so init without --force should fail
    let output = cli.run(&["config", "init"]).unwrap();
    output.assert_failure();

    // stderr should mention the file already exists or suggest --force
    let combined = format!("{}{}", output.stdout, output.stderr);
    assert!(
        combined.contains("already exists") || combined.contains("--force"),
        "Expected error about existing config or --force hint, got stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

// ============================================================================
// config set / config show
// ============================================================================

/// `flowplane config set base_url <value>` should persist and be visible via `config show`.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_config_set_base_url() {
    let harness = dev_harness("dev_cli_cfg_set").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let new_url = "http://localhost:9999";

    // Set base_url
    let set_output = cli.run(&["config", "set", "base_url", new_url]).unwrap();
    set_output.assert_success();
    set_output.assert_stdout_contains("Base URL set to");

    // Verify via config show --output json
    let show_output = cli.run(&["config", "show", "--output", "json"]).unwrap();
    show_output.assert_success();
    assert!(
        show_output.stdout.contains(new_url),
        "config show should contain the new base_url '{}', got: {}",
        new_url,
        show_output.stdout
    );
}

/// `flowplane config set team <value>` should persist.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_config_set_team() {
    let harness = dev_harness("dev_cli_cfg_set_team").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let set_output = cli.run(&["config", "set", "team", "my-new-team"]).unwrap();
    set_output.assert_success();
    set_output.assert_stdout_contains("Team set to");

    // Verify via config show
    let show_output = cli.run(&["config", "show", "--output", "json"]).unwrap();
    show_output.assert_success();
    assert!(
        show_output.stdout.contains("my-new-team"),
        "config show should contain 'my-new-team', got: {}",
        show_output.stdout
    );
}

/// `flowplane config set` with an invalid key should fail.
/// clap validates the key against a fixed set, so this should be rejected.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_config_set_invalid_key() {
    let harness = dev_harness("dev_cli_cfg_set_bad").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["config", "set", "nonexistent_key", "value"]).unwrap();
    output.assert_failure();

    // clap should reject the invalid key
    let combined = format!("{}{}", output.stdout, output.stderr);
    assert!(
        combined.contains("invalid value")
            || combined.contains("error")
            || combined.contains("Error"),
        "Expected error for invalid config key, got stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

// ============================================================================
// auth logout
// ============================================================================

/// `flowplane auth logout` should clear credentials so `auth whoami` fails.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_auth_logout_clears_credentials() {
    let harness = dev_harness("dev_cli_auth_logout").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // First verify we have a working token by running a command that requires auth
    let before = cli.run(&["config", "show"]).unwrap();
    before.assert_success();

    // Run logout
    let logout_output = cli.run(&["auth", "logout"]).unwrap();
    logout_output.assert_success();
    logout_output.assert_stdout_contains("Logged out");

    // After logout, `auth whoami` should fail (no credentials)
    let whoami_output = cli.run(&["auth", "whoami"]).unwrap();
    whoami_output.assert_failure();

    let combined = format!("{}{}", whoami_output.stdout, whoami_output.stderr);
    assert!(
        combined.contains("No credentials")
            || combined.contains("credentials")
            || combined.contains("login"),
        "Expected auth whoami to mention missing credentials after logout, got stdout={}, stderr={}",
        whoami_output.stdout,
        whoami_output.stderr
    );
}

/// `flowplane auth logout` when already logged out should still succeed (idempotent).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_auth_logout_when_already_logged_out() {
    let harness = dev_harness("dev_cli_auth_logout2").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Logout first time
    let first = cli.run(&["auth", "logout"]).unwrap();
    first.assert_success();

    // Logout second time — should not crash or error
    let second = cli.run(&["auth", "logout"]).unwrap();
    second.assert_success();
    second.assert_stdout_contains("Logged out");
}

// ============================================================================
// wasm download
// ============================================================================

/// `flowplane wasm download` with a nonexistent ID should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_download_nonexistent_id() {
    let harness = dev_harness("dev_cli_wasm_dl_bad").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output =
        cli.run(&["wasm", "download", "nonexistent-filter-id", "-o", "/tmp/test.wasm"]).unwrap();
    output.assert_failure();

    let combined = format!("{}{}", output.stdout, output.stderr);
    assert!(
        combined.contains("error")
            || combined.contains("Error")
            || combined.contains("not found")
            || combined.contains("Not Found")
            || combined.contains("404"),
        "Expected error for nonexistent WASM filter, got stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

/// `flowplane wasm download` without required args should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_download_missing_args() {
    let harness = dev_harness("dev_cli_wasm_dl_noarg").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Missing both ID and output path
    let output = cli.run(&["wasm", "download"]).unwrap();
    output.assert_failure();
}
