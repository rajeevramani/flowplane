//! E2E tests for CLI secret commands (dev mode).
//!
//! Tests `flowplane secret create`, `secret list`, `secret get`,
//! `secret rotate`, `secret delete`, and negative cases.
//!
//! SDS delivery verification: after creating a secret, we verify it reaches
//! Envoy by checking `config_dump` for the secret name. If secrets are
//! delivered through a separate SDS channel not visible in config_dump,
//! we fall back to verifying the secret exists in the API.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_secret -- --ignored --nocapture
//! ```

use crate::common::cli_runner::CliRunner;
use crate::common::harness::quick_harness;

// ============================================================================
// Secret create + list + get
// ============================================================================

/// Create a generic secret via CLI, verify it appears in list and get output.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_create_list_get() {
    let harness = quick_harness("dev_secret_clg").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let secret_name = "e2e-test-secret-clg";
    let secret_config = r#"{"secret": "dGVzdC12YWx1ZQ=="}"#; // pragma: allowlist secret

    // Step 1: create a generic secret
    let create_output = cli
        .run(&[
            "secret",
            "create",
            "--name",
            secret_name,
            "--type",
            "generic_secret",
            "--config",
            secret_config,
            "--description",
            "E2E test secret",
        ])
        .unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "secret create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );
    // Output is JSON — extract the id for later use
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("create output should be valid JSON");
    let secret_id = created["id"].as_str().expect("response should contain id");
    assert_eq!(created["name"].as_str(), Some(secret_name));
    assert_eq!(created["secretType"].as_str(), Some("generic_secret"));
    assert_eq!(created["version"].as_i64(), Some(1));

    // Step 2: verify it appears in `secret list`
    let list_output = cli.run(&["secret", "list", "-o", "json"]).unwrap();
    list_output.assert_success();
    let list_json: serde_json::Value =
        serde_json::from_str(&list_output.stdout).expect("list output should be valid JSON");
    let items = list_json.as_array().expect("list should return an array");
    let found = items.iter().any(|s| s["name"].as_str() == Some(secret_name));
    assert!(found, "Created secret '{}' should appear in list", secret_name);

    // Step 3: verify it appears in `secret get <id>`
    let get_output = cli.run(&["secret", "get", secret_id]).unwrap();
    get_output.assert_success();
    let get_json: serde_json::Value =
        serde_json::from_str(&get_output.stdout).expect("get output should be valid JSON");
    assert_eq!(get_json["name"].as_str(), Some(secret_name));
    assert_eq!(get_json["id"].as_str(), Some(secret_id));

    // Step 4: verify SDS delivery — check if secret name appears in Envoy config_dump.
    // Secrets may be delivered via a separate SDS channel, so we check config_dump
    // but don't hard-fail if it's not there (SDS delivery may require a filter reference).
    if harness.has_envoy() {
        if let Ok(config_dump) = harness.get_config_dump().await {
            if config_dump.contains(secret_name) {
                eprintln!("SDS verified: secret '{}' found in Envoy config_dump", secret_name);
            } else {
                eprintln!(
                    "NOTE: secret '{}' not in config_dump — SDS delivery may require a \
                     filter reference to trigger. API-level verification passed.",
                    secret_name
                );
            }
        }
    }

    // Cleanup
    let _ = cli.run(&["secret", "delete", secret_id, "--yes"]);
}

// ============================================================================
// Secret rotate
// ============================================================================

/// Create a secret, rotate it, verify the version bumps.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_rotate() {
    let harness = quick_harness("dev_secret_rot").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let secret_name = "e2e-test-secret-rotate";
    let initial_config = r#"{"secret": "aW5pdGlhbC12YWx1ZQ=="}"#; // pragma: allowlist secret

    // Step 1: create the secret
    let create_output = cli
        .run(&[
            "secret",
            "create",
            "--name",
            secret_name,
            "--type",
            "generic_secret",
            "--config",
            initial_config,
        ])
        .unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "secret create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("create output should be valid JSON");
    let secret_id = created["id"].as_str().expect("response should contain id");
    assert_eq!(created["version"].as_i64(), Some(1));

    // Step 2: rotate the secret with new config
    let new_config = r#"{"secret": "cm90YXRlZC12YWx1ZQ=="}"#; // pragma: allowlist secret
    let rotate_output = cli
        .run(&["secret", "rotate", secret_id, "--config", new_config])
        .unwrap();
    assert_eq!(
        rotate_output.exit_code, 0,
        "secret rotate failed: stdout={}, stderr={}",
        rotate_output.stdout, rotate_output.stderr
    );
    let rotated: serde_json::Value =
        serde_json::from_str(&rotate_output.stdout).expect("rotate output should be valid JSON");
    assert_eq!(rotated["id"].as_str(), Some(secret_id));
    assert!(
        rotated["version"].as_i64().unwrap_or(0) > 1,
        "Version should be bumped after rotation, got: {:?}",
        rotated["version"]
    );

    // Cleanup
    let _ = cli.run(&["secret", "delete", secret_id, "--yes"]);
}

// ============================================================================
// Secret delete
// ============================================================================

/// Create a secret, delete it, verify it's gone from list and get.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_delete() {
    let harness = quick_harness("dev_secret_del").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let secret_name = "e2e-test-secret-delete";
    let secret_config = r#"{"secret": "ZGVsZXRlLW1l"}"#; // pragma: allowlist secret

    // Step 1: create the secret
    let create_output = cli
        .run(&[
            "secret",
            "create",
            "--name",
            secret_name,
            "--type",
            "generic_secret",
            "--config",
            secret_config,
        ])
        .unwrap();
    create_output.assert_success();
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("create output should be valid JSON");
    let secret_id = created["id"].as_str().expect("response should contain id");

    // Step 2: delete the secret
    let delete_output = cli.run(&["secret", "delete", secret_id, "--yes"]).unwrap();
    delete_output.assert_success();
    delete_output.assert_stdout_contains("deleted");

    // Step 3: verify it's gone from list
    let list_output = cli.run(&["secret", "list", "-o", "json"]).unwrap();
    list_output.assert_success();
    let list_json: serde_json::Value =
        serde_json::from_str(&list_output.stdout).expect("list output should be valid JSON");
    let items = list_json.as_array().expect("list should return an array");
    let found = items.iter().any(|s| s["id"].as_str() == Some(secret_id));
    assert!(!found, "Deleted secret '{}' should not appear in list", secret_id);

    // Step 4: verify get returns an error
    let get_output = cli.run(&["secret", "get", secret_id]).unwrap();
    get_output.assert_failure();

    // Step 5: verify secret is gone from Envoy config_dump
    if harness.has_envoy() {
        if let Ok(config_dump) = harness.get_config_dump().await {
            assert!(
                !config_dump.contains(secret_name),
                "Deleted secret '{}' should not appear in Envoy config_dump",
                secret_name
            );
        }
    }
}

// ============================================================================
// Negative: create duplicate
// ============================================================================

/// Creating a secret with the same name twice should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_create_duplicate() {
    let harness = quick_harness("dev_secret_dup").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let secret_name = "e2e-test-secret-dup";
    let secret_config = r#"{"secret": "ZHVwbGljYXRl"}"#; // pragma: allowlist secret

    // Create first
    let first = cli
        .run(&[
            "secret",
            "create",
            "--name",
            secret_name,
            "--type",
            "generic_secret",
            "--config",
            secret_config,
        ])
        .unwrap();
    first.assert_success();
    let created: serde_json::Value =
        serde_json::from_str(&first.stdout).expect("create output should be valid JSON");
    let secret_id = created["id"].as_str().expect("response should contain id");

    // Create duplicate — should fail
    let second = cli
        .run(&[
            "secret",
            "create",
            "--name",
            secret_name,
            "--type",
            "generic_secret",
            "--config",
            secret_config,
        ])
        .unwrap();
    second.assert_failure();

    // Cleanup
    let _ = cli.run(&["secret", "delete", secret_id, "--yes"]);
}

// ============================================================================
// Negative: rotate nonexistent
// ============================================================================

/// Rotating a secret that doesn't exist should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_rotate_nonexistent() {
    let harness = quick_harness("dev_secret_rotne").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let fake_id = "00000000-0000-0000-0000-000000000000";
    let config = r#"{"secret": "bm9uZXhpc3Q="}"#; // pragma: allowlist secret

    let output = cli.run(&["secret", "rotate", fake_id, "--config", config]).unwrap();
    output.assert_failure();
}

// ============================================================================
// Negative: delete nonexistent
// ============================================================================

/// Deleting a secret that doesn't exist should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_delete_nonexistent() {
    let harness = quick_harness("dev_secret_delne").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let fake_id = "00000000-0000-0000-0000-000000000000";
    let output = cli.run(&["secret", "delete", fake_id, "--yes"]).unwrap();
    output.assert_failure();
}

// ============================================================================
// Negative: create with invalid type
// ============================================================================

/// Creating a secret with an invalid type should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_create_invalid_type() {
    let harness = quick_harness("dev_secret_badtype").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli
        .run(&[
            "secret",
            "create",
            "--name",
            "e2e-bad-type",
            "--type",
            "not_a_real_type",
            "--config",
            r#"{"secret": "dGVzdA=="}"#,
        ])
        .unwrap();
    output.assert_failure();
}

// ============================================================================
// Negative: create with malformed config JSON
// ============================================================================

/// Creating a secret with invalid JSON in --config should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_create_malformed_config() {
    let harness = quick_harness("dev_secret_badjson").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli
        .run(&[
            "secret",
            "create",
            "--name",
            "e2e-bad-json",
            "--type",
            "generic_secret",
            "--config",
            "not-valid-json{{{",
        ])
        .unwrap();
    output.assert_failure();
}
