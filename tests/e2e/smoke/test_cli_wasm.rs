//! E2E tests for CLI WASM custom filter commands (dev mode).
//!
//! Tests `flowplane wasm create`, `wasm update`, `wasm delete`, and
//! negative cases (invalid config, nonexistent resources).
//!
//! WASM filters are custom Envoy extensions managed via the custom-filters API.
//! The CLI reads a JSON definition file and POSTs/PATCHes to the API.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_wasm -- --ignored --nocapture
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::quick_harness;
use crate::common::test_helpers::{
    create_chain_for_cluster, verify_in_config_dump, write_temp_file,
};

/// Minimal valid WASM filter definition JSON for testing.
/// Uses a tiny valid WASM module (magic + version header only, base64-encoded).
fn sample_wasm_filter_json(name: &str) -> String {
    // Minimal valid WASM binary: \0asm\x01\x00\x00\x00 (8 bytes)
    let wasm_base64 = "AGFzbQEAAAA=";
    serde_json::json!({
        "name": name,
        "display_name": format!("E2E Test Filter {}", name),
        "description": "Created by E2E test",
        "wasm_binary_base64": wasm_base64,
        "config_schema": {
            "type": "object",
            "properties": {
                "enabled": { "type": "boolean" }
            }
        },
        "attachment_points": ["listener", "route"],
        "runtime": "envoy.wasm.runtime.v8",
        "failure_policy": "FAIL_CLOSED"
    })
    .to_string()
}

/// Updated WASM filter definition for patch operations.
fn updated_wasm_filter_json() -> String {
    serde_json::json!({
        "display_name": "E2E Updated Filter",
        "description": "Updated by E2E test",
        "config_schema": {
            "type": "object",
            "properties": {
                "enabled": { "type": "boolean" },
                "log_level": { "type": "string" }
            }
        },
        "attachment_points": ["listener"]
    })
    .to_string()
}

// ============================================================================
// wasm create
// ============================================================================

/// Create a WASM filter via CLI, verify it exists via `wasm list` and `wasm get`.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_create() {
    let harness = quick_harness("dev_wasm_create").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let filter_name = "e2e-wasm-create";
    let json_content = sample_wasm_filter_json(filter_name);
    let file = write_temp_file(&json_content, ".json");

    // Step 1: create the WASM filter
    let create_output = cli.run(&["wasm", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "wasm create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Parse response to get the filter ID
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("create output should be valid JSON");
    let filter_id = created["id"].as_str().expect("response should contain id");
    assert_eq!(created["name"].as_str(), Some(filter_name));
    assert_eq!(
        created["display_name"].as_str(),
        Some(&format!("E2E Test Filter {}", filter_name) as &str)
    );

    // Step 2: verify it appears in `wasm list`
    let list_output = cli.run(&["wasm", "list", "-o", "json"]).unwrap();
    list_output.assert_success();
    let list_text = &list_output.stdout;
    assert!(
        list_text.contains(filter_name),
        "Created WASM filter '{}' should appear in list output, got: {}",
        filter_name,
        list_text
    );

    // Step 3: verify it appears in `wasm get <id>`
    let get_output = cli.run(&["wasm", "get", filter_id]).unwrap();
    get_output.assert_success();
    let get_json: serde_json::Value =
        serde_json::from_str(&get_output.stdout).expect("get output should be valid JSON");
    assert_eq!(get_json["name"].as_str(), Some(filter_name));
    assert_eq!(get_json["id"].as_str(), Some(filter_id));
    assert_eq!(get_json["failure_policy"].as_str(), Some("FAIL_CLOSED"));

    // Step 4: verify in Envoy config_dump if available.
    // Custom WASM filters may not appear directly in config_dump until attached
    // to a listener/route, but the filter_type identifier should be registered.
    // We check for the custom filter type pattern.
    let filter_type = format!("custom_wasm_{}", filter_id);
    if harness.has_envoy() {
        if let Ok(config_dump) = harness.get_config_dump().await {
            eprintln!(
                "NOTE: custom WASM filter type '{}' {} in Envoy config_dump \
                 (may require attachment to a filter chain to appear)",
                filter_type,
                if config_dump.contains(&filter_type) { "found" } else { "not found" }
            );
        }
    }

    // Cleanup
    let _ = cli.run(&["wasm", "delete", filter_id, "--yes"]);
}

// ============================================================================
// wasm update
// ============================================================================

/// Create a WASM filter, update it, verify the update took effect.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_update() {
    let harness = quick_harness("dev_wasm_update").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let filter_name = "e2e-wasm-update";
    let json_content = sample_wasm_filter_json(filter_name);
    let create_file = write_temp_file(&json_content, ".json");

    // Step 1: create the filter
    let create_output =
        cli.run(&["wasm", "create", "-f", create_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "wasm create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("create output should be valid JSON");
    let filter_id = created["id"].as_str().expect("response should contain id");

    // Step 2: update the filter
    let update_content = updated_wasm_filter_json();
    let update_file = write_temp_file(&update_content, ".json");
    let update_output = cli
        .run(&["wasm", "update", filter_id, "-f", update_file.path().to_str().unwrap()])
        .unwrap();
    assert_eq!(
        update_output.exit_code, 0,
        "wasm update failed: stdout={}, stderr={}",
        update_output.stdout, update_output.stderr
    );

    // Step 3: verify the update via `wasm get`
    let get_output = cli.run(&["wasm", "get", filter_id]).unwrap();
    get_output.assert_success();
    let get_json: serde_json::Value =
        serde_json::from_str(&get_output.stdout).expect("get output should be valid JSON");
    assert_eq!(
        get_json["display_name"].as_str(),
        Some("E2E Updated Filter"),
        "display_name should be updated, got: {:?}",
        get_json["display_name"]
    );
    assert_eq!(
        get_json["description"].as_str(),
        Some("Updated by E2E test"),
        "description should be updated, got: {:?}",
        get_json["description"]
    );
    // attachment_points should now be just ["listener"]
    let attachment_points =
        get_json["attachment_points"].as_array().expect("attachment_points should be an array");
    assert_eq!(
        attachment_points.len(),
        1,
        "Expected 1 attachment point after update, got: {:?}",
        attachment_points
    );
    assert_eq!(
        attachment_points[0].as_str(),
        Some("listener"),
        "attachment_point should be 'listener', got: {:?}",
        attachment_points[0]
    );

    // Cleanup
    let _ = cli.run(&["wasm", "delete", filter_id, "--yes"]);
}

// ============================================================================
// wasm delete
// ============================================================================

/// Create a WASM filter, delete it, verify it's gone from list and get returns error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_delete() {
    let harness = quick_harness("dev_wasm_delete").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let filter_name = "e2e-wasm-delete";
    let json_content = sample_wasm_filter_json(filter_name);
    let file = write_temp_file(&json_content, ".json");

    // Step 1: create the filter
    let create_output = cli.run(&["wasm", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "wasm create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("create output should be valid JSON");
    let filter_id = created["id"].as_str().expect("response should contain id");

    // Step 2: verify it exists
    let get_output = cli.run(&["wasm", "get", filter_id]).unwrap();
    get_output.assert_success();

    // Step 3: delete the filter
    let delete_output = cli.run(&["wasm", "delete", filter_id, "--yes"]).unwrap();
    delete_output.assert_success();
    delete_output.assert_stdout_contains("deleted");

    // Step 4: verify it's gone — `wasm get` should fail or return error
    let get_after = cli.run(&["wasm", "get", filter_id]).unwrap();
    let is_gone = get_after.exit_code != 0
        || get_after.stderr.to_lowercase().contains("not found")
        || get_after.stderr.to_lowercase().contains("error")
        || get_after.stdout.to_lowercase().contains("not found")
        || get_after.stdout.to_lowercase().contains("error");
    assert!(
        is_gone,
        "Expected error after deleting WASM filter, got exit_code={}, stdout={}, stderr={}",
        get_after.exit_code, get_after.stdout, get_after.stderr
    );

    // Step 5: verify it's gone from list
    let list_output = cli.run(&["wasm", "list", "-o", "json"]).unwrap();
    list_output.assert_success();
    assert!(
        !list_output.stdout.contains(filter_id),
        "Deleted WASM filter '{}' should not appear in list output",
        filter_id
    );

    // Step 6: verify removed from Envoy config if applicable
    let filter_type = format!("custom_wasm_{}", filter_id);
    if harness.has_envoy() {
        if let Ok(config_dump) = harness.get_config_dump().await {
            assert!(
                !config_dump.contains(&filter_type),
                "Deleted WASM filter type '{}' should not appear in Envoy config_dump",
                filter_type
            );
        }
    }
}

// ============================================================================
// WASM filter → Envoy delivery (full chain)
// ============================================================================

/// Full Envoy delivery test: create WASM filter → create a filter instance of
/// that custom type → set up routing infra → attach filter to listener →
/// verify `envoy.filters.http.wasm` appears in Envoy config_dump.
///
/// WASM filters only appear in Envoy when instantiated and attached to a listener.
/// This mirrors the SDS pattern from test_cli_secrets.rs.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_envoy_delivery() {
    let harness = quick_harness("dev_wasm_envoy").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    if !harness.has_envoy() {
        eprintln!("SKIP: Envoy not available — cannot verify WASM delivery");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Step 1: create the WASM filter definition
    let wasm_name = "e2e-wasm-envoy";
    let json_content = sample_wasm_filter_json(wasm_name);
    let file = write_temp_file(&json_content, ".json");

    let create_output = cli.run(&["wasm", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    create_output.assert_success();
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("create output should be valid JSON");
    let wasm_id = created["id"].as_str().expect("response should contain id");
    let filter_type = format!("custom_wasm_{}", wasm_id);

    // Step 2: create routing infra (cluster → route + listener)
    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let cluster_name = "wasm-envoy-cluster";
    let cluster_yaml = format!(
        "name: {cluster_name}\nendpoints:\n  - host: {}\n    port: {}\nconnectTimeoutSeconds: 5\n",
        parts[0], parts[1]
    );
    let cluster_file = write_temp_file(&cluster_yaml, ".yaml");
    cli.run(&["cluster", "create", "-f", cluster_file.path().to_str().unwrap()])
        .unwrap()
        .assert_success();

    let (_route_name, _domain) =
        create_chain_for_cluster(&harness, cluster_name, "wasm-envoy").await;
    verify_in_config_dump(&harness, cluster_name).await;

    // Step 3: create a filter instance using the custom WASM type
    // The filter type is `custom_wasm_{id}` which maps to envoy.filters.http.wasm
    let filter_name = "wasm-envoy-filter";
    let filter_yaml = format!(
        r#"name: {filter_name}
filterType: {filter_type}
config:
  type: {filter_type}
  config:
    enabled: true
"#
    );
    let filter_file = write_temp_file(&filter_yaml, ".yaml");
    let filter_output =
        cli.run(&["filter", "create", "-f", filter_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        filter_output.exit_code, 0,
        "filter create with WASM type failed: stdout={}, stderr={}",
        filter_output.stdout, filter_output.stderr
    );

    // Step 4: attach the filter to the listener
    let listener_name = "wasm-envoy-ls";
    let attach_output =
        cli.run(&["filter", "attach", filter_name, "--listener", listener_name]).unwrap();
    assert_eq!(
        attach_output.exit_code, 0,
        "filter attach failed: stdout={}, stderr={}",
        attach_output.stdout, attach_output.stderr
    );

    // Step 5: wait for xDS delivery and verify in Envoy config_dump
    tokio::time::sleep(Duration::from_secs(3)).await;

    let config_dump = harness.get_config_dump().await.expect("config_dump should succeed");

    // WASM filters appear as envoy.filters.http.wasm in Envoy config
    let wasm_in_envoy = config_dump.contains("envoy.filters.http.wasm")
        || config_dump.contains(&filter_type)
        || config_dump.contains(filter_name);

    assert!(
        wasm_in_envoy,
        "WASM filter should appear in Envoy config_dump after attachment. \
         Looked for 'envoy.filters.http.wasm', '{}', or '{}'. \
         Config dump does not contain any of them.",
        filter_type, filter_name
    );

    eprintln!("WASM ENVOY VERIFIED: filter type '{}' delivered to Envoy", filter_type);

    // Cleanup
    let _ = cli.run(&["filter", "delete", filter_name, "--yes"]);
    let _ = cli.run(&["wasm", "delete", wasm_id, "--yes"]);
}

/// Delete a WASM filter that has an attached instance, verify removal from Envoy.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_delete_verify_envoy_removal() {
    let harness = quick_harness("dev_wasm_del_envoy").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    if !harness.has_envoy() {
        eprintln!("SKIP: Envoy not available");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Create WASM filter + routing + filter instance + attach (same setup as above)
    let wasm_name = "e2e-wasm-del-envoy";
    let json_content = sample_wasm_filter_json(wasm_name);
    let file = write_temp_file(&json_content, ".json");
    let create_output = cli.run(&["wasm", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    create_output.assert_success();
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("valid JSON");
    let wasm_id = created["id"].as_str().expect("id");
    let filter_type = format!("custom_wasm_{}", wasm_id);

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let cluster_yaml = format!(
        "name: wasm-del-cluster\nendpoints:\n  - host: {}\n    port: {}\nconnectTimeoutSeconds: 5\n",
        parts[0], parts[1]
    );
    let cluster_file = write_temp_file(&cluster_yaml, ".yaml");
    cli.run(&["cluster", "create", "-f", cluster_file.path().to_str().unwrap()])
        .unwrap()
        .assert_success();

    create_chain_for_cluster(&harness, "wasm-del-cluster", "wasm-del").await;
    verify_in_config_dump(&harness, "wasm-del-cluster").await;

    let filter_name = "wasm-del-filter";
    let filter_yaml = format!(
        "name: {filter_name}\nfilterType: {filter_type}\nconfig:\n  type: {filter_type}\n  config:\n    enabled: true\n"
    );
    let filter_file = write_temp_file(&filter_yaml, ".yaml");
    cli.run(&["filter", "create", "-f", filter_file.path().to_str().unwrap()])
        .unwrap()
        .assert_success();
    cli.run(&["filter", "attach", filter_name, "--listener", "wasm-del-ls"])
        .unwrap()
        .assert_success();

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify filter is in Envoy before deletion
    let config_before = harness.get_config_dump().await.expect("config_dump should work");
    let was_present = config_before.contains("envoy.filters.http.wasm")
        || config_before.contains(&filter_type)
        || config_before.contains(filter_name);

    // Delete the filter instance first, then the WASM definition
    let _ = cli.run(&["filter", "delete", filter_name, "--yes"]);
    let _ = cli.run(&["wasm", "delete", wasm_id, "--yes"]);

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify WASM filter is gone from Envoy
    let config_after = harness.get_config_dump().await.expect("config_dump should work");
    let still_present = config_after.contains(&filter_type);
    assert!(
        !still_present,
        "WASM filter type '{}' should be gone from Envoy config_dump after deletion",
        filter_type
    );

    if was_present {
        eprintln!("WASM REMOVAL VERIFIED: '{}' was in Envoy, now gone after delete", filter_type);
    } else {
        eprintln!(
            "NOTE: WASM filter was not visible in Envoy before deletion \
             (may not have been delivered in time)"
        );
    }
}

// ============================================================================
// Negative tests
// ============================================================================

/// `wasm create -f <malformed-json>` should fail with a non-zero exit code.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_create_invalid_json() {
    let harness = quick_harness("dev_wasm_bad_json").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let bad_json = "{ this is not valid json at all!!!";
    let file = write_temp_file(bad_json, ".json");

    let output = cli.run(&["wasm", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "wasm create with malformed JSON should fail, got stdout={}, stderr={}",
        output.stdout, output.stderr
    );
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    assert!(
        combined.contains("invalid") || combined.contains("error") || combined.contains("json"),
        "Error output should mention invalid/error/json, got stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

/// `wasm create -f <missing-required-fields>` should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_create_missing_fields() {
    let harness = quick_harness("dev_wasm_no_fields").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Valid JSON but missing required fields (no name, no wasm_binary_base64)
    let incomplete = serde_json::json!({
        "description": "missing required fields"
    })
    .to_string();
    let file = write_temp_file(&incomplete, ".json");

    let output = cli.run(&["wasm", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "wasm create with missing required fields should fail, got stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// `wasm create -f <nonexistent-file>` should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_create_nonexistent_file() {
    let harness = quick_harness("dev_wasm_no_file").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["wasm", "create", "-f", "/tmp/does-not-exist-e2e-wasm.json"]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "wasm create with nonexistent file should fail, got stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// `wasm update <nonexistent-id>` should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_update_nonexistent() {
    let harness = quick_harness("dev_wasm_upd_noex").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let update_content = updated_wasm_filter_json();
    let file = write_temp_file(&update_content, ".json");

    let output = cli
        .run(&["wasm", "update", "totally-fake-wasm-id-xyz", "-f", file.path().to_str().unwrap()])
        .unwrap();

    let has_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error,
        "Expected error for updating nonexistent WASM filter, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// `wasm delete <nonexistent-id> --yes` should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_delete_nonexistent() {
    let harness = quick_harness("dev_wasm_del_noex").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["wasm", "delete", "totally-fake-wasm-id-xyz", "--yes"]).unwrap();

    let has_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error,
        "Expected error for deleting nonexistent WASM filter, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// `wasm delete <id>` without --yes should fail in non-interactive (piped) mode.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_wasm_delete_no_confirm() {
    let harness = quick_harness("dev_wasm_del_noconfirm").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Create a filter first so the delete would succeed if confirmation wasn't required
    let filter_name = "e2e-wasm-del-noconfirm";
    let json_content = sample_wasm_filter_json(filter_name);
    let file = write_temp_file(&json_content, ".json");

    let create_output = cli.run(&["wasm", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "wasm create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("create output should be valid JSON");
    let filter_id = created["id"].as_str().expect("response should contain id");

    // Try delete WITHOUT --yes — should fail because stdin is not a terminal
    let output = cli.run(&["wasm", "delete", filter_id]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "wasm delete without --yes should fail in non-interactive mode, got stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify filter still exists (delete was aborted)
    let get_output = cli.run(&["wasm", "get", filter_id]).unwrap();
    get_output.assert_success();

    // Cleanup
    let _ = cli.run(&["wasm", "delete", filter_id, "--yes"]);
}
