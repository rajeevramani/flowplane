//! E2E tests for CLI read commands — dev mode
//!
//! Tests basic list/get/show/path read commands and observability reads
//! (route-views stats, stats clusters, stats cluster) that currently have
//! zero E2E coverage. All tests use `dev_harness` (API-only) for basic reads
//! or `envoy_harness` (with per-test Envoy) for observability reads.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_reads -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::{dev_harness, envoy_harness};
use crate::common::test_helpers::{parse_expose_port, verify_in_config_dump};

// ============================================================================
// listener list
// ============================================================================

/// Create a listener via expose, then verify `flowplane listener list` shows it.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_listener_list() {
    let harness = dev_harness("dev_reads_ls_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service to create a listener
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "reads-ls-list"]).unwrap();
    expose.assert_success();

    // List listeners — should contain the one we just created
    let output = cli.run(&["listener", "list"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("reads-ls-list");

    // Cleanup: release port
    let _ = cli.run(&["unexpose", "reads-ls-list"]);
}

// ============================================================================
// route list
// ============================================================================

/// Create a route via expose, then verify `flowplane route list` shows it.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_route_list() {
    let harness = dev_harness("dev_reads_rt_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service to create a route config
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "reads-rt-list"]).unwrap();
    expose.assert_success();

    // List routes — should contain the route config from expose
    let output = cli.run(&["route", "list"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("reads-rt-list");

    // Cleanup: release port
    let _ = cli.run(&["unexpose", "reads-rt-list"]);
}

// ============================================================================
// learn get
// ============================================================================

/// Start a learning session, then `flowplane learn get <name>` should return details.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_learn_get() {
    let harness = dev_harness("dev_reads_learn_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Start a learning session
    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/api/.*",
            "--target-sample-count",
            "10",
            "--name",
            "reads-learn-get",
        ])
        .unwrap();
    start.assert_success();

    // Get session details
    let get = cli.run(&["learn", "get", "reads-learn-get", "-o", "json"]).unwrap();
    get.assert_success();
    get.assert_stdout_contains("reads-learn-get");
    get.assert_stdout_contains("^/api/.*");
}

// ============================================================================
// learn health
// ============================================================================

/// Start a learning session, then `flowplane learn health <name>` returns info.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_learn_health() {
    let harness = dev_harness("dev_reads_learn_hlth").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Start a learning session
    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/.*",
            "--target-sample-count",
            "10",
            "--name",
            "reads-learn-health",
        ])
        .unwrap();
    start.assert_success();

    // Check health of the session
    let health = cli.run(&["learn", "health", "reads-learn-health"]).unwrap();
    health.assert_success();

    // Health output should reference the session name or status
    let stdout_lower = health.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("reads-learn-health")
            || stdout_lower.contains("status")
            || stdout_lower.contains("health")
            || stdout_lower.contains("active"),
        "Expected learn health to show session info, got:\n{}",
        health.stdout
    );
}

// ============================================================================
// learn export
// ============================================================================

/// Start a learn session, stop it, then `flowplane learn export --session <name>`
/// should produce output (may be empty spec if no traffic, but should not error).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_learn_export() {
    let harness = dev_harness("dev_reads_learn_exp").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let session_name = format!("reads-learn-export-{id}");

    // Start a learning session
    let start = cli
        .run(&[
            "learn",
            "start",
            "--route-pattern",
            "^/.*",
            "--target-sample-count",
            "10",
            "--name",
            &session_name,
        ])
        .unwrap();
    start.assert_success();

    // Brief pause then stop
    tokio::time::sleep(Duration::from_secs(1)).await;
    let stop = cli.run(&["learn", "stop", &session_name]).unwrap();
    stop.assert_success();

    // Export schemas from the session
    let export = cli.run(&["learn", "export", "--session", &session_name]).unwrap();
    export.assert_success();

    // Export should produce some output (OpenAPI spec, even if minimal)
    assert!(
        !export.stdout.trim().is_empty(),
        "Expected learn export to produce output, got empty stdout"
    );
}

// ============================================================================
// schema get
// ============================================================================

/// `flowplane schema get <id>` for a nonexistent ID should fail gracefully.
/// We test this as a negative case since schemas require a full learn+activate cycle.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_schema_get_nonexistent() {
    let harness = dev_harness("dev_reads_schema_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Schema ID 999999 should not exist
    let get = cli.run(&["schema", "get", "999999"]).unwrap();
    get.assert_failure();
}

// ============================================================================
// schema compare
// ============================================================================

/// `flowplane schema compare` with nonexistent IDs should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_schema_compare_nonexistent() {
    let harness = dev_harness("dev_reads_schema_cmp").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Compare two nonexistent schemas
    let compare = cli.run(&["schema", "compare", "999998", "--with", "999999"]).unwrap();
    compare.assert_failure();
}

// ============================================================================
// schema export
// ============================================================================

/// `flowplane schema export --all` should succeed even with no schemas.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_schema_export_empty() {
    let harness = dev_harness("dev_reads_schema_exp").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Export all schemas — may return exit 1 with "No schemas found" message
    // when no learning sessions have been run, which is acceptable
    let export = cli.run(&["schema", "export", "--all"]).unwrap();
    let is_acceptable = export.exit_code == 0
        || export.stdout.contains("No schemas found")
        || export.stderr.contains("No schemas found");
    assert!(
        is_acceptable,
        "Expected schema export to succeed or report 'No schemas found', got exit_code={}, stdout={}, stderr={}",
        export.exit_code, export.stdout, export.stderr
    );
}

// ============================================================================
// vhost get
// ============================================================================

/// Expose a service, then get the vhost by route config + vhost name.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_vhost_get() {
    let harness = dev_harness("dev_reads_vhost_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service — creates route config and vhost
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "reads-vhget"]).unwrap();
    expose.assert_success();

    // List vhosts for the route config to discover the actual vhost name
    let vhost_list = cli.run(&["vhost", "list", "--route-config", "reads-vhget-routes"]).unwrap();
    vhost_list.assert_success();

    // Parse the vhost name from the list output — look for a line containing a vhost name
    let vhost_name = vhost_list
        .stdout
        .lines()
        .find(|l| l.contains("reads-vhget"))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("reads-vhget-routes-vhost");

    // Get the vhost using the discovered name
    let get = cli.run(&["vhost", "get", "reads-vhget-routes", vhost_name]).unwrap();
    get.assert_success();

    // Should contain domain or name info
    let stdout_lower = get.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("reads-vhget") || stdout_lower.contains("domain"),
        "Expected vhost get to show vhost details, got:\n{}",
        get.stdout
    );

    // Cleanup: release port
    let _ = cli.run(&["unexpose", "reads-vhget"]);
}

// ============================================================================
// config show
// ============================================================================

/// `flowplane config show` should exit 0 and produce output.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_config_show() {
    let harness = dev_harness("dev_reads_cfg_show").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["config", "show"]).unwrap();
    output.assert_success();

    // Config show should produce some output (at least base_url or token)
    assert!(
        !output.stdout.trim().is_empty(),
        "Expected config show to produce output, got empty stdout"
    );
}

// ============================================================================
// config path
// ============================================================================

/// `flowplane config path` should exit 0 and show a file path.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_config_path() {
    let harness = dev_harness("dev_reads_cfg_path").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["config", "path"]).unwrap();
    output.assert_success();

    // Should contain a path-like string (e.g., ".flowplane" or "config.toml")
    let stdout = &output.stdout;
    assert!(
        stdout.contains("flowplane") || stdout.contains("config"),
        "Expected config path to show a file path, got:\n{}",
        stdout
    );
}

// ============================================================================
// Observability reads (require Envoy + traffic)
// ============================================================================

/// Expose a service, send traffic through Envoy, then verify
/// `flowplane route-views stats` returns non-empty data.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_route_views_stats() {
    let harness = envoy_harness("dev_reads_rv_stats").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service — auto-allocate port
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "reads-rv-stats"]).unwrap();
    expose.assert_success();

    let port = parse_expose_port(&expose);

    // Verify the resources reached Envoy
    verify_in_config_dump(&harness, "reads-rv-stats").await;

    // Send traffic through Envoy on the allocated port
    let _ = harness.wait_for_route_on_port(port, "reads-rv-stats.local", "/test", 200).await;

    // Now check route-views stats
    let output = cli.run(&["route-views", "stats", "-o", "json"]).unwrap();
    output.assert_success();

    // Should have non-empty output with stats data
    assert!(
        !output.stdout.trim().is_empty() && output.stdout.trim() != "{}",
        "Expected route-views stats to return data after traffic, got:\n{}",
        output.stdout
    );

    // Cleanup: release port
    let _ = cli.run(&["unexpose", "reads-rv-stats"]);
}

/// Expose a service, send traffic, then verify `flowplane stats clusters`
/// returns non-empty cluster stats.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_stats_clusters() {
    let harness = envoy_harness("dev_reads_st_cls").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Expose a service — auto-allocate port
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", "reads-st-cls"]).unwrap();
    expose.assert_success();

    let port = parse_expose_port(&expose);

    // Verify in Envoy and send traffic on the allocated port
    verify_in_config_dump(&harness, "reads-st-cls").await;
    let _ = harness.wait_for_route_on_port(port, "reads-st-cls.local", "/test", 200).await;

    // Check cluster stats
    let output = cli.run(&["stats", "clusters", "-o", "json"]).unwrap();
    output.assert_success();

    assert!(
        !output.stdout.trim().is_empty() && output.stdout.trim() != "[]",
        "Expected stats clusters to return data after traffic, got:\n{}",
        output.stdout
    );

    // Cleanup: release port
    let _ = cli.run(&["unexpose", "reads-st-cls"]);
}

/// Expose a service, send traffic, then verify `flowplane stats cluster <name>`
/// returns stats for the specific cluster.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_stats_cluster_specific() {
    let harness = envoy_harness("dev_reads_st_cl1").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let id = &uuid::Uuid::new_v4().as_simple().to_string()[..8];
    let svc_name = format!("rd-st-cl1-{id}");
    let domain = format!("{svc_name}.local");

    // Expose a service — auto-allocate port
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", &svc_name]).unwrap();
    expose.assert_success();

    let port = parse_expose_port(&expose);

    // Verify in Envoy and send traffic on the allocated port
    verify_in_config_dump(&harness, &svc_name).await;
    let _ = harness.wait_for_route_on_port(port, &domain, "/test", 200).await;

    // Check specific cluster stats — the cluster name from expose equals the service name
    let output = cli.run(&["stats", "cluster", &svc_name, "-o", "json"]).unwrap();
    output.assert_success();

    // Should contain stats for the specific cluster
    assert!(
        !output.stdout.trim().is_empty(),
        "Expected stats cluster to return data, got empty stdout"
    );

    // Cleanup: release port
    let _ = cli.run(&["unexpose", &svc_name]);
}

// ============================================================================
// Negative tests
// ============================================================================

/// `flowplane listener list` with bogus --protocol filter should still exit 0
/// but return empty or no matching results.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_listener_list_empty_filter() {
    let harness = dev_harness("dev_reads_ls_empty").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Filter by a protocol that doesn't exist — should succeed with empty results
    let output = cli.run(&["listener", "list", "--protocol", "nonexistent_proto_xyz"]).unwrap();
    output.assert_success();

    // Should not contain any listener entries (no match for this protocol)
    assert!(
        !output.stdout.contains("reads-ls"),
        "Expected no listeners for nonexistent protocol, got:\n{}",
        output.stdout
    );
}

/// `flowplane vhost get` with nonexistent route config should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_vhost_get_nonexistent() {
    let harness = dev_harness("dev_reads_vhost_noex").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let get = cli
        .run(&["vhost", "get", "nonexistent-route-config-xyz", "nonexistent-vhost-xyz"])
        .unwrap();
    get.assert_failure();
}

/// `flowplane stats cluster <nonexistent>` should fail or return error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_reads_stats_cluster_nonexistent() {
    let harness = dev_harness("dev_reads_st_noex").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["stats", "cluster", "nonexistent-cluster-xyz-999"]).unwrap();

    let has_error_indication = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error_indication,
        "Expected error for nonexistent cluster stats, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}
