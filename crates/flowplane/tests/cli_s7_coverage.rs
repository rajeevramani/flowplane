//! S7 — whole-tree CLI coverage capstone (black-box).
//!
//! This is the coverage capstone for the Flowplane CLI conformance epic. The CLI's output
//! rules live in a shared layer (`render()`, the typed `{schemaVersion, kind, data}` envelope,
//! and `GlobalOptions`), so they apply to every command *by construction* — but this file
//! PROVES that coverage instead of assuming it, with NO silent sampling.
//!
//! The serialized clap command tree is read **black-box** from `flowplane schema -o json`
//! (envelope `{schemaVersion, kind:"cliSchema", data:{catalogVersion, command}}`). The root
//! `command` node is recursive: `{name, about, args, subcommands}`; each arg carries the
//! documented keys `name`/`long`/`short`/`type`/`required`/`global`/`takesValue`/`valueNames`/
//! `possibleValues`/`defaults`/`help`. GLOBAL flags (output/json/no-color/quiet/verbose/
//! dry-run/yes/revision/fields/timeout/out/token/server/team/org/context) live on the ROOT
//! command's `args`; per-subcommand `args` hold only that command's own flags.
//!
//! What this file proves:
//!   * Test 1 (criterion 13): a frozen, hand-listed inventory of EVERY leaf command path,
//!     partitioned into SNAPSHOT_COVERED / EXEMPT / SHARED_LAYER_COVERED, whose union must
//!     equal the live leaf set EXACTLY — a new or removed command fails CI loudly.
//!   * Test 2 (criterion 4 / CLI-R-15): frozen `-o json` envelopes for the whole cluster
//!     family, driven against the harness mock — any shape/field drift in the shared envelope
//!     fails CI.
//!   * Test 3 (criterion 13): structural clap-lints read off `flowplane schema` — output is a
//!     universal global, `-o` is format-only, `--revision` stays global, file-driven mutators
//!     carry `-f`.
//!   * Test 4 (criterion 13): `-o`/`--quiet` honoured on a representative command.
//!
//! Parallel-safety (criterion 13a): every test uses a per-test `unique_tempdir()`, a per-test
//! ephemeral-port mock, and per-child env; no shared global state and no `--test-threads=1`,
//! so the suite passes run twice concurrently.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;
use std::process::Output;

use serde_json::{json, Value};

// =============================================================================================
// Frozen leaf inventories (Test 1). The UNION of these three must equal the live leaf set.
// =============================================================================================

/// Leaf paths whose exact `-o json` envelope is frozen live against the mock in Test 2.
const SNAPSHOT_COVERED: &[&str] = &[
    "cluster list",
    "cluster get",
    "cluster create",
    "cluster update",
    "cluster delete",
];

/// Leaf paths that do NOT emit the resource-JSON envelope and so have no envelope to snapshot.
/// Each carries a one-line reason for the exemption.
const EXEMPT: &[&str] = &[
    "serve",                   // long-running daemon, never returns an envelope
    "completion",              // emits a shell completion script, not the envelope
    "db migrate",              // database migration runner
    "auth login",              // interactive OIDC browser/device flow
    "auth logout",             // clears local credentials
    "auth token",              // prints the raw bearer token to stdout
    "auth whoami",             // live-auth identity probe
    "openapi",                 // emits an OpenAPI document, not the envelope
    "dataplane bootstrap",     // emits Envoy bootstrap YAML
    "dataplane cert list",     // PKI/cert material surface
    "dataplane cert register", // PKI/cert material surface
    "dataplane cert issue",    // PKI/cert material surface
    "dataplane cert revoke",   // PKI/cert material surface
    "config path",             // prints a filesystem path
    "config set-context",      // mutates local config (no server envelope)
    "config use-context",      // mutates local config (no server envelope)
    "config get-contexts",     // local-config introspection (covered by S3)
    "config show",             // local-config introspection (covered by S3)
];

/// Resource readers/mutators that flow through the SAME `render()` + RestClient envelope path
/// already proven by the cluster snapshots (Test 2) and their own slice tests. Listing every
/// one is the no-silent-sampling guarantee: the union check below fails if any command is added
/// or removed without being classified here.
const SHARED_LAYER_COVERED: &[&str] = &[
    // org
    "org list",
    "org get",
    "org create",
    "org delete",
    "org member list",
    "org member add",
    "org member remove",
    // team
    "team list",
    "team create",
    "team delete",
    "team member list",
    "team member add",
    "team member remove",
    "team grant list",
    "team grant add",
    "team grant remove",
    // listener
    "listener list",
    "listener get",
    "listener create",
    "listener update",
    "listener delete",
    // route
    "route list",
    "route get",
    "route create",
    "route update",
    "route delete",
    "route generate",
    "route apply",
    // api
    "api list",
    "api get",
    "api status",
    "api create",
    "api delete",
    "api spec reject",
    "api spec publish",
    // mcp
    "mcp status",
    "mcp connections",
    "mcp enable",
    "mcp disable",
    // ai providers
    "ai providers list",
    "ai providers get",
    "ai providers create",
    "ai providers update",
    "ai providers delete",
    // ai routes
    "ai routes list",
    "ai routes get",
    "ai routes create",
    "ai routes update",
    "ai routes delete",
    // ai budgets
    "ai budgets list",
    "ai budgets get",
    "ai budgets create",
    "ai budgets update",
    "ai budgets delete",
    // ai usage
    "ai usage",
    // rate-limit domain
    "rate-limit domain list",
    "rate-limit domain get",
    "rate-limit domain create",
    "rate-limit domain update",
    "rate-limit domain delete",
    // rate-limit policy
    "rate-limit policy list",
    "rate-limit policy get",
    "rate-limit policy create",
    "rate-limit policy update",
    "rate-limit policy delete",
    // rate-limit override
    "rate-limit override get",
    "rate-limit override set",
    "rate-limit override update",
    "rate-limit override delete",
    // rate-limit force-repush
    "rate-limit force-repush",
    // learn discover
    "learn discover start",
    "learn discover list",
    "learn discover status",
    "learn discover stop",
    "learn discover generate-spec",
    // learn
    "learn start",
    "learn list",
    "learn get",
    "learn stop",
    "learn generate-spec",
    "learn cancel",
    // secret
    "secret list",
    "secret get",
    "secret create",
    "secret rotate",
    // dataplane
    "dataplane list",
    "dataplane get",
    "dataplane create",
    "dataplane telemetry",
    // stats
    "stats overview",
    // ops
    "ops xds status",
    "ops xds nacks",
    "ops trace",
    // top-level
    "expose",
    "unexpose",
    "apply",
    "version",
    "schema",
];

// =============================================================================================
// Helpers
// =============================================================================================

fn exit_code(out: &Output) -> i32 {
    out.status
        .code()
        .unwrap_or_else(|| panic!("process terminated without an exit code (killed by signal?)"))
}

/// Run `flowplane schema -o json` with NO server configured (schema makes no network call) and
/// return the parsed envelope. Uses an isolated temp HOME like every other test.
fn live_schema() -> Value {
    let home = common::unique_tempdir();
    let out = common::flowplane_cmd(&home)
        .args(["schema", "-o", "json"])
        .output()
        .expect("run schema -o json");
    assert_eq!(
        exit_code(&out),
        0,
        "schema -o json must exit 0; stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "schema stdout is not a JSON envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The recursive root `command` node from a parsed schema envelope.
fn root_command(schema: &Value) -> &Value {
    let cmd = &schema["data"]["command"];
    assert!(
        cmd.is_object(),
        "data.command must be the root command object: {schema}"
    );
    cmd
}

/// Collect every LEAF command path (space-joined) from the command tree. A leaf is a node with
/// an empty `subcommands` array. The root itself is excluded (its name is the binary).
fn collect_leaves(command: &Value) -> BTreeSet<String> {
    fn walk(node: &Value, path: &[String], acc: &mut BTreeSet<String>) {
        let subs = node["subcommands"].as_array().cloned().unwrap_or_default();
        if subs.is_empty() {
            if !path.is_empty() {
                acc.insert(path.join(" "));
            }
            return;
        }
        for s in &subs {
            let name = s["name"]
                .as_str()
                .unwrap_or_else(|| panic!("each subcommand must have a string `name`: {s}"));
            let mut next = path.to_vec();
            next.push(name.to_string());
            walk(s, &next, acc);
        }
    }
    let mut acc = BTreeSet::new();
    walk(command, &[], &mut acc);
    acc
}

/// Walk every command node (root + all descendants), invoking `f(path, node)` for each.
fn for_each_command<F: FnMut(&str, &Value)>(command: &Value, mut f: F) {
    fn walk<F: FnMut(&str, &Value)>(node: &Value, path: &[String], f: &mut F) {
        f(&path.join(" "), node);
        if let Some(subs) = node["subcommands"].as_array() {
            for s in subs {
                let name = s["name"].as_str().unwrap_or_default();
                let mut next = path.to_vec();
                next.push(name.to_string());
                walk(s, &next, f);
            }
        }
    }
    walk(command, &[], &mut f);
}

/// Find a leaf command node by its space-joined path.
fn find_command<'a>(command: &'a Value, target: &str) -> Option<&'a Value> {
    fn walk<'a>(node: &'a Value, path: &[String], target: &str) -> Option<&'a Value> {
        if path.join(" ") == target {
            return Some(node);
        }
        if let Some(subs) = node["subcommands"].as_array() {
            for s in subs {
                let name = s["name"].as_str().unwrap_or_default();
                let mut next = path.to_vec();
                next.push(name.to_string());
                if let Some(found) = walk(s, &next, target) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(command, &[], target)
}

/// True if this command node declares its own arg with `long == long` and `short == short`.
fn has_arg(node: &Value, long: &str, short: Option<&str>) -> bool {
    node["args"]
        .as_array()
        .map(|args| {
            args.iter().any(|a| {
                a["long"].as_str() == Some(long)
                    && (short.is_none() || a["short"].as_str() == short)
            })
        })
        .unwrap_or(false)
}

/// Parse a child's stdout as the JSON success envelope, asserting exit 0.
fn parse_envelope(out: &Output, ctx: &str) -> Value {
    assert_eq!(
        exit_code(out),
        0,
        "{ctx}: expected exit 0, got {:?}; stderr: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{ctx}: stdout is not a JSON envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Write the minimal valid-JSON body file the mock ignores (no `name` → mock defaults to
/// `created`), return its path string.
fn write_body(home: &std::path::Path) -> String {
    let path = home.join("body.json");
    std::fs::write(&path, r#"{"spec":{"endpoints":[]}}"#).expect("write body file");
    path.to_str().expect("body path is utf-8").to_string()
}

// =============================================================================================
// Test 1 — Frozen leaf inventory (criterion 13): union of the three lists == live leaf set.
// =============================================================================================
#[test]
fn frozen_leaf_inventory_equals_live_tree() {
    let schema = live_schema();
    let live = collect_leaves(root_command(&schema));

    // The three inventories must be internally disjoint (no leaf classified twice).
    let mut union: BTreeSet<String> = BTreeSet::new();
    for (label, list) in [
        ("SNAPSHOT_COVERED", SNAPSHOT_COVERED),
        ("EXEMPT", EXEMPT),
        ("SHARED_LAYER_COVERED", SHARED_LAYER_COVERED),
    ] {
        for &leaf in list {
            assert!(
                union.insert(leaf.to_string()),
                "leaf `{leaf}` is classified in more than one inventory (found again in \
                 {label}); each leaf must appear in exactly ONE list"
            );
        }
    }

    // The core no-silent-sampling guarantee: union == live, exactly.
    let missing_from_lists: Vec<&String> = live.difference(&union).collect();
    let stale_in_lists: Vec<&String> = union.difference(&live).collect();

    assert!(
        missing_from_lists.is_empty() && stale_in_lists.is_empty(),
        "CLI leaf inventory drifted from the live `flowplane schema` tree.\n\
         \n  NEW leaves present live but NOT classified (add a snapshot or classify each into \
         SNAPSHOT_COVERED / EXEMPT / SHARED_LAYER_COVERED): {missing_from_lists:?}\n\
         \n  STALE leaves in the inventory but NO LONGER live (remove them): {stale_in_lists:?}\n\
         \n  live leaf count = {}, classified leaf count = {}",
        live.len(),
        union.len(),
    );
}

// =============================================================================================
// Test 2 — Live `-o json` snapshots for the cluster family (criterion 4 / CLI-R-15).
// A field/shape diff in the shared envelope fails CI. Discovered `kind` values:
//   list → "clusterList"  |  get/create/update → "cluster"  |  delete → "mutationResult".
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_family_json_envelopes_are_frozen() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    let body = write_body(&home);

    let run = |args: Vec<String>| {
        common::flowplane_cmd(&home)
            .env("FLOWPLANE_SERVER", mock.base_url())
            .env("FLOWPLANE_TOKEN", "t")
            .args(&args)
            .output()
            .expect("run cluster command")
    };

    // cluster list → clusterList, data is the two-item array.
    let list = run(vec![
        "cluster".into(),
        "list".into(),
        "--team".into(),
        "payments".into(),
        "-o".into(),
        "json".into(),
    ]);
    assert_eq!(
        parse_envelope(&list, "cluster list"),
        json!({
            "schemaVersion": 1,
            "kind": "clusterList",
            "data": [
                { "name": "alpha", "revision": 1, "service_name": "alpha-svc" },
                { "name": "beta", "revision": 2, "service_name": "beta-svc" }
            ]
        }),
        "cluster list -o json envelope drifted"
    );

    // cluster get alpha → cluster, single object.
    let get = run(vec![
        "cluster".into(),
        "get".into(),
        "alpha".into(),
        "--team".into(),
        "payments".into(),
        "-o".into(),
        "json".into(),
    ]);
    assert_eq!(
        parse_envelope(&get, "cluster get alpha"),
        json!({
            "schemaVersion": 1,
            "kind": "cluster",
            "data": { "name": "alpha", "revision": 1, "service_name": "svc" }
        }),
        "cluster get -o json envelope drifted"
    );

    // cluster create -f <body> → cluster; mock derives name from body (none → "created").
    let create = run(vec![
        "cluster".into(),
        "create".into(),
        "--team".into(),
        "payments".into(),
        "-f".into(),
        body.clone(),
        "-o".into(),
        "json".into(),
    ]);
    assert_eq!(
        parse_envelope(&create, "cluster create"),
        json!({
            "schemaVersion": 1,
            "kind": "cluster",
            "data": { "name": "created", "revision": 1 }
        }),
        "cluster create -o json envelope drifted"
    );

    // cluster update demo -f <body> --revision 1 → cluster; applied_revision echoes If-Match (1).
    let update = run(vec![
        "cluster".into(),
        "update".into(),
        "demo".into(),
        "--team".into(),
        "payments".into(),
        "-f".into(),
        body.clone(),
        "--revision".into(),
        "1".into(),
        "-o".into(),
        "json".into(),
    ]);
    assert_eq!(
        parse_envelope(&update, "cluster update demo"),
        json!({
            "schemaVersion": 1,
            "kind": "cluster",
            "data": { "name": "demo", "revision": 2, "applied_revision": 1 }
        }),
        "cluster update -o json envelope drifted"
    );

    // cluster delete alpha --yes --revision 1 → mutationResult (mock returns 204). `--yes` is
    // required because the subprocess stdin is non-TTY (else the confirm prompt exits 2).
    let delete = run(vec![
        "cluster".into(),
        "delete".into(),
        "alpha".into(),
        "--team".into(),
        "payments".into(),
        "--yes".into(),
        "--revision".into(),
        "1".into(),
        "-o".into(),
        "json".into(),
    ]);
    assert_eq!(
        parse_envelope(&delete, "cluster delete alpha"),
        json!({
            "schemaVersion": 1,
            "kind": "mutationResult",
            "data": {
                "method": "DELETE",
                "path": "/api/v1/teams/payments/clusters/alpha",
                "result": "ok"
            }
        }),
        "cluster delete -o json envelope drifted"
    );
}

// =============================================================================================
// Test 3 — clap-lints (criterion 13): structural invariants, read black-box off the schema.
// =============================================================================================
#[test]
fn clap_lints_hold_across_the_whole_tree() {
    let schema = live_schema();
    let command = root_command(&schema);

    // ---- output-flag-universal: the root has a global `--output`/`-o` of type enum. ----
    let output = command["args"]
        .as_array()
        .and_then(|args| args.iter().find(|a| a["long"].as_str() == Some("output")))
        .unwrap_or_else(|| panic!("root command must declare a global `--output` arg"));
    assert_eq!(
        output["short"].as_str(),
        Some("o"),
        "global --output must have short `-o`: {output}"
    );
    assert_eq!(
        output["type"].as_str(),
        Some("enum"),
        "global --output must be of type `enum`: {output}"
    );
    assert_eq!(
        output["global"].as_bool(),
        Some(true),
        "--output must be a global so it applies to every command: {output}"
    );

    // ---- o-is-format-only: the ONLY arg anywhere whose short == "o" is the root `output`. ----
    let mut o_claimers: Vec<(String, String)> = Vec::new();
    for_each_command(command, |path, node| {
        if let Some(args) = node["args"].as_array() {
            for a in args {
                if a["short"].as_str() == Some("o") {
                    let where_ = if path.is_empty() {
                        "ROOT".to_string()
                    } else {
                        path.to_string()
                    };
                    o_claimers.push((where_, a["long"].as_str().unwrap_or("?").to_string()));
                }
            }
        }
    });
    assert_eq!(
        o_claimers,
        vec![("ROOT".to_string(), "output".to_string())],
        "the short flag `-o` must be claimed by exactly ONE arg (root `--output`); other \
         claimants collide with the format flag: {o_claimers:?}"
    );

    // ---- revision-uniform: `--revision` is a global integer on root, applying to every ----
    // update/delete by construction. The ONLY legitimate per-subcommand re-declaration is
    // `secret rotate`, where revision is a required local positional of the rotate semantic.
    let revision = command["args"]
        .as_array()
        .and_then(|args| args.iter().find(|a| a["long"].as_str() == Some("revision")))
        .unwrap_or_else(|| panic!("root command must declare a global `--revision` arg"));
    assert_eq!(
        revision["global"].as_bool(),
        Some(true),
        "--revision must be a global so every update/delete accepts it: {revision}"
    );
    assert_eq!(
        revision["type"].as_str(),
        Some("integer"),
        "--revision must be of type `integer`: {revision}"
    );
    let mut local_revisions: BTreeSet<String> = BTreeSet::new();
    for_each_command(command, |path, node| {
        if path.is_empty() {
            return; // the root's global revision is expected.
        }
        if let Some(args) = node["args"].as_array() {
            if args.iter().any(|a| a["long"].as_str() == Some("revision")) {
                local_revisions.insert(path.to_string());
            }
        }
    });
    let expected_local: BTreeSet<String> = ["secret rotate".to_string()].into_iter().collect();
    assert_eq!(
        local_revisions, expected_local,
        "`--revision` must stay global and NOT be re-declared per-subcommand, with the single \
         documented exception of `secret rotate` (required local revision). Unexpected/duplicated \
         declarations: {local_revisions:?}"
    );

    // ---- file-driven mutators carry `-f`: every file-bearing leaf is a known body-driven ----
    // create/update/rotate/set/telemetry/register/apply, each with short `f`; readers must not.
    // (NOTE: not every `create` leaf takes -f — org/team/api/dataplane create use flags, not a
    // manifest body — so the lint pins the EXACT file-driven set rather than "all creates".)
    let mut file_bearing: BTreeSet<String> = BTreeSet::new();
    for leaf in collect_leaves(command) {
        let node = find_command(command, &leaf).expect("leaf exists in tree");
        if node["args"]
            .as_array()
            .map(|args| args.iter().any(|a| a["long"].as_str() == Some("file")))
            .unwrap_or(false)
        {
            // Every file arg must use the `-f` short form.
            assert!(
                has_arg(node, "file", Some("f")),
                "`{leaf}` declares a `--file` arg but not with short `-f`: {node}"
            );
            file_bearing.insert(leaf);
        }
    }
    let expected_file_bearing: BTreeSet<String> = [
        "apply",
        "cluster create",
        "cluster update",
        "listener create",
        "listener update",
        "route create",
        "route update",
        "ai providers create",
        "ai providers update",
        "ai routes create",
        "ai routes update",
        "ai budgets create",
        "ai budgets update",
        "rate-limit domain create",
        "rate-limit domain update",
        "rate-limit policy create",
        "rate-limit policy update",
        "rate-limit override set",
        "rate-limit override update",
        "secret create",
        "secret rotate",
        "dataplane telemetry",
        "dataplane cert register",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        file_bearing, expected_file_bearing,
        "the set of leaves carrying a `-f`/`--file` arg drifted; every file-driven mutator must \
         keep `-f` and no reader may gain one"
    );

    // Spot-check the canonical body-driven mutators positively...
    for leaf in ["cluster create", "cluster update", "secret rotate"] {
        let node = find_command(command, leaf).expect("leaf exists");
        assert!(
            has_arg(node, "file", Some("f")),
            "`{leaf}` must carry a per-command `-f`/`--file` arg: {node}"
        );
    }
    // ...and confirm readers do NOT require a file body.
    for leaf in ["cluster list", "cluster get", "cluster delete"] {
        let node = find_command(command, leaf).expect("leaf exists");
        assert!(
            !has_arg(node, "file", None),
            "reader `{leaf}` must NOT declare a `--file` arg: {node}"
        );
    }
}

// =============================================================================================
// Test 4 — `-o` / `--quiet` honoured on a representative command (criterion 13).
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_format_and_quiet_are_honoured() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();

    let run = |extra: &[&str]| {
        let mut args = vec!["cluster", "list", "--team", "payments"];
        args.extend_from_slice(extra);
        common::flowplane_cmd(&home)
            .env("FLOWPLANE_SERVER", mock.base_url())
            .env("FLOWPLANE_TOKEN", "t")
            .args(&args)
            .output()
            .expect("run cluster list")
    };

    // `-o json` parses as the data envelope.
    let json_out = run(&["-o", "json"]);
    let json_env = parse_envelope(&json_out, "cluster list -o json");
    assert_eq!(
        json_env["kind"], "clusterList",
        "json kind must be clusterList"
    );

    // `-o yaml` differs from json: the same payload rendered as YAML is NOT valid JSON.
    let yaml_out = run(&["-o", "yaml"]);
    assert_eq!(
        exit_code(&yaml_out),
        0,
        "cluster list -o yaml must exit 0; stderr: {:?}",
        String::from_utf8_lossy(&yaml_out.stderr)
    );
    assert_ne!(
        yaml_out.stdout, json_out.stdout,
        "`-o yaml` output must differ from `-o json`"
    );
    assert!(
        serde_json::from_slice::<Value>(&yaml_out.stdout).is_err(),
        "`-o yaml` must NOT parse as JSON: {:?}",
        String::from_utf8_lossy(&yaml_out.stdout)
    );

    // `-o json --quiet` still emits the data envelope on stdout (quiet silences chrome, not the
    // requested data), and stdout is exactly that one JSON document with no extra prose.
    let quiet_out = run(&["-o", "json", "--quiet"]);
    let quiet_env = parse_envelope(&quiet_out, "cluster list -o json --quiet");
    assert_eq!(
        quiet_env["kind"], "clusterList",
        "`--quiet` must not suppress the requested data envelope"
    );
    assert!(
        quiet_env["data"].is_array(),
        "`--quiet` data envelope must still carry the list array: {quiet_env}"
    );
    // No trailing prose: stdout is a single parseable JSON document and nothing else.
    let mut docs = serde_json::Deserializer::from_slice(&quiet_out.stdout).into_iter::<Value>();
    assert!(
        docs.next().is_some(),
        "quiet stdout must contain the envelope"
    );
    assert!(
        docs.next().is_none(),
        "quiet stdout must be exactly ONE JSON document with no extra prose: {:?}",
        String::from_utf8_lossy(&quiet_out.stdout)
    );
}

// =============================================================================================
// Test 5 — chk:nonempty-about (CLI-R-05): every command node (root + every descendant) must
// render a non-empty one-line `about`. Walks the serialized clap tree from `flowplane schema
// -o json` and FAILS, naming EVERY offender, if any node has a missing/null/empty/whitespace
// `about` — so adding a command with no `///` doc summary fails CI loudly.
// =============================================================================================
#[test]
fn chk_nonempty_about() {
    let schema = live_schema();
    let command = root_command(&schema);

    // Collect EVERY offending command path (do not stop at the first) so a developer sees the
    // full list. The root has path "" — label it explicitly for a readable message.
    let mut offenders: Vec<String> = Vec::new();
    for_each_command(command, |path, node| {
        let ok = node["about"]
            .as_str()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false); // null / non-string / absent all fail
        if !ok {
            let where_ = if path.is_empty() {
                "<root>".to_string()
            } else {
                path.to_string()
            };
            offenders.push(where_);
        }
    });

    assert!(
        offenders.is_empty(),
        "CLI-R-05 chk:nonempty-about: every command and sub-command must render a non-empty \
         one-line `about` (add a `///` doc summary). Commands with a missing/null/empty/\
         whitespace-only `about`: {offenders:?}"
    );
}

// =============================================================================================
// Test 6 — chk:nonempty-arg-help (CLI-R-07): every flag and positional argument of every command
// (root + every descendant) must render a non-empty `help` description. Walks the serialized clap
// tree from `flowplane schema -o json` and FAILS, naming EVERY offender, if any arg has a
// missing/null/empty/whitespace `help` — so adding a new flag/positional with no `///` doc summary
// fails CI loudly.
//
// NOTE on clap globals: clap serializes GLOBAL args (output/server/team/revision/...) ONLY on the
// ROOT command's `args`, never repeated on descendant subcommands. So walking each node's OWN
// `args` and asserting non-empty help is correct and complete — globals are covered once at the
// root, per-command args at their own node. The clap builtin `--help`/`--version` do NOT appear in
// the schema, so there is nothing to exempt.
// =============================================================================================
#[test]
fn chk_nonempty_arg_help() {
    let schema = live_schema();
    let command = root_command(&schema);

    // Collect EVERY offending arg (do not stop at the first) so a developer sees the full list.
    // Each offender is identified as "<command path> :: <arg name>" — the root uses `<root>`.
    let mut offenders: Vec<String> = Vec::new();
    for_each_command(command, |path, node| {
        if let Some(args) = node["args"].as_array() {
            for arg in args {
                let ok = arg["help"]
                    .as_str()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false); // null / non-string / absent all fail
                if !ok {
                    let where_ = if path.is_empty() { "<root>" } else { path };
                    let arg_name = arg["name"].as_str().unwrap_or("<unnamed>");
                    offenders.push(format!("{where_} :: {arg_name}"));
                }
            }
        }
    });

    assert!(
        offenders.is_empty(),
        "CLI-R-07 chk:nonempty-arg-help: every flag and positional argument of every command must \
         render a non-empty `help` description (add a `///` doc summary). Args with a \
         missing/null/empty/whitespace-only `help`: {offenders:?}"
    );
}
