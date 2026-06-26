//! S2 — CLI error-model conformance (black-box).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the documented error contract (CLI-R-30/31/32/33). They never look at CLI internals: every
//! assertion is derived from the acceptance criteria, not the implementation.
//!
//! Contract under test:
//!   * Every error renders as a structured JSON envelope on **stderr** (never raw `Error: ...`),
//!     and stdout stays empty (CLI-R-30, CLI-R-5/empty-stdout).
//!   * The envelope carries at least `code`, `message`, `retryable` (bool). HTTP errors also
//!     carry the integer `status`; a server-supplied request id surfaces as `request_id`.
//!   * `retryable` classifier (CLI-R-32): 429/503/5xx and transport failures → true;
//!     404 and 400/validation → false.
//!   * Auth hint (CLI-R-33): a 401 with no server `hint` still yields a `hint` mentioning
//!     `flowplane auth login`; a 403 server `hint` naming resource/action survives.
//!   * Exit-code table (CLI-R-31), full 0–7 range: 0 ok, 2 clap usage, 3 auth (401/403),
//!     4 not-found/conflict (404), 5 validation (400), 6 rate-limited (429),
//!     7 server error (503) and transport/connection-refused.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::process::Output;

use serde_json::Value;

/// Run `flowplane cluster get <name> --team payments -o json` against `server`, isolated env.
fn run_cluster_get(server: &str, name: &str) -> Output {
    let home = common::unique_tempdir();
    common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", server)
        .env("FLOWPLANE_TOKEN", "t")
        .args(["cluster", "get", name, "--team", "payments", "-o", "json"])
        .output()
        .unwrap_or_else(|e| panic!("run cluster get {name}: {e}"))
}

/// Run `flowplane cluster delete <name> --team payments -o json` against `server`, isolated env.
fn run_cluster_delete(server: &str, name: &str) -> Output {
    let home = common::unique_tempdir();
    common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", server)
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster", "delete", name, "--team", "payments", "-o", "json",
        ])
        .output()
        .unwrap_or_else(|e| panic!("run cluster delete {name}: {e}"))
}

fn exit_code(out: &Output) -> i32 {
    out.status
        .code()
        .unwrap_or_else(|| panic!("process terminated without an exit code (killed by signal?)"))
}

/// Parse stderr as a single JSON error envelope, asserting nothing leaked onto stdout.
fn parse_error_envelope(out: &Output, ctx: &str) -> Value {
    assert!(
        out.stdout.is_empty(),
        "{ctx}: stdout MUST be empty on error (all error output goes to stderr), got: {:?}",
        String::from_utf8_lossy(&out.stdout),
    );
    let v: Value = serde_json::from_slice(&out.stderr).unwrap_or_else(|e| {
        panic!(
            "{ctx}: stderr is not a JSON error envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let obj = v
        .as_object()
        .unwrap_or_else(|| panic!("{ctx}: error envelope is not a JSON object: {v}"));
    // CLI-R-30: at least `code`, `message`, `retryable` (bool).
    assert!(
        obj.get("code").map(Value::is_string).unwrap_or(false),
        "{ctx}: envelope `code` must be a string: {v}"
    );
    assert!(
        obj.get("message").map(Value::is_string).unwrap_or(false),
        "{ctx}: envelope `message` must be a string: {v}"
    );
    assert!(
        obj.get("retryable").map(Value::is_boolean).unwrap_or(false),
        "{ctx}: envelope `retryable` must be a bool: {v}"
    );
    v
}

// ---------------------------------------------------------------------------------------------
// Criterion 1: structured error envelope on stderr, empty stdout, with HTTP `status` (CLI-R-30).
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_error_is_structured_envelope_on_stderr_empty_stdout() {
    let mock = common::start_mock().await;
    let out = run_cluster_get(mock.base_url(), "err-404");

    // No raw `Error: ...` prose — must be parseable JSON, on stderr, stdout empty.
    let v = parse_error_envelope(&out, "cluster get err-404");

    // HTTP errors carry the integer HTTP status.
    let status = v["status"]
        .as_i64()
        .unwrap_or_else(|| panic!("HTTP error envelope must carry integer `status`: {v}"));
    assert_eq!(status, 404, "status must mirror the HTTP status code: {v}");

    // Sanity: the prose form must NOT be how the error surfaces.
    assert!(
        !String::from_utf8_lossy(&out.stderr).starts_with("Error:"),
        "errors must render as a structured envelope, not a raw `Error: ...` line: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 2: retryable classifier (CLI-R-32) — 429/503 → true, 404/400 → false, transport → true.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retryable_classifier_per_status_class() {
    let mock = common::start_mock().await;

    // (err-name, expected retryable)
    let http_cases: &[(&str, bool)] = &[
        ("err-429", true),  // rate-limited → retryable
        ("err-503", true),  // 5xx → retryable
        ("err-404", false), // not-found → not retryable
        ("err-400", false), // validation → not retryable
    ];
    for (name, want_retryable) in http_cases {
        let out = run_cluster_get(mock.base_url(), name);
        let v = parse_error_envelope(&out, name);
        assert_eq!(
            v["retryable"].as_bool().unwrap(),
            *want_retryable,
            "{name}: retryable must be {want_retryable}: {v}"
        );
    }

    // Transport/connection-refused failure → retryable true (no HTTP status).
    let out = run_cluster_get(&common::dead_base_url(), "anything");
    let v = parse_error_envelope(&out, "transport (connection refused)");
    assert!(
        v["retryable"].as_bool().unwrap(),
        "transport/connection failures must be retryable: {v}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 3a: a 401 with no server hint still synthesizes a `flowplane auth login` hint.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthorized_401_synthesizes_login_hint() {
    let mock = common::start_mock().await;
    let out = run_cluster_get(mock.base_url(), "err-401");
    let v = parse_error_envelope(&out, "cluster get err-401");

    let hint = v["hint"]
        .as_str()
        .unwrap_or_else(|| panic!("401 envelope must carry a synthesized `hint`: {v}"));
    assert!(
        hint.contains("flowplane auth login"),
        "401 hint must point the user at `flowplane auth login`, got: {hint:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 3b: a 403 server hint naming the resource/action survives into the envelope.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_403_server_hint_survives() {
    let mock = common::start_mock().await;
    let out = run_cluster_delete(mock.base_url(), "err-403");
    let v = parse_error_envelope(&out, "cluster delete err-403");

    let hint = v["hint"]
        .as_str()
        .unwrap_or_else(|| panic!("403 envelope must carry the server-supplied `hint`: {v}"));
    assert!(
        hint.contains("clusters") && hint.contains("delete"),
        "403 server hint naming resource/action must survive, got: {hint:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 4: exit-code table (CLI-R-31), the full 0–7 range, adversarially per status class.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_code_table_covers_full_range() {
    let mock = common::start_mock().await;

    // HTTP status class → exit code. Use delete for 403 (its server hint names action=delete).
    assert_eq!(
        exit_code(&run_cluster_get(mock.base_url(), "err-404")),
        4,
        "404 not-found/conflict/precondition → exit 4"
    );
    assert_eq!(
        exit_code(&run_cluster_get(mock.base_url(), "err-400")),
        5,
        "400 validation → exit 5"
    );
    assert_eq!(
        exit_code(&run_cluster_get(mock.base_url(), "err-429")),
        6,
        "429 rate-limited → exit 6"
    );
    assert_eq!(
        exit_code(&run_cluster_get(mock.base_url(), "err-503")),
        7,
        "503 server error → exit 7"
    );
    let code_401 = exit_code(&run_cluster_get(mock.base_url(), "err-401"));
    assert_eq!(code_401, 3, "401 auth → exit 3");
    assert_eq!(
        exit_code(&run_cluster_delete(mock.base_url(), "err-403")),
        3,
        "403 auth → exit 3"
    );

    // Transport/connection-refused → exit 7 (NOT a generic 1).
    let transport = run_cluster_get(&common::dead_base_url(), "anything");
    let code_transport = exit_code(&transport);
    assert_eq!(
        code_transport, 7,
        "connection-refused transport failure → exit 7, not 1"
    );
    assert_ne!(
        code_transport, 1,
        "transport failure must NOT collapse to the generic exit 1"
    );

    // Success → exit 0.
    assert_eq!(
        exit_code(&run_cluster_get(mock.base_url(), "alpha")),
        0,
        "a normal (200) get → exit 0"
    );

    // clap-native usage error (unknown flag) → exit 2; no server needed.
    let home = common::unique_tempdir();
    let usage = common::flowplane_cmd(&home)
        .args(["cluster", "--nonexistent-flag"])
        .output()
        .expect("run clap usage error");
    let code_usage = exit_code(&usage);
    assert_eq!(code_usage, 2, "clap usage error (unknown flag) → exit 2");

    // Adversarial: clap usage (2) and auth 401 (3) must be DISTINCT codes.
    assert_ne!(
        code_usage, code_401,
        "a clap usage error (2) and a 401 (3) must produce different exit codes"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 5: empty stdout on error for both the HTTP and transport error cases.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn error_paths_keep_stdout_empty() {
    let mock = common::start_mock().await;

    let http = run_cluster_get(mock.base_url(), "err-503");
    assert!(
        http.stdout.is_empty(),
        "HTTP error must leave stdout empty, got: {:?}",
        String::from_utf8_lossy(&http.stdout)
    );

    let transport = run_cluster_get(&common::dead_base_url(), "anything");
    assert!(
        transport.stdout.is_empty(),
        "transport error must leave stdout empty, got: {:?}",
        String::from_utf8_lossy(&transport.stdout)
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 1 (transport variant): connection-refused renders the same structured stderr envelope,
// with no HTTP `status` field (there was no HTTP response). Pure transport, but runs the binary.
// ---------------------------------------------------------------------------------------------
#[test]
fn transport_error_is_structured_envelope() {
    let out = run_cluster_get(&common::dead_base_url(), "anything");
    let v = parse_error_envelope(&out, "transport (connection refused)");
    // A transport failure has no HTTP status to report.
    assert!(
        v.get("status").map(Value::is_null).unwrap_or(true),
        "transport error has no HTTP response, so `status` must be absent/null: {v}"
    );
    assert_eq!(exit_code(&out), 7, "transport failure → exit 7");
}

/// CLI-R-30: an `apply` partial/total failure is reported as ONE structured error envelope
/// on stderr (kind-tagged `failures` list), with empty stdout and a non-zero exit — not a
/// per-subrequest pile of envelopes nor an `applyResult` document leaking onto stdout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_partial_failure_is_single_error_envelope_empty_stdout() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    // `err-500` makes the existence check (GET) return 500, so this target fails to apply.
    let manifest = home.join("manifest.json");
    std::fs::write(
        &manifest,
        r#"{"kind":"cluster","name":"err-500","team":"payments","spec":{"endpoints":[{"host":"example.com","port":80}]}}"#,
    )
    .unwrap();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["apply", "-f", manifest.to_str().unwrap(), "-o", "json"])
        .output()
        .unwrap();

    assert!(
        out.stdout.is_empty(),
        "apply failure: stdout MUST be empty (no applyResult leak), got: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stderr).unwrap_or_else(|e| {
        panic!(
            "apply failure: stderr must be ONE JSON error envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(v["code"], "apply_failed", "envelope: {v}");
    assert!(
        v.get("failures").and_then(Value::as_array).is_some(),
        "envelope must list `failures`: {v}"
    );
    assert!(
        !out.status.success(),
        "apply with a failed resource must exit non-zero"
    );
}
