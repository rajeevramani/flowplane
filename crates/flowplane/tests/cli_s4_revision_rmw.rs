//! S4 — CLI revision / read-modify-write conformance (black-box).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the documented `--revision` contract (CLI-R-47). They never look at CLI internals: every
//! assertion is derived from the acceptance criteria, not the implementation.
//!
//! Contract under test (CLI-R-47), `--revision` uniform on every update/delete:
//!   1. Read-modify-write (no `--revision`): an update/delete with no explicit `--revision`
//!      first READS the resource's current revision, then sends it as the `If-Match`
//!      precondition — so concurrent edits are detected instead of last-write-wins.
//!   2. Explicit `--revision N`: sent as the `If-Match` precondition directly.
//!   3. Stale revision → 409 naming BOTH revisions: the conflict surfaces the structured error
//!      envelope on stderr with `attempted_revision` (what the CLI sent) AND the server's
//!      current revision (carried in the envelope `message`), exit code 4, empty stdout.
//!
//! Observation technique (harness mock): `cluster update <name>` succeeds with
//! `data.applied_revision` echoing the `If-Match` the CLI actually sent (null if none). A GET
//! of any normal cluster returns `revision: 1`. The reserved name `conflict` returns HTTP 409
//! `{"code":"conflict","message":"revision mismatch: current revision is 7"}` regardless of the
//! sent revision.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::process::Output;

use serde_json::Value;

/// Write a minimal valid-JSON body file the mock ignores, return its path string.
fn write_body(home: &std::path::Path) -> String {
    let path = home.join("body.json");
    std::fs::write(&path, r#"{"spec":{"endpoints":[]}}"#).expect("write body file");
    path.to_str().expect("body path is utf-8").to_string()
}

fn exit_code(out: &Output) -> i32 {
    out.status
        .code()
        .unwrap_or_else(|| panic!("process terminated without an exit code (killed by signal?)"))
}

/// Parse stdout as the `{schemaVersion,kind,data}` success envelope.
fn parse_success_envelope(out: &Output, ctx: &str) -> Value {
    assert!(
        out.status.success(),
        "{ctx}: expected exit 0, got {:?}; stderr: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{ctx}: stdout is not a JSON success envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Parse stderr as a single JSON error envelope, asserting nothing leaked onto stdout.
fn parse_error_envelope(out: &Output, ctx: &str) -> Value {
    assert!(
        out.stdout.is_empty(),
        "{ctx}: stdout MUST be empty on error, got: {:?}",
        String::from_utf8_lossy(&out.stdout),
    );
    serde_json::from_slice(&out.stderr).unwrap_or_else(|e| {
        panic!(
            "{ctx}: stderr is not a JSON error envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

// ---------------------------------------------------------------------------------------------
// Criterion 1: read-modify-write — no `--revision` makes the CLI READ the current revision (1)
// and send it as `If-Match`. Asserting exactly 1 proves a real GET happened, not a hardcode.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rmw_sends_current_revision_when_no_flag() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    let body = write_body(&home);

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster", "update", "demo", "--team", "payments", "-f", &body, "-o", "json",
        ])
        .output()
        .expect("run cluster update demo (rmw)");

    let v = parse_success_envelope(&out, "cluster update demo (rmw)");
    let applied = v["data"]["applied_revision"].as_i64().unwrap_or_else(|| {
        panic!("rmw: data.applied_revision must be an integer (the read revision): {v}")
    });
    assert_eq!(
        applied, 1,
        "rmw: with no --revision the CLI must READ the current revision (1) and send it as \
         If-Match — a hardcoded/absent precondition would not echo 1: {v}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 2: an explicit `--revision N` is sent verbatim as `If-Match` (no read needed).
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_revision_is_sent_as_if_match() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    let body = write_body(&home);

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster",
            "update",
            "demo",
            "--team",
            "payments",
            "-f",
            &body,
            "--revision",
            "9",
            "-o",
            "json",
        ])
        .output()
        .expect("run cluster update demo --revision 9");

    let v = parse_success_envelope(&out, "cluster update demo --revision 9");
    let applied = v["data"]["applied_revision"]
        .as_i64()
        .unwrap_or_else(|| panic!("explicit: data.applied_revision must be an integer: {v}"));
    assert_eq!(
        applied, 9,
        "explicit --revision 9 must be sent verbatim as the If-Match precondition: {v}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 3: a stale revision on update → 409 surfacing BOTH the attempted revision (3, what
// the CLI sent) and the server's current revision (7, in the envelope message), exit 4, empty
// stdout. Adversarial: assert exit is exactly 4 (not 1/2), and both numbers appear.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_revision_update_conflict_names_both() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    let body = write_body(&home);

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster",
            "update",
            "conflict",
            "--team",
            "payments",
            "-f",
            &body,
            "--revision",
            "3",
            "-o",
            "json",
        ])
        .output()
        .expect("run cluster update conflict --revision 3");

    assert_eq!(
        exit_code(&out),
        4,
        "stale-revision conflict (409) must exit 4 (conflict/precondition), not 1/2: stderr {:?}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v = parse_error_envelope(&out, "cluster update conflict --revision 3");

    let attempted = v["attempted_revision"].as_i64().unwrap_or_else(|| {
        panic!("conflict envelope must carry integer `attempted_revision` (what CLI sent): {v}")
    });
    assert_eq!(
        attempted, 3,
        "attempted_revision must be the revision the CLI sent (3): {v}"
    );

    let message = v["message"]
        .as_str()
        .unwrap_or_else(|| panic!("conflict envelope must carry a string `message`: {v}"));
    assert!(
        message.contains('7'),
        "conflict message must name the server's current revision (7): {message:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 4: same stale-revision conflict semantics on `delete` (no body file needed).
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_revision_delete_conflict_names_both() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster",
            "delete",
            "conflict",
            "--team",
            "payments",
            "--revision",
            "3",
            "-o",
            "json",
        ])
        .output()
        .expect("run cluster delete conflict --revision 3");

    assert_eq!(
        exit_code(&out),
        4,
        "stale-revision delete conflict (409) must exit 4, not 1/2: stderr {:?}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v = parse_error_envelope(&out, "cluster delete conflict --revision 3");

    let attempted = v["attempted_revision"].as_i64().unwrap_or_else(|| {
        panic!("conflict envelope must carry integer `attempted_revision`: {v}")
    });
    assert_eq!(
        attempted, 3,
        "attempted_revision must be the revision the CLI sent (3): {v}"
    );

    let message = v["message"]
        .as_str()
        .unwrap_or_else(|| panic!("conflict envelope must carry a string `message`: {v}"));
    assert!(
        message.contains('7'),
        "conflict message must name the server's current revision (7): {message:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 5: sanity — `--revision` parses and is accepted on BOTH update and delete; a normal
// name succeeds (delete returns 204 → still exit 0).
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revision_flag_accepted_on_update_and_delete() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    let body = write_body(&home);

    let update = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster",
            "update",
            "demo",
            "--team",
            "payments",
            "-f",
            &body,
            "--revision",
            "1",
            "-o",
            "json",
        ])
        .output()
        .expect("run cluster update demo --revision 1");
    assert_eq!(
        exit_code(&update),
        0,
        "update with --revision 1 must parse and succeed: stderr {:?}",
        String::from_utf8_lossy(&update.stderr)
    );

    let delete = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster",
            "delete",
            "demo",
            "--team",
            "payments",
            "--revision",
            "1",
            "-o",
            "json",
        ])
        .output()
        .expect("run cluster delete demo --revision 1");
    assert_eq!(
        exit_code(&delete),
        0,
        "delete with --revision 1 must parse and succeed (204): stderr {:?}",
        String::from_utf8_lossy(&delete.stderr)
    );
}
