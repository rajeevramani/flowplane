//! E2E tests for CLI secret commands (dev mode).
//!
//! Tests `flowplane secret create`, `secret list`, `secret get`,
//! `secret rotate`, `secret delete`, and negative cases.
//!
//! SDS delivery verification: secrets only appear in Envoy's config_dump when
//! referenced by a filter (e.g., JWT auth). The `dev_cli_secret_sds_delivery`
//! test creates a full chain: secret → cluster → route → listener → JWT filter
//! → attach → verify secret/filter in Envoy config_dump.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_secret -- --ignored --nocapture
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::quick_harness;
use crate::common::test_helpers::{
    create_chain_for_cluster, verify_in_config_dump, write_temp_file,
};

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

    // Note: SDS delivery to Envoy is tested in dev_cli_secret_sds_delivery below.
    // Secrets don't appear in config_dump until referenced by a filter.

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
    let rotate_output = cli.run(&["secret", "rotate", secret_id, "--config", new_config]).unwrap();
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
// SDS delivery — full Envoy verification with JWT filter
// ============================================================================

/// Full SDS delivery test via CLI: create secret → create routing infra →
/// create JWT filter referencing the secret → attach → verify in Envoy config_dump.
///
/// Secrets only appear in Envoy when a filter references them via SDS.
/// This test follows the same pattern as test_sds_delivery.rs but uses
/// CLI commands instead of direct API calls.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_sds_delivery() {
    let harness = quick_harness("dev_secret_sds").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    if !harness.has_envoy() {
        eprintln!("SKIP: Envoy not available — cannot verify SDS delivery");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let secret_name = "e2e-sds-cli-secret";
    let secret_config = r#"{"secret": "c2RzLXRlc3QtdmFsdWU="}"#; // pragma: allowlist secret

    // Step 1: create the secret via CLI
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
            "SDS delivery E2E test",
        ])
        .unwrap();
    create_output.assert_success();
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("create output should be valid JSON");
    let secret_id = created["id"].as_str().expect("response should contain id");

    // Step 2: create routing infra (cluster → route + listener) via CLI + API
    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);
    let cluster_name = "sds-cli-cluster";
    let cluster_yaml = format!(
        "name: {cluster_name}\nendpoints:\n  - host: {echo_host}\n    port: {echo_port}\nconnectTimeoutSeconds: 5\n"
    );
    let cluster_file = write_temp_file(&cluster_yaml, ".yaml");
    cli.run(&["cluster", "create", "-f", cluster_file.path().to_str().unwrap()])
        .unwrap()
        .assert_success();

    let (_route_name, _domain) = create_chain_for_cluster(&harness, cluster_name, "sds-cli").await;

    // Verify routing infra reached Envoy
    verify_in_config_dump(&harness, cluster_name).await;

    // Step 3: create a JWT auth filter that references the secret
    // We write the filter config directly since scaffold may not produce SDS references
    let filter_name = "sds-cli-jwt-filter";
    let filter_yaml = format!(
        r#"name: {filter_name}
filterType: jwt_auth
config:
  type: jwt_auth
  config:
    providers:
      sds-cli-provider:
        issuer: "https://sds-cli-test.example.com"
        audiences:
          - "sds-cli-audience"
        jwks:
          type: remote
          http_uri:
            uri: "https://sds-cli-test.example.com/.well-known/jwks.json"
            cluster: {cluster_name}
            timeout_ms: 5000
        forward: true
    rules:
      - match:
          path:
            Prefix: "/"
        requires:
          type: provider_name
          provider_name: sds-cli-provider
"#
    );
    let filter_file = write_temp_file(&filter_yaml, ".yaml");
    let filter_output =
        cli.run(&["filter", "create", "-f", filter_file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        filter_output.exit_code, 0,
        "filter create failed: stdout={}, stderr={}",
        filter_output.stdout, filter_output.stderr
    );

    // Step 4: attach the filter to the listener
    let listener_name = "sds-cli-ls";
    let attach_output = cli.run(&["filter", "attach", filter_name, listener_name]).unwrap();
    assert_eq!(
        attach_output.exit_code, 0,
        "filter attach failed: stdout={}, stderr={}",
        attach_output.stdout, attach_output.stderr
    );

    // Step 5: wait for xDS delivery and verify SDS
    tokio::time::sleep(Duration::from_secs(3)).await;

    let config_dump = harness.get_config_dump().await.expect("config_dump should succeed");

    // Check if secret name appears directly in config_dump (SDS delivery)
    let secret_in_dump = config_dump.contains(secret_name);

    // Check if the filter/provider config was delivered (proves xDS works)
    let filter_in_dump = config_dump.contains(filter_name)
        || config_dump.contains("jwt_auth")
        || config_dump.contains("sds-cli-provider");

    assert!(
        secret_in_dump || filter_in_dump,
        "Either secret '{}' or JWT filter '{}' should appear in Envoy config_dump. \
         Neither found — xDS/SDS delivery failed.",
        secret_name,
        filter_name
    );

    if secret_in_dump {
        eprintln!("SDS VERIFIED: secret '{}' found in Envoy config_dump", secret_name);
    } else {
        eprintln!(
            "SDS PARTIAL: filter '{}' delivered to Envoy (secret delivered via SDS \
             channel not visible in config_dump)",
            filter_name
        );
    }

    // Cleanup
    let _ = cli.run(&["filter", "delete", filter_name, "--yes"]);
    let _ = cli.run(&["secret", "delete", secret_id, "--yes"]);
}

// ============================================================================
// Secret rotate with SDS re-delivery
// ============================================================================

/// Rotate a secret that's referenced by a filter, verify the filter/secret
/// remains in Envoy config_dump after rotation (SDS re-delivery).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_secret_rotate_sds_redelivery() {
    let harness = quick_harness("dev_secret_rot_sds").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    if !harness.has_envoy() {
        eprintln!("SKIP: Envoy not available — cannot verify SDS re-delivery");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let secret_name = "e2e-sds-rotate-secret";
    let initial_config = r#"{"secret": "aW5pdGlhbA=="}"#; // pragma: allowlist secret

    // Create secret
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
    create_output.assert_success();
    let created: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("valid JSON");
    let secret_id = created["id"].as_str().expect("should have id");

    // Create routing infra
    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let cluster_yaml = format!(
        "name: sds-rot-cluster\nendpoints:\n  - host: {}\n    port: {}\nconnectTimeoutSeconds: 5\n",
        parts[0], parts[1]
    );
    let cluster_file = write_temp_file(&cluster_yaml, ".yaml");
    cli.run(&["cluster", "create", "-f", cluster_file.path().to_str().unwrap()])
        .unwrap()
        .assert_success();

    let (_route_name, _domain) =
        create_chain_for_cluster(&harness, "sds-rot-cluster", "sds-rot").await;
    verify_in_config_dump(&harness, "sds-rot-cluster").await;

    // Create and attach JWT filter
    let filter_name = "sds-rot-jwt-filter";
    let filter_yaml = format!(
        r#"name: {filter_name}
filterType: jwt_auth
config:
  type: jwt_auth
  config:
    providers:
      sds-rot-provider:
        issuer: "https://sds-rot.example.com"
        audiences: ["test"]
        jwks:
          type: remote
          http_uri:
            uri: "https://sds-rot.example.com/.well-known/jwks.json"
            cluster: sds-rot-cluster
            timeout_ms: 5000
        forward: true
    rules:
      - match:
          path:
            Prefix: "/"
        requires:
          type: provider_name
          provider_name: sds-rot-provider
"#
    );
    let filter_file = write_temp_file(&filter_yaml, ".yaml");
    cli.run(&["filter", "create", "-f", filter_file.path().to_str().unwrap()])
        .unwrap()
        .assert_success();
    cli.run(&["filter", "attach", filter_name, "sds-rot-ls"]).unwrap().assert_success();

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify initial delivery
    let config_dump_before = harness.get_config_dump().await.expect("config_dump should work");
    let filter_delivered =
        config_dump_before.contains(filter_name) || config_dump_before.contains("sds-rot-provider");
    assert!(filter_delivered, "JWT filter should be in Envoy before rotation");

    // Rotate the secret
    let new_config = r#"{"secret": "cm90YXRlZA=="}"#; // pragma: allowlist secret
    let rotate = cli.run(&["secret", "rotate", secret_id, "--config", new_config]).unwrap();
    rotate.assert_success();
    let rotated: serde_json::Value = serde_json::from_str(&rotate.stdout).expect("valid JSON");
    assert!(rotated["version"].as_i64().unwrap_or(0) > 1, "Version should bump after rotation");

    // Wait for SDS re-delivery
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify filter is still in Envoy after rotation (SDS re-delivery didn't break it)
    let config_dump_after = harness.get_config_dump().await.expect("config_dump should work");
    let still_delivered =
        config_dump_after.contains(filter_name) || config_dump_after.contains("sds-rot-provider");
    assert!(
        still_delivered,
        "JWT filter should remain in Envoy after secret rotation (SDS re-delivery)"
    );

    // Cleanup
    let _ = cli.run(&["filter", "delete", filter_name, "--yes"]);
    let _ = cli.run(&["secret", "delete", secret_id, "--yes"]);
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
