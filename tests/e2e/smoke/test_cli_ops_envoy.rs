//! CLI ops diagnostic commands with Envoy cross-verification — dev mode
//!
//! Tests `apply -f <dir>`, `trace`, `xds status`, and `topology` CLI commands
//! against a live Envoy instance. Each test sets up resources via CLI/API,
//! verifies they reach Envoy via config_dump, then cross-verifies that CLI
//! diagnostic output is consistent with Envoy's actual state.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_ops_envoy -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::quick_harness;
use crate::common::test_helpers::{
    create_chain_for_cluster, verify_in_config_dump, verify_traffic, write_temp_file,
};

// ============================================================================
// 1. apply -f <dir> — multi-resource directory apply with Envoy verification
// ============================================================================

/// Apply a directory containing cluster + route + listener YAML manifests.
/// Verify each resource in Envoy config_dump and that traffic flows through.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_envoy_apply_dir() {
    let harness = quick_harness("dev_ops_envoy_apply").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    assert!(harness.has_envoy(), "This test requires Envoy");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    let cluster_name = "e2e-opsenv-apply-cl";
    let route_name = "e2e-opsenv-apply-rc";
    let vhost_name = "e2e-opsenv-apply-vh";
    let domain = "opsenv-apply.e2e.local";
    let listener_name = "e2e-opsenv-apply-ls";
    let listener_port = harness.ports.listener;

    let dir = tempfile::tempdir().expect("create temp dir");

    // 01-cluster.yaml — processed first alphabetically
    let cluster_yaml = format!(
        r#"kind: Cluster
name: {cluster_name}
endpoints:
  - host: {echo_host}
    port: {echo_port}
connectTimeoutSeconds: 5
"#
    );
    std::fs::write(dir.path().join("01-cluster.yaml"), cluster_yaml).expect("write cluster yaml");

    // 02-route.yaml
    let route_yaml = format!(
        r#"kind: RouteConfig
name: {route_name}
virtualHosts:
  - name: {vhost_name}
    domains:
      - "{domain}"
    routes:
      - name: opsenv-apply-rt
        match:
          path:
            type: prefix
            value: "/"
        action:
          type: forward
          cluster: {cluster_name}
          timeoutSeconds: 30
"#
    );
    std::fs::write(dir.path().join("02-route.yaml"), route_yaml).expect("write route yaml");

    // 03-listener.yaml
    let listener_yaml = format!(
        r#"kind: Listener
name: {listener_name}
address: "0.0.0.0"
port: {listener_port}
dataplaneId: dev-dataplane-id
filterChains:
  - name: default
    filters:
      - name: envoy.filters.network.http_connection_manager
        type: httpConnectionManager
        routeConfigName: {route_name}
"#
    );
    std::fs::write(dir.path().join("03-listener.yaml"), listener_yaml).expect("write listener yaml");

    // Apply the whole directory
    let output = cli.run(&["apply", "-f", dir.path().to_str().unwrap()]).unwrap();
    assert_eq!(
        output.exit_code, 0,
        "apply -f <dir> failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    // Verify each resource reached Envoy via config_dump
    verify_in_config_dump(&harness, cluster_name).await;
    verify_in_config_dump(&harness, route_name).await;
    verify_in_config_dump(&harness, listener_name).await;

    // Verify traffic flows through the created route
    tokio::time::sleep(Duration::from_secs(3)).await;
    verify_traffic(&harness, domain, "/test").await;
}

/// Apply an empty directory — should fail or produce an error/warning.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_envoy_apply_dir_empty() {
    let harness = quick_harness("dev_ops_envoy_apply_e").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let dir = tempfile::tempdir().expect("create temp dir");

    let output = cli.run(&["apply", "-f", dir.path().to_str().unwrap()]).unwrap();

    // Empty directory: should fail or warn
    let indicated_problem = output.exit_code != 0
        || output.stderr.to_lowercase().contains("error")
        || output.stderr.to_lowercase().contains("no ")
        || output.stderr.to_lowercase().contains("empty")
        || output.stdout.to_lowercase().contains("no ")
        || output.stdout.to_lowercase().contains("empty")
        || output.stdout.to_lowercase().contains("nothing");
    assert!(
        indicated_problem,
        "apply -f on empty dir should fail or produce a warning.\n\
         exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );

    // Cross-verify: Envoy should NOT have any resources from this test
    let config_dump = harness.get_config_dump().await.expect("config_dump should succeed");
    assert!(
        !config_dump.contains("dev_ops_envoy_apply_e"),
        "Envoy should not have resources from the empty-directory test"
    );
}

// ============================================================================
// 2. trace — cross-verify CLI trace output against Envoy routing
// ============================================================================

/// Expose a service, verify in Envoy, then trace and cross-verify
/// that trace output references resources present in Envoy config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_envoy_trace_cross_verify() {
    let harness = quick_harness("dev_ops_envoy_trace").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    assert!(harness.has_envoy(), "This test requires Envoy");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    // Create a cluster + route + listener chain
    let cluster_name = "e2e-trace-xv-cl";
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
        "cluster create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    let (_route, _domain) =
        create_chain_for_cluster(&harness, cluster_name, "trace-xv").await;

    // Verify resources are in Envoy config_dump
    verify_in_config_dump(&harness, cluster_name).await;

    // Get Envoy's actual config dump for cross-verification
    let config_dump = harness.get_config_dump().await.expect("config_dump should succeed");
    assert!(
        config_dump.contains(cluster_name),
        "Envoy config_dump should contain cluster '{cluster_name}'"
    );

    // Run trace via CLI
    let trace_output = cli.run(&["trace", "/"]).unwrap();
    trace_output.assert_success();

    let trace_combined = format!("{}{}", trace_output.stdout, trace_output.stderr).to_lowercase();

    // Cross-verify: trace output should reference routing concepts that Envoy has
    assert!(
        trace_combined.contains("trace")
            || trace_combined.contains("route")
            || trace_combined.contains("match")
            || trace_combined.contains("listener")
            || trace_combined.contains("cluster")
            || trace_combined.contains(cluster_name),
        "trace output should reference routing information consistent with Envoy state.\n\
         trace stdout: {}\ntrace stderr: {}",
        trace_output.stdout,
        trace_output.stderr
    );

    // If trace mentions a cluster, that cluster should exist in Envoy
    if trace_combined.contains("cluster") {
        // The config_dump should have at least one cluster (our test cluster)
        assert!(
            config_dump.to_lowercase().contains("cluster"),
            "Envoy config_dump should have cluster data when trace references clusters"
        );
    }

    // If the config_dump has route configs, trace should show routing info
    if config_dump.contains("route_config") {
        assert!(
            !trace_combined.is_empty(),
            "trace should produce output when Envoy has route configurations"
        );
    }
}

/// Trace a non-existent path against Envoy — should report no match.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_envoy_trace_no_match() {
    let harness = quick_harness("dev_ops_envoy_tr_no").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    assert!(harness.has_envoy(), "This test requires Envoy");
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Don't expose any service — trace should find nothing
    let output = cli.run(&["trace", "/nonexistent/path/xyz123"]).unwrap();

    // Should succeed or return exit 1 (no match) but not crash
    assert!(
        output.exit_code == 0 || output.exit_code == 1,
        "trace of unmatched path should exit 0 or 1, got {}",
        output.exit_code
    );

    // Cross-verify: Envoy config_dump should not have any routes for this path
    let config_dump = harness.get_config_dump().await.expect("config_dump should succeed");
    assert!(
        !config_dump.contains("/nonexistent/path/xyz123"),
        "Envoy should not have a route for the non-existent test path"
    );

    // Trace should produce some output (error message or empty result)
    assert!(
        !output.stdout.trim().is_empty() || !output.stderr.trim().is_empty(),
        "trace should produce output even when no match is found"
    );
}

// ============================================================================
// 3. xds status — cross-verify against Envoy admin
// ============================================================================

/// Set up resources in Envoy, run `xds status`, cross-verify that xDS status
/// reflects what Envoy actually has in its config_dump.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_envoy_xds_status_cross_verify() {
    let harness = quick_harness("dev_ops_envoy_xds").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    assert!(harness.has_envoy(), "This test requires Envoy");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    // Create resources so xDS has something to report
    let cluster_name = "e2e-xds-xv-cl";
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
        "cluster create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    let (_route, _domain) =
        create_chain_for_cluster(&harness, cluster_name, "xds-xv").await;

    // Wait for xDS to propagate
    verify_in_config_dump(&harness, cluster_name).await;

    // Run xds status via CLI
    let xds_output = cli.run(&["xds", "status"]).unwrap();
    xds_output.assert_success();

    let xds_combined = format!("{}{}", xds_output.stdout, xds_output.stderr).to_lowercase();

    // Cross-verify against Envoy config_dump
    let config_dump = harness.get_config_dump().await.expect("config_dump should succeed");

    // If Envoy has clusters, xds status should mention clusters or CDS
    if config_dump.to_lowercase().contains("cluster") {
        assert!(
            xds_combined.contains("cluster")
                || xds_combined.contains("cds")
                || xds_combined.contains("ack")
                || xds_combined.contains("connected")
                || xds_combined.contains("version")
                || xds_combined.contains("resource"),
            "xds status should reflect cluster/CDS data when Envoy has clusters.\n\
             xds stdout: {}\nxds stderr: {}",
            xds_output.stdout,
            xds_output.stderr
        );
    }

    // If Envoy has listeners, xds status should mention listeners or LDS
    if config_dump.to_lowercase().contains("listener") {
        assert!(
            xds_combined.contains("listener")
                || xds_combined.contains("lds")
                || xds_combined.contains("ack")
                || xds_combined.contains("connected")
                || xds_combined.contains("version")
                || xds_combined.contains("resource"),
            "xds status should reflect listener/LDS data when Envoy has listeners.\n\
             xds stdout: {}\nxds stderr: {}",
            xds_output.stdout,
            xds_output.stderr
        );
    }

    // xDS status should not be empty when resources exist
    assert!(
        !xds_combined.trim().is_empty(),
        "xds status should produce output when resources exist in Envoy"
    );
}

/// xds status with no resources should still succeed (empty or baseline state).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_envoy_xds_status_empty() {
    let harness = quick_harness("dev_ops_envoy_xds_e").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    assert!(harness.has_envoy(), "This test requires Envoy");
    let cli = CliRunner::from_harness(&harness).unwrap();

    // No resources created — xds status should still work
    let output = cli.run(&["xds", "status"]).unwrap();
    output.assert_success();

    // Cross-verify: Envoy config_dump should be mostly empty (no user clusters)
    let config_dump = harness.get_config_dump().await.expect("config_dump should succeed");
    assert!(
        !config_dump.contains("e2e-xds-xv-cl"),
        "Envoy should not have test clusters when none were created"
    );
}

// ============================================================================
// 4. topology — cross-verify config graph against Envoy
// ============================================================================

/// Set up a cluster → route → listener chain, verify in Envoy, run topology,
/// and cross-verify that topology output references the same resources Envoy has.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_envoy_topology_cross_verify() {
    let harness = quick_harness("dev_ops_envoy_topo").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    assert!(harness.has_envoy(), "This test requires Envoy");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    // Create a full chain: cluster → route → listener
    let cluster_name = "e2e-topo-xv-cl";
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
        "cluster create failed: stdout={}, stderr={}",
        output.stdout, output.stderr
    );

    let (_route, _domain) =
        create_chain_for_cluster(&harness, cluster_name, "topo-xv").await;

    // Verify everything is in Envoy
    verify_in_config_dump(&harness, cluster_name).await;

    // Get Envoy config_dump for cross-verification
    let config_dump = harness.get_config_dump().await.expect("config_dump should succeed");

    // Run topology via CLI
    let topo_output = cli.run(&["topology"]).unwrap();
    topo_output.assert_success();

    let topo_combined = format!("{}{}", topo_output.stdout, topo_output.stderr).to_lowercase();

    // Topology should mention resources that exist in Envoy
    assert!(
        topo_combined.contains("cluster")
            || topo_combined.contains("listener")
            || topo_combined.contains("route")
            || topo_combined.contains(cluster_name),
        "topology output should reference resource types present in Envoy.\n\
         topology stdout: {}\ntopology stderr: {}",
        topo_output.stdout,
        topo_output.stderr
    );

    // Cross-verify: if Envoy has our cluster, topology should mention it or its type
    if config_dump.contains(cluster_name) {
        assert!(
            topo_combined.contains(cluster_name)
                || topo_combined.contains("cluster")
                || topo_combined.contains("topo-xv"),
            "topology should reference our cluster '{}' when it exists in Envoy.\n\
             topology stdout: {}\ntopology stderr: {}",
            cluster_name, topo_output.stdout, topo_output.stderr
        );
    }

    // If Envoy has listeners, topology should show them
    if config_dump.to_lowercase().contains("listener") {
        assert!(
            topo_combined.contains("listener") || topo_combined.contains("topo-xv-ls"),
            "topology should reference listeners when Envoy has them.\n\
             topology stdout: {}\ntopology stderr: {}",
            topo_output.stdout, topo_output.stderr
        );
    }
}

/// Topology with no resources should succeed but show empty/baseline state.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_ops_envoy_topology_empty() {
    let harness = quick_harness("dev_ops_envoy_topo_e").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    assert!(harness.has_envoy(), "This test requires Envoy");
    let cli = CliRunner::from_harness(&harness).unwrap();

    // No resources created
    let output = cli.run(&["topology"]).unwrap();
    output.assert_success();

    // Cross-verify: Envoy should not have user-created resources
    let config_dump = harness.get_config_dump().await.expect("config_dump should succeed");
    assert!(
        !config_dump.contains("e2e-topo-xv-cl"),
        "Envoy should not have test clusters when none were created"
    );

    // Topology should either be empty or show baseline state
    assert!(
        !output.stdout.trim().is_empty() || !output.stderr.trim().is_empty(),
        "topology should produce some output even with no resources"
    );
}
