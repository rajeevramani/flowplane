//! CLI subprocess smoke tests
//!
//! Validates that the `flowplane` binary can be invoked as a child process
//! with correct argument parsing, output formatting, and exit codes.
//!
//! These tests require `RUN_E2E=1` and a compiled binary (`cargo build`).

use crate::common::cli_runner::CliRunner;
use crate::common::harness::{TestHarness, TestHarnessConfig};

// ============================================================================
// Negative / edge-case tests (no harness needed)
// ============================================================================

#[tokio::test]
#[ignore] // Requires RUN_E2E=1
async fn test_cli_no_args_shows_help() {
    let harness =
        TestHarness::start(TestHarnessConfig::new("cli_no_args").without_envoy()).await.unwrap();
    let cli = CliRunner::from_harness(&harness).unwrap();

    // No subcommand → prints help and exits 0 (per cli/mod.rs None arm)
    let output = cli.run(&[]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("Flowplane Envoy Control Plane");
}

#[tokio::test]
#[ignore] // Requires RUN_E2E=1
async fn test_cli_invalid_subcommand() {
    let harness = TestHarness::start(TestHarnessConfig::new("cli_invalid_subcmd").without_envoy())
        .await
        .unwrap();
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["nonexistent-command"]).unwrap();
    output.assert_failure();
    // clap writes unrecognized subcommand errors to stderr
    output.assert_stderr_contains("error");
}

#[tokio::test]
#[ignore] // Requires RUN_E2E=1
async fn test_cli_without_credentials() {
    let harness =
        TestHarness::start(TestHarnessConfig::new("cli_no_creds").without_envoy()).await.unwrap();

    // Create a CliRunner then delete the credentials file to simulate missing auth
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Remove credentials by creating a new runner with an empty credentials file
    // We need to construct a runner that has config but no credentials
    // The simplest approach: run a command that requires auth (like `cluster list`)
    // with the --token flag pointing to nothing.
    // Actually, let's use with_env to override HOME to a fresh temp dir with no creds.
    let empty_home = tempfile::tempdir().unwrap();
    let fp_dir = empty_home.path().join(".flowplane");
    std::fs::create_dir_all(&fp_dir).unwrap();
    // Write config.toml with base_url but NO credentials file
    let config_content = format!("base_url = \"{}\"\nteam = \"default\"\n", harness.api_url());
    std::fs::write(fp_dir.join("config.toml"), config_content).unwrap();

    let cli = cli.with_env("HOME", &empty_home.path().to_string_lossy());
    let output = cli.run(&["cluster", "list"]).unwrap();
    output.assert_failure();
    // Verify stderr is non-empty — the exact wording may change across versions
    assert!(
        !output.stderr.trim().is_empty() || !output.stdout.trim().is_empty(),
        "Expected non-empty output on failure, got empty stdout and stderr"
    );
}

// ============================================================================
// Positive tests against a running control plane
// ============================================================================

#[tokio::test]
#[ignore] // Requires RUN_E2E=1
async fn test_cli_cluster_list() {
    let harness = TestHarness::start(TestHarnessConfig::new("cli_cluster_list").without_envoy())
        .await
        .unwrap();
    let cli = CliRunner::from_harness(&harness).unwrap();

    // `cluster list` should succeed against the running CP
    let output = cli.run(&["cluster", "list"]).unwrap();
    output.assert_success();
}

#[tokio::test]
#[ignore] // Requires RUN_E2E=1
async fn test_cli_version_flag() {
    let harness =
        TestHarness::start(TestHarnessConfig::new("cli_version").without_envoy()).await.unwrap();
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["--version"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("flowplane");
}
