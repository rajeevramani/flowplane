//! Phase 5 CLI E2E tests — dev mode
//!
//! Tests the CLI commands added in Phases 3-5: mTLS/cert, admin, MCP tools,
//! WASM, apply lifecycle, scaffold, and negative cases. All tests use the
//! dev-mode harness (bearer token, no Zitadel) and the CliRunner subprocess
//! driver.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_phase5 -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use std::io::Write;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;

/// Write content to a named temp file with the given extension.
fn write_temp_file(content: &str, extension: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new().suffix(extension).tempfile().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

// ============================================================================
// mTLS / cert commands
// ============================================================================

/// `flowplane mtls status` should exit 0 in dev mode.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mtls_status() {
    let harness = dev_harness("dev_cli_mtls_status").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["mtls", "status"]).unwrap();
    output.assert_success();
}

/// `flowplane cert list` should exit 0 (may return empty list).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cert_list() {
    let harness = dev_harness("dev_cli_cert_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["cert", "list"]).unwrap();
    output.assert_success();
}

// ============================================================================
// admin commands (may fail in dev mode — check gracefully)
// ============================================================================

/// `flowplane admin resources` should exit 0 or produce an appropriate error
/// in dev mode (admin commands may require prod auth).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_admin_resources() {
    let harness = dev_harness("dev_cli_admin_res").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["admin", "resources"]).unwrap();

    // In dev mode, admin commands may succeed or return a permission/auth error.
    // Either is acceptable — it should not crash or hang.
    let is_reasonable = output.exit_code == 0
        || output.stderr.to_lowercase().contains("error")
        || output.stderr.to_lowercase().contains("permission")
        || output.stderr.to_lowercase().contains("unauthorized")
        || output.stderr.to_lowercase().contains("forbidden")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        is_reasonable,
        "Expected admin resources to either succeed or produce an auth/permission error, \
         got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// `flowplane admin scopes` should exit 0 or produce an appropriate error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_admin_scopes() {
    let harness = dev_harness("dev_cli_admin_scopes").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["admin", "scopes"]).unwrap();

    let is_reasonable = output.exit_code == 0
        || output.stderr.to_lowercase().contains("error")
        || output.stderr.to_lowercase().contains("permission")
        || output.stderr.to_lowercase().contains("unauthorized")
        || output.stderr.to_lowercase().contains("forbidden")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        is_reasonable,
        "Expected admin scopes to either succeed or produce an auth/permission error, \
         got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// MCP tools
// ============================================================================

/// `flowplane mcp tools` should exit 0 and list available MCP tools.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mcp_tools_list() {
    let harness = dev_harness("dev_cli_mcp_tools").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["mcp", "tools"]).unwrap();
    output.assert_success();
}

// ============================================================================
// WASM
// ============================================================================

/// `flowplane wasm list` should exit 0 (empty list is fine).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_list() {
    let harness = dev_harness("dev_cli_wasm_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["wasm", "list"]).unwrap();
    output.assert_success();
}

// ============================================================================
// apply lifecycle
// ============================================================================

/// `flowplane apply -f <file>` with a Cluster manifest should create a cluster
/// that is visible via the API.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_apply_creates_cluster() {
    let harness = dev_harness("dev_cli_apply_create").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();
    let team = &harness.team;

    let manifest = r#"kind: Cluster
name: e2e-apply-cluster
endpoints:
  - host: 127.0.0.1
    port: 9090
"#;
    let file = write_temp_file(manifest, ".yaml");

    let output = cli.run(&["apply", "-f", file.path().to_str().unwrap()]).unwrap();
    output.assert_success();

    // Verify cluster exists via API
    let resp = harness
        .authed_get(&format!("/api/v1/teams/{team}/clusters/e2e-apply-cluster"))
        .await
        .expect("authed_get cluster should succeed");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "cluster should exist after apply");
}

/// After creating a cluster via apply, modify the manifest and apply again —
/// the update should succeed.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_apply_updates_cluster() {
    let harness = dev_harness("dev_cli_apply_update").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Create initial cluster
    let manifest_v1 = r#"kind: Cluster
name: e2e-apply-upd
endpoints:
  - host: 127.0.0.1
    port: 9090
"#;
    let file_v1 = write_temp_file(manifest_v1, ".yaml");
    let create = cli.run(&["apply", "-f", file_v1.path().to_str().unwrap()]).unwrap();
    create.assert_success();

    // Update with different port
    let manifest_v2 = r#"kind: Cluster
name: e2e-apply-upd
endpoints:
  - host: 127.0.0.1
    port: 9091
"#;
    let file_v2 = write_temp_file(manifest_v2, ".yaml");
    let update = cli.run(&["apply", "-f", file_v2.path().to_str().unwrap()]).unwrap();
    update.assert_success();
}

/// `flowplane apply -f <dir>` with multiple manifests should create all resources.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_apply_directory() {
    let harness = dev_harness("dev_cli_apply_dir").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();
    let team = &harness.team;

    let dir = tempfile::tempdir().expect("create temp dir");

    let cluster_a = r#"kind: Cluster
name: e2e-apply-dir-a
endpoints:
  - host: 127.0.0.1
    port: 9092
"#;
    let cluster_b = r#"kind: Cluster
name: e2e-apply-dir-b
endpoints:
  - host: 127.0.0.1
    port: 9093
"#;
    std::fs::write(dir.path().join("cluster-a.yaml"), cluster_a).unwrap();
    std::fs::write(dir.path().join("cluster-b.yaml"), cluster_b).unwrap();

    let output = cli.run(&["apply", "-f", dir.path().to_str().unwrap()]).unwrap();
    output.assert_success();

    // Verify both clusters exist
    let resp_a = harness
        .authed_get(&format!("/api/v1/teams/{team}/clusters/e2e-apply-dir-a"))
        .await
        .expect("authed_get cluster-a should succeed");
    assert_eq!(resp_a.status(), reqwest::StatusCode::OK, "cluster-a should exist after apply");

    let resp_b = harness
        .authed_get(&format!("/api/v1/teams/{team}/clusters/e2e-apply-dir-b"))
        .await
        .expect("authed_get cluster-b should succeed");
    assert_eq!(resp_b.status(), reqwest::StatusCode::OK, "cluster-b should exist after apply");
}

/// Apply a manifest with an unknown kind — should produce an error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_apply_invalid_kind() {
    let harness = dev_harness("dev_cli_apply_badkind").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let manifest = r#"kind: TotallyFakeResource
name: e2e-apply-fake
"#;
    let file = write_temp_file(manifest, ".yaml");

    let output = cli.run(&["apply", "-f", file.path().to_str().unwrap()]).unwrap();

    // Should fail with a non-zero exit or error message about unknown kind
    let has_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("unknown")
        || output.stderr.to_lowercase().contains("unsupported")
        || output.stderr.to_lowercase().contains("invalid")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("unknown")
        || output.stdout.to_lowercase().contains("unsupported")
        || output.stdout.to_lowercase().contains("invalid");
    assert!(
        has_error,
        "Expected error for unknown kind, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// Apply a manifest missing the name field — should produce an error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_apply_missing_name() {
    let harness = dev_harness("dev_cli_apply_noname").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let manifest = r#"kind: Cluster
endpoints:
  - host: 127.0.0.1
    port: 9090
"#;
    let file = write_temp_file(manifest, ".yaml");

    let output = cli.run(&["apply", "-f", file.path().to_str().unwrap()]).unwrap();

    let has_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("name")
        || output.stderr.to_lowercase().contains("required")
        || output.stderr.to_lowercase().contains("missing")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("name")
        || output.stdout.to_lowercase().contains("required")
        || output.stdout.to_lowercase().contains("missing");
    assert!(
        has_error,
        "Expected error for missing name, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// scaffold
// ============================================================================

/// `flowplane filter scaffold local_rate_limit` should output YAML containing
/// kind and config fields.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold() {
    let harness = dev_harness("dev_cli_filter_scaff").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["filter", "scaffold", "local_rate_limit"]).unwrap();
    output.assert_success();

    let stdout_lower = output.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("kind") || stdout_lower.contains("type"),
        "Expected scaffold output to contain 'kind' or 'type', got:\n{}",
        output.stdout
    );
    assert!(
        stdout_lower.contains("config"),
        "Expected scaffold output to contain 'config', got:\n{}",
        output.stdout
    );
}

/// `flowplane cluster scaffold` should output YAML containing kind: Cluster.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold() {
    let harness = dev_harness("dev_cli_cluster_scaff").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["cluster", "scaffold"]).unwrap();
    output.assert_success();

    // Should contain kind: Cluster (case insensitive match for flexibility)
    let stdout_lower = output.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("kind") && stdout_lower.contains("cluster"),
        "Expected scaffold output to contain 'kind: Cluster', got:\n{}",
        output.stdout
    );
}

/// `flowplane cluster scaffold -o json` should output valid JSON.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_scaffold_json_output() {
    let harness = dev_harness("dev_cli_scaff_json").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["cluster", "scaffold", "-o", "json"]).unwrap();
    output.assert_success();

    // Output should be valid JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(output.stdout.trim());
    assert!(
        parsed.is_ok(),
        "Expected valid JSON from scaffold -o json, got parse error: {:?}\nstdout: {}",
        parsed.err(),
        output.stdout
    );
}

// ============================================================================
// negative tests
// ============================================================================

/// `flowplane apply -f /nonexistent` should fail with an error about the
/// missing file/path.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_apply_nonexistent_file() {
    let harness = dev_harness("dev_cli_apply_nofile").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output =
        cli.run(&["apply", "-f", "/tmp/totally-nonexistent-flowplane-manifest.yaml"]).unwrap();

    let has_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("no such file")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent file, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// `flowplane wasm get <nonexistent-id>` should fail with an error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_get_nonexistent() {
    let harness = dev_harness("dev_cli_wasm_noexist").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["wasm", "get", "totally-fake-wasm-module-xyz"]).unwrap();

    let has_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent wasm module, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// scaffold → create/update/apply roundtrip tests
// ============================================================================

use crate::common::cli_runner::CliOutput;

/// Run scaffold, replace placeholders in output, write to temp file, then run
/// a create/update/apply command with `-f <file>`. Returns the CliOutput from
/// the final command WITHOUT asserting — callers are responsible for assertions
/// so failures point at the test, not this helper.
fn scaffold_and_create(
    cli: &CliRunner,
    scaffold_args: &[&str],
    replacements: &[(&str, &str)],
    extension: &str,
    create_args_prefix: &[&str],
) -> CliOutput {
    let scaffold_output = cli.run(scaffold_args).unwrap();
    assert_eq!(
        scaffold_output.exit_code, 0,
        "scaffold command failed: stderr={}",
        scaffold_output.stderr
    );

    let mut content = scaffold_output.stdout.clone();
    for (from, to) in replacements {
        content = content.replace(from, to);
    }

    // For YAML: strip comment lines so serde_yaml can parse cleanly
    if extension == ".yaml" || extension == ".yml" {
        content = content
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
    }

    let file = write_temp_file(&content, extension);
    let file_path = file.path().to_str().unwrap().to_string();

    let mut args: Vec<&str> = create_args_prefix.to_vec();
    args.push(&file_path);

    cli.run(&args).unwrap()
}

/// Scaffold YAML → replace placeholders → `cluster create -f file.yaml` → verify
/// cluster exists via `cluster get`.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_create_yaml() {
    let harness = dev_harness("dev_cli_scaff_cr_yaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let cluster_name = "e2e-scaff-yaml-cr";
    let replacements = &[
        ("<your-cluster-name>", cluster_name),
        ("<your-service-name>", "my-svc"),
        ("<host>", "httpbin.org"),
        ("<port>", "443"),
        ("your-cluster-name", cluster_name),
        ("your-service-name", "my-svc"),
        ("host.example.com", "httpbin.org"),
        ("8080", "443"),
    ];

    let output = scaffold_and_create(
        &cli,
        &["cluster", "scaffold"],
        replacements,
        ".yaml",
        &["cluster", "create", "-f"],
    );
    assert_eq!(
        output.exit_code, 0,
        "cluster create -f yaml failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify the cluster exists
    let get_output = cli.run(&["cluster", "get", cluster_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(cluster_name),
        "Expected cluster name in get output, got: {}",
        get_output.stdout
    );
}

/// Scaffold JSON → replace placeholders → `cluster create -f file.json` → verify
/// cluster exists via `cluster get`.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_create_json() {
    let harness = dev_harness("dev_cli_scaff_cr_json").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let cluster_name = "e2e-scaff-json-cr";
    let replacements = &[
        ("<your-cluster-name>", cluster_name),
        ("<your-service-name>", "my-json-svc"),
        ("<host>", "httpbin.org"),
        ("<port>", "443"),
        ("your-cluster-name", cluster_name),
        ("your-service-name", "my-json-svc"),
        ("host.example.com", "httpbin.org"),
        ("8080", "443"),
    ];

    let output = scaffold_and_create(
        &cli,
        &["cluster", "scaffold", "-o", "json"],
        replacements,
        ".json",
        &["cluster", "create", "-f"],
    );
    assert_eq!(
        output.exit_code, 0,
        "cluster create -f json failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify the cluster exists
    let get_output = cli.run(&["cluster", "get", cluster_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(cluster_name),
        "Expected cluster name in get output, got: {}",
        get_output.stdout
    );
}

/// Manually write a YAML cluster spec → `cluster create -f file.yaml` succeeds.
/// Proves YAML acceptance without depending on scaffold output format.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_create_yaml_file() {
    let harness = dev_harness("dev_cli_cr_yaml_file").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let yaml_content = r#"name: e2e-manual-yaml-cr
endpoints:
  - host: api.example.com
    port: 8443
connectTimeoutSeconds: 10
useTls: true
lbPolicy: ROUND_ROBIN
"#;
    let file = write_temp_file(yaml_content, ".yaml");

    let output = cli.run(&["cluster", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "cluster create from manual YAML failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    let get_output = cli.run(&["cluster", "get", "e2e-manual-yaml-cr"]).unwrap();
    get_output.assert_success();
}

/// Create a cluster via JSON, then update it via YAML file. Proves `update -f`
/// accepts YAML, not just JSON.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_update_yaml_file() {
    let harness = dev_harness("dev_cli_upd_yaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Create cluster via JSON first
    let json_content = r#"{
  "name": "e2e-upd-yaml-cl",
  "endpoints": [{"host": "backend.example.com", "port": 8080}],
  "connectTimeoutSeconds": 5
}"#;
    let json_file = write_temp_file(json_content, ".json");
    let create_output =
        cli.run(&["cluster", "create", "-f", json_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "initial create via JSON failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Update the same cluster via YAML — change timeout and add TLS
    let yaml_update = r#"name: e2e-upd-yaml-cl
endpoints:
  - host: backend.example.com
    port: 8443
connectTimeoutSeconds: 15
useTls: true
"#;
    let yaml_file = write_temp_file(yaml_update, ".yaml");
    let update_output = cli
        .run(&["cluster", "update", "e2e-upd-yaml-cl", "-f", yaml_file.path().to_str().unwrap()])
        .unwrap();
    assert_eq!(
        update_output.exit_code, 0,
        "update via YAML failed: stdout={}, stderr={}",
        update_output.stdout, update_output.stderr
    );
}

/// Scaffold YAML → create cluster → modify a field → `update -f file.yaml` with
/// `kind: Cluster` still present in the file. Proves `strip_kind_field` works in
/// the update code path, not just create.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_update_yaml() {
    let harness = dev_harness("dev_cli_scaff_upd").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let cluster_name = "e2e-scaff-upd-cl";

    // Step 1: Create cluster from scaffold
    let replacements = &[
        ("<your-cluster-name>", cluster_name),
        ("<your-service-name>", "upd-svc"),
        ("<host>", "original.example.com"),
        ("<port>", "8080"),
        ("your-cluster-name", cluster_name),
        ("your-service-name", "upd-svc"),
        ("host.example.com", "original.example.com"),
        ("8080", "8080"),
    ];
    let create_output = scaffold_and_create(
        &cli,
        &["cluster", "scaffold"],
        replacements,
        ".yaml",
        &["cluster", "create", "-f"],
    );
    assert_eq!(
        create_output.exit_code, 0,
        "scaffold create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 2: Build an update YAML that includes `kind: Cluster` (as a scaffold
    // file would) and changes the endpoint host. The `kind` field must be
    // stripped by the CLI before sending to the API.
    let update_yaml = format!(
        r#"kind: Cluster
name: {cluster_name}
endpoints:
  - host: updated.example.com
    port: 9090
connectTimeoutSeconds: 20
"#
    );
    let update_file = write_temp_file(&update_yaml, ".yaml");
    let update_output = cli
        .run(&["cluster", "update", cluster_name, "-f", update_file.path().to_str().unwrap()])
        .unwrap();
    assert_eq!(
        update_output.exit_code, 0,
        "update with kind field in YAML failed: stdout={}, stderr={}",
        update_output.stdout, update_output.stderr
    );

    // Verify the cluster still exists after update
    let get_output = cli.run(&["cluster", "get", cluster_name]).unwrap();
    get_output.assert_success();
}

/// Scaffold YAML → replace placeholders → `apply -f file.yaml` → verify cluster
/// exists. Tests the apply path with scaffold output (apply uses `kind` to route).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_apply() {
    let harness = dev_harness("dev_cli_scaff_apply").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let cluster_name = "e2e-scaff-apply-cl";
    let replacements = &[
        ("<your-cluster-name>", cluster_name),
        ("<your-service-name>", "apply-svc"),
        ("<host>", "apply-backend.example.com"),
        ("<port>", "443"),
        ("your-cluster-name", cluster_name),
        ("your-service-name", "apply-svc"),
        ("host.example.com", "apply-backend.example.com"),
        ("8080", "443"),
    ];

    let output = scaffold_and_create(
        &cli,
        &["cluster", "scaffold"],
        replacements,
        ".yaml",
        &["apply", "-f"],
    );
    assert_eq!(
        output.exit_code, 0,
        "apply -f scaffold yaml failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify the cluster exists via get
    let get_output = cli.run(&["cluster", "get", cluster_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(cluster_name),
        "Expected cluster name in get output, got: {}",
        get_output.stdout
    );
}

/// `cluster create -f file.txt` should fail with an error mentioning supported
/// file extensions (.yaml, .yml, .json).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_create_bad_extension() {
    let harness = dev_harness("dev_cli_cr_badext").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let content = r#"{"name": "bad-ext", "endpoints": [{"host": "x.com", "port": 80}]}"#;
    let file = write_temp_file(content, ".txt");

    let output = cli.run(&["cluster", "create", "-f", file.path().to_str().unwrap()]).unwrap();

    assert_ne!(
        output.exit_code, 0,
        "Expected non-zero exit for .txt extension, got exit_code=0, stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Error should mention supported extensions
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    assert!(
        combined.contains(".yaml")
            || combined.contains(".yml")
            || combined.contains(".json")
            || combined.contains("extension")
            || combined.contains("unsupported"),
        "Expected error to mention supported extensions, got: stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

/// `cluster create -f file.yaml` with syntactically invalid YAML should produce
/// a user-friendly error (not a panic or raw stack trace).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_create_malformed_yaml() {
    let harness = dev_harness("dev_cli_cr_badyaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Intentionally broken YAML: bad indentation, unclosed quote, mixed types
    let bad_yaml = r#"name: [unterminated
  endpoints:
    - host: "missing-close
    port not-a-number
  garbage: {{{
"#;
    let file = write_temp_file(bad_yaml, ".yaml");

    let output = cli.run(&["cluster", "create", "-f", file.path().to_str().unwrap()]).unwrap();

    assert_ne!(
        output.exit_code, 0,
        "Expected non-zero exit for malformed YAML, got exit_code=0, stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Should NOT contain a raw Rust panic/backtrace
    let combined = format!("{} {}", output.stdout, output.stderr);
    assert!(
        !combined.contains("panicked at") && !combined.contains("stack backtrace"),
        "Malformed YAML should produce a user-friendly error, not a panic: {}",
        combined
    );

    // Should contain something indicating a parse/format error
    let combined_lower = combined.to_lowercase();
    assert!(
        combined_lower.contains("error")
            || combined_lower.contains("invalid")
            || combined_lower.contains("parse")
            || combined_lower.contains("yaml"),
        "Expected parse error message, got: {}",
        combined
    );
}
