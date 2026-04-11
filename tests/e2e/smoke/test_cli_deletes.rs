//! E2E tests for CLI resource delete commands (dev mode).
//!
//! Tests `flowplane cluster delete`, `listener delete`, `route delete`,
//! and negative cases (deleting nonexistent resources).
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_delete -- --ignored --nocapture
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::envoy_harness;
use crate::common::test_helpers::{
    create_chain_for_cluster, create_chain_for_route, verify_in_config_dump,
    verify_not_in_config_dump, verify_traffic, write_temp_file,
};

// ============================================================================
// Cluster delete
// ============================================================================

/// Create a cluster via scaffold+create, verify it exists in Envoy,
/// delete it, verify it's gone from both CLI and Envoy config.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_cluster() {
    let harness = envoy_harness("dev_del_cluster").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    // Step 1: scaffold and create a cluster
    let cluster_name = format!("del-cl-{id}");
    let scaffold_output = cli.run(&["cluster", "scaffold"]).unwrap();
    scaffold_output.assert_success();

    let content = scaffold_output
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-cluster-name>", &cluster_name)
        .replace("your-cluster-name", &cluster_name)
        .replace("<your-service-name>", &format!("del-svc-{id}"))
        .replace("your-service-name", &format!("del-svc-{id}"))
        .replace("<host-or-ip>", echo_host)
        .replace("<host>", echo_host)
        .replace("host.example.com", echo_host)
        .replace("<port>", echo_port)
        .replace("8080", echo_port);

    let file = write_temp_file(&content, ".yaml");
    let file_path = file.path().to_str().unwrap().to_string();

    let create_output = cli.run(&["cluster", "create", "-f", &file_path]).unwrap();
    assert_eq!(
        create_output.exit_code, 0,
        "cluster create failed: stdout={}, stderr={}",
        create_output.stdout, create_output.stderr
    );

    // Step 2: verify cluster exists via CLI
    let get_output = cli.run(&["cluster", "get", &cluster_name]).unwrap();
    get_output.assert_success();
    get_output.assert_stdout_contains(&cluster_name);

    // Step 3: build full Envoy chain and verify traffic
    let chain_prefix = format!("dc-{id}");
    let (_route, domain) = create_chain_for_cluster(&harness, &cluster_name, &chain_prefix).await;
    verify_in_config_dump(&harness, &cluster_name).await;
    verify_traffic(&harness, &domain, "/test").await;

    // Step 4: delete in reverse dependency order — listener → route → cluster
    // The chain creates "{chain_prefix}-ls" listener and "{chain_prefix}-rc" route config.
    // Cluster cannot be deleted while referenced by a route config.
    // All deletes must succeed — otherwise stale route configs referencing the cluster
    // name will keep the name in config_dump and cause verify_not_in_config_dump to fail.
    let chain_ls = format!("{chain_prefix}-ls");
    let chain_rc = format!("{chain_prefix}-rc");
    let ls_del = cli.run(&["listener", "delete", &chain_ls, "--yes"]).unwrap();
    ls_del.assert_success();
    let rc_del = cli.run(&["route", "delete", &chain_rc, "--yes"]).unwrap();
    rc_del.assert_success();

    let delete_output = cli.run(&["cluster", "delete", &cluster_name, "--yes"]).unwrap();
    delete_output.assert_success();
    delete_output.assert_stdout_contains("deleted");

    // Step 5: verify cluster is gone from CLI
    let get_after = cli.run(&["cluster", "get", &cluster_name]).unwrap();
    get_after.assert_failure();

    // Step 6: verify cluster is gone from Envoy config_dump
    // Uses name_regex filter — only checks cluster resources, not cross-references in routes
    verify_not_in_config_dump(&harness, &cluster_name).await;
}

// ============================================================================
// Listener delete
// ============================================================================

/// Create a listener (with prerequisite cluster+route), verify it in Envoy,
/// delete it, verify it's gone.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_listener() {
    let harness = envoy_harness("dev_del_listener").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    // Step 1: create a cluster first (prerequisite)
    let cluster_name = format!("dl-cl-{id}");
    let scaffold_output = cli.run(&["cluster", "scaffold"]).unwrap();
    scaffold_output.assert_success();

    let cluster_content = scaffold_output
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-cluster-name>", &cluster_name)
        .replace("your-cluster-name", &cluster_name)
        .replace("<your-service-name>", &format!("dl-svc-{id}"))
        .replace("your-service-name", &format!("dl-svc-{id}"))
        .replace("<host-or-ip>", echo_host)
        .replace("<host>", echo_host)
        .replace("host.example.com", echo_host)
        .replace("<port>", echo_port)
        .replace("8080", echo_port);

    let cluster_file = write_temp_file(&cluster_content, ".yaml");
    let cluster_path = cluster_file.path().to_str().unwrap().to_string();
    let create_cluster = cli.run(&["cluster", "create", "-f", &cluster_path]).unwrap();
    create_cluster.assert_success();

    // Step 2: create a route config pointing to the cluster
    let route_name = format!("dl-rc-{id}");
    let domain = format!("dl-{id}.e2e.local");
    let route_scaffold = cli.run(&["route", "scaffold"]).unwrap();
    route_scaffold.assert_success();

    let route_content = route_scaffold
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-route-config-name>", &route_name)
        .replace("your-route-config-name", &route_name)
        .replace("<your-route-name>", &format!("dl-rt-{id}"))
        .replace("your-route-name", &format!("dl-rt-{id}"))
        .replace("<your-virtual-host-name>", &format!("dl-vh-{id}"))
        .replace("your-virtual-host-name", &format!("dl-vh-{id}"))
        .replace("<your-domain>", &domain)
        .replace("your-domain.com", &domain)
        .replace("example.com", &domain)
        .replace("<your-cluster-name>", &cluster_name)
        .replace("your-cluster-name", &cluster_name)
        .replace("my-cluster", &cluster_name);

    let route_file = write_temp_file(&route_content, ".yaml");
    let route_path = route_file.path().to_str().unwrap().to_string();
    let create_route = cli.run(&["route", "create", "-f", &route_path]).unwrap();
    create_route.assert_success();

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Step 3: scaffold and create a listener pointing to the route config
    let listener_name = format!("dl-ls-{id}");
    let listener_scaffold = cli.run(&["listener", "scaffold"]).unwrap();
    listener_scaffold.assert_success();

    let listener_content = listener_scaffold
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-listener-name>", &listener_name)
        .replace("your-listener-name", &listener_name)
        .replace("<your-route-config-name>", &route_name)
        .replace("your-route-config-name", &route_name)
        .replace("my-route-config", &route_name)
        .replace("<your-dataplane-id>", "dev-dataplane-id")
        .replace("your-dataplane-id", "dev-dataplane-id")
        .replace("<port>", &harness.ports.listener.to_string())
        .replace("10000", &harness.ports.listener.to_string())
        .replace("10001", &harness.ports.listener.to_string())
        .replace("8080", &harness.ports.listener.to_string());

    let listener_file = write_temp_file(&listener_content, ".yaml");
    let listener_path = listener_file.path().to_str().unwrap().to_string();
    let create_listener = cli.run(&["listener", "create", "-f", &listener_path]).unwrap();
    assert_eq!(
        create_listener.exit_code, 0,
        "listener create failed: stdout={}, stderr={}",
        create_listener.stdout, create_listener.stderr
    );

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Step 4: verify listener exists in CLI and Envoy
    let get_output = cli.run(&["listener", "get", &listener_name]).unwrap();
    get_output.assert_success();
    get_output.assert_stdout_contains(&listener_name);
    verify_in_config_dump(&harness, &listener_name).await;

    // Step 5: delete the listener
    let delete_output = cli.run(&["listener", "delete", &listener_name, "--yes"]).unwrap();
    delete_output.assert_success();
    delete_output.assert_stdout_contains("deleted");

    // Step 6: verify listener is gone from CLI
    let get_after = cli.run(&["listener", "get", &listener_name]).unwrap();
    get_after.assert_failure();

    // Step 7: verify listener is gone from Envoy config_dump (polls with retry)
    verify_not_in_config_dump(&harness, &listener_name).await;
}

// ============================================================================
// Route delete
// ============================================================================

/// Create a route config (with prerequisite cluster), verify it in Envoy,
/// delete it, verify it's gone.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_route() {
    let harness = envoy_harness("dev_del_route").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    // Step 1: create a cluster first (prerequisite for route)
    let cluster_name = format!("dr-cl-{id}");
    let scaffold_output = cli.run(&["cluster", "scaffold"]).unwrap();
    scaffold_output.assert_success();

    let cluster_content = scaffold_output
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-cluster-name>", &cluster_name)
        .replace("your-cluster-name", &cluster_name)
        .replace("<your-service-name>", &format!("dr-svc-{id}"))
        .replace("your-service-name", &format!("dr-svc-{id}"))
        .replace("<host-or-ip>", echo_host)
        .replace("<host>", echo_host)
        .replace("host.example.com", echo_host)
        .replace("<port>", echo_port)
        .replace("8080", echo_port);

    let cluster_file = write_temp_file(&cluster_content, ".yaml");
    let cluster_path = cluster_file.path().to_str().unwrap().to_string();
    let create_cluster = cli.run(&["cluster", "create", "-f", &cluster_path]).unwrap();
    create_cluster.assert_success();

    // Step 2: scaffold and create a route config
    let route_name = format!("dr-rc-{id}");
    let domain = format!("dr-{id}.e2e.local");
    let route_scaffold = cli.run(&["route", "scaffold"]).unwrap();
    route_scaffold.assert_success();

    let route_content = route_scaffold
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-route-config-name>", &route_name)
        .replace("your-route-config-name", &route_name)
        .replace("<your-route-name>", &format!("dr-rt-{id}"))
        .replace("your-route-name", &format!("dr-rt-{id}"))
        .replace("<your-virtual-host-name>", &format!("dr-vh-{id}"))
        .replace("your-virtual-host-name", &format!("dr-vh-{id}"))
        .replace("<your-domain>", &domain)
        .replace("your-domain.com", &domain)
        .replace("example.com", &domain)
        .replace("<your-cluster-name>", &cluster_name)
        .replace("your-cluster-name", &cluster_name)
        .replace("my-cluster", &cluster_name);

    let route_file = write_temp_file(&route_content, ".yaml");
    let route_path = route_file.path().to_str().unwrap().to_string();
    let create_route = cli.run(&["route", "create", "-f", &route_path]).unwrap();
    assert_eq!(
        create_route.exit_code, 0,
        "route create failed: stdout={}, stderr={}",
        create_route.stdout, create_route.stderr
    );

    // Step 3: complete the Envoy chain with a listener, verify traffic
    let chain_prefix = format!("dr-{id}");
    let domain = create_chain_for_route(&harness, &route_name, &chain_prefix, &domain).await;
    verify_in_config_dump(&harness, &route_name).await;
    verify_traffic(&harness, &domain, "/test").await;

    // Step 4: verify route exists via CLI
    let get_output = cli.run(&["route", "get", &route_name]).unwrap();
    get_output.assert_success();
    get_output.assert_stdout_contains(&route_name);

    // Step 5: delete the listener (references route via routeConfigName)
    let chain_ls = format!("{chain_prefix}-ls");
    let ls_del = cli.run(&["listener", "delete", &chain_ls, "--yes"]).unwrap();
    ls_del.assert_success();

    // Step 6: delete the route
    let delete_output = cli.run(&["route", "delete", &route_name, "--yes"]).unwrap();
    delete_output.assert_success();
    delete_output.assert_stdout_contains("deleted");

    // Step 7: verify route is gone from CLI
    let get_after = cli.run(&["route", "get", &route_name]).unwrap();
    get_after.assert_failure();

    // Step 8: verify route is gone from Envoy config_dump (polls with retry)
    verify_not_in_config_dump(&harness, &route_name).await;
}

// ============================================================================
// Negative tests — deleting nonexistent resources
// ============================================================================

/// Deleting a cluster that doesn't exist should fail with a meaningful error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_nonexistent_cluster() {
    let harness = envoy_harness("dev_del_no_cluster").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["cluster", "delete", "does-not-exist-cluster-xyz", "--yes"]).unwrap();
    output.assert_failure();
}

/// Deleting a listener that doesn't exist should fail with a meaningful error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_nonexistent_listener() {
    let harness = envoy_harness("dev_del_no_listener").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["listener", "delete", "does-not-exist-listener-xyz", "--yes"]).unwrap();
    output.assert_failure();
}

/// Deleting a route that doesn't exist should fail with a meaningful error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_nonexistent_route() {
    let harness = envoy_harness("dev_del_no_route").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["route", "delete", "does-not-exist-route-xyz", "--yes"]).unwrap();
    output.assert_failure();
}
