//! E2E tests for CLI filter operations: update, detach, delete, and negative cases.
//!
//! These tests exercise the full filter lifecycle beyond create+attach,
//! verifying that updates propagate to Envoy, detach removes filter behavior
//! while preserving traffic flow, and delete cleans up resources.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_filter_ops -- --ignored --nocapture
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::quick_harness;
use crate::common::test_helpers::{verify_in_config_dump, write_temp_file};

// ============================================================================
// Helpers (local to this module)
// ============================================================================

/// Create cluster + route + listener chain via CLI, returning (listener_name, route_name, domain).
fn create_filter_chain(
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
    assert_eq!(output.exit_code, 0, "cluster create failed: {}", output.stderr);

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
    assert_eq!(output.exit_code, 0, "route create failed: {}", output.stderr);

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
    assert_eq!(output.exit_code, 0, "listener create failed: {}", output.stderr);

    (listener_name, route_name, domain)
}

/// Create a header_mutation filter via CLI.
fn create_header_mutation_filter(cli: &CliRunner, filter_name: &str, header_value: &str) {
    let filter_json = serde_json::json!({
        "name": filter_name,
        "filterType": "header_mutation",
        "config": {
            "type": "header_mutation",
            "config": {
                "response_headers_to_add": [
                    {"key": "X-Filter-Ops-Test", "value": header_value}
                ]
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&filter_json).unwrap(), ".json");
    let output = cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "filter create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// Attach filter to listener via CLI.
fn attach_filter(cli: &CliRunner, filter_name: &str, listener_name: &str) {
    let output = cli.run(&["filter", "attach", filter_name, "--listener", listener_name]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "filter attach failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

// ============================================================================
// filter update -f <file>
// ============================================================================

/// E2E: create header_mutation filter → attach → verify header → update config
/// with new header value → verify updated header appears in Envoy response.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_update_from_file() {
    let harness = quick_harness("dev_flt_update").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-upd";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Setup: cluster + route + listener + filter + attach
    let (listener_name, _route_name, domain) =
        create_filter_chain(&cli, prefix, echo_host, echo_port, listener_port);
    create_header_mutation_filter(&cli, &filter_name, "original-value");
    attach_filter(&cli, &filter_name, &listener_name);

    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;

    // Verify original header value
    if harness.has_envoy() {
        let envoy = harness.envoy().expect("Envoy should be available");
        let resp = envoy
            .proxy_get_with_headers(listener_port, &domain, "/test-update")
            .await
            .expect("proxy request should succeed");
        assert_eq!(resp.status, 200, "Expected 200, got {}", resp.status);
        assert_eq!(
            resp.headers.get("x-filter-ops-test").map(|v| v.as_str()),
            Some("original-value"),
            "Expected original header value. Headers: {:?}",
            resp.headers
        );
    }

    // Update filter with new header value
    let updated_json = serde_json::json!({
        "name": filter_name,
        "filterType": "header_mutation",
        "config": {
            "type": "header_mutation",
            "config": {
                "response_headers_to_add": [
                    {"key": "X-Filter-Ops-Test", "value": "updated-value"}
                ]
            }
        }
    });
    let update_file =
        write_temp_file(&serde_json::to_string_pretty(&updated_json).unwrap(), ".json");
    let update_output = cli
        .run(&["filter", "update", &filter_name, "-f", update_file.path().to_str().unwrap()])
        .unwrap();
    assert_eq!(
        update_output.exit_code, 0,
        "filter update failed: stdout={}, stderr={}",
        update_output.stdout, update_output.stderr
    );

    // Wait for updated config to propagate
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify updated header value in Envoy response
    if harness.has_envoy() {
        let envoy = harness.envoy().expect("Envoy should be available");
        let resp = envoy
            .proxy_get_with_headers(listener_port, &domain, "/test-update-after")
            .await
            .expect("proxy request after update should succeed");
        assert_eq!(resp.status, 200, "Expected 200 after update, got {}", resp.status);
        assert_eq!(
            resp.headers.get("x-filter-ops-test").map(|v| v.as_str()),
            Some("updated-value"),
            "Expected updated header value after filter update. Headers: {:?}",
            resp.headers
        );
    }
}

// ============================================================================
// filter detach
// ============================================================================

/// E2E: create filter → attach → verify header present → detach → verify header
/// gone but traffic still flows.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_detach() {
    let harness = quick_harness("dev_flt_detach").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-det";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Setup: cluster + route + listener + filter + attach
    let (listener_name, _route_name, domain) =
        create_filter_chain(&cli, prefix, echo_host, echo_port, listener_port);
    create_header_mutation_filter(&cli, &filter_name, "detach-test-header");
    attach_filter(&cli, &filter_name, &listener_name);

    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_in_config_dump(&harness, &listener_name).await;

    // Verify header is present before detach
    if harness.has_envoy() {
        let envoy = harness.envoy().expect("Envoy should be available");
        let resp = envoy
            .proxy_get_with_headers(listener_port, &domain, "/test-before-detach")
            .await
            .expect("proxy request should succeed");
        assert_eq!(resp.status, 200, "Expected 200 before detach, got {}", resp.status);
        assert_eq!(
            resp.headers.get("x-filter-ops-test").map(|v| v.as_str()),
            Some("detach-test-header"),
            "Expected filter header before detach. Headers: {:?}",
            resp.headers
        );
    }

    // Detach filter from listener
    let detach_output =
        cli.run(&["filter", "detach", &filter_name, "--listener", &listener_name]).unwrap();
    assert_eq!(
        detach_output.exit_code, 0,
        "filter detach failed: stdout={}, stderr={}",
        detach_output.stdout, detach_output.stderr
    );

    // Wait for detach to propagate
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify traffic still flows but header is gone
    if harness.has_envoy() {
        let envoy = harness.envoy().expect("Envoy should be available");
        let resp = envoy
            .proxy_get_with_headers(listener_port, &domain, "/test-after-detach")
            .await
            .expect("proxy request after detach should succeed");
        assert_eq!(resp.status, 200, "Expected 200 after detach — traffic should still flow");
        assert!(
            !resp.headers.contains_key("x-filter-ops-test"),
            "X-Filter-Ops-Test header should be GONE after detach. Headers: {:?}",
            resp.headers
        );
    }
}

// ============================================================================
// filter delete
// ============================================================================

/// E2E: create filter → verify exists → delete → verify gone.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_delete() {
    let harness = quick_harness("dev_flt_del").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-del";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Setup: need a cluster+route+listener to create a filter in this harness context
    create_filter_chain(&cli, prefix, echo_host, echo_port, listener_port);
    create_header_mutation_filter(&cli, &filter_name, "will-be-deleted");

    // Verify filter exists
    let get_output = cli.run(&["filter", "get", &filter_name]).unwrap();
    get_output.assert_success();
    assert!(get_output.stdout.contains(&filter_name), "Filter should exist before delete");

    // Delete filter
    let delete_output = cli.run(&["filter", "delete", &filter_name, "--yes"]).unwrap();
    assert_eq!(
        delete_output.exit_code, 0,
        "filter delete failed: stdout={}, stderr={}",
        delete_output.stdout, delete_output.stderr
    );

    // Verify filter is gone
    let get_after = cli.run(&["filter", "get", &filter_name]).unwrap();
    assert_ne!(
        get_after.exit_code, 0,
        "filter get should fail after delete, but got: stdout={}, stderr={}",
        get_after.stdout, get_after.stderr
    );
}

/// E2E: delete filter while attached → should detach and delete, or refuse
/// depending on implementation. Either way, verify consistent state.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_delete_while_attached() {
    let harness = quick_harness("dev_flt_del_att").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-delatt";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Setup: create full chain + filter + attach
    let (listener_name, _route_name, _domain) =
        create_filter_chain(&cli, prefix, echo_host, echo_port, listener_port);
    create_header_mutation_filter(&cli, &filter_name, "attached-delete-test");
    attach_filter(&cli, &filter_name, &listener_name);

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Try to delete while attached
    let delete_output = cli.run(&["filter", "delete", &filter_name, "--yes"]).unwrap();

    if delete_output.exit_code == 0 {
        // Server auto-detached and deleted — verify filter is gone
        let get_after = cli.run(&["filter", "get", &filter_name]).unwrap();
        assert_ne!(get_after.exit_code, 0, "Filter should be gone after successful delete");
    } else {
        // Server refused — filter should still exist and be attached
        let get_after = cli.run(&["filter", "get", &filter_name]).unwrap();
        get_after.assert_success();
        assert!(
            delete_output.stderr.contains("attach")
                || delete_output.stdout.contains("attach")
                || delete_output.stderr.contains("in use")
                || delete_output.stdout.contains("in use"),
            "Delete rejection should mention attachment. stdout={}, stderr={}",
            delete_output.stdout,
            delete_output.stderr
        );
    }
}

// ============================================================================
// Negative tests
// ============================================================================

/// E2E: delete a filter that does not exist → should fail with non-zero exit.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_delete_nonexistent() {
    let harness = quick_harness("dev_flt_del_ne").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output =
        cli.run(&["filter", "delete", "no-such-filter-ever-existed", "--yes"]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Deleting nonexistent filter should fail, but got exit 0. stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// E2E: detach a filter from a listener it is not attached to → should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_detach_wrong_listener() {
    let harness = quick_harness("dev_flt_det_wl").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-detwl";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    // Create chain and filter but do NOT attach
    create_filter_chain(&cli, prefix, echo_host, echo_port, listener_port);
    create_header_mutation_filter(&cli, &filter_name, "not-attached");

    // Try detaching from the listener we never attached to
    let listener_name = format!("{}-ls", prefix);
    let output =
        cli.run(&["filter", "detach", &filter_name, "--listener", &listener_name]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Detaching unattached filter should fail, but got exit 0. stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// E2E: detach a filter from a listener that does not exist → should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_detach_nonexistent_listener() {
    let harness = quick_harness("dev_flt_det_nel").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-detnl";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    create_filter_chain(&cli, prefix, echo_host, echo_port, listener_port);
    create_header_mutation_filter(&cli, &filter_name, "irrelevant");

    let output = cli
        .run(&["filter", "detach", &filter_name, "--listener", "no-such-listener-exists"])
        .unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Detaching from nonexistent listener should fail. stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// E2E: update filter with invalid/malformed config → should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_update_bad_config() {
    let harness = quick_harness("dev_flt_upd_bad").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let prefix = "e2e-flt-updbad";
    let filter_name = format!("{}-filter", prefix);
    let listener_port = harness.ports.listener;

    create_filter_chain(&cli, prefix, echo_host, echo_port, listener_port);
    create_header_mutation_filter(&cli, &filter_name, "good-value");

    // Try updating with completely invalid JSON
    let bad_file = write_temp_file("{ this is not valid json at all", ".json");
    let output = cli
        .run(&["filter", "update", &filter_name, "-f", bad_file.path().to_str().unwrap()])
        .unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Update with malformed JSON should fail. stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// E2E: update a filter that does not exist → should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_filter_update_nonexistent() {
    let harness = quick_harness("dev_flt_upd_ne").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let update_json = serde_json::json!({
        "name": "ghost-filter-does-not-exist",
        "filterType": "header_mutation",
        "config": {
            "type": "header_mutation",
            "config": {
                "response_headers_to_add": [
                    {"key": "X-Ghost", "value": "boo"}
                ]
            }
        }
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&update_json).unwrap(), ".json");
    let output = cli
        .run(&[
            "filter",
            "update",
            "ghost-filter-does-not-exist",
            "-f",
            file.path().to_str().unwrap(),
        ])
        .unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Updating nonexistent filter should fail. stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}
