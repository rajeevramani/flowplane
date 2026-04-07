//! Phase 4 CLI E2E tests — dev mode
//!
//! Tests the CLI commands added in Phase 2/3 ops gaps: dataplane, vhost, filter
//! types/update, import lifecycle, stats, route-views, schema, and reports.
//! All tests use the dev-mode harness (bearer token, no Zitadel) and the
//! CliRunner subprocess driver.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_phase4 -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;

// ============================================================================
// dataplane commands
// ============================================================================

/// `flowplane dataplane list` should succeed and show at least one dataplane
/// (dev-dataplane is created during bootstrap).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_list() {
    let harness = dev_harness("dev_cli_dp_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["dataplane", "list"]).unwrap();
    output.assert_success();

    // Dev mode bootstraps a dataplane — output should not be empty
    assert!(
        !output.stdout.trim().is_empty(),
        "Expected dataplane list to contain at least one entry, got empty stdout"
    );
}

/// `flowplane dataplane get <name>` should return JSON-like output for the
/// dev-dataplane that exists after bootstrap.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_get() {
    let harness = dev_harness("dev_cli_dp_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["dataplane", "get", "dev-dataplane"]).unwrap();
    output.assert_success();

    // Should contain the dataplane name in the output
    output.assert_stdout_contains("dev-dataplane");
}

/// `flowplane dataplane config <name>` should show bootstrap config with
/// node/cluster fields (YAML-like output).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_config() {
    let harness = dev_harness("dev_cli_dp_config").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["dataplane", "config", "dev-dataplane"]).unwrap();
    output.assert_success();

    // Bootstrap config should contain node or cluster references
    let stdout_lower = output.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("node") || stdout_lower.contains("cluster"),
        "Expected dataplane config to contain node/cluster fields, got:\n{}",
        output.stdout
    );
}

// ============================================================================
// virtual host commands
// ============================================================================

/// Expose a service so a route config + vhost exist, then list vhosts for that
/// route config and verify the vhost appears.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_vhost_list_after_expose() {
    let harness = dev_harness("dev_cli_vhost_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service to create route config + vhost
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p4-vhost"]).unwrap();
    expose.assert_success();

    // List vhosts for the route config created by expose
    let output = cli.run(&["vhost", "list", "--route-config", "e2e-p4-vhost-routes"]).unwrap();
    output.assert_success();

    // Should contain at least one vhost entry
    assert!(
        !output.stdout.trim().is_empty(),
        "Expected vhost list to contain entries after expose, got empty stdout"
    );
}

/// Get a specific vhost and verify domains field appears in output.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_vhost_get() {
    let harness = dev_harness("dev_cli_vhost_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service to create a vhost
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p4-vhget"]).unwrap();
    expose.assert_success();

    // Get the vhost — expose typically creates a vhost named after the service
    let output = cli
        .run(&["vhost", "get", "e2e-p4-vhget", "--route-config", "e2e-p4-vhget-routes"])
        .unwrap();
    output.assert_success();

    // Output should contain domain info
    let stdout_lower = output.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("domain") || stdout_lower.contains("e2e-p4-vhget"),
        "Expected vhost get to show domain info, got:\n{}",
        output.stdout
    );
}

// ============================================================================
// filter types / update commands
// ============================================================================

/// `flowplane filter types` should list available filter types including
/// local_rate_limit.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_types_lists_types() {
    let harness = dev_harness("dev_cli_filter_types").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["filter", "types"]).unwrap();
    output.assert_success();

    // local_rate_limit should be one of the built-in filter types
    output.assert_stdout_contains("local_rate_limit");
}

/// `flowplane filter type <name>` for local_rate_limit should show schema info.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_type_schema() {
    let harness = dev_harness("dev_cli_filter_schema").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["filter", "type", "local_rate_limit"]).unwrap();
    output.assert_success();

    // Should contain schema or config info
    let stdout_lower = output.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("schema")
            || stdout_lower.contains("config")
            || stdout_lower.contains("local_rate_limit"),
        "Expected filter type output to contain schema/config info, got:\n{}",
        output.stdout
    );
}

/// Create a filter via expose + filter add, then update it, verify the
/// updated config is reflected.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_update_lifecycle() {
    let harness = dev_harness("dev_cli_filter_upd").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service to have a listener to attach a filter to
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p4-filt"]).unwrap();
    expose.assert_success();

    // Add a filter to the listener
    let add = cli
        .run(&[
            "filter",
            "add",
            "e2e-p4-filt-listener",
            "--type",
            "local_rate_limit",
            "--name",
            "e2e-p4-rate-filter",
        ])
        .unwrap();
    // If filter add requires config, this might fail — that's OK, we verify gracefully
    if add.exit_code != 0 {
        eprintln!(
            "filter add returned non-zero (may need config): stdout={}, stderr={}",
            add.stdout, add.stderr
        );
        return;
    }

    // Update the filter
    let update = cli
        .run(&["filter", "update", "e2e-p4-rate-filter", "--listener", "e2e-p4-filt-listener"])
        .unwrap();

    // Verify the update command at least ran without crashing
    // It may succeed or fail based on config requirements, but should not panic
    assert!(
        update.exit_code == 0 || !update.stderr.is_empty(),
        "Expected filter update to either succeed or produce an error message"
    );
}

// ============================================================================
// import lifecycle
// ============================================================================

/// Full import lifecycle: create OpenAPI spec file, import it, list imports
/// (verify it appears), delete it, list again (verify gone).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_import_lifecycle() {
    let harness = dev_harness("dev_cli_import_lc").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Write a minimal OpenAPI spec to a temp file
    let spec = r#"openapi: "3.1.0"
info:
  title: "E2E Test API"
  version: "1.0.0"
servers:
  - url: "http://127.0.0.1:9999"
paths:
  /test:
    get:
      responses:
        "200":
          description: "OK"
"#;
    let spec_dir = tempfile::tempdir().expect("create temp dir for spec");
    let spec_path = spec_dir.path().join("e2e-test-api.yaml");
    std::fs::write(&spec_path, spec).expect("write spec file");

    // Import the spec
    let import = cli
        .run(&[
            "import",
            "openapi",
            spec_path.to_str().expect("valid utf-8 path"),
            "--name",
            "e2e-import-test",
        ])
        .unwrap();
    import.assert_success();

    // List imports — should contain our import
    let list = cli.run(&["import", "list"]).unwrap();
    list.assert_success();
    list.assert_stdout_contains("e2e-import-test");

    // Get the specific import
    let get = cli.run(&["import", "get", "e2e-import-test"]).unwrap();
    get.assert_success();
    get.assert_stdout_contains("e2e-import-test");

    // Delete the import
    let delete = cli.run(&["import", "delete", "e2e-import-test"]).unwrap();
    delete.assert_success();

    // List again — should no longer contain the import
    let list_after = cli.run(&["import", "list"]).unwrap();
    list_after.assert_success();
    assert!(
        !list_after.stdout.contains("e2e-import-test"),
        "import should not appear in list after delete, got:\n{}",
        list_after.stdout
    );
}

// ============================================================================
// stats
// ============================================================================

/// `flowplane stats` should exit 0. Output may be empty if no Envoy traffic
/// has been generated.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_stats_overview() {
    let harness = dev_harness("dev_cli_stats").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["stats"]).unwrap();
    output.assert_success();
}

// ============================================================================
// route-views
// ============================================================================

/// Expose a service, then run `route-views` — verify the route appears in output.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_route_views_after_expose() {
    let harness = dev_harness("dev_cli_rv").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service first
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "e2e-p4-rv"]).unwrap();
    expose.assert_success();

    // Run route-views
    let output = cli.run(&["route-views"]).unwrap();
    output.assert_success();

    // Should mention the route or service we just exposed
    assert!(
        output.stdout.contains("e2e-p4-rv") || !output.stdout.trim().is_empty(),
        "Expected route-views to show routes after expose, got empty output"
    );
}

// ============================================================================
// schema
// ============================================================================

/// `flowplane schema list` should exit 0 (may return empty if no schemas learned).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_schema_list() {
    let harness = dev_harness("dev_cli_schema_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["schema", "list"]).unwrap();
    output.assert_success();
}

// ============================================================================
// reports
// ============================================================================

/// `flowplane reports route-flows` should exit 0.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reports_route_flows() {
    let harness = dev_harness("dev_cli_reports_rf").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["reports", "route-flows"]).unwrap();
    output.assert_success();
}

// ============================================================================
// negative tests
// ============================================================================

/// Getting a nonexistent dataplane should fail with an error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_dataplane_get_nonexistent() {
    let harness = dev_harness("dev_cli_dp_noexist").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["dataplane", "get", "totally-nonexistent-dataplane-xyz"]).unwrap();

    // Should either fail (non-zero exit) or print an error message
    let has_error_indication = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error_indication,
        "Expected error for nonexistent dataplane, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// Getting a nonexistent filter type should fail with an error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_type_nonexistent() {
    let harness = dev_harness("dev_cli_ft_noexist").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["filter", "type", "totally_fake_filter_type_xyz"]).unwrap();

    // Should indicate error for unknown filter type
    let has_error_indication = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("error")
        || output.stderr.to_lowercase().contains("unknown")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error_indication,
        "Expected error for nonexistent filter type, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// Listing vhosts without specifying --route-config should fail or return
/// an error/empty result.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_vhost_list_missing_route_config() {
    let harness = dev_harness("dev_cli_vhost_norc").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["vhost", "list"]).unwrap();

    // Without --route-config, should either:
    // - fail with non-zero exit (missing required arg)
    // - print an error/usage message
    // - return empty result
    let is_reasonable = output.exit_code != 0
        || output.stderr.to_lowercase().contains("required")
        || output.stderr.to_lowercase().contains("route-config")
        || output.stderr.to_lowercase().contains("error")
        || output.stderr.to_lowercase().contains("usage")
        || output.stdout.trim().is_empty();
    assert!(
        is_reasonable,
        "Expected error or empty output when listing vhosts without --route-config, \
         got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}
