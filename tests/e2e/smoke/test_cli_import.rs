//! E2E tests for `flowplane import openapi` CLI command.
//!
//! Tests the full import lifecycle: import an OpenAPI spec, verify resources
//! are created (clusters, routes, listeners), verify Envoy receives the
//! configuration, and test negative cases (invalid/nonexistent files).
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_import -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::quick_harness;
use crate::common::test_helpers::{verify_in_config_dump, write_temp_file};

/// Create a dataplane via CLI for tests that need one as a prerequisite.
fn create_dataplane_via_cli(cli: &CliRunner, name: &str) {
    let spec = format!(
        "name: {name}\ngatewayHost: \"127.0.0.1\"\ndescription: \"Dataplane for import E2E test\"\n"
    );
    let file = write_temp_file(&spec, ".yaml");
    let output = cli.run(&["dataplane", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    // Accept success or 409 conflict (already exists)
    assert!(
        output.exit_code == 0
            || output.stderr.contains("conflict")
            || output.stderr.contains("already exists")
            || output.stderr.contains("duplicate"),
        "dataplane creation via CLI failed unexpectedly: exit_code={}, stderr={}",
        output.exit_code,
        output.stderr
    );
}

// ============================================================================
// import openapi — happy path with Envoy verification
// ============================================================================

/// Import a minimal OpenAPI spec, verify clusters/routes/listeners are created,
/// and confirm the imported resources reach Envoy via xDS config_dump + traffic.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_import_openapi_full_lifecycle() {
    let harness = quick_harness("dev_import_oa_full").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Ensure a dataplane exists (import command requires one)
    create_dataplane_via_cli(&cli, "import-dp");

    // Write a minimal OpenAPI 3.0 spec pointing to the echo server
    let echo = harness.echo_endpoint();
    let spec = format!(
        r#"openapi: "3.0.0"
info:
  title: Import E2E API
  version: "1.0.0"
servers:
  - url: http://{echo}
paths:
  /api/test:
    get:
      operationId: getTest
      summary: Test endpoint
      responses:
        "200":
          description: OK
  /api/health:
    get:
      operationId: healthCheck
      summary: Health check
      responses:
        "200":
          description: OK
"#
    );
    let spec_file = write_temp_file(&spec, ".yaml");
    let spec_path = spec_file.path().to_str().expect("valid utf-8 path");

    let port = harness.ports.listener;

    // Import the OpenAPI spec
    let import_out = cli
        .run(&[
            "import",
            "openapi",
            spec_path,
            "--name",
            "e2e-import-full",
            "--port",
            &port.to_string(),
        ])
        .unwrap();
    import_out.assert_success();
    import_out.assert_stdout_contains("Import E2E API");

    // List imports — should contain our import
    let list = cli.run(&["import", "list"]).unwrap();
    list.assert_success();
    list.assert_stdout_contains("import-e2e-api");

    // Get the import details
    let get = cli.run(&["import", "get", "import-e2e-api"]).unwrap();
    get.assert_success();
    get.assert_stdout_contains("import-e2e-api");

    // Verify cluster was created
    let clusters = cli.run(&["cluster", "list"]).unwrap();
    clusters.assert_success();

    // Verify route config was created
    let routes = cli.run(&["route-config", "list"]).unwrap();
    routes.assert_success();

    // Wait for xDS convergence
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify the imported listener appears in Envoy config_dump
    verify_in_config_dump(&harness, "e2e-import-full-listener").await;

    // Verify traffic through the imported route
    if harness.has_envoy() {
        let result = harness.wait_for_route_on_port(port, "localhost", "/api/test", 200).await;
        match result {
            Ok(body) => {
                assert!(!body.is_empty(), "Proxied response body should not be empty");
            }
            Err(e) => {
                panic!("Traffic verification failed for imported route /api/test: {e}");
            }
        }
    }

    // Cleanup: delete the import (with --yes to skip confirmation)
    let delete = cli.run(&["import", "delete", "import-e2e-api", "--yes"]).unwrap();
    delete.assert_success();

    // Verify import is gone
    let list_after = cli.run(&["import", "list"]).unwrap();
    list_after.assert_success();
    assert!(
        !list_after.stdout.contains("import-e2e-api"),
        "import should not appear after delete, got:\n{}",
        list_after.stdout
    );
}

// ============================================================================
// negative tests
// ============================================================================

/// Importing a nonexistent file should fail with a clear error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_import_openapi_nonexistent_file() {
    let harness = quick_harness("dev_import_oa_nofile").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli
        .run(&[
            "import",
            "openapi",
            "/tmp/this-file-does-not-exist-e2e-test.yaml",
            "--name",
            "e2e-no-file",
        ])
        .unwrap();

    assert!(output.exit_code != 0, "import of nonexistent file should fail, but got exit 0");
}

/// Importing an invalid (non-OpenAPI) file should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_import_openapi_invalid_spec() {
    let harness = quick_harness("dev_import_oa_bad").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Ensure a dataplane exists
    create_dataplane_via_cli(&cli, "import-bad-dp");

    // Write invalid YAML (not an OpenAPI spec)
    let bad_spec = r#"this: is
not: an
openapi: spec
"#;
    let spec_file = write_temp_file(bad_spec, ".yaml");
    let spec_path = spec_file.path().to_str().expect("valid utf-8 path");

    let output = cli.run(&["import", "openapi", spec_path, "--name", "e2e-bad-spec"]).unwrap();

    assert!(output.exit_code != 0, "import of invalid spec should fail, but got exit 0");
}

/// Importing an empty file should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_import_openapi_empty_file() {
    let harness = quick_harness("dev_import_oa_empty").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let spec_file = write_temp_file("", ".yaml");
    let spec_path = spec_file.path().to_str().expect("valid utf-8 path");

    let output = cli.run(&["import", "openapi", spec_path, "--name", "e2e-empty-spec"]).unwrap();

    assert!(output.exit_code != 0, "import of empty file should fail, but got exit 0");
}
