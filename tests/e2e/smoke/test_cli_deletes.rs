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
use crate::common::harness::quick_harness;
use crate::common::test_helpers::{
    create_chain_for_cluster, create_chain_for_route, verify_in_config_dump, verify_traffic,
    write_temp_file,
};

// ============================================================================
// Cluster delete
// ============================================================================

/// Create a cluster via scaffold+create, verify it exists in Envoy,
/// delete it, verify it's gone from both CLI and Envoy config.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_cluster() {
    let harness = quick_harness("dev_del_cluster").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    // Step 1: scaffold and create a cluster
    let cluster_name = "e2e-del-cluster";
    let scaffold_output = cli.run(&["cluster", "scaffold"]).unwrap();
    scaffold_output.assert_success();

    let content = scaffold_output
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-cluster-name>", cluster_name)
        .replace("your-cluster-name", cluster_name)
        .replace("<your-service-name>", "del-svc")
        .replace("your-service-name", "del-svc")
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
    let get_output = cli.run(&["cluster", "get", cluster_name]).unwrap();
    get_output.assert_success();
    get_output.assert_stdout_contains(cluster_name);

    // Step 3: build full Envoy chain and verify traffic
    let (_route, domain) =
        create_chain_for_cluster(&harness, cluster_name, "del-cluster").await;
    verify_in_config_dump(&harness, cluster_name).await;
    verify_traffic(&harness, &domain, "/test").await;

    // Step 4: delete the cluster
    let delete_output = cli.run(&["cluster", "delete", cluster_name, "--yes"]).unwrap();
    delete_output.assert_success();
    delete_output.assert_stdout_contains("deleted");

    // Step 5: verify cluster is gone from CLI
    let get_after = cli.run(&["cluster", "get", cluster_name]).unwrap();
    get_after.assert_failure();

    // Step 6: verify cluster is gone from Envoy config_dump
    tokio::time::sleep(Duration::from_secs(3)).await;
    if harness.has_envoy() {
        let dump = harness.get_config_dump().await.expect("config_dump should work");
        assert!(
            !dump.contains(cluster_name),
            "Deleted cluster '{}' should not appear in Envoy config_dump",
            cluster_name
        );
    }
}

// ============================================================================
// Listener delete
// ============================================================================

/// Create a listener (with prerequisite cluster+route), verify it in Envoy,
/// delete it, verify it's gone.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_listener() {
    let harness = quick_harness("dev_del_listener").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    // Step 1: create a cluster first (prerequisite)
    let cluster_name = "e2e-del-ls-cluster";
    let scaffold_output = cli.run(&["cluster", "scaffold"]).unwrap();
    scaffold_output.assert_success();

    let cluster_content = scaffold_output
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-cluster-name>", cluster_name)
        .replace("your-cluster-name", cluster_name)
        .replace("<your-service-name>", "del-ls-svc")
        .replace("your-service-name", "del-ls-svc")
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
    let route_name = "e2e-del-ls-rc";
    let domain = "del-listener.e2e.local";
    let route_scaffold = cli.run(&["route", "scaffold"]).unwrap();
    route_scaffold.assert_success();

    let route_content = route_scaffold
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-route-config-name>", route_name)
        .replace("your-route-config-name", route_name)
        .replace("<your-route-name>", "del-ls-rt")
        .replace("your-route-name", "del-ls-rt")
        .replace("<your-virtual-host-name>", "del-ls-vh")
        .replace("your-virtual-host-name", "del-ls-vh")
        .replace("<your-domain>", domain)
        .replace("your-domain.com", domain)
        .replace("example.com", domain)
        .replace("<your-cluster-name>", cluster_name)
        .replace("your-cluster-name", cluster_name)
        .replace("my-cluster", cluster_name);

    let route_file = write_temp_file(&route_content, ".yaml");
    let route_path = route_file.path().to_str().unwrap().to_string();
    let create_route = cli.run(&["route", "create", "-f", &route_path]).unwrap();
    create_route.assert_success();

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Step 3: scaffold and create a listener pointing to the route config
    let listener_name = "e2e-del-listener";
    let listener_scaffold = cli.run(&["listener", "scaffold"]).unwrap();
    listener_scaffold.assert_success();

    let listener_content = listener_scaffold
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-listener-name>", listener_name)
        .replace("your-listener-name", listener_name)
        .replace("<your-route-config-name>", route_name)
        .replace("your-route-config-name", route_name)
        .replace("my-route-config", route_name)
        .replace("<port>", &harness.ports.listener.to_string())
        .replace("10000", &harness.ports.listener.to_string())
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
    let get_output = cli.run(&["listener", "get", listener_name]).unwrap();
    get_output.assert_success();
    get_output.assert_stdout_contains(listener_name);
    verify_in_config_dump(&harness, listener_name).await;

    // Step 5: delete the listener
    let delete_output = cli.run(&["listener", "delete", listener_name, "--yes"]).unwrap();
    delete_output.assert_success();
    delete_output.assert_stdout_contains("deleted");

    // Step 6: verify listener is gone from CLI
    let get_after = cli.run(&["listener", "get", listener_name]).unwrap();
    get_after.assert_failure();

    // Step 7: verify listener is gone from Envoy config_dump
    tokio::time::sleep(Duration::from_secs(3)).await;
    if harness.has_envoy() {
        let dump = harness.get_config_dump().await.expect("config_dump should work");
        assert!(
            !dump.contains(listener_name),
            "Deleted listener '{}' should not appear in Envoy config_dump",
            listener_name
        );
    }
}

// ============================================================================
// Route delete
// ============================================================================

/// Create a route config (with prerequisite cluster), verify it in Envoy,
/// delete it, verify it's gone.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_route() {
    let harness = quick_harness("dev_del_route").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let echo = harness.echo_endpoint();
    let parts: Vec<&str> = echo.split(':').collect();
    let (echo_host, echo_port) = (parts[0], parts[1]);

    // Step 1: create a cluster first (prerequisite for route)
    let cluster_name = "e2e-del-rt-cluster";
    let scaffold_output = cli.run(&["cluster", "scaffold"]).unwrap();
    scaffold_output.assert_success();

    let cluster_content = scaffold_output
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-cluster-name>", cluster_name)
        .replace("your-cluster-name", cluster_name)
        .replace("<your-service-name>", "del-rt-svc")
        .replace("your-service-name", "del-rt-svc")
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
    let route_name = "e2e-del-route";
    let domain = "del-route.e2e.local";
    let route_scaffold = cli.run(&["route", "scaffold"]).unwrap();
    route_scaffold.assert_success();

    let route_content = route_scaffold
        .stdout
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<your-route-config-name>", route_name)
        .replace("your-route-config-name", route_name)
        .replace("<your-route-name>", "del-rt")
        .replace("your-route-name", "del-rt")
        .replace("<your-virtual-host-name>", "del-rt-vh")
        .replace("your-virtual-host-name", "del-rt-vh")
        .replace("<your-domain>", domain)
        .replace("your-domain.com", domain)
        .replace("example.com", domain)
        .replace("<your-cluster-name>", cluster_name)
        .replace("your-cluster-name", cluster_name)
        .replace("my-cluster", cluster_name);

    let route_file = write_temp_file(&route_content, ".yaml");
    let route_path = route_file.path().to_str().unwrap().to_string();
    let create_route = cli.run(&["route", "create", "-f", &route_path]).unwrap();
    assert_eq!(
        create_route.exit_code, 0,
        "route create failed: stdout={}, stderr={}",
        create_route.stdout, create_route.stderr
    );

    // Step 3: complete the Envoy chain with a listener, verify traffic
    let domain =
        create_chain_for_route(&harness, route_name, "del-route", domain).await;
    verify_in_config_dump(&harness, route_name).await;
    verify_traffic(&harness, &domain, "/test").await;

    // Step 4: verify route exists via CLI
    let get_output = cli.run(&["route", "get", route_name]).unwrap();
    get_output.assert_success();
    get_output.assert_stdout_contains(route_name);

    // Step 5: delete the route
    let delete_output = cli.run(&["route", "delete", route_name, "--yes"]).unwrap();
    delete_output.assert_success();
    delete_output.assert_stdout_contains("deleted");

    // Step 6: verify route is gone from CLI
    let get_after = cli.run(&["route", "get", route_name]).unwrap();
    get_after.assert_failure();

    // Step 7: verify route is gone from Envoy config_dump
    tokio::time::sleep(Duration::from_secs(3)).await;
    if harness.has_envoy() {
        let dump = harness.get_config_dump().await.expect("config_dump should work");
        assert!(
            !dump.contains(route_name),
            "Deleted route '{}' should not appear in Envoy config_dump",
            route_name
        );
    }
}

// ============================================================================
// Negative tests — deleting nonexistent resources
// ============================================================================

/// Deleting a cluster that doesn't exist should fail with a meaningful error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_nonexistent_cluster() {
    let harness = quick_harness("dev_del_no_cluster").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli
        .run(&["cluster", "delete", "does-not-exist-cluster-xyz", "--yes"])
        .unwrap();
    output.assert_failure();
}

/// Deleting a listener that doesn't exist should fail with a meaningful error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_nonexistent_listener() {
    let harness = quick_harness("dev_del_no_listener").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli
        .run(&["listener", "delete", "does-not-exist-listener-xyz", "--yes"])
        .unwrap();
    output.assert_failure();
}

/// Deleting a route that doesn't exist should fail with a meaningful error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_delete_nonexistent_route() {
    let harness = quick_harness("dev_del_no_route").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli
        .run(&["route", "delete", "does-not-exist-route-xyz", "--yes"])
        .unwrap();
    output.assert_failure();
}
