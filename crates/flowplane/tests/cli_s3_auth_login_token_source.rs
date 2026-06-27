//! S3 — `auth login` credential-input precedence (black-box).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against the
//! documented behavior contract for `auth login` (GH #193 F-2 / "Fix 3"). They never look at CLI
//! internals: every assertion is derived from the acceptance matrix, not the implementation.
//!
//! Contract under test:
//!   * The GLOBAL `--token` flag is bound to env `FLOWPLANE_TOKEN`, but an AMBIENT
//!     `FLOWPLANE_TOKEN` in the environment MUST NOT count as an explicit login *method*. Only an
//!     explicit `--token <v>` on the command line conflicts with `--token-stdin` / `--device` /
//!     `--pkce`.
//!   * `auth login` login methods (`--token <v>`, `--token-stdin`, `--device`/`--device-code`,
//!     `--pkce`) are mutually exclusive. More than one explicit method →
//!     `use only one login input: --token, --token-stdin, --device-code, or --pkce` (exit 2).
//!   * No method chosen and nothing available →
//!     `pass --token, --token-stdin, --device-code, or configure OIDC for PKCE` (exit 1).
//!   * On success the bearer token is written to the credentials file (path echoed as
//!     `token saved to <path>` on stdout) and exit is 0. The saved file content is exactly the
//!     token string.
//!   * An ambient `FLOWPLANE_TOKEN` with no method flags is a valid sole fallback (saved as-is).
//!
//! Observation technique: each case uses a UNIQUE recognizable token value per source
//! (`flag-tok-*`, `stdin-tok-*`, `env-tok-*`) so the saved credential proves WHICH source won.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Output, Stdio};

const CONFLICT_MSG: &str =
    "use only one login input: --token, --token-stdin, --device-code, or --pkce";
const NOTHING_MSG: &str = "pass --token, --token-stdin, --device-code, or configure OIDC for PKCE";

/// Parse the `token saved to <path>` line from stdout, then read+trim that file's content.
/// Asserts exit 0. Returns the saved token string (trimmed).
fn saved_token(out: &Output, ctx: &str) -> String {
    assert!(
        out.status.success(),
        "{ctx}: auth login must exit 0, got {:?}; stderr: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("token saved to "))
        .unwrap_or_else(|| {
            panic!("{ctx}: stdout must contain `token saved to <path>`, got: {stdout:?}")
        });
    let path = line.trim();
    let bytes =
        std::fs::read(path).unwrap_or_else(|e| panic!("{ctx}: read saved credentials {path}: {e}"));
    String::from_utf8_lossy(&bytes).trim().to_string()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Run `auth login` with the given args, feeding `stdin_token` (if `Some`) to the child's stdin.
/// `env_token` sets/omits the ambient `FLOWPLANE_TOKEN`.
fn run_login(
    home: &Path,
    env_token: Option<&str>,
    args: &[&str],
    stdin_token: Option<&str>,
) -> Output {
    let mut cmd = common::flowplane_cmd(home);
    cmd.arg("auth").arg("login");
    cmd.args(args);
    if let Some(tok) = env_token {
        cmd.env("FLOWPLANE_TOKEN", tok);
    }
    match stdin_token {
        Some(tok) => {
            cmd.stdin(Stdio::piped());
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
            let mut child = cmd.spawn().expect("spawn flowplane auth login");
            child
                .stdin
                .take()
                .expect("child stdin")
                .write_all(tok.as_bytes())
                .expect("write token to child stdin");
            child.wait_with_output().expect("wait_with_output")
        }
        None => cmd.output().expect("run flowplane auth login"),
    }
}

// 1. env SET + --token-stdin → exit 0; saved == stdin value (NOT env). [primary bug fix]
#[test]
fn env_set_token_stdin_saves_stdin_value() {
    let home = common::unique_tempdir();
    let out = run_login(
        &home,
        Some("env-tok-1"),
        &["--token-stdin"],
        Some("stdin-tok-1"),
    );
    assert_eq!(saved_token(&out, "row1"), "stdin-tok-1");
}

// 2. env UNSET + --token-stdin → exit 0; saved == stdin value.
#[test]
fn env_unset_token_stdin_saves_stdin_value() {
    let home = common::unique_tempdir();
    let out = run_login(&home, None, &["--token-stdin"], Some("stdin-tok-2"));
    assert_eq!(saved_token(&out, "row2"), "stdin-tok-2");
}

// 3. env SET + explicit --token <v> + --token-stdin → exit 2; conflict. [explicit token conflicts]
#[test]
fn env_set_explicit_token_and_stdin_conflicts() {
    let home = common::unique_tempdir();
    let out = run_login(
        &home,
        Some("env-tok-3"),
        &["--token", "flag-tok-3", "--token-stdin"],
        Some("stdin-tok-3"),
    );
    assert!(!out.status.success(), "row3: must fail");
    assert_eq!(out.status.code(), Some(2), "row3: exit code");
    let err = stderr_of(&out);
    assert!(
        err.contains(CONFLICT_MSG),
        "row3: stderr must contain conflict message, got: {err:?}"
    );
}

// 4. env UNSET + explicit --token <v> + --token-stdin → exit 2; conflict.
#[test]
fn env_unset_explicit_token_and_stdin_conflicts() {
    let home = common::unique_tempdir();
    let out = run_login(
        &home,
        None,
        &["--token", "flag-tok-4", "--token-stdin"],
        Some("stdin-tok-4"),
    );
    assert!(!out.status.success(), "row4: must fail");
    assert_eq!(out.status.code(), Some(2), "row4: exit code");
    let err = stderr_of(&out);
    assert!(
        err.contains(CONFLICT_MSG),
        "row4: stderr must contain conflict message, got: {err:?}"
    );
}

// 5. env SET, NO method flags → exit 0; saved == env value. [env is a valid sole fallback]
#[test]
fn env_set_no_method_saves_env_value() {
    let home = common::unique_tempdir();
    let out = run_login(&home, Some("env-tok-5"), &[], None);
    assert_eq!(saved_token(&out, "row5"), "env-tok-5");
}

// 6. env UNSET, NO method flags, no OIDC config → exit 1; "nothing available" message.
#[test]
fn env_unset_no_method_no_oidc_errors() {
    let home = common::unique_tempdir();
    let out = run_login(&home, None, &[], None);
    assert!(!out.status.success(), "row6: must fail");
    assert_eq!(out.status.code(), Some(1), "row6: exit code");
    let err = stderr_of(&out);
    assert!(
        err.contains(NOTHING_MSG),
        "row6: stderr must contain nothing-available message, got: {err:?}"
    );
}

// 7. env UNSET + explicit --token <v> only → exit 0; saved == flag value.
#[test]
fn env_unset_explicit_token_only_saves_flag_value() {
    let home = common::unique_tempdir();
    let out = run_login(&home, None, &["--token", "flag-tok-7"], None);
    assert_eq!(saved_token(&out, "row7"), "flag-tok-7");
}

// 8. env SET + explicit --token <v> only → exit 0; saved == flag value (flag beats env).
#[test]
fn env_set_explicit_token_only_saves_flag_value() {
    let home = common::unique_tempdir();
    let out = run_login(&home, Some("env-tok-8"), &["--token", "flag-tok-8"], None);
    assert_eq!(saved_token(&out, "row8"), "flag-tok-8");
}

// 9. env SET + --device (unreachable OIDC) → non-zero exit; NOT a conflict error. [device path]
#[test]
fn env_set_device_attempts_network_not_conflict() {
    let home = common::unique_tempdir();
    let out = run_login(
        &home,
        Some("env-tok-9"),
        &[
            "--device",
            "--issuer",
            "http://127.0.0.1:1",
            "--client-id",
            "x",
        ],
        None,
    );
    assert!(!out.status.success(), "row9: must fail (no reachable OIDC)");
    let err = stderr_of(&out);
    assert!(
        !err.contains(CONFLICT_MSG),
        "row9: ambient env token must NOT count as a 2nd method; stderr: {err:?}"
    );
}

// 10. env SET + --pkce (unreachable OIDC) → non-zero exit; NOT a conflict error. [pkce path]
#[test]
fn env_set_pkce_attempts_network_not_conflict() {
    let home = common::unique_tempdir();
    let out = run_login(
        &home,
        Some("env-tok-10"),
        &[
            "--pkce",
            "--issuer",
            "http://127.0.0.1:1",
            "--client-id",
            "x",
        ],
        None,
    );
    assert!(
        !out.status.success(),
        "row10: must fail (no reachable OIDC)"
    );
    let err = stderr_of(&out);
    assert!(
        !err.contains(CONFLICT_MSG),
        "row10: ambient env token must NOT count as a 2nd method; stderr: {err:?}"
    );
}

// 11. --device --pkce (env unset) → exit 2; conflict. [two real methods conflict]
#[test]
fn device_and_pkce_conflicts() {
    let home = common::unique_tempdir();
    let out = run_login(
        &home,
        None,
        &[
            "--device",
            "--pkce",
            "--issuer",
            "http://127.0.0.1:1",
            "--client-id",
            "x",
        ],
        None,
    );
    assert!(!out.status.success(), "row11: must fail");
    assert_eq!(out.status.code(), Some(2), "row11: exit code");
    let err = stderr_of(&out);
    assert!(
        err.contains(CONFLICT_MSG),
        "row11: stderr must contain conflict message, got: {err:?}"
    );
}

// 12. env SET + explicit --token v + --device → exit 2; conflict.
#[test]
fn env_set_explicit_token_and_device_conflicts() {
    let home = common::unique_tempdir();
    let out = run_login(
        &home,
        Some("env-tok-12"),
        &[
            "--token",
            "flag-tok-12",
            "--device",
            "--issuer",
            "http://127.0.0.1:1",
            "--client-id",
            "x",
        ],
        None,
    );
    assert!(!out.status.success(), "row12: must fail");
    assert_eq!(out.status.code(), Some(2), "row12: exit code");
    let err = stderr_of(&out);
    assert!(
        err.contains(CONFLICT_MSG),
        "row12: stderr must contain conflict message, got: {err:?}"
    );
}
