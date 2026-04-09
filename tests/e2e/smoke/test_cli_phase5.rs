//! Phase 5 CLI E2E tests — dev mode
//!
//! Tests the CLI commands added in Phases 3-5: mTLS/cert, admin, MCP tools,
//! WASM, apply lifecycle, scaffold, and negative cases.
//!
//! Scaffold create/update tests use an Envoy-enabled harness and verify that
//! resources reach Envoy via xDS (config_dump) and that traffic flows through
//! the proxy. Error tests (bad extension, malformed YAML) remain API-only.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_phase5 -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use std::io::Write;
use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::{dev_harness, quick_harness, TestHarness};

/// Write content to a named temp file with the given extension.
fn write_temp_file(content: &str, extension: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new().suffix(extension).tempfile().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

// ============================================================================
// Envoy verification helpers
// ============================================================================

/// Poll Envoy config_dump until the given resource name appears, or timeout.
async fn verify_in_config_dump(harness: &TestHarness, resource_name: &str) {
    if !harness.has_envoy() {
        eprintln!("SKIP: Envoy not available — cannot verify config_dump");
        return;
    }
    let envoy = harness.envoy().expect("Envoy should be available");
    envoy.wait_for_config_content(resource_name).await.unwrap_or_else(|e| {
        panic!("Resource '{}' not found in Envoy config_dump: {}", resource_name, e)
    });
}

/// Create a route config + listener via the API to complete the Envoy chain
/// for a cluster that was already created via CLI. Returns (route_name, domain)
/// for traffic verification.
async fn create_chain_for_cluster(
    harness: &TestHarness,
    cluster_name: &str,
    test_prefix: &str,
) -> (String, String) {
    let team = &harness.team;
    let route_name = format!("{}-rc", test_prefix);
    let domain = format!("{}.e2e.local", test_prefix);

    // Create route config pointing to the cluster
    let route_body = serde_json::json!({
        "name": route_name,
        "virtualHosts": [{
            "name": format!("{}-vh", test_prefix),
            "domains": [&domain],
            "routes": [{
                "name": format!("{}-rt", test_prefix),
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": {"type": "forward", "cluster": cluster_name, "timeoutSeconds": 30}
            }]
        }]
    });
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{}/route-configs", team), &route_body)
        .await
        .expect("route create should succeed");
    assert!(
        resp.status().is_success(),
        "route create returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Create listener pointing to the route config
    let listener_body = serde_json::json!({
        "name": format!("{}-ls", test_prefix),
        "address": "0.0.0.0",
        "port": harness.ports.listener,
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
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{}/listeners", team), &listener_body)
        .await
        .expect("listener create should succeed");
    assert!(
        resp.status().is_success(),
        "listener create returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    tokio::time::sleep(Duration::from_secs(2)).await;
    (route_name, domain)
}

/// Create a listener via the API to complete the Envoy chain for a route config
/// that was already created via CLI (with its prerequisite cluster already in place).
/// Returns the domain for traffic verification.
async fn create_chain_for_route(
    harness: &TestHarness,
    route_name: &str,
    test_prefix: &str,
    domain: &str,
) -> String {
    let team = &harness.team;

    // Create listener pointing to the route config
    let listener_body = serde_json::json!({
        "name": format!("{}-ls", test_prefix),
        "address": "0.0.0.0",
        "port": harness.ports.listener,
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
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{}/listeners", team), &listener_body)
        .await
        .expect("listener create should succeed");
    assert!(
        resp.status().is_success(),
        "listener create returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    tokio::time::sleep(Duration::from_secs(2)).await;
    domain.to_string()
}

/// Verify traffic flows through Envoy for a given domain and path.
async fn verify_traffic(harness: &TestHarness, domain: &str, path: &str) {
    if !harness.has_envoy() {
        eprintln!("SKIP: Envoy not available — cannot verify traffic");
        return;
    }
    let body = harness
        .wait_for_route(domain, path, 200)
        .await
        .unwrap_or_else(|e| panic!("Traffic verification failed for {}:{} — {}", domain, path, e));
    assert!(!body.is_empty(), "Proxied response body should not be empty");
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
/// cluster exists via `cluster get`, appears in Envoy config_dump, and traffic flows.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_create_yaml() {
    let harness = quick_harness("dev_cli_scaff_cr_yaml").await.expect("harness should start");
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
    let harness = quick_harness("dev_cli_scaff_cr_json").await.expect("harness should start");
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
    let harness = quick_harness("dev_cli_cr_yaml_file").await.expect("harness should start");
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
    let harness = quick_harness("dev_cli_upd_yaml").await.expect("harness should start");
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
}

/// Scaffold YAML → create cluster → modify a field → `update -f file.yaml` with
/// `kind: Cluster` still present in the file. Proves `strip_kind_field` works in
/// the update code path, not just create. Verifies updated cluster in Envoy config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_update_yaml() {
    let harness = quick_harness("dev_cli_scaff_upd").await.expect("harness should start");
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
}

/// Scaffold YAML → replace placeholders → `apply -f file.yaml` → verify cluster
/// exists, appears in Envoy config_dump, and traffic flows.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cluster_scaffold_apply() {
    let harness = quick_harness("dev_cli_scaff_apply").await.expect("harness should start");
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
    let harness = quick_harness("dev_cli_rt_scaff_yaml").await.expect("harness should start");
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
    let harness = quick_harness("dev_cli_rt_scaff_json").await.expect("harness should start");
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
    let harness = quick_harness("dev_cli_rt_scaff_apply").await.expect("harness should start");
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
    let harness = quick_harness("dev_cli_rt_upd_yaml").await.expect("harness should start");
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
    let harness = quick_harness("dev_cli_rt_scaff_upd").await.expect("harness should start");
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
