//! S1 — CLI output-model conformance (black-box).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the documented output contract (CLI-R-10/11/12/15/16). They never look at CLI internals:
//! every assertion is derived from the acceptance criteria, not the implementation.
//!
//! Contract under test:
//!   * Every `-o json` success payload is exactly `{schemaVersion, kind, data}` (CLI-R-15).
//!   * `version`, `cluster delete`, `cluster list`, and `apply` all honour `-o json`.
//!   * `--json` is byte-for-byte identical to `-o json` for the same command (CLI-R-10/11).
//!   * Under a captured (non-TTY) subprocess and no `-o`, reader output is the JSON envelope;
//!     an explicit `-o table` overrides that with non-JSON text (CLI-R-12).
//!   * Table output carries zero ANSI escape bytes under `NO_COLOR=1` / `--no-color` (CLI-R-16).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;
use std::process::Output;

use serde_json::Value;

/// Assert the payload is a JSON object with *exactly* the three envelope keys and nothing else.
/// Returns the parsed value for further inspection.
fn assert_exact_envelope(stdout: &[u8]) -> Value {
    let v: Value = serde_json::from_slice(stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not valid JSON ({e}): {:?}",
            String::from_utf8_lossy(stdout)
        )
    });
    let obj = v
        .as_object()
        .unwrap_or_else(|| panic!("envelope is not a JSON object: {v}"));
    let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    let expected: std::collections::BTreeSet<&str> =
        ["schemaVersion", "kind", "data"].into_iter().collect();
    assert_eq!(
        keys, expected,
        "envelope must have EXACTLY {{schemaVersion, kind, data}}, got keys: {keys:?}"
    );
    assert!(
        obj["schemaVersion"].is_i64() || obj["schemaVersion"].is_u64(),
        "schemaVersion must be an integer, got {}",
        obj["schemaVersion"]
    );
    assert!(
        obj["kind"].is_string(),
        "kind must be a string, got {}",
        obj["kind"]
    );
    v
}

/// Loosely assert envelope shape (three keys present, integer schemaVersion, string kind) without
/// forbidding extra keys. Used where the contract only requires the keys to be present.
fn assert_envelope_present(stdout: &[u8]) -> Value {
    let v: Value = serde_json::from_slice(stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not valid JSON ({e}): {:?}",
            String::from_utf8_lossy(stdout)
        )
    });
    let obj = v
        .as_object()
        .unwrap_or_else(|| panic!("envelope is not a JSON object: {v}"));
    for k in ["schemaVersion", "kind", "data"] {
        assert!(obj.contains_key(k), "envelope missing key `{k}`: {v}");
    }
    assert!(
        obj["schemaVersion"].is_i64() || obj["schemaVersion"].is_u64(),
        "schemaVersion must be an integer, got {}",
        obj["schemaVersion"]
    );
    assert!(
        obj["kind"].is_string(),
        "kind must be a string, got {}",
        obj["kind"]
    );
    v
}

fn assert_ok(out: &Output, ctx: &str) {
    assert!(
        out.status.success(),
        "{ctx}: expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !out.stdout.is_empty(),
        "{ctx}: expected non-empty stdout\nstderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
}

fn write_cluster_manifest(home: &Path) -> std::path::PathBuf {
    let path = home.join("alpha.json");
    let manifest = serde_json::json!({
        "kind": "cluster",
        "name": "alpha",
        "team": "payments",
        "spec": { "endpoints": [ { "host": "example.com", "port": 80 } ] }
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap())
        .expect("write apply manifest");
    path
}

// ---------------------------------------------------------------------------------------------
// Criterion 1 + 2: typed JSON envelope, exact three keys, for `cluster list -o json`.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_list_json_is_exact_typed_envelope() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["-o", "json", "cluster", "list", "--team", "payments"])
        .output()
        .expect("run cluster list -o json");

    assert_ok(&out, "cluster list -o json");
    let v = assert_exact_envelope(&out.stdout);

    // Collection kind ends in `List`; data is (or contains) the array.
    let kind = v["kind"].as_str().unwrap();
    assert!(
        kind.ends_with("List"),
        "collection kind must end in `List`, got `{kind}`"
    );
    let data = &v["data"];
    let has_list = data.is_array()
        || data
            .as_object()
            .map(|o| o.values().any(Value::is_array))
            .unwrap_or(false);
    assert!(
        has_list,
        "list envelope `data` must be (or contain) an array, got {data}"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 2: `--json` is byte-for-byte identical to `-o json`.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_flag_is_byte_identical_to_output_json() {
    let mock = common::start_mock().await;

    let home_a = common::unique_tempdir();
    let out_dasho = common::flowplane_cmd(&home_a)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["-o", "json", "cluster", "list", "--team", "payments"])
        .output()
        .expect("run -o json");

    let home_b = common::unique_tempdir();
    let out_json = common::flowplane_cmd(&home_b)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["--json", "cluster", "list", "--team", "payments"])
        .output()
        .expect("run --json");

    assert_ok(&out_dasho, "-o json");
    assert_ok(&out_json, "--json");
    assert_eq!(
        out_dasho.stdout, out_json.stdout,
        "`--json` stdout must be byte-for-byte identical to `-o json`\n  -o json: {}\n  --json : {}",
        String::from_utf8_lossy(&out_dasho.stdout),
        String::from_utf8_lossy(&out_json.stdout),
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 3 + 8: non-TTY default → JSON envelope; explicit `-o table` overrides → non-JSON.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_tty_default_is_json_and_table_flag_overrides() {
    let mock = common::start_mock().await;

    // No `-o` flag: captured subprocess stdout is NOT a TTY, so default is the JSON envelope.
    let home_default = common::unique_tempdir();
    let out_default = common::flowplane_cmd(&home_default)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["cluster", "list", "--team", "payments"])
        .output()
        .expect("run cluster list (no -o)");
    assert_ok(&out_default, "cluster list (no -o)");
    assert_envelope_present(&out_default.stdout);

    // Explicit `-o table` must override the non-TTY default and produce non-JSON table text.
    let home_table = common::unique_tempdir();
    let out_table = common::flowplane_cmd(&home_table)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["-o", "table", "cluster", "list", "--team", "payments"])
        .output()
        .expect("run cluster list -o table");
    assert_ok(&out_table, "cluster list -o table");
    assert!(
        serde_json::from_slice::<Value>(&out_table.stdout).is_err(),
        "`-o table` must produce non-JSON text, but it parsed as JSON: {}",
        String::from_utf8_lossy(&out_table.stdout)
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 4: color off — zero ANSI escape bytes with NO_COLOR=1 and with --no-color.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn table_output_has_no_ansi_escapes() {
    let mock = common::start_mock().await;

    // NO_COLOR=1 set.
    let home_nc = common::unique_tempdir();
    let out_nc = common::flowplane_cmd(&home_nc)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .env("NO_COLOR", "1")
        .args(["-o", "table", "cluster", "list", "--team", "payments"])
        .output()
        .expect("run cluster list -o table (NO_COLOR=1)");
    assert_ok(&out_nc, "cluster list -o table (NO_COLOR=1)");
    assert!(
        !out_nc.stdout.contains(&0x1b),
        "NO_COLOR=1 table output must contain zero ESC (0x1b) bytes: {:?}",
        String::from_utf8_lossy(&out_nc.stdout)
    );

    // --no-color flag.
    let home_flag = common::unique_tempdir();
    let out_flag = common::flowplane_cmd(&home_flag)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "--no-color",
            "-o",
            "table",
            "cluster",
            "list",
            "--team",
            "payments",
        ])
        .output()
        .expect("run cluster list -o table --no-color");
    assert_ok(&out_flag, "cluster list -o table --no-color");
    assert!(
        !out_flag.stdout.contains(&0x1b),
        "--no-color table output must contain zero ESC (0x1b) bytes: {:?}",
        String::from_utf8_lossy(&out_flag.stdout)
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 5: `version -o json` → {schemaVersion, kind:"version", data:{version:"<...>"}}.
// No server needed.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_json_envelope() {
    let home = common::unique_tempdir();
    let out = common::flowplane_cmd(&home)
        .args(["version", "-o", "json"])
        .output()
        .expect("run version -o json");

    assert_ok(&out, "version -o json");
    let v = assert_exact_envelope(&out.stdout);
    assert_eq!(
        v["kind"], "version",
        "version envelope kind must be \"version\""
    );
    let version = v["data"]["version"]
        .as_str()
        .unwrap_or_else(|| panic!("version envelope data.version must be a string: {v}"));
    assert!(
        !version.is_empty(),
        "data.version must be a non-empty string"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 6: `cluster delete alpha --team payments -o json` → JSON envelope (mock returns 204).
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_json_envelope() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args([
            "cluster", "delete", "alpha", "--team", "payments", "--yes", "-o", "json",
        ])
        .output()
        .expect("run cluster delete -o json");

    assert_ok(&out, "cluster delete -o json");
    // Even though the server returns 204 No Content, the CLI must emit a JSON envelope, not prose.
    assert_envelope_present(&out.stdout);
}

// ---------------------------------------------------------------------------------------------
// Criterion 7: `apply -f <manifest> -o json` → applyResult envelope with data.items array.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_json_envelope() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    let manifest = write_cluster_manifest(&home);

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", mock.base_url())
        .env("FLOWPLANE_TOKEN", "t")
        .args(["apply", "-f"])
        .arg(&manifest)
        .args(["-o", "json"])
        .output()
        .expect("run apply -f <manifest> -o json");

    assert_ok(&out, "apply -o json");

    // CLI-R-15: a `-o json` success payload is a SINGLE envelope; stdout must be one parseable
    // JSON document. (If apply also streams per-resource envelopes to stdout, this fails — and it
    // should, because a machine consumer piping stdout into a JSON parser would choke.)
    let mut de = serde_json::Deserializer::from_slice(&out.stdout).into_iter::<Value>();
    let first = de
        .next()
        .expect("apply -o json must emit a JSON envelope")
        .expect("apply -o json stdout must be valid JSON");
    assert!(
        de.next().is_none(),
        "apply -o json must emit exactly ONE JSON envelope on stdout, but found more than one \
         document: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let v = assert_envelope_present(serde_json::to_vec(&first).unwrap().as_slice());
    assert_eq!(
        v["kind"], "applyResult",
        "apply envelope kind must be \"applyResult\""
    );
    assert!(
        v["data"]["items"].is_array(),
        "apply envelope data.items must be an array: {v}"
    );
}
