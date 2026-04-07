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
address: 127.0.0.1
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
address: 127.0.0.1
port: 9090
"#;
    let file_v1 = write_temp_file(manifest_v1, ".yaml");
    let create = cli.run(&["apply", "-f", file_v1.path().to_str().unwrap()]).unwrap();
    create.assert_success();

    // Update with different port
    let manifest_v2 = r#"kind: Cluster
name: e2e-apply-upd
address: 127.0.0.1
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
address: 127.0.0.1
port: 9092
"#;
    let cluster_b = r#"kind: Cluster
name: e2e-apply-dir-b
address: 127.0.0.1
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
address: 127.0.0.1
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
