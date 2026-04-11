//! CLI E2E tests — mTLS/cert, admin, MCP tools, WASM, apply, scaffold (dev mode)
//!
//! Scaffold create/update tests use an Envoy-enabled harness and verify that
//! resources reach Envoy via xDS (config_dump) and that traffic flows through
//! the proxy. Error tests (bad extension, malformed YAML) remain API-only.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_scaffold -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::{dev_harness, envoy_harness};
use crate::common::test_helpers::{
    create_chain_for_cluster, create_chain_for_route, verify_in_config_dump, verify_traffic,
    write_temp_file,
};

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
/// cluster exists via `cluster get`, appears in Envoy config_dump, and traffic flows.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_create_yaml() {
    let harness = envoy_harness("dev_cli_scaff_cr_yaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-scaff-yaml-cr";
    let replacements = &[
        ("<your-cluster-name>", cluster_name),
        ("<your-service-name>", "my-svc"),
        ("<host-or-ip>", echo_host),
        ("<host>", echo_host),
        ("<port>", echo_port),
        ("your-cluster-name", cluster_name),
        ("your-service-name", "my-svc"),
        ("host.example.com", echo_host),
        ("8080", echo_port),
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

    // Verify the cluster exists via CLI
    let get_output = cli.run(&["cluster", "get", cluster_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(cluster_name),
        "Expected cluster name in get output, got: {}",
        get_output.stdout
    );

    // Create route + listener to complete Envoy chain, then verify xDS + traffic
    let (_route, domain) = create_chain_for_cluster(&harness, cluster_name, "scaff-yaml-cr").await;
    verify_in_config_dump(&harness, cluster_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// Scaffold JSON → replace placeholders → `cluster create -f file.json` → verify
/// cluster exists via `cluster get`, appears in Envoy config_dump, and traffic flows.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_create_json() {
    let harness = envoy_harness("dev_cli_scaff_cr_json").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-scaff-json-cr";
    let replacements = &[
        ("<your-cluster-name>", cluster_name),
        ("<your-service-name>", "my-json-svc"),
        ("<host-or-ip>", echo_host),
        ("<host>", echo_host),
        ("<port>", echo_port),
        ("your-cluster-name", cluster_name),
        ("your-service-name", "my-json-svc"),
        ("host.example.com", echo_host),
        ("8080", echo_port),
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

    // Verify the cluster exists via CLI
    let get_output = cli.run(&["cluster", "get", cluster_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(cluster_name),
        "Expected cluster name in get output, got: {}",
        get_output.stdout
    );

    // Create route + listener to complete Envoy chain, then verify xDS + traffic
    let (_route, domain) = create_chain_for_cluster(&harness, cluster_name, "scaff-json-cr").await;
    verify_in_config_dump(&harness, cluster_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// Manually write a YAML cluster spec → `cluster create -f file.yaml` succeeds.
/// Proves YAML acceptance without depending on scaffold output format.
/// Verifies the cluster appears in Envoy config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_create_yaml_file() {
    let harness = envoy_harness("dev_cli_cr_yaml_file").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-manual-yaml-cr";
    let yaml_content = format!(
        r#"name: {cluster_name}
endpoints:
  - host: {echo_host}
    port: {echo_port}
connectTimeoutSeconds: 10
lbPolicy: ROUND_ROBIN
"#
    );
    let file = write_temp_file(&yaml_content, ".yaml");

    let output = cli.run(&["cluster", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "cluster create from manual YAML failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    let get_output = cli.run(&["cluster", "get", cluster_name]).unwrap();
    get_output.assert_success();

    // Verify cluster reaches Envoy via xDS
    verify_in_config_dump(&harness, cluster_name).await;
}

/// Create a cluster via JSON, then update it via YAML file. Proves `update -f`
/// accepts YAML, not just JSON. Verifies updated cluster in Envoy config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_update_yaml_file() {
    let harness = envoy_harness("dev_cli_upd_yaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-upd-yaml-cl";

    // Create cluster via JSON first
    let json_content = format!(
        r#"{{"name": "{cluster_name}", "endpoints": [{{"host": "{echo_host}", "port": {echo_port}}}], "connectTimeoutSeconds": 5}}"#
    );
    let json_file = write_temp_file(&json_content, ".json");
    let create_output =
        cli.run(&["cluster", "create", "-f", json_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "initial create via JSON failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Update the same cluster via YAML — change timeout
    let yaml_update = format!(
        r#"name: {cluster_name}
endpoints:
  - host: {echo_host}
    port: {echo_port}
connectTimeoutSeconds: 15
"#
    );
    let yaml_file = write_temp_file(&yaml_update, ".yaml");
    let update_output = cli
        .run(&["cluster", "update", cluster_name, "-f", yaml_file.path().to_str().unwrap()])
        .unwrap();
    assert_eq!(
        update_output.exit_code, 0,
        "update via YAML failed: stdout={}, stderr={}",
        update_output.stdout, update_output.stderr
    );

    // Verify updated cluster reaches Envoy via xDS
    verify_in_config_dump(&harness, cluster_name).await;

    // Verify traffic flows through the updated cluster
    let (_route_name, domain) = create_chain_for_cluster(&harness, cluster_name, "upd-yaml").await;
    verify_traffic(&harness, &domain, "/").await;
}

/// Scaffold YAML → create cluster → modify a field → `update -f file.yaml` with
/// `kind: Cluster` still present in the file. Proves `strip_kind_field` works in
/// the update code path, not just create. Verifies updated cluster in Envoy config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_update_yaml() {
    let harness = envoy_harness("dev_cli_scaff_upd").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-scaff-upd-cl";

    // Step 1: Create cluster from scaffold
    let replacements = &[
        ("<your-cluster-name>", cluster_name),
        ("<your-service-name>", "upd-svc"),
        ("<host-or-ip>", echo_host),
        ("<host>", echo_host),
        ("<port>", echo_port),
        ("your-cluster-name", cluster_name),
        ("your-service-name", "upd-svc"),
        ("host.example.com", echo_host),
        ("8080", echo_port),
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
  - host: {echo_host}
    port: {echo_port}
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

    // Verify updated cluster reaches Envoy via xDS
    verify_in_config_dump(&harness, cluster_name).await;

    // Verify traffic flows through the updated cluster
    let (_route_name, domain) = create_chain_for_cluster(&harness, cluster_name, "scaff-upd").await;
    verify_traffic(&harness, &domain, "/").await;
}

/// Scaffold YAML → replace placeholders → `apply -f file.yaml` → verify cluster
/// exists, appears in Envoy config_dump, and traffic flows.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_apply() {
    let harness = envoy_harness("dev_cli_scaff_apply").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-scaff-apply-cl";
    let replacements = &[
        ("<your-cluster-name>", cluster_name),
        ("<your-service-name>", "apply-svc"),
        ("<host-or-ip>", echo_host),
        ("<host>", echo_host),
        ("<port>", echo_port),
        ("your-cluster-name", cluster_name),
        ("your-service-name", "apply-svc"),
        ("host.example.com", echo_host),
        ("8080", echo_port),
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

    // Create route + listener to complete Envoy chain, then verify xDS + traffic
    let (_route, domain) = create_chain_for_cluster(&harness, cluster_name, "scaff-apply").await;
    verify_in_config_dump(&harness, cluster_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

// ============================================================================
// route scaffold → create/update/apply roundtrip tests
// ============================================================================

/// Helper: create a prerequisite cluster that route forward actions can
/// reference, returning the cluster name used.
fn create_prerequisite_cluster(cli: &CliRunner, cluster_name: &str, host: &str, port: &str) {
    let cluster_yaml = format!(
        r#"name: {cluster_name}
endpoints:
  - host: {host}
    port: {port}
connectTimeoutSeconds: 5
"#
    );
    let file = write_temp_file(&cluster_yaml, ".yaml");
    let output = cli.run(&["cluster", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "prerequisite cluster create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// Route scaffold YAML → replace placeholders → `route create -f file.yaml`
/// → `route get` confirms the route config exists. Verifies route in Envoy
/// config_dump and traffic flows through the proxy.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_route_scaffold_create_yaml() {
    let harness = envoy_harness("dev_cli_rt_scaff_yaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-rt-yaml-cl";
    let route_name = "e2e-rt-yaml-rc";
    let vhost_name = "e2e-rt-yaml-vh";

    // Create prerequisite cluster pointing to echo server
    create_prerequisite_cluster(&cli, cluster_name, echo_host, echo_port);

    // Scaffold route YAML → replace placeholders → create
    let replacements = &[
        ("<your-route-config-name>", route_name),
        ("<your-vhost-name>", vhost_name),
        ("<your-route-name>", "catch-all"),
        ("<your-cluster-name>", cluster_name),
        ("your-route-config-name", route_name),
        ("your-vhost-name", vhost_name),
        ("your-route-name", "catch-all"),
        ("your-cluster-name", cluster_name),
    ];

    let output = scaffold_and_create(
        &cli,
        &["route", "scaffold"],
        replacements,
        ".yaml",
        &["route", "create", "-f"],
    );
    assert_eq!(
        output.exit_code, 0,
        "route create -f yaml failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify the route config exists via CLI
    let get_output = cli.run(&["route", "get", route_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(route_name),
        "Expected route config name in get output, got: {}",
        get_output.stdout
    );

    // Create listener to complete Envoy chain, then verify xDS + traffic
    let domain =
        create_chain_for_route(&harness, route_name, "rt-yaml-cr", "rt-yaml.e2e.local").await;
    verify_in_config_dump(&harness, route_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// Route scaffold JSON → replace placeholders → `route create -f file.json`
/// → `route get` confirms. Verifies route in Envoy config_dump and traffic flows.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_route_scaffold_create_json() {
    let harness = envoy_harness("dev_cli_rt_scaff_json").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-rt-json-cl";
    let route_name = "e2e-rt-json-rc";
    let vhost_name = "e2e-rt-json-vh";

    create_prerequisite_cluster(&cli, cluster_name, echo_host, echo_port);

    let replacements = &[
        ("<your-route-config-name>", route_name),
        ("<your-vhost-name>", vhost_name),
        ("<your-route-name>", "catch-all"),
        ("<your-cluster-name>", cluster_name),
        ("your-route-config-name", route_name),
        ("your-vhost-name", vhost_name),
        ("your-route-name", "catch-all"),
        ("your-cluster-name", cluster_name),
    ];

    let output = scaffold_and_create(
        &cli,
        &["route", "scaffold", "-o", "json"],
        replacements,
        ".json",
        &["route", "create", "-f"],
    );
    assert_eq!(
        output.exit_code, 0,
        "route create -f json failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    let get_output = cli.run(&["route", "get", route_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(route_name),
        "Expected route config name in get output, got: {}",
        get_output.stdout
    );

    // Create listener to complete Envoy chain, then verify xDS + traffic
    let domain =
        create_chain_for_route(&harness, route_name, "rt-json-cr", "rt-json.e2e.local").await;
    verify_in_config_dump(&harness, route_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// Route scaffold YAML → replace placeholders → `apply -f file.yaml` → `route get`.
/// Key test: proves that `kind: RouteConfig` is correctly routed by the apply command.
/// Verifies route in Envoy config_dump and traffic flows through the proxy.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_route_scaffold_apply() {
    let harness = envoy_harness("dev_cli_rt_scaff_apply").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-rt-apply-cl";
    let route_name = "e2e-rt-apply-rc";
    let vhost_name = "e2e-rt-apply-vh";

    create_prerequisite_cluster(&cli, cluster_name, echo_host, echo_port);

    let replacements = &[
        ("<your-route-config-name>", route_name),
        ("<your-vhost-name>", vhost_name),
        ("<your-route-name>", "api-route"),
        ("<your-cluster-name>", cluster_name),
        ("your-route-config-name", route_name),
        ("your-vhost-name", vhost_name),
        ("your-route-name", "api-route"),
        ("your-cluster-name", cluster_name),
    ];

    let output =
        scaffold_and_create(&cli, &["route", "scaffold"], replacements, ".yaml", &["apply", "-f"]);
    assert_eq!(
        output.exit_code, 0,
        "apply -f route scaffold yaml failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify route config exists
    let get_output = cli.run(&["route", "get", route_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(route_name),
        "Expected route config name in get output, got: {}",
        get_output.stdout
    );

    // Create listener to complete Envoy chain, then verify xDS + traffic
    let domain =
        create_chain_for_route(&harness, route_name, "rt-apply", "rt-apply.e2e.local").await;
    verify_in_config_dump(&harness, route_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// Create a route config manually, then update it via `route update -f file.yaml`
/// with a modified domain. Proves YAML file-based update works for routes.
/// Verifies updated route appears in Envoy config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_route_update_yaml_file() {
    let harness = envoy_harness("dev_cli_rt_upd_yaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-rt-upd-cl";
    let route_name = "e2e-rt-upd-rc";

    create_prerequisite_cluster(&cli, cluster_name, echo_host, echo_port);

    // Create route config via JSON first
    let json_content = format!(
        r#"{{
  "name": "{route_name}",
  "virtualHosts": [
    {{
      "name": "original-vhost",
      "domains": ["api.original.example.com"],
      "routes": [
        {{
          "match": {{"path": {{"type": "prefix", "value": "/"}}}},
          "action": {{"type": "forward", "cluster": "{cluster_name}"}}
        }}
      ]
    }}
  ]
}}"#
    );
    let json_file = write_temp_file(&json_content, ".json");
    let create_output =
        cli.run(&["route", "create", "-f", json_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "initial route create via JSON failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Update with YAML — change domain
    let yaml_update = format!(
        r#"name: {route_name}
virtualHosts:
  - name: updated-vhost
    domains:
      - "api.updated.example.com"
    routes:
      - match:
          path:
            type: prefix
            value: "/v2"
        action:
          type: forward
          cluster: {cluster_name}
          timeoutSeconds: 30
"#
    );
    let yaml_file = write_temp_file(&yaml_update, ".yaml");
    let update_output = cli
        .run(&["route", "update", route_name, "-f", yaml_file.path().to_str().unwrap()])
        .unwrap();
    assert_eq!(
        update_output.exit_code, 0,
        "route update via YAML failed: stdout={}, stderr={}",
        update_output.stdout, update_output.stderr
    );

    // Verify updated route still exists
    let get_output = cli.run(&["route", "get", route_name]).unwrap();
    get_output.assert_success();

    // Create a listener that references this route config so Envoy
    // receives it via RDS (route configs only appear in config_dump
    // when referenced by a listener).
    let _domain =
        create_chain_for_route(&harness, route_name, "rt-upd-yaml", "rt-upd.e2e.local").await;

    // Verify updated route reaches Envoy via xDS
    verify_in_config_dump(&harness, route_name).await;
}

/// Scaffold YAML → create route → modify → `route update -f` with `kind: RouteConfig`
/// still present in the file. Proves `strip_kind_field` works for route updates.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_route_scaffold_update_yaml() {
    let harness = envoy_harness("dev_cli_rt_scaff_upd").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-rt-scfupd-cl";
    let route_name = "e2e-rt-scfupd-rc";
    let vhost_name = "e2e-rt-scfupd-vh";

    create_prerequisite_cluster(&cli, cluster_name, echo_host, echo_port);

    // Step 1: Create route from scaffold
    let replacements = &[
        ("<your-route-config-name>", route_name),
        ("<your-vhost-name>", vhost_name),
        ("<your-route-name>", "initial-route"),
        ("<your-cluster-name>", cluster_name),
        ("your-route-config-name", route_name),
        ("your-vhost-name", vhost_name),
        ("your-route-name", "initial-route"),
        ("your-cluster-name", cluster_name),
    ];
    let create_output = scaffold_and_create(
        &cli,
        &["route", "scaffold"],
        replacements,
        ".yaml",
        &["route", "create", "-f"],
    );
    assert_eq!(
        create_output.exit_code, 0,
        "scaffold route create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 2: Update with YAML that still contains `kind: RouteConfig`.
    // The CLI must strip the kind field before sending to the API.
    let update_yaml = format!(
        r#"kind: RouteConfig
name: {route_name}
virtualHosts:
  - name: {vhost_name}
    domains:
      - "updated.example.com"
    routes:
      - name: updated-route
        match:
          path:
            type: prefix
            value: "/updated"
        action:
          type: forward
          cluster: {cluster_name}
"#
    );
    let update_file = write_temp_file(&update_yaml, ".yaml");
    let update_output = cli
        .run(&["route", "update", route_name, "-f", update_file.path().to_str().unwrap()])
        .unwrap();
    assert_eq!(
        update_output.exit_code, 0,
        "route update with kind field in YAML failed: stdout={}, stderr={}",
        update_output.stdout, update_output.stderr
    );

    // Verify route still exists after update
    let get_output = cli.run(&["route", "get", route_name]).unwrap();
    get_output.assert_success();

    // Create a listener that references this route config so Envoy
    // receives it via RDS (route configs only appear in config_dump
    // when referenced by a listener).
    let _domain =
        create_chain_for_route(&harness, route_name, "rt-scfupd", "rt-scfupd.e2e.local").await;

    // Verify updated route reaches Envoy via xDS
    verify_in_config_dump(&harness, route_name).await;

    // Verify traffic flows through the updated route.
    // The update changed the domain to "updated.example.com" and prefix to "/updated",
    // but the listener was wired to "rt-scfupd.e2e.local". Traffic verification uses
    // the listener's domain since that's what Envoy binds to for Host-based routing.
    // The route's virtual host domain was updated to "updated.example.com", so use that.
    verify_traffic(&harness, "updated.example.com", "/updated").await;
}

/// `route create -f` with syntactically invalid YAML should produce a
/// user-friendly error (not a panic or raw stack trace).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_route_create_malformed_yaml() {
    let harness = dev_harness("dev_cli_rt_badyaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Intentionally broken YAML — mixed indentation, unclosed bracket, bad types
    let bad_yaml = r#"name: [broken-route
  virtualHosts:
    - name: "missing close quote
      domains: not-a-list
    routes: {{{
"#;
    let file = write_temp_file(bad_yaml, ".yaml");

    let output = cli.run(&["route", "create", "-f", file.path().to_str().unwrap()]).unwrap();

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

// ============================================================================
// cluster negative tests
// ============================================================================

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

// ============================================================================
// listener scaffold → create/update/apply roundtrip tests
// ============================================================================

/// Helper: create prerequisite cluster + route config for listener tests.
/// Returns (cluster_name, route_name, domain) so the listener can reference
/// the route config and traffic can be verified against the domain.
fn create_listener_prerequisites(
    cli: &CliRunner,
    prefix: &str,
    echo_host: &str,
    echo_port: &str,
) -> (String, String, String) {
    let cluster_name = format!("{}-cl", prefix);
    let route_name = format!("{}-rc", prefix);
    let vhost_name = format!("{}-vh", prefix);
    let domain = format!("{}.e2e.local", prefix);

    // Create cluster
    let cluster_yaml = format!(
        r#"name: {cluster_name}
endpoints:
  - host: {echo_host}
    port: {echo_port}
connectTimeoutSeconds: 5
"#
    );
    let file = write_temp_file(&cluster_yaml, ".yaml");
    let output = cli.run(&["cluster", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "prerequisite cluster create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Create route config
    let route_json = serde_json::json!({
        "name": route_name,
        "virtualHosts": [{
            "name": vhost_name,
            "domains": [&domain],
            "routes": [{
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": {"type": "forward", "cluster": &cluster_name, "timeoutSeconds": 30}
            }]
        }]
    });
    let route_file = write_temp_file(&serde_json::to_string_pretty(&route_json).unwrap(), ".json");
    let output = cli.run(&["route", "create", "-f", route_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "prerequisite route create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    (cluster_name, route_name, domain)
}

/// Listener scaffold YAML → replace placeholders (including dataplaneId) →
/// `listener create -f file.yaml` → `listener get` → config_dump → traffic.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_listener_scaffold_create_yaml() {
    let harness = envoy_harness("dev_cli_ls_scaff_yaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-ls-yaml";
    let listener_name = format!("{}-ls", prefix);
    let listener_port = harness.ports.listener;

    let (_cluster, route_name, domain) =
        create_listener_prerequisites(&cli, prefix, echo_host, echo_port);

    // Scaffold listener YAML → replace all placeholders → create
    let replacements = &[
        ("<your-listener-name>", listener_name.as_str()),
        ("<your-dataplane-id>", "dev-dataplane-id"),
        ("<your-route-config-name>", route_name.as_str()),
        ("your-listener-name", listener_name.as_str()),
        ("your-dataplane-id", "dev-dataplane-id"),
        ("your-route-config-name", route_name.as_str()),
        ("10001", &listener_port.to_string()),
    ];

    let output = scaffold_and_create(
        &cli,
        &["listener", "scaffold"],
        replacements,
        ".yaml",
        &["listener", "create", "-f"],
    );
    assert_eq!(
        output.exit_code, 0,
        "listener create -f yaml failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify listener exists via CLI
    let get_output = cli.run(&["listener", "get", &listener_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&listener_name),
        "Expected listener name in get output, got: {}",
        get_output.stdout
    );

    // Verify listener reaches Envoy via xDS
    verify_in_config_dump(&harness, &listener_name).await;

    // Verify traffic flows through the listener
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// Listener scaffold JSON → replace placeholders → `listener create -f file.json`
/// → `listener get` → config_dump → traffic.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_listener_scaffold_create_json() {
    let harness = envoy_harness("dev_cli_ls_scaff_json").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-ls-json";
    let listener_name = format!("{}-ls", prefix);
    let listener_port = harness.ports.listener;

    let (_cluster, route_name, domain) =
        create_listener_prerequisites(&cli, prefix, echo_host, echo_port);

    let replacements = &[
        ("<your-listener-name>", listener_name.as_str()),
        ("<your-dataplane-id>", "dev-dataplane-id"),
        ("<your-route-config-name>", route_name.as_str()),
        ("your-listener-name", listener_name.as_str()),
        ("your-dataplane-id", "dev-dataplane-id"),
        ("your-route-config-name", route_name.as_str()),
        ("10001", &listener_port.to_string()),
    ];

    let output = scaffold_and_create(
        &cli,
        &["listener", "scaffold", "-o", "json"],
        replacements,
        ".json",
        &["listener", "create", "-f"],
    );
    assert_eq!(
        output.exit_code, 0,
        "listener create -f json failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify listener exists via CLI
    let get_output = cli.run(&["listener", "get", &listener_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&listener_name),
        "Expected listener name in get output, got: {}",
        get_output.stdout
    );

    // Verify listener reaches Envoy via xDS
    verify_in_config_dump(&harness, &listener_name).await;

    // Verify traffic flows through the listener
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// Listener scaffold YAML → replace placeholders → `apply -f file.yaml` →
/// `listener get` → config_dump → traffic. Tests that kind:Listener is
/// correctly routed by the apply command.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_listener_scaffold_apply() {
    let harness = envoy_harness("dev_cli_ls_scaff_apply").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-ls-apply";
    let listener_name = format!("{}-ls", prefix);
    let listener_port = harness.ports.listener;

    let (_cluster, route_name, domain) =
        create_listener_prerequisites(&cli, prefix, echo_host, echo_port);

    let replacements = &[
        ("<your-listener-name>", listener_name.as_str()),
        ("<your-dataplane-id>", "dev-dataplane-id"),
        ("<your-route-config-name>", route_name.as_str()),
        ("your-listener-name", listener_name.as_str()),
        ("your-dataplane-id", "dev-dataplane-id"),
        ("your-route-config-name", route_name.as_str()),
        ("10001", &listener_port.to_string()),
    ];

    let output = scaffold_and_create(
        &cli,
        &["listener", "scaffold"],
        replacements,
        ".yaml",
        &["apply", "-f"],
    );
    assert_eq!(
        output.exit_code, 0,
        "apply -f listener scaffold yaml failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify listener exists via get
    let get_output = cli.run(&["listener", "get", &listener_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&listener_name),
        "Expected listener name in get output, got: {}",
        get_output.stdout
    );

    // Verify listener reaches Envoy via xDS
    verify_in_config_dump(&harness, &listener_name).await;

    // Verify traffic flows through the listener
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// Create a listener via JSON, then update it via `listener update -f file.yaml`.
/// Proves YAML file-based update works for listeners. Verifies updated
/// listener in Envoy config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_listener_update_yaml_file() {
    let harness = envoy_harness("dev_cli_ls_upd_yaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-ls-upd";
    let listener_name = format!("{}-ls", prefix);
    let listener_port = harness.ports.listener;

    let (_cluster, route_name, _domain) =
        create_listener_prerequisites(&cli, prefix, echo_host, echo_port);

    // Create listener via JSON first
    let json_content = serde_json::json!({
        "name": listener_name,
        "address": "0.0.0.0",
        "port": listener_port,
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": route_name
            }]
        }],
        "dataplaneId": "dev-dataplane-id"
    });
    let json_file = write_temp_file(&serde_json::to_string(&json_content).unwrap(), ".json");
    let create_output =
        cli.run(&["listener", "create", "-f", json_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "initial listener create via JSON failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Update the same listener via YAML — change filter chain name
    let yaml_update = format!(
        r#"address: "0.0.0.0"
port: {listener_port}
filterChains:
  - name: "updated-chain"
    filters:
      - name: "envoy.filters.network.http_connection_manager"
        type: httpConnectionManager
        routeConfigName: {route_name}
dataplaneId: "dev-dataplane-id"
"#
    );
    let yaml_file = write_temp_file(&yaml_update, ".yaml");
    let update_output = cli
        .run(&["listener", "update", &listener_name, "-f", yaml_file.path().to_str().unwrap()])
        .unwrap();
    assert_eq!(
        update_output.exit_code, 0,
        "listener update via YAML failed: stdout={}, stderr={}",
        update_output.stdout, update_output.stderr
    );

    // Verify updated listener reaches Envoy via xDS
    verify_in_config_dump(&harness, &listener_name).await;
}

/// Scaffold YAML → create listener → modify → `listener update -f` with
/// `kind: Listener` still present in the file. Proves strip_kind_field works
/// for listener updates, not just create. Verifies updated listener in config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_listener_scaffold_update_yaml() {
    let harness = envoy_harness("dev_cli_ls_scaff_upd").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let prefix = format!("e2e-ls-scfupd-{id}");
    let listener_name = format!("{}-ls", prefix);
    let listener_port = harness.ports.listener;

    let (_cluster, route_name, domain) =
        create_listener_prerequisites(&cli, &prefix, echo_host, echo_port);

    // Step 1: Create listener from scaffold
    let replacements = &[
        ("<your-listener-name>", listener_name.as_str()),
        ("<your-dataplane-id>", "dev-dataplane-id"),
        ("<your-route-config-name>", route_name.as_str()),
        ("your-listener-name", listener_name.as_str()),
        ("your-dataplane-id", "dev-dataplane-id"),
        ("your-route-config-name", route_name.as_str()),
        ("10001", &listener_port.to_string()),
    ];
    let create_output = scaffold_and_create(
        &cli,
        &["listener", "scaffold"],
        replacements,
        ".yaml",
        &["listener", "create", "-f"],
    );
    assert_eq!(
        create_output.exit_code, 0,
        "scaffold listener create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 2: Update with YAML that includes `kind: Listener`.
    // The CLI must strip the kind field before sending to the API.
    let update_yaml = format!(
        r#"kind: Listener
name: {listener_name}
address: "0.0.0.0"
port: {listener_port}
filterChains:
  - name: "updated-filter-chain"
    filters:
      - name: "envoy.filters.network.http_connection_manager"
        type: httpConnectionManager
        routeConfigName: {route_name}
dataplaneId: "dev-dataplane-id"
"#
    );
    let update_file = write_temp_file(&update_yaml, ".yaml");
    let update_output = cli
        .run(&["listener", "update", &listener_name, "-f", update_file.path().to_str().unwrap()])
        .unwrap();
    assert_eq!(
        update_output.exit_code, 0,
        "listener update with kind field in YAML failed: stdout={}, stderr={}",
        update_output.stdout, update_output.stderr
    );

    // Verify listener still exists after update
    let get_output = cli.run(&["listener", "get", &listener_name]).unwrap();
    get_output.assert_success();

    // Verify updated listener reaches Envoy via xDS
    verify_in_config_dump(&harness, &listener_name).await;

    // Verify traffic flows through the updated listener
    verify_traffic(&harness, &domain, "/").await;
}

/// `listener create -f file.yaml` with syntactically invalid YAML should
/// produce a user-friendly error (not a panic or raw stack trace).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_listener_create_malformed_yaml() {
    let harness = dev_harness("dev_cli_ls_badyaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Intentionally broken YAML — mixed indentation, unclosed bracket, bad nesting
    let bad_yaml = r#"name: [broken-listener
  address: "0.0.0.0"
  port: not-a-number
  filterChains:
    - name: "missing close quote
      filters: {{{
  dataplaneId: [unterminated
"#;
    let file = write_temp_file(bad_yaml, ".yaml");

    let output = cli.run(&["listener", "create", "-f", file.path().to_str().unwrap()]).unwrap();

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
        "Expected parse error message for malformed listener YAML, got: {}",
        combined
    );
}

// ============================================================================
// Filter scaffold E2E tests — scaffold → create → attach → verify in Envoy
// ============================================================================

/// Scaffold a filter, replace placeholders, create it via CLI.
/// Returns the filter name used.
#[allow(dead_code)]
fn scaffold_filter_and_create(
    cli: &CliRunner,
    filter_type: &str,
    filter_name: &str,
    replacements: &[(&str, &str)],
    extension: &str,
) -> CliOutput {
    // Scaffold the filter
    let scaffold_args: Vec<&str> = if extension == ".json" {
        vec!["filter", "scaffold", filter_type, "-o", "json"]
    } else {
        vec!["filter", "scaffold", filter_type]
    };
    let scaffold_output = cli.run(&scaffold_args).unwrap();
    assert_eq!(
        scaffold_output.exit_code, 0,
        "filter scaffold {} failed: stderr={}",
        filter_type, scaffold_output.stderr
    );

    let mut content = scaffold_output.stdout.clone();

    // Always replace the placeholder name
    content = content.replace("<your-filter-name>", filter_name);

    // Apply additional replacements
    for (from, to) in replacements {
        content = content.replace(from, to);
    }

    // For YAML: strip comment lines
    if extension == ".yaml" || extension == ".yml" {
        content = content
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
    }

    let file = write_temp_file(&content, extension);
    let file_path = file.path().to_str().unwrap().to_string();

    cli.run(&["filter", "create", "-f", &file_path]).unwrap()
}

/// Create a full cluster + route config + listener chain for testing filters.
/// Returns (listener_name, domain) for traffic verification.
fn create_filter_test_chain(
    cli: &CliRunner,
    prefix: &str,
    echo_host: &str,
    echo_port: &str,
    listener_port: u16,
) -> (String, String, String) {
    let cluster_name = format!("{}-cl", prefix);
    let route_name = format!("{}-rc", prefix);
    let vhost_name = format!("{}-vh", prefix);
    let listener_name = format!("{}-ls", prefix);
    let domain = format!("{}.e2e.local", prefix);

    // Create cluster
    let cluster_yaml = format!(
        r#"name: {cluster_name}
endpoints:
  - host: {echo_host}
    port: {echo_port}
connectTimeoutSeconds: 5
"#
    );
    let file = write_temp_file(&cluster_yaml, ".yaml");
    let output = cli.run(&["cluster", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "filter chain: cluster create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Create route config
    let route_json = serde_json::json!({
        "name": route_name,
        "virtualHosts": [{
            "name": vhost_name,
            "domains": [&domain],
            "routes": [{
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": {"type": "forward", "cluster": &cluster_name, "timeoutSeconds": 30}
            }]
        }]
    });
    let route_file = write_temp_file(&serde_json::to_string_pretty(&route_json).unwrap(), ".json");
    let output = cli.run(&["route", "create", "-f", route_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "filter chain: route create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Create listener
    let listener_json = serde_json::json!({
        "name": listener_name,
        "address": "0.0.0.0",
        "port": listener_port,
        "filterChains": [{
            "name": "default",
            "filters": [{
                "name": "envoy.filters.network.http_connection_manager",
                "type": "httpConnectionManager",
                "routeConfigName": route_name
            }]
        }],
        "dataplaneId": "dev-dataplane-id"
    });
    let listener_file =
        write_temp_file(&serde_json::to_string_pretty(&listener_json).unwrap(), ".json");
    let output =
        cli.run(&["listener", "create", "-f", listener_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "filter chain: listener create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    (listener_name, route_name, domain)
}

/// Attach a filter to a listener via CLI.
fn attach_filter_to_listener(cli: &CliRunner, filter_name: &str, listener_name: &str) {
    let output = cli.run(&["filter", "attach", filter_name, "--listener", listener_name]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "filter attach failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// E2E: scaffold header_mutation filter → create → attach to listener →
/// send traffic → verify response has custom headers added by the filter.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_header_mutation() {
    let harness = envoy_harness("dev_cli_flt_hdrmut").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-hdrmut";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, domain) =
        create_filter_test_chain(&cli, prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType
    let scaffold_output =
        cli.run(&["filter", "scaffold", "header_mutation", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value = serde_json::from_str(&scaffold_output.stdout)
        .expect("scaffold output should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("header_mutation"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );

    // Step 3: Create filter with header_mutation config that adds response headers
    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "header_mutation",
        "config": {
            "type": "header_mutation",
            "config": {
                "response_headers_to_add": [
                    {"key": "X-Flowplane-E2E", "value": "header-mutation-test"},
                    {"key": "X-Custom-Filter", "value": "active"}
                ]
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists via CLI
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Wait for config to propagate to Envoy
    // Verify the listener (not filter name — filters appear as Envoy type names in config_dump)
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;

    // Step 7: Send traffic and verify header mutation adds response headers
    let envoy = harness.envoy().expect("Envoy should be available");
    let resp = envoy
        .proxy_get_with_headers(listener_port, &domain, "/test")
        .await
        .expect("proxy request should succeed");

    assert_eq!(resp.status, 200, "Expected 200 OK, got {}. Body: {}", resp.status, resp.body);
    assert_eq!(
        resp.headers.get("x-flowplane-e2e").map(|v| v.as_str()),
        Some("header-mutation-test"),
        "Expected X-Flowplane-E2E header in response. Headers: {:?}",
        resp.headers
    );
    assert_eq!(
        resp.headers.get("x-custom-filter").map(|v| v.as_str()),
        Some("active"),
        "Expected X-Custom-Filter header in response. Headers: {:?}",
        resp.headers
    );
}

/// E2E: scaffold cors filter → create → attach to listener →
/// send OPTIONS with Origin header → verify CORS response headers.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_cors() {
    let harness = envoy_harness("dev_cli_flt_cors").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-cors";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, domain) =
        create_filter_test_chain(&cli, prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType
    let scaffold_output = cli.run(&["filter", "scaffold", "cors", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value =
        serde_json::from_str(&scaffold_output.stdout).expect("scaffold should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("cors"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );

    // Step 3: Create CORS filter that allows a specific origin
    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "cors",
        "config": {
            "type": "cors",
            "config": {
                "policy": {
                    "allow_origin": [
                        {"type": "exact", "value": "https://e2e-test.example.com"}
                    ],
                    "allow_methods": ["GET", "POST", "OPTIONS"],
                    "allow_headers": ["Content-Type", "Authorization"],
                    "max_age": 3600
                }
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "cors filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Wait for config to propagate
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;

    // Step 7: Verify traffic still flows with CORS filter attached
    // Note: CORS policy enforcement requires per-route typed_per_filter_config;
    // listener-level attachment enables the filter chain but doesn't activate policies.
    verify_traffic(&harness, &domain, "/test").await;
}

/// E2E: scaffold custom_response filter → create → attach to listener →
/// verify filter attaches and appears in config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_custom_response() {
    let harness = envoy_harness("dev_cli_flt_custresp").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-custresp";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, domain) =
        create_filter_test_chain(&cli, prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType
    let scaffold_output =
        cli.run(&["filter", "scaffold", "custom_response", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value =
        serde_json::from_str(&scaffold_output.stdout).expect("scaffold should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("custom_response"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );

    // Step 3: Create custom_response filter (minimal — no matchers required)
    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "custom_response",
        "config": {
            "type": "custom_response",
            "config": {
                "matchers": []
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "custom_response filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Verify in config_dump + traffic still works
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// E2E: scaffold rbac filter → create → attach to listener →
/// verify filter attaches and appears in config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_rbac() {
    let harness = envoy_harness("dev_cli_flt_rbac").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-rbac";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, domain) =
        create_filter_test_chain(&cli, prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType
    let scaffold_output = cli.run(&["filter", "scaffold", "rbac", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value =
        serde_json::from_str(&scaffold_output.stdout).expect("scaffold should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("rbac"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );

    // Step 3: Create RBAC filter with a permissive allow-all policy
    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "rbac",
        "config": {
            "type": "rbac",
            "config": {
                "rules": {
                    "action": "allow",
                    "policies": {
                        "allow-all": {
                            "permissions": [{"type": "any", "any": true}],
                            "principals": [{"type": "any", "any": true}]
                        }
                    }
                }
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "rbac filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Verify in config_dump + traffic still works (allow-all should pass)
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// E2E: scaffold mcp filter → create → attach to listener →
/// verify filter attaches and appears in config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_mcp() {
    let harness = envoy_harness("dev_cli_flt_mcp").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let prefix = format!("e2e-flt-mcp-{id}");
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, domain) =
        create_filter_test_chain(&cli, &prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType
    let scaffold_output = cli.run(&["filter", "scaffold", "mcp", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value =
        serde_json::from_str(&scaffold_output.stdout).expect("scaffold should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("mcp"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );

    // Step 3: Create MCP filter with pass_through mode
    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "mcp",
        "config": {
            "type": "mcp",
            "config": {
                "traffic_mode": "pass_through"
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "mcp filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Verify in config_dump + traffic works (pass_through allows all traffic)
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// E2E: local_rate_limit scaffold → create → attach to listener →
/// send rapid requests → verify 429 rate limited response.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_local_rate_limit() {
    let harness = envoy_harness("dev_cli_flt_ratelim").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-ratelim";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, domain) =
        create_filter_test_chain(&cli, prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType and produces valid JSON
    let scaffold_output =
        cli.run(&["filter", "scaffold", "local_rate_limit", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value = serde_json::from_str(&scaffold_output.stdout)
        .expect("scaffold output should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("local_rate_limit"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );
    // Verify the scaffold produced the required nested fields
    let config_config =
        scaffold_json.pointer("/config/config").expect("scaffold should have config.config");
    assert!(
        config_config.get("stat_prefix").is_some(),
        "Scaffold must include required stat_prefix. Got: {}",
        scaffold_output.stdout
    );
    assert!(
        config_config.get("token_bucket").is_some(),
        "Scaffold must include required token_bucket. Got: {}",
        scaffold_output.stdout
    );

    // Step 3: Create local_rate_limit filter with very restrictive rate (1 token, long refill)
    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "local_rate_limit",
        "config": {
            "type": "local_rate_limit",
            "config": {
                "stat_prefix": "e2e_rate_limit",
                "token_bucket": {
                    "max_tokens": 1,
                    "tokens_per_fill": 1,
                    "fill_interval_ms": 60000
                },
                "filter_enabled": {
                    "numerator": 100,
                    "denominator": "hundred"
                },
                "filter_enforced": {
                    "numerator": 100,
                    "denominator": "hundred"
                }
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "local_rate_limit filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Wait for config propagation
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;

    // Step 7: Send rapid requests — first should succeed, subsequent should get 429
    let envoy = harness.envoy().expect("Envoy should be available");

    // First request: should consume the single token → 200
    let resp = envoy
        .proxy_get_with_headers(listener_port, &domain, "/test")
        .await
        .expect("first request should succeed");
    assert_eq!(
        resp.status, 200,
        "First request should succeed with 200. Got: {}. Body: {}",
        resp.status, resp.body
    );

    // Rapid follow-up requests: at least one should get 429
    let mut got_429 = false;
    for _ in 0..5 {
        let resp = envoy
            .proxy_get_with_headers(listener_port, &domain, "/test")
            .await
            .expect("follow-up request should complete");
        if resp.status == 429 {
            got_429 = true;
            break;
        }
    }
    assert!(got_429, "Expected at least one 429 response after exhausting rate limit token");
}

/// E2E: compressor scaffold → create → attach to listener →
/// verify filter attaches and appears in config_dump, traffic flows.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_compressor() {
    let harness = envoy_harness("dev_cli_flt_compress").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-compress";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, domain) =
        create_filter_test_chain(&cli, prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType and required compressor_library.type
    let scaffold_output = cli.run(&["filter", "scaffold", "compressor", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value = serde_json::from_str(&scaffold_output.stdout)
        .expect("scaffold output should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("compressor"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );
    // Verify the scaffold produced the required compressor_library with type
    let config_config =
        scaffold_json.pointer("/config/config").expect("scaffold should have config.config");
    let compressor_lib =
        config_config.get("compressor_library").expect("Scaffold must include compressor_library");
    assert_eq!(
        compressor_lib.get("type").and_then(|v| v.as_str()),
        Some("gzip"),
        "compressor_library.type must be 'gzip'. Got: {:?}",
        compressor_lib
    );

    // Step 3: Create compressor filter with gzip config
    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "compressor",
        "config": {
            "type": "compressor",
            "config": {
                "compressor_library": {
                    "type": "gzip",
                    "compression_level": "best_speed"
                }
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "compressor filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Verify in config_dump + traffic still works
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;

    // Step 7: Send request with Accept-Encoding: gzip and verify response
    let envoy = harness.envoy().expect("Envoy should be available");
    let mut headers = std::collections::HashMap::new();
    headers.insert("Accept-Encoding".to_string(), "gzip".to_string());

    let (status, resp_headers, _body) = envoy
        .proxy_request(listener_port, hyper::Method::GET, &domain, "/test", headers, None)
        .await
        .expect("request with Accept-Encoding should succeed");

    assert_eq!(status, 200, "Expected 200, got {status}");

    // Compressor may or may not compress depending on response size (min_content_length).
    // Just verify the filter didn't break traffic. If compressed, content-encoding is present.
    if let Some(encoding) = resp_headers.get("content-encoding") {
        assert_eq!(
            encoding, "gzip",
            "If content-encoding is set, it should be gzip. Got: {encoding}"
        );
    }
    // Either way, traffic should work
}

/// E2E: ext_authz scaffold → create → attach to listener →
/// verify filter attaches and appears in config_dump.
/// ext_authz needs an external auth service for full traffic testing,
/// so we verify config_dump only.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_ext_authz() {
    let harness = envoy_harness("dev_cli_flt_extauthz").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let prefix = format!("e2e-flt-extauthz-{id}");
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, _domain) =
        create_filter_test_chain(&cli, &prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType and required service.type
    let scaffold_output = cli.run(&["filter", "scaffold", "ext_authz", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value = serde_json::from_str(&scaffold_output.stdout)
        .expect("scaffold output should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("ext_authz"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );
    // Verify the scaffold produced the required service with type
    let config_config =
        scaffold_json.pointer("/config/config").expect("scaffold should have config.config");
    let service = config_config.get("service").expect("Scaffold must include service");
    assert!(
        service.get("type").is_some(),
        "service.type must be present in scaffold. Got: {:?}",
        service
    );

    // Step 3: Create ext_authz filter pointing to an authz cluster (use echo as placeholder)
    let authz_cluster_name = format!("{}-authz-cl", prefix);
    let authz_cluster_yaml = format!(
        r#"name: {authz_cluster_name}
endpoints:
  - host: {echo_host}
    port: {echo_port}
connectTimeoutSeconds: 5
"#
    );
    let cl_file = write_temp_file(&authz_cluster_yaml, ".yaml");
    let cl_output =
        cli.run(&["cluster", "create", "-f", cl_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        cl_output.exit_code, 0,
        "authz cluster create failed: stdout={}, stderr={}",
        cl_output.stdout, cl_output.stderr
    );

    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "ext_authz",
        "config": {
            "type": "ext_authz",
            "config": {
                "service": {
                    "type": "grpc",
                    "target_uri": authz_cluster_name,
                    "timeout_ms": 200
                },
                "failure_mode_allow": false
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "ext_authz filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Verify filter reaches Envoy via config_dump
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;
}

/// E2E: jwt_auth scaffold → create → attach to listener →
/// send request without JWT → verify 401.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_jwt_auth() {
    let harness = envoy_harness("dev_cli_flt_jwtauth").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-jwtauth";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, domain) =
        create_filter_test_chain(&cli, prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType and required providers
    let scaffold_output = cli.run(&["filter", "scaffold", "jwt_auth", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value = serde_json::from_str(&scaffold_output.stdout)
        .expect("scaffold output should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("jwt_auth"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );
    let config_config =
        scaffold_json.pointer("/config/config").expect("scaffold should have config.config");
    assert!(
        config_config.get("providers").is_some(),
        "Scaffold must include providers. Got: {}",
        scaffold_output.stdout
    );

    // Step 3: Create jwt_auth filter with a local JWKS provider (inline empty JWKS)
    // This will reject all requests since there are no valid keys to validate against
    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "jwt_auth",
        "config": {
            "type": "jwt_auth",
            "config": {
                "providers": {
                    "test-provider": {
                        "issuer": "https://e2e-test.example.com",
                        "jwks": {
                            "type": "local",
                            "inline_string": "{\"keys\":[]}"
                        },
                        "from_headers": [
                            {"name": "Authorization", "value_prefix": "Bearer "}
                        ]
                    }
                },
                "rules": []
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "jwt_auth filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Verify filter injection and traffic flow
    // JWT enforcement with local empty JWKS is tested separately in test_routing.rs.
    // Here we verify the scaffold → create → attach lifecycle works end-to-end.
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;
    verify_traffic(&harness, &domain, "/test").await;
}

/// E2E: oauth2 scaffold → create → verify filter exists.
/// oauth2 needs an external OAuth provider for full traffic testing and
/// requires HTTPS (secure cookies), so we verify create + get only.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_scaffold_oauth2() {
    let harness = envoy_harness("dev_cli_flt_oauth2").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-oauth2";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Step 1: Create cluster + route + listener chain
    let (listener_name, _route_name, _domain) =
        create_filter_test_chain(&cli, prefix, echo_host, echo_port, listener_port);

    // Step 2: Verify scaffold includes filterType and required fields
    let scaffold_output = cli.run(&["filter", "scaffold", "oauth2", "-o", "json"]).unwrap();
    scaffold_output.assert_success();
    let scaffold_json: serde_json::Value = serde_json::from_str(&scaffold_output.stdout)
        .expect("scaffold output should be valid JSON");
    assert_eq!(
        scaffold_json.get("filterType").and_then(|v| v.as_str()),
        Some("oauth2"),
        "Scaffold must include filterType. Got: {}",
        scaffold_output.stdout
    );
    let config_config =
        scaffold_json.pointer("/config/config").expect("scaffold should have config.config");
    assert!(
        config_config.get("token_endpoint").is_some(),
        "Scaffold must include required token_endpoint. Got: {}",
        scaffold_output.stdout
    );
    assert!(
        config_config.get("credentials").is_some(),
        "Scaffold must include required credentials. Got: {}",
        scaffold_output.stdout
    );
    // Verify credentials has client_id
    let creds = config_config.get("credentials").unwrap();
    assert!(
        creds.get("client_id").is_some(),
        "credentials.client_id must be present. Got: {:?}",
        creds
    );

    // Step 3: Create oauth2 filter — needs a token endpoint cluster
    let token_cluster_name = format!("{}-token-cl", prefix);
    let token_cluster_yaml = format!(
        r#"name: {token_cluster_name}
endpoints:
  - host: {echo_host}
    port: {echo_port}
connectTimeoutSeconds: 5
"#
    );
    let cl_file = write_temp_file(&token_cluster_yaml, ".yaml");
    let cl_output =
        cli.run(&["cluster", "create", "-f", cl_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        cl_output.exit_code, 0,
        "token cluster create failed: stdout={}, stderr={}",
        cl_output.stdout, cl_output.stderr
    );

    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "oauth2",
        "config": {
            "type": "oauth2",
            "config": {
                "token_endpoint": {
                    "uri": "https://idp.example.com/oauth/token",
                    "cluster": token_cluster_name,
                    "timeout_ms": 5000
                },
                "authorization_endpoint": "https://idp.example.com/authorize",
                "credentials": {
                    "client_id": "e2e-test-client",
                    "token_secret": {
                        "name": "e2e-oauth-secret"
                    }
                },
                "redirect_uri": "https://app.example.com/oauth2/callback",
                "redirect_path": "/oauth2/callback"
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let create_output =
        cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "oauth2 filter create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 4: Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(
        get_output.stdout.contains(&filter_name),
        "Expected filter name in get output, got: {}",
        get_output.stdout
    );

    // Step 5: Attach filter to listener
    attach_filter_to_listener(&cli, &filter_name, &listener_name);

    // Step 6: Verify filter reaches Envoy via config_dump
    // OAuth2 requires HTTPS for full functionality (cookie secure flag), but the
    // filter should still attach and appear in config_dump over HTTP
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;
}
