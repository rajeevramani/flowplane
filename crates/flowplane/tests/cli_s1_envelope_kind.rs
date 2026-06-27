//! S1 — `-o json` envelope `kind` correctness (black-box).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against the
//! documented success-envelope contract (`{schemaVersion, kind, data}`, CLI-R-15). They never
//! look at CLI internals: every assertion is derived from the acceptance criteria, not the
//! implementation.
//!
//! Contract under test (bead fpv2-86m.1): the envelope `kind` must be a correct, non-truncated,
//! non-id value. Two endpoints were previously wrong and are now fixed:
//!   * `mcp status` returned `kind:"statu"` (a truncated "status"); it MUST now be `"mcpStatus"`.
//!   * `unexpose <name>` returned `kind:"<the resource name>"` (e.g. `"local"`); it MUST now be
//!     `"mutationResult"` (a mutation action).
//!
//! Control (known-good): `cluster list -o json` → `kind:"clusterList"`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::process::Output;

use serde_json::Value;

fn exit_code(out: &Output) -> i32 {
    out.status
        .code()
        .unwrap_or_else(|| panic!("process terminated without an exit code (killed by signal?)"))
}

/// Parse stdout as the `{schemaVersion, kind, data}` success envelope, asserting exit 0 and that
/// nothing leaked onto stderr.
fn parse_success_envelope(out: &Output, ctx: &str) -> Value {
    assert_eq!(
        exit_code(out),
        0,
        "{ctx}: expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        out.stderr.is_empty(),
        "{ctx}: nothing must leak to stderr on success, got: {:?}",
        String::from_utf8_lossy(&out.stderr),
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{ctx}: stdout is not a JSON success envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    let obj = v
        .as_object()
        .unwrap_or_else(|| panic!("{ctx}: envelope is not a JSON object: {v}"));
    for k in ["schemaVersion", "kind", "data"] {
        assert!(
            obj.contains_key(k),
            "{ctx}: envelope missing key `{k}`: {v}"
        );
    }
    assert!(
        obj["kind"].is_string(),
        "{ctx}: kind must be a string, got {}",
        obj["kind"]
    );
    v
}

// ---------------------------------------------------------------------------------------------
// `mcp status -o json` → kind MUST be "mcpStatus" (and explicitly NOT the old truncated "statu").
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_status_kind_is_mcp_status_not_truncated() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["mcp", "status", "--team", "payments", "-o", "json"])
        .output()
        .expect("run mcp status -o json");

    let v = parse_success_envelope(&out, "mcp status -o json");
    let kind = v["kind"].as_str().unwrap();
    assert_ne!(
        kind, "statu",
        "regression: mcp status kind must not be the truncated \"statu\": {v}"
    );
    assert_eq!(
        kind, "mcpStatus",
        "mcp status -o json envelope kind must be \"mcpStatus\": {v}"
    );
}

// ---------------------------------------------------------------------------------------------
// `unexpose <name> -o json` → kind MUST be "mutationResult" (and explicitly NOT the resource
// name, which is the bug it previously echoed). `unexpose` is destructive → pass `--yes`
// (subprocess stdin is non-TTY, else the confirm prompt exits 2). `<NAME>` is a positional.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unexpose_kind_is_mutation_result_not_resource_name() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    // A distinctive name so "kind must not equal the name" is a meaningful assertion.
    let name = "local";
    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "unexpose", name, "--team", "payments", "--yes", "-o", "json",
        ])
        .output()
        .expect("run unexpose <name> -o json");

    let v = parse_success_envelope(&out, "unexpose local -o json");
    let kind = v["kind"].as_str().unwrap();
    assert_ne!(
        kind, name,
        "regression: unexpose kind must not echo the resource name \"{name}\": {v}"
    );
    // unexpose is a destructive mutation action → the stable `mutationResult` kind.
    assert_eq!(
        kind, "mutationResult",
        "unexpose -o json envelope kind must be \"mutationResult\": {v}"
    );
}

// ---------------------------------------------------------------------------------------------
// Control: a known-good envelope kind — `cluster list -o json` → "clusterList".
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_list_kind_is_cluster_list_control() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["cluster", "list", "--team", "payments", "-o", "json"])
        .output()
        .expect("run cluster list -o json");

    let v = parse_success_envelope(&out, "cluster list -o json");
    assert_eq!(
        v["kind"], "clusterList",
        "control: cluster list -o json envelope kind must be \"clusterList\": {v}"
    );
}
