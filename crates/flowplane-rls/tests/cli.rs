//! Black-box CLI contract tests for `flowplane-rls` (fpv2-lrd.1).
//!
//! These tests execute the real Cargo-built binary, remove ambient RLS TLS and
//! plaintext acknowledgement settings, and never bind or open a port.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::process::{Command, Output};

const STARTUP_GUARD_ENV_VARS: &[&str] = &[
    "FLOWPLANE_RLS_GRPC_TLS_CERT",
    "FLOWPLANE_RLS_GRPC_TLS_KEY",
    "FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA",
    "FLOWPLANE_RLS_ADMIN_TLS_CERT",
    "FLOWPLANE_RLS_ADMIN_TLS_KEY",
    "FLOWPLANE_RLS_ALLOW_INSECURE_GRPC",
    "FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN",
];

fn run_rls(args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flowplane-rls"));
    command.args(args);
    for variable in STARTUP_GUARD_ENV_VARS {
        command.env_remove(variable);
    }
    command
        .output()
        .expect("run the Cargo-built flowplane-rls binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn version_succeeds_without_startup_configuration() {
    let output = run_rls(&["--version"]);

    assert!(
        output.status.success(),
        "--version must exit successfully without startup configuration; status={:?}, stderr={}",
        output.status,
        stderr(&output)
    );
    assert_eq!(
        stdout(&output).trim(),
        format!("flowplane-rls {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_succeeds_and_describes_the_product_without_startup_configuration() {
    let output = run_rls(&["--help"]);
    let stdout = stdout(&output);

    assert!(
        output.status.success(),
        "--help must exit successfully without startup configuration; status={:?}, stderr={}",
        output.status,
        stderr(&output)
    );
    assert!(
        stdout.contains("Usage:"),
        "help must contain usage: {stdout}"
    );
    assert!(
        stdout
            .to_ascii_lowercase()
            .contains("global rate-limit service"),
        "help must describe the product purpose: {stdout}"
    );
}

#[test]
fn unknown_argument_is_rejected_before_startup_validation() {
    let output = run_rls(&["--definitely-unknown"]);
    let stderr = stderr(&output);

    assert!(
        !output.status.success(),
        "an unknown argument must exit non-zero; stdout={}",
        stdout(&output)
    );
    assert!(
        stderr.to_ascii_lowercase().contains("unexpected argument"),
        "stderr must identify the unexpected argument: {stderr}"
    );
    for startup_guard in STARTUP_GUARD_ENV_VARS {
        assert!(
            !stderr.contains(startup_guard),
            "CLI parsing must precede the RLS TLS/plaintext startup guard; stderr={stderr}"
        );
    }
}
