//! E2E boot smoke for the fail-closed startup contract (fpv2-9sf S1): drives the REAL
//! `flowplane-rls` binary (`CARGO_BIN_EXE_*`) with a controlled environment and asserts the
//! documented refusal/boot behavior. Parallel-safe: loopback binds on distinct ephemeral
//! ports, no shared state.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn rls_cmd(envs: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flowplane-rls"));
    cmd.env_clear();
    // Keep the platform basics the process runner needs.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    cmd
}

fn stderr_of(child: Child) -> String {
    let out = child.wait_with_output().unwrap();
    assert!(
        !out.status.success(),
        "process must exit non-zero, got {:?}",
        out.status
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Non-loopback plaintext gRPC bind: the binary refuses to start and names the TLS material.
#[test]
fn non_loopback_plaintext_refuses_to_boot() {
    let child = rls_cmd(&[("FLOWPLANE_RLS_GRPC_LISTEN", "0.0.0.0:0")])
        .spawn()
        .unwrap();
    let err = stderr_of(child);
    assert!(
        err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT"),
        "refusal must name the missing TLS material: {err}"
    );
}

/// Loopback bind without the explicit escape hatch: still refuses (plaintext is opt-in).
#[test]
fn loopback_plaintext_without_hatch_refuses_to_boot() {
    let child = rls_cmd(&[
        ("FLOWPLANE_RLS_GRPC_LISTEN", "127.0.0.1:0"),
        ("FLOWPLANE_RLS_ADMIN_LISTEN", "127.0.0.1:0"),
    ])
    .spawn()
    .unwrap();
    let err = stderr_of(child);
    assert!(
        err.contains("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC"),
        "refusal must name the escape hatch: {err}"
    );
}

/// The documented loopback dev path (hatch set, ephemeral ports) boots and stays up.
#[test]
fn loopback_dev_path_boots_with_hatch() {
    let mut child = rls_cmd(&[
        ("FLOWPLANE_RLS_GRPC_LISTEN", "127.0.0.1:0"),
        ("FLOWPLANE_RLS_ADMIN_LISTEN", "127.0.0.1:0"),
        (
            "FLOWPLANE_RLS_ALLOW_INSECURE_GRPC",
            "yes-this-is-local-only",
        ),
    ])
    .spawn()
    .unwrap();
    // Give it time to fail if it is going to; a fail-closed refusal exits within millis.
    std::thread::sleep(Duration::from_millis(700));
    match child.try_wait().unwrap() {
        None => {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        Some(status) => {
            let out = child.wait_with_output().unwrap();
            panic!(
                "dev path must keep running, exited {status:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
