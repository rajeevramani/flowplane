//! S6 — CLI destructive confirmation conformance (black-box).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the documented confirmation contract (CLI-R-22 / CLI-R-26). They never look at CLI
//! internals: every assertion is derived from the acceptance criteria, not the implementation.
//!
//! Contract under test:
//!   * CLI-R-22: Destructive actions (e.g. `cluster delete <name>`) confirm on a TTY;
//!     `--yes`/`-y` bypasses the confirmation. A declared `--yes` must gate a real
//!     confirmation (no dead flag).
//!   * CLI-R-26: When stdin is NOT a TTY, no command ever blocks on a prompt. A destructive
//!     action on a non-TTY WITHOUT `--yes` fails fast with exit code 2 and a hint to pass
//!     `--yes`, and must NEVER read/block on stdin. Example:
//!     `flowplane cluster delete db < /dev/null`
//!     → stderr contains `error (confirmation_required): ... pass --yes`, exit 2.
//!
//! Scope boundary — PTY path NOT covered here: the interactive `[y/N]` "y" acceptance path
//! requires a real PTY. `std::process::Command::output()` gives the child a NULL (closed,
//! non-TTY) stdin, so only the non-TTY paths are exercised here. The interactive accept path
//! is covered by unit tests; a PTY dependency is not worth pulling in for this slice.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::process::{Output, Stdio};

fn exit_code(out: &Output) -> i32 {
    out.status
        .code()
        .unwrap_or_else(|| panic!("process terminated without an exit code (killed by signal?)"))
}

// ---------------------------------------------------------------------------------------------
// Case 1: `--yes` skips confirm on a non-TTY → delete proceeds (exit 0). Proves `--yes` is NOT
// a dead flag — it bypasses the confirmation guard and the (204) network call succeeds.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn yes_skips_confirm_and_delete_proceeds() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster", "delete", "alpha", "--team", "payments", "--yes", "-o", "json",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("run cluster delete alpha --yes");

    assert_eq!(
        exit_code(&out),
        0,
        "--yes must bypass confirmation on non-TTY and let the delete (204) succeed (exit 0): \
         stderr {:?}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !out.stdout.is_empty(),
        "--yes delete must emit a JSON success envelope on stdout, got empty: stderr {:?}",
        String::from_utf8_lossy(&out.stderr),
    );
}

// ---------------------------------------------------------------------------------------------
// Case 2: non-TTY WITHOUT `--yes` → fail fast exit 2, never blocks. The captured subprocess
// already has a non-TTY stdin; we set `Stdio::null()` explicitly to be unambiguous. The CLI
// must refuse rather than prompt: exit 2, `confirmation_required` + `--yes` on stderr, empty
// stdout. (`.output()` waits for the child, so the test returning at all proves it did not
// block on stdin — the timeout-bounded liveness check.)
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_tty_without_yes_fails_fast_exit_2() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster", "delete", "alpha", "--team", "payments", "-o", "json",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("run cluster delete alpha (no --yes, non-TTY)");

    assert_eq!(
        exit_code(&out),
        2,
        "destructive action on non-TTY without --yes must fail fast with exit 2: stderr {:?}",
        String::from_utf8_lossy(&out.stderr),
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("confirmation_required"),
        "stderr must carry the `confirmation_required` error code: {stderr:?}",
    );
    assert!(
        stderr.contains("--yes"),
        "stderr must hint to pass `--yes`: {stderr:?}",
    );
    assert!(
        out.stdout.is_empty(),
        "confirmation-required error must keep stdout empty, got: {:?}",
        String::from_utf8_lossy(&out.stdout),
    );
}

// ---------------------------------------------------------------------------------------------
// Case 3: the short alias `-y` also skips confirmation (exit 0). The spec says `--yes/-y`, so
// if `-y` is not wired as a valid alias this fails — a legitimate contract check.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn short_y_alias_skips_confirm() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster", "delete", "alpha", "--team", "payments", "-y", "-o", "json",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("run cluster delete alpha -y");

    assert_eq!(
        exit_code(&out),
        0,
        "-y (short alias of --yes) must bypass confirmation and let the delete succeed (exit 0): \
         stderr {:?}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !out.stdout.is_empty(),
        "-y delete must emit a JSON success envelope on stdout, got empty: stderr {:?}",
        String::from_utf8_lossy(&out.stderr),
    );
}

// ---------------------------------------------------------------------------------------------
// Case 4: confirmation fires BEFORE the network call. Point the CLI at a DEAD server and delete
// with NO `--yes` on a non-TTY: the guard must short-circuit with exit 2 / `confirmation_required`
// rather than ever reaching the (failing) transport — a transport error would be a different
// exit code. This locks in "confirm gates before network".
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirm_gates_before_network() {
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", common::dead_base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster", "delete", "alpha", "--team", "payments", "-o", "json",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("run cluster delete alpha (no --yes) against a dead server");

    assert_eq!(
        exit_code(&out),
        2,
        "confirmation guard must short-circuit BEFORE any network call (exit 2, not a transport \
         error exit code): stderr {:?}",
        String::from_utf8_lossy(&out.stderr),
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("confirmation_required"),
        "stderr must carry `confirmation_required` (proving confirm ran before network): {stderr:?}",
    );
    assert!(
        out.stdout.is_empty(),
        "confirmation-required error must keep stdout empty, got: {:?}",
        String::from_utf8_lossy(&out.stdout),
    );
}
