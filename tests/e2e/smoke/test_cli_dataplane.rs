//! E2E tests for `flowplane dataplane` CLI commands (create, update, delete).
//!
//! Tests the full dataplane lifecycle via CLI: create from file, verify via get,
//! update from file, verify changes, delete, verify removal. Also covers
//! negative cases: missing file, invalid YAML, nonexistent dataplane, etc.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_dataplane -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;
use crate::common::test_helpers::write_temp_file;

// ============================================================================
// dataplane create
// ============================================================================

/// Create a dataplane from a YAML file, verify it appears in `dataplane get`.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_create_yaml() {
    let harness = dev_harness("dev_cli_dp_create").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let spec = r#"name: e2e-dp-create
gatewayHost: "127.0.0.1"
description: "Created via CLI E2E test"
"#;
    let file = write_temp_file(spec, ".yaml");

    let create = cli.run(&["dataplane", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    create.assert_success();
    create.assert_stdout_contains("e2e-dp-create");

    // Verify via get
    let get = cli.run(&["dataplane", "get", "e2e-dp-create"]).unwrap();
    get.assert_success();
    get.assert_stdout_contains("e2e-dp-create");
    get.assert_stdout_contains("127.0.0.1");
}

/// Create a dataplane from a JSON file.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_create_json() {
    let harness = dev_harness("dev_cli_dp_cjson").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let spec =
        r#"{"name": "e2e-dp-json", "gatewayHost": "10.0.0.1", "description": "JSON create test"}"#;
    let file = write_temp_file(spec, ".json");

    let create = cli.run(&["dataplane", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    create.assert_success();
    create.assert_stdout_contains("e2e-dp-json");

    // Verify it shows up in list
    let list = cli.run(&["dataplane", "list"]).unwrap();
    list.assert_success();
    list.assert_stdout_contains("e2e-dp-json");
}

// ============================================================================
// dataplane update
// ============================================================================

/// Create a dataplane, then update its description and gateway_host via CLI.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_update() {
    let harness = dev_harness("dev_cli_dp_update").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Create first
    let create_spec = r#"name: e2e-dp-upd
gatewayHost: "10.0.0.1"
description: "Before update"
"#;
    let create_file = write_temp_file(create_spec, ".yaml");
    let create =
        cli.run(&["dataplane", "create", "-f", create_file.path().to_str().unwrap()]).unwrap();
    create.assert_success();

    // Update with new values
    let update_spec = r#"gatewayHost: "192.168.1.1"
description: "After update"
"#;
    let update_file = write_temp_file(update_spec, ".yaml");
    let update = cli
        .run(&["dataplane", "update", "e2e-dp-upd", "-f", update_file.path().to_str().unwrap()])
        .unwrap();
    update.assert_success();

    // Verify update took effect
    let get = cli.run(&["dataplane", "get", "e2e-dp-upd"]).unwrap();
    get.assert_success();
    get.assert_stdout_contains("192.168.1.1");
    get.assert_stdout_contains("After update");
}

// ============================================================================
// dataplane delete
// ============================================================================

/// Create a dataplane, delete it with --yes, verify it is gone.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_delete() {
    let harness = dev_harness("dev_cli_dp_delete").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Create a dataplane to delete
    let spec = r#"name: e2e-dp-del
gatewayHost: "127.0.0.1"
description: "Will be deleted"
"#;
    let file = write_temp_file(spec, ".yaml");
    let create = cli.run(&["dataplane", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    create.assert_success();

    // Confirm it exists
    let get = cli.run(&["dataplane", "get", "e2e-dp-del"]).unwrap();
    get.assert_success();

    // Delete it
    let delete = cli.run(&["dataplane", "delete", "e2e-dp-del", "--yes"]).unwrap();
    delete.assert_success();

    // Verify it is gone — get should fail or return not found
    let get_after = cli.run(&["dataplane", "get", "e2e-dp-del"]).unwrap();
    let gone = get_after.exit_code != 0
        || get_after.stderr.to_lowercase().contains("not found")
        || get_after.stdout.to_lowercase().contains("not found")
        || get_after.stderr.to_lowercase().contains("error");
    assert!(
        gone,
        "Expected dataplane to be gone after delete, got exit_code={}, stdout={}, stderr={}",
        get_after.exit_code, get_after.stdout, get_after.stderr
    );
}

// ============================================================================
// negative tests — create
// ============================================================================

/// Create with a nonexistent file should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_create_nonexistent_file() {
    let harness = dev_harness("dev_cli_dp_nofile").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output =
        cli.run(&["dataplane", "create", "-f", "/tmp/this-file-does-not-exist-e2e.yaml"]).unwrap();
    assert!(
        output.exit_code != 0,
        "create with nonexistent file should fail, got exit 0: stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

/// Create with invalid YAML should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_create_invalid_yaml() {
    let harness = dev_harness("dev_cli_dp_badyaml").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let bad_yaml = ":\n  - :\n  invalid: [unterminated";
    let file = write_temp_file(bad_yaml, ".yaml");

    let output = cli.run(&["dataplane", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert!(
        output.exit_code != 0,
        "create with invalid YAML should fail, got exit 0: stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

/// Create with unsupported file extension should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_create_bad_extension() {
    let harness = dev_harness("dev_cli_dp_badext").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("name: test\n", ".txt");

    let output = cli.run(&["dataplane", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert!(
        output.exit_code != 0,
        "create with .txt extension should fail, got exit 0: stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

/// Create with empty name should fail with a validation error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_create_empty_name() {
    let harness = dev_harness("dev_cli_dp_noname").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let spec = r#"name: ""
gatewayHost: "127.0.0.1"
"#;
    let file = write_temp_file(spec, ".yaml");

    let output = cli.run(&["dataplane", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert!(
        output.exit_code != 0,
        "create with empty name should fail, got exit 0: stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

/// Creating a duplicate dataplane should fail (name is unique per team).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_create_duplicate() {
    let harness = dev_harness("dev_cli_dp_dup").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let spec = r#"name: e2e-dp-dup
gatewayHost: "127.0.0.1"
"#;
    let file = write_temp_file(spec, ".yaml");

    // First create should succeed
    let first = cli.run(&["dataplane", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    first.assert_success();

    // Second create with same name should fail
    let second = cli.run(&["dataplane", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert!(
        second.exit_code != 0,
        "duplicate create should fail, got exit 0: stdout={}, stderr={}",
        second.stdout,
        second.stderr
    );
}

// ============================================================================
// negative tests — update
// ============================================================================

/// Update a nonexistent dataplane should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_update_nonexistent() {
    let harness = dev_harness("dev_cli_dp_upd_ne").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let spec = r#"description: "update a ghost"
"#;
    let file = write_temp_file(spec, ".yaml");

    let output = cli
        .run(&["dataplane", "update", "nonexistent-dp-xyz", "-f", file.path().to_str().unwrap()])
        .unwrap();
    assert!(
        output.exit_code != 0,
        "update of nonexistent dataplane should fail, got exit 0: stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

// ============================================================================
// negative tests — delete
// ============================================================================

/// Delete a nonexistent dataplane should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_delete_nonexistent() {
    let harness = dev_harness("dev_cli_dp_del_ne").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["dataplane", "delete", "nonexistent-dp-xyz", "--yes"]).unwrap();
    assert!(
        output.exit_code != 0,
        "delete of nonexistent dataplane should fail, got exit 0: stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}
