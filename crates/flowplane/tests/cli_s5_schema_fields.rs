//! S5 — CLI introspection: `schema` subcommand + `--fields` projection (black-box).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the documented output contract (CLI-R-50/51). They never look at CLI internals: every
//! assertion is derived from the acceptance criteria, not the implementation.
//!
//! Contract under test:
//!   * `flowplane schema -o json` (CLI-R-50) prints the machine-readable CLI catalog with NO
//!     network call. Envelope is `{schemaVersion, kind, data}` with `kind == "cliSchema"`,
//!     integer `data.catalogVersion`, and `data.command` the recursive root command tree
//!     (`name`, `about`, `args`, `subcommands`). Each arg has the documented arg-shape keys.
//!     The catalog contains EVERY top-level command (25 of them) including `schema` itself.
//!   * `--fields a,b,c` (CLI-R-51) projects reader output to exactly those keys INSIDE `data`
//!     (per item for lists). The envelope `schemaVersion`/`kind` always survive; an absent
//!     requested key is omitted (no null injected).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;
use std::process::Output;

use serde_json::Value;

fn assert_ok(out: &Output, ctx: &str) {
    assert!(
        out.status.success(),
        "{ctx}: expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

fn parse_envelope(stdout: &[u8]) -> Value {
    serde_json::from_slice(stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not valid JSON ({e}): {:?}",
            String::from_utf8_lossy(stdout)
        )
    })
}

fn key_set(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .unwrap_or_else(|| panic!("expected a JSON object, got {v}"))
        .keys()
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// CLI-R-50: `flowplane schema -o json` — machine-readable catalog, no network call.
// ---------------------------------------------------------------------------------------------
#[test]
fn schema_is_valid_cli_schema_envelope_no_network() {
    // No mock, and deliberately NO `FLOWPLANE_SERVER`/token: schema must make no network call.
    let home = common::unique_tempdir();
    let out = common::flowplane_cmd(&home)
        .args(["schema", "-o", "json"])
        .output()
        .expect("run schema -o json");

    assert_ok(&out, "schema -o json");
    // No server is configured, so any attempt to reach the network would surface as an error.
    assert!(
        out.stderr.is_empty(),
        "schema must produce no stderr (no network, no error): {:?}",
        String::from_utf8_lossy(&out.stderr),
    );

    let v = parse_envelope(&out.stdout);
    let obj = v
        .as_object()
        .unwrap_or_else(|| panic!("envelope is not a JSON object: {v}"));

    // Standard envelope keys present, with integer schemaVersion + string kind.
    for k in ["schemaVersion", "kind", "data"] {
        assert!(obj.contains_key(k), "envelope missing key `{k}`: {v}");
    }
    assert!(
        obj["schemaVersion"].is_i64() || obj["schemaVersion"].is_u64(),
        "schemaVersion must be an integer, got {}",
        obj["schemaVersion"]
    );
    assert_eq!(
        v["kind"], "cliSchema",
        "schema envelope kind must be \"cliSchema\""
    );

    let data = &v["data"];

    // catalogVersion is an integer, and distinct field from the envelope schemaVersion.
    assert!(
        data["catalogVersion"].is_i64() || data["catalogVersion"].is_u64(),
        "data.catalogVersion must be an integer, got {}",
        data["catalogVersion"]
    );

    // data.command is the root command tree: name/about/args/subcommands.
    let command = &data["command"];
    for k in ["name", "about", "args", "subcommands"] {
        assert!(
            command.get(k).is_some(),
            "data.command must have `{k}`: {command}"
        );
    }
    assert!(
        command["args"].is_array(),
        "data.command.args must be an array, got {}",
        command["args"]
    );
    assert!(
        command["subcommands"].is_array(),
        "data.command.subcommands must be an array, got {}",
        command["subcommands"]
    );

    // The catalog contains EVERY top-level command, including `schema` itself.
    let subs = command["subcommands"].as_array().unwrap();
    let names: BTreeSet<&str> = subs
        .iter()
        .map(|s| {
            s["name"]
                .as_str()
                .unwrap_or_else(|| panic!("each subcommand must have a string `name`: {s}"))
        })
        .collect();
    for required in ["cluster", "listener", "route", "apply", "version", "schema"] {
        assert!(
            names.contains(required),
            "catalog must list top-level command `{required}` (no self-exemption); got {names:?}"
        );
    }
    assert_eq!(
        subs.len(),
        25,
        "catalog must list EXACTLY 25 top-level commands, got {}: {names:?}",
        subs.len()
    );

    // Drill into one arg and confirm the documented arg-shape keys exist. Prefer the global
    // `--output`/`-o` arg, but fall back to any arg present in the tree.
    let arg = find_any_arg(command).unwrap_or_else(|| {
        panic!("expected at least one arg somewhere in the command tree: {command}")
    });
    for k in [
        "name",
        "long",
        "short",
        "help",
        "required",
        "global",
        "takesValue",
        "valueNames",
        "possibleValues",
        "defaults",
    ] {
        assert!(
            arg.get(k).is_some(),
            "arg object must have documented key `{k}`: {arg}"
        );
    }
}

/// Walk the command tree and return the first arg object found. Prefers the global
/// `--output`/`-o` arg if present anywhere, otherwise any arg.
fn find_any_arg(command: &Value) -> Option<Value> {
    let mut fallback: Option<Value> = None;
    fn walk(command: &Value, fallback: &mut Option<Value>) -> Option<Value> {
        if let Some(args) = command["args"].as_array() {
            for a in args {
                let long = a["long"].as_str();
                let short = a["short"].as_str();
                if long == Some("output") || short == Some("o") {
                    return Some(a.clone());
                }
                if fallback.is_none() {
                    *fallback = Some(a.clone());
                }
            }
        }
        if let Some(subs) = command["subcommands"].as_array() {
            for s in subs {
                if let Some(found) = walk(s, fallback) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(command, &mut fallback).or(fallback)
}

// ---------------------------------------------------------------------------------------------
// CLI-R-51: `--fields` projects list items inside `data`, per item.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fields_projects_list_items_inside_data() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster",
            "list",
            "--team",
            "payments",
            "-o",
            "json",
            "--fields",
            "name,revision",
        ])
        .output()
        .expect("run cluster list --fields name,revision");

    assert_ok(&out, "cluster list --fields name,revision");
    let v = parse_envelope(&out.stdout);

    // Envelope schemaVersion + kind survive projection.
    assert!(
        v["schemaVersion"].is_i64() || v["schemaVersion"].is_u64(),
        "schemaVersion must survive projection: {v}"
    );
    assert!(v["kind"].is_string(), "kind must survive projection: {v}");

    // `cluster list` is Page-backed: --fields projects each item inside `data.items`.
    let data = v["data"]["items"].as_array().unwrap_or_else(|| {
        panic!(
            "Page-backed list `data.items` must be an array: {}",
            v["data"]
        )
    });
    assert!(
        !data.is_empty(),
        "mock list must return at least one item: {v}"
    );

    let expected: BTreeSet<String> = ["name".to_string(), "revision".to_string()]
        .into_iter()
        .collect();
    for (i, item) in data.iter().enumerate() {
        assert_eq!(
            key_set(item),
            expected,
            "data[{i}] must have EXACTLY {{name, revision}} (service_name projected away), got {item}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// CLI-R-51: `--fields` projects a single GET object inside `data`.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fields_projects_single_object() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster", "get", "demo", "--team", "payments", "-o", "json", "--fields", "name",
        ])
        .output()
        .expect("run cluster get demo --fields name");

    assert_ok(&out, "cluster get --fields name");
    let v = parse_envelope(&out.stdout);

    assert!(v["kind"].is_string(), "kind must survive projection: {v}");
    let expected: BTreeSet<String> = ["name".to_string()].into_iter().collect();
    assert_eq!(
        key_set(&v["data"]),
        expected,
        "single-object `data` must have ONLY {{name}}, got {}",
        v["data"]
    );
}

// ---------------------------------------------------------------------------------------------
// CLI-R-51: a requested key that is absent is OMITTED, not injected as null.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fields_absent_key_is_omitted_not_null() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster",
            "get",
            "demo",
            "--team",
            "payments",
            "-o",
            "json",
            "--fields",
            "name,does_not_exist",
        ])
        .output()
        .expect("run cluster get demo --fields name,does_not_exist");

    assert_ok(&out, "cluster get --fields name,does_not_exist");
    let v = parse_envelope(&out.stdout);

    let data = v["data"]
        .as_object()
        .unwrap_or_else(|| panic!("single-object `data` must be an object: {}", v["data"]));
    let expected: BTreeSet<String> = ["name".to_string()].into_iter().collect();
    assert_eq!(
        key_set(&v["data"]),
        expected,
        "absent requested key must be OMITTED, leaving ONLY {{name}}, got {}",
        v["data"]
    );
    assert!(
        !data.contains_key("does_not_exist"),
        "`does_not_exist` must NOT be present (not even as null): {}",
        v["data"]
    );
}
