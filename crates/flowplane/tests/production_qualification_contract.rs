//! fpv2-d23.2 — exact-artifact qualification contract (independent black-box tests).
//!
//! Independence boundary: this suite is derived only from the approved 3.1.3 design/plan,
//! checked-in public artifacts, and existing integration-test conventions. It drives the built
//! `flowplane` binary and never imports or reads qualification implementation modules.
//!
//! Public contract proposed by these acceptance tests:
//! `flowplane qualification {inventory,validate,assemble} --input <json> [--output-path <json>]`.
//! Validation failures are non-zero and identify the rejected JSON field on stderr. Successful
//! commands write canonical JSON to `--output-path`; running inventory generation twice from identical
//! exact-artifact inputs must produce byte-identical output.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const CANARY: &str = "fp-secret-canary-d23-2-DO-NOT-LEAK-7f41c8";
const REPOSITORY_URL: &str = "https://github.com/rajeevramani/flowplane";
const MILESTONE_URL: &str = "https://github.com/rajeevramani/flowplane/milestone/1";

static SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after Unix epoch")
            .as_nanos();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "flowplane-qualification-contract-{}-{nanos}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create isolated test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write_json(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.0.join(name);
        fs::write(
            &path,
            serde_json::to_vec_pretty(value).expect("serialize fixture"),
        )
        .expect("write fixture");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn flowplane() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flowplane"));
    command.env_clear().env("PATH", "/usr/bin:/bin");
    command
}

fn run(args: &[&str]) -> Output {
    flowplane()
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run flowplane {args:?}: {error}"))
}

fn assert_ok(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}: expected success, got {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_rejected(output: &Output, field: &str) {
    assert!(
        !output.status.success(),
        "invalid contract unexpectedly passed for `{field}`\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(field),
        "rejection must identify `{field}`; stderr:\n{stderr}"
    );
}

fn validate(document: &Value) -> Output {
    let temp = TempDir::new();
    let input = temp.write_json("contract.json", document);
    run(&[
        "qualification",
        "validate",
        "--input",
        input.to_str().expect("UTF-8 fixture path"),
    ])
}

fn evidence_dimensions() -> Value {
    json!({
        "configuration": ["evidence/config.json"],
        "runtime": ["evidence/runtime.json"],
        "isolation": ["evidence/isolation.json"],
        "diagnostics": ["evidence/diagnostics.json"],
        "presentation": ["evidence/presentation.json"],
        "cleanup": ["evidence/cleanup.json"]
    })
}

fn valid_contract() -> Value {
    json!({
        "schema_version": 1,
        "artifact": {
            "release": "3.1.3",
            "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        },
        "inventory": [{
            "id": "cli.cluster.list",
            "classification": "supported-core",
            "classification_rationale": "Stable CLI and documentation contract",
            "observed_in": ["cli-schema", "cli-help", "stable-docs"],
            "evidence_dimensions": evidence_dimensions()
        }],
        "scenarios": [{
            "id": "FPQ-G4-001",
            "requirement": "Supported CLI capability is qualified",
            "gate": 4,
            "feature_class": "supported-core",
            "persona": "alpha-payments-member",
            "fixture": {"org": "alpha", "team": "payments", "dataplane": "dp-alpha-payments"},
            "preconditions": ["candidate digest verified"],
            "action": "list clusters",
            "positive_result": "only Alpha/Payments clusters are returned",
            "negative_assertion": "Beta and sibling-team clusters are absent",
            "probe_origin": "alpha-payments-vm",
            "evidence_dimensions": evidence_dimensions(),
            "cleanup": {"action": "delete run-owned resources", "verified_by": "residue scan"},
            "rerun": {"safe": true, "strategy": "unique run id and idempotent cleanup"},
            "destructive": false,
            "timeout_seconds": 60
        }],
        "triage_classes": [
            "platform", "harness", "docs", "product", "unsupported", "shared-responsibility"
        ],
        "github": {
            "repository_url": REPOSITORY_URL,
            "milestone": {"title": "3.1.3", "number": 1, "url": MILESTONE_URL},
            "labels": {
                "severity": ["severity:blocker", "severity:major", "severity:minor"],
                "gate": ["gate:0", "gate:1", "gate:2", "gate:3", "gate:4", "gate:5", "gate:6"],
                "capability": [
                    "capability:auth", "capability:tenancy", "capability:xds",
                    "capability:gateway", "capability:learning", "capability:ai",
                    "capability:mcp", "capability:dashboard", "capability:operations",
                    "capability:packaging"
                ]
            }
        },
        "scheduled_beads": [{
            "bead_id": "fpv2-example",
            "github_issue_number": 42,
            "github_issue_url": "https://github.com/rajeevramani/flowplane/issues/42"
        }]
    })
}

#[test]
fn qualification_help_exposes_the_public_contract() {
    let output = run(&["qualification", "--help"]);
    assert_ok(&output, "flowplane qualification --help");
    let help = String::from_utf8_lossy(&output.stdout);
    for subcommand in ["inventory", "validate", "assemble"] {
        assert!(
            help.contains(subcommand),
            "qualification help must expose `{subcommand}`; help:\n{help}"
        );
    }
}

#[test]
fn qualification_accepts_global_flags_before_the_subcommand_without_panicking() {
    let output = run(&["--quiet", "qualification", "--help"]);
    assert_ok(&output, "global flag before qualification");
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("panicked"),
        "qualification must remain a normal clap subcommand"
    );
}

#[test]
fn inventory_reconciles_every_exact_artifact_surface_deterministically() {
    let temp = TempDir::new();
    let input = temp.write_json(
        "inventory-input.json",
        &json!({
            "artifact": {
                "release": "3.1.3",
                "digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            },
            "surfaces": {
                "openapi": [{"id": "api.clusters.list"}, {"id": "api.only"}],
                "cli_schema": [{"id": "cli.cluster.list"}],
                "cli_help": [{"id": "cli.cluster.list"}, {"id": "cli.help-only"}],
                "stable_docs": [
                    {"id": "api.clusters.list", "support": "supported-core"},
                    {"id": "dashboard.resources", "support": "supported-core"}
                ],
                "dashboard_routes": [{"id": "dashboard.resources"}],
                "config": [{"id": "config.oidc.issuer"}],
                "binaries": [{"id": "binary.flowplane"}, {"id": "binary.flowplane-agent"}]
            },
            "classifications": {
                "api.clusters.list": {"classification": "supported-core", "rationale": "stable docs"},
                "api.only": {"classification": "incomplete", "rationale": "OpenAPI presence alone is not support"},
                "cli.cluster.list": {"classification": "supported-core", "rationale": "schema and help"},
                "cli.help-only": {"classification": "incomplete", "rationale": "help mismatch requires triage"},
                "dashboard.resources": {"classification": "supported-core", "rationale": "stable dashboard route"},
                "config.oidc.issuer": {"classification": "incomplete", "rationale": "configuration declaration lacks executable corroboration"},
                "binary.flowplane": {"classification": "supported-core", "rationale": "release binary"},
                "binary.flowplane-agent": {"classification": "supported-core", "rationale": "release binary"}
            }
        }),
    );
    let first = temp.path().join("inventory-one.json");
    let second = temp.path().join("inventory-two.json");

    for output in [&first, &second] {
        let result = run(&[
            "qualification",
            "inventory",
            "--input",
            input.to_str().expect("UTF-8 input path"),
            "--output-path",
            output.to_str().expect("UTF-8 output path"),
        ]);
        assert_ok(&result, "generate exact-artifact inventory");
    }

    let first_bytes = fs::read(&first).expect("read first inventory");
    let second_bytes = fs::read(&second).expect("read second inventory");
    assert_eq!(
        first_bytes, second_bytes,
        "regeneration must be byte-identical"
    );

    let generated: Value = serde_json::from_slice(&first_bytes).expect("inventory is JSON");
    let rows = generated["inventory"].as_array().expect("inventory rows");
    let actual: BTreeSet<&str> = rows
        .iter()
        .map(|row| row["id"].as_str().expect("row id"))
        .collect();
    let expected: BTreeSet<&str> = [
        "api.clusters.list",
        "api.only",
        "binary.flowplane",
        "binary.flowplane-agent",
        "cli.cluster.list",
        "cli.help-only",
        "config.oidc.issuer",
        "dashboard.resources",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        actual, expected,
        "inventory must contain the exact union of all six surface families"
    );
    assert!(
        rows.iter().all(|row| {
            row["classification"]
                .as_str()
                .is_some_and(|value| !value.trim().is_empty())
                && row["classification_rationale"]
                    .as_str()
                    .is_some_and(|value| !value.trim().is_empty())
        }),
        "every inventory row requires a non-blank classification and rationale"
    );
    let code_only = rows
        .iter()
        .find(|row| row["id"] == "api.only")
        .expect("code-only row");
    assert_ne!(
        code_only["classification"], "supported-core",
        "presence in OpenAPI/code must never promote a capability to supported"
    );
    assert_ne!(
        code_only["classification"], "supported-with-dependency",
        "presence in OpenAPI/code must never promote a capability to supported"
    );
}

#[test]
fn validation_rejects_blank_classification_and_code_presence_promotion() {
    let mut blank = valid_contract();
    blank["inventory"][0]["classification"] = json!("   ");
    assert_rejected(&validate(&blank), "classification");

    for surface in [
        "openapi",
        "cli-schema",
        "dashboard-routes",
        "cli-help",
        "stable-docs",
        "config",
    ] {
        let mut promoted = valid_contract();
        promoted["inventory"][0]["observed_in"] = json!([surface]);
        promoted["inventory"][0]["classification"] = json!("supported-core");
        assert_rejected(&validate(&promoted), "observed_in");
    }
}

#[test]
fn validation_rejects_missing_scenario_evidence_probe_cleanup_and_rerun_metadata() {
    for field in ["probe_origin", "cleanup", "rerun"] {
        let mut contract = valid_contract();
        contract["scenarios"][0]
            .as_object_mut()
            .expect("scenario object")
            .remove(field);
        assert_rejected(&validate(&contract), field);
    }

    for dimension in [
        "configuration",
        "runtime",
        "isolation",
        "diagnostics",
        "presentation",
        "cleanup",
    ] {
        let mut contract = valid_contract();
        contract["scenarios"][0]["evidence_dimensions"]
            .as_object_mut()
            .expect("evidence dimensions object")
            .remove(dimension);
        assert_rejected(&validate(&contract), dimension);
    }
}

#[test]
fn evidence_assembler_redacts_synthetic_secret_canaries_everywhere() {
    let temp = TempDir::new();
    let input = temp.write_json(
        "raw-evidence.json",
        &json!({
            "command": format!("FLOWPLANE_TOKEN={CANARY} flowplane cluster list"),
            "stdout": {"authorization": format!("Bearer {CANARY}")},
            "stderr": format!("upstream rejected token {CANARY}"),
            "metadata": {"nested": [CANARY, {"certificate": CANARY}]}
        }),
    );
    let output_path = temp.path().join("public-evidence.json");
    let output = run(&[
        "qualification",
        "assemble",
        "--input",
        input.to_str().expect("UTF-8 input path"),
        "--output-path",
        output_path.to_str().expect("UTF-8 output path"),
    ]);
    assert_ok(&output, "assemble redacted evidence");
    let public = fs::read(&output_path).expect("read assembled evidence");
    assert!(
        !public
            .windows(CANARY.len())
            .any(|window| window == CANARY.as_bytes()),
        "synthetic secret canary leaked into public evidence: {}",
        String::from_utf8_lossy(&public)
    );
}

#[test]
fn triage_taxonomy_separates_all_six_responsibility_classes() {
    let output = validate(&valid_contract());
    assert_ok(&output, "valid six-class triage contract");

    for missing in [
        "platform",
        "harness",
        "docs",
        "product",
        "unsupported",
        "shared-responsibility",
    ] {
        let mut contract = valid_contract();
        contract["triage_classes"]
            .as_array_mut()
            .expect("triage class array")
            .retain(|class| class != missing);
        assert_rejected(&validate(&contract), "triage_classes");
    }
}

#[test]
fn github_taxonomy_and_scheduled_bead_links_are_exact_and_pinned() {
    let output = validate(&valid_contract());
    assert_ok(&output, "pinned GitHub contract");

    let mut wrong_milestone = valid_contract();
    wrong_milestone["github"]["milestone"]["number"] = json!(2);
    assert_rejected(&validate(&wrong_milestone), "milestone.number");

    let mut wrong_url = valid_contract();
    wrong_url["github"]["milestone"]["url"] =
        json!("https://github.com/rajeevramani/flowplane/milestone/2");
    assert_rejected(&validate(&wrong_url), "milestone.url");

    for family in ["severity", "gate", "capability"] {
        let mut missing_label = valid_contract();
        missing_label["github"]["labels"][family]
            .as_array_mut()
            .expect("label array")
            .pop();
        assert_rejected(&validate(&missing_label), family);
    }

    for field in ["github_issue_number", "github_issue_url"] {
        let mut missing_link = valid_contract();
        missing_link["scheduled_beads"][0]
            .as_object_mut()
            .expect("scheduled bead object")
            .remove(field);
        assert_rejected(&validate(&missing_link), field);
    }

    let mut mismatched_link = valid_contract();
    mismatched_link["scheduled_beads"][0]["github_issue_url"] =
        json!("https://github.com/rajeevramani/flowplane/issues/41");
    assert_rejected(&validate(&mismatched_link), "github_issue_url");
}
