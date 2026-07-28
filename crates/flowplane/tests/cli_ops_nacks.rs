//! S2 (fpv2-55x.2) — `ops xds nacks` window flags, black-box.
//!
//! Drives the built `flowplane` binary as a subprocess against the mock server and asserts only
//! the documented contract: `--since/--until/--limit/--before` become the right query string, the
//! new `{items, window_total, next_cursor}` envelope is surfaced in `-o json`, an unfiltered call
//! hits the endpoint with no query, and a malformed `--since` (mock 400) is a non-zero exit with a
//! rendered error. Assertions come from the acceptance criteria, not the CLI internals.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::process::Output;

use serde_json::Value;

fn exit_code(out: &Output) -> i32 {
    out.status
        .code()
        .unwrap_or_else(|| panic!("process terminated without an exit code (killed by signal?)"))
}

fn success_data(out: &Output, ctx: &str) -> Value {
    assert_eq!(
        exit_code(out),
        0,
        "{ctx}: expected exit 0\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{ctx}: stdout not JSON ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    v["data"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nacks_window_flags_build_the_query_and_surface_the_envelope() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "ops",
            "xds",
            "nacks",
            "--team",
            "payments",
            "--since",
            "2026-07-24T00:00:00Z",
            "--until",
            "2026-07-25T00:00:00Z",
            "--limit",
            "2",
            "--before",
            "2026-07-25T00:00:00.000000000+00:00,019f9999-0000-7000-8000-000000000009",
            "-o",
            "json",
        ])
        .output()
        .expect("run ops xds nacks with window flags");

    let data = success_data(&out, "ops xds nacks (filtered)");
    let received = data["received_query"]
        .as_str()
        .expect("received_query echoed by mock");
    // Every flag must reach the URL as a query parameter (order-independent assertions).
    assert!(
        received.contains("since=2026-07-24T00%3A00%3A00Z"),
        "since missing/unencoded: {received}"
    );
    assert!(
        received.contains("until=2026-07-25T00%3A00%3A00Z"),
        "until missing: {received}"
    );
    assert!(received.contains("limit=2"), "limit missing: {received}");
    // Assert the FULL encoded cursor round-trips (comma percent-encoded), not just the key.
    assert!(
        received.contains(
            "before=2026-07-25T00%3A00%3A00.000000000%2B00%3A00%2C019f9999-0000-7000-8000-000000000009"
        ),
        "before cursor not sent verbatim/encoded: {received}"
    );
    // The new envelope shape must be surfaced, not just the item rows.
    assert_eq!(
        data["window_total"],
        serde_json::json!(7),
        "window_total must be surfaced: {data}"
    );
    assert!(
        data["next_cursor"].is_string(),
        "next_cursor must be surfaced: {data}"
    );
    assert!(data["items"].is_array(), "items must be an array: {data}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nacks_unfiltered_sends_no_query() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["ops", "xds", "nacks", "--team", "payments", "-o", "json"])
        .output()
        .expect("run ops xds nacks unfiltered");

    let data = success_data(&out, "ops xds nacks (unfiltered)");
    // No flags → bare path, empty query string.
    assert_eq!(
        data["received_query"].as_str().unwrap_or(""),
        "",
        "unfiltered call must send no query string: {data}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nacks_malformed_since_is_a_client_error() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    // The mock returns 400 for since=err-400; the CLI must surface a non-zero exit + error body.
    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "ops", "xds", "nacks", "--team", "payments", "--since", "err-400", "-o", "json",
        ])
        .output()
        .expect("run ops xds nacks with bad since");

    assert_ne!(
        exit_code(&out),
        0,
        "a 400 from the server must be a non-zero CLI exit"
    );
    // The error must be surfaced to the operator, not swallowed.
    assert!(
        !out.stderr.is_empty(),
        "a client error must render an error message to stderr, got empty stderr"
    );
}
