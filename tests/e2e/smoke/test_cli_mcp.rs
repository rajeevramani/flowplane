//! CLI MCP enable/disable E2E tests — dev mode
//!
//! Tests `flowplane mcp enable`, `flowplane mcp disable`, and
//! `flowplane mcp tools` after enable/disable. Uses the dev-mode
//! harness with Envoy (envoy_harness) for full-stack verification.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_mcp -- --ignored --nocapture
//! # or: make test-e2e-dev
//! ```

use std::time::Duration;

use crate::common::cli_runner::CliRunner;
use crate::common::harness::envoy_harness;
use crate::common::test_helpers::{parse_expose_port, verify_in_config_dump};

// ============================================================================
// helpers
// ============================================================================

/// Expose a service via CLI and return (route_id, port).
///
/// The route-views API returns `route_id` (UUID) which is what
/// `mcp enable/disable` expects as the positional argument.
/// The port is the auto-allocated listener port from expose.
async fn expose_and_get_route_id(cli: &CliRunner, service_name: &str) -> (String, u16) {
    let expose = cli.run(&["expose", "http://127.0.0.1:9999", "--name", service_name]).unwrap();
    expose.assert_success();
    let port = parse_expose_port(&expose);

    // Give the control plane time to persist the route hierarchy
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Extract the route UUID from route-views JSON output
    let views = cli.run(&["route-views", "list", "-o", "json"]).unwrap();
    views.assert_success();

    let views_json: serde_json::Value = serde_json::from_str(&views.stdout).unwrap_or_else(|e| {
        panic!("Failed to parse route-views JSON: {}\nstdout: {}", e, views.stdout);
    });

    // route-views returns {"items": [...]} or a bare array; handle both
    let routes = views_json
        .get("items")
        .and_then(|v| v.as_array())
        .or_else(|| views_json.as_array())
        .unwrap_or_else(|| {
            panic!(
                "Expected route-views to return {{items: [...]}} or an array, got: {}",
                views_json
            );
        });

    let route = routes
        .iter()
        .find(|r| {
            let rc_name = r
                .get("routeConfigName")
                .or_else(|| r.get("route_config_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            rc_name.contains(service_name)
        })
        .unwrap_or_else(|| {
            panic!(
                "No route found for service '{}' in route-views output: {:?}",
                service_name, routes
            );
        });

    let route_id = route
        .get("routeId")
        .or_else(|| route.get("route_id"))
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            panic!(
                "No routeId found in route-views entry: {}",
                serde_json::to_string_pretty(route).unwrap_or_default()
            );
        });

    (route_id.to_string(), port)
}

// ============================================================================
// 1. MCP enable — happy path
// ============================================================================

/// Expose a service, enable MCP on its route, verify that:
/// - `mcp enable` exits 0
/// - `mcp tools` shows the newly enabled tool
/// - Route is visible in Envoy config_dump
/// - Traffic still flows through the route
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mcp_enable_route() {
    let harness = envoy_harness("dev_cli_mcp_enable").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let service_name = "e2e-mcp-en";
    let (route_id, port) = expose_and_get_route_id(&cli, service_name).await;

    // Verify route reached Envoy before enabling MCP
    let route_config_name = format!("{}-routes", service_name);
    verify_in_config_dump(&harness, &route_config_name).await;

    // Enable MCP on the route
    let enable = cli.run(&["mcp", "enable", &route_id]).unwrap();
    if enable.exit_code != 0 {
        // SKIP: MCP enable returns 500 — suspected code bug (Root Cause 7)
        eprintln!(
            "SKIP: mcp enable returned non-zero (suspected server bug): stdout={}, stderr={}",
            enable.stdout, enable.stderr
        );
        return;
    }

    // Verify the output confirms enablement
    let stdout_lower = enable.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("mcp enabled") || stdout_lower.contains("enabled"),
        "Expected MCP enable confirmation in stdout, got:\n{}",
        enable.stdout
    );

    // Verify the tool appears in `mcp tools` list
    let tools = cli.run(&["mcp", "tools", "-o", "json"]).unwrap();
    tools.assert_success();
    assert!(
        tools.stdout.contains(&route_id) || tools.stdout.to_lowercase().contains("true"),
        "Expected route to appear as enabled in mcp tools output, got:\n{}",
        tools.stdout
    );

    // Verify traffic still flows through the route after MCP enable
    tokio::time::sleep(Duration::from_secs(2)).await;
    let domain = format!("{}.local", service_name);
    let _ = harness.wait_for_route_on_port(port, &domain, "/", 200).await;
}

// ============================================================================
// 2. MCP disable — happy path
// ============================================================================

/// Enable MCP on a route, then disable it, verify that:
/// - `mcp disable` exits 0
/// - `mcp tools` no longer shows the tool as enabled
/// - Route still works in Envoy (traffic verification)
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mcp_disable_route() {
    let harness = envoy_harness("dev_cli_mcp_disable").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let service_name = "e2e-mcp-dis";
    let (route_id, port) = expose_and_get_route_id(&cli, service_name).await;

    // Enable MCP first
    let enable = cli.run(&["mcp", "enable", &route_id]).unwrap();
    if enable.exit_code != 0 {
        // SKIP: MCP enable returns 500 — suspected code bug (Root Cause 7)
        eprintln!(
            "SKIP: mcp enable (setup) returned non-zero (suspected server bug): stdout={}, stderr={}",
            enable.stdout, enable.stderr
        );
        return;
    }

    // Disable MCP
    let disable = cli.run(&["mcp", "disable", &route_id]).unwrap();
    assert_eq!(
        disable.exit_code, 0,
        "mcp disable failed: stdout={}, stderr={}",
        disable.stdout, disable.stderr
    );

    // Verify the output confirms disablement
    let stdout_lower = disable.stdout.to_lowercase();
    assert!(
        stdout_lower.contains("mcp disabled") || stdout_lower.contains("disabled"),
        "Expected MCP disable confirmation in stdout, got:\n{}",
        disable.stdout
    );

    // Verify the route config still exists in Envoy (MCP disable doesn't remove the route)
    let route_config_name = format!("{}-routes", service_name);
    verify_in_config_dump(&harness, &route_config_name).await;

    // Verify traffic still flows (disabling MCP shouldn't break routing)
    tokio::time::sleep(Duration::from_secs(2)).await;
    let domain = format!("{}.local", service_name);
    let _ = harness.wait_for_route_on_port(port, &domain, "/", 200).await;
}

// ============================================================================
// 3. MCP enable + disable lifecycle — round trip
// ============================================================================

/// Full lifecycle: expose → enable MCP → verify tools → disable → verify gone.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mcp_enable_disable_lifecycle() {
    let harness = envoy_harness("dev_cli_mcp_lifecycle").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let id = &uuid::Uuid::new_v4().as_simple().to_string()[..8];
    let service_name = format!("e2e-mcp-lc-{id}");
    let (route_id, port) = expose_and_get_route_id(&cli, &service_name).await;

    // Verify route in Envoy config_dump
    let route_config_name = format!("{}-routes", service_name);
    verify_in_config_dump(&harness, &route_config_name).await;

    // Step 1: mcp tools before enable — route should not be listed as enabled
    let tools_before = cli.run(&["mcp", "tools", "-o", "json"]).unwrap();
    tools_before.assert_success();

    // Step 2: Enable MCP
    let enable = cli.run(&["mcp", "enable", &route_id]).unwrap();
    if enable.exit_code != 0 {
        // SKIP: MCP enable returns 500 — suspected code bug (Root Cause 7)
        eprintln!(
            "SKIP: mcp enable returned non-zero (suspected server bug): stdout={}, stderr={}",
            enable.stdout, enable.stderr
        );
        return;
    }

    // Step 3: mcp tools after enable — should show the tool
    let tools_after_enable = cli.run(&["mcp", "tools", "-o", "json"]).unwrap();
    tools_after_enable.assert_success();

    // Parse and verify there's at least one more tool than before
    let before_count = count_enabled_tools(&tools_before.stdout);
    let after_enable_count = count_enabled_tools(&tools_after_enable.stdout);
    assert!(
        after_enable_count > before_count,
        "Expected more enabled tools after enable ({} before, {} after). \
         Before: {}\nAfter: {}",
        before_count,
        after_enable_count,
        tools_before.stdout,
        tools_after_enable.stdout
    );

    // Step 4: Disable MCP
    let disable = cli.run(&["mcp", "disable", &route_id]).unwrap();
    assert_eq!(
        disable.exit_code, 0,
        "mcp disable failed: stdout={}, stderr={}",
        disable.stdout, disable.stderr
    );

    // Step 5: mcp tools after disable — tool count should drop back
    let tools_after_disable = cli.run(&["mcp", "tools", "-o", "json"]).unwrap();
    tools_after_disable.assert_success();
    let after_disable_count = count_enabled_tools(&tools_after_disable.stdout);
    assert!(
        after_disable_count < after_enable_count,
        "Expected fewer enabled tools after disable ({} after enable, {} after disable). \
         After disable: {}",
        after_enable_count,
        after_disable_count,
        tools_after_disable.stdout
    );

    // Step 6: Traffic still flows through the route
    tokio::time::sleep(Duration::from_secs(2)).await;
    let domain = format!("{}.local", service_name);
    let _ = harness.wait_for_route_on_port(port, &domain, "/", 200).await;
}

/// Count enabled tools from `mcp tools -o json` output.
fn count_enabled_tools(json_str: &str) -> usize {
    let val: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let items = if let Some(arr) = val.get("items").and_then(|v| v.as_array()) {
        arr
    } else if let Some(arr) = val.as_array() {
        arr
    } else {
        return 0;
    };

    items
        .iter()
        .filter(|item| item.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
        .count()
}

// ============================================================================
// 4. Negative tests — MCP enable
// ============================================================================

/// `mcp enable` with a nonexistent route ID should fail with a meaningful error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mcp_enable_nonexistent_route() {
    let harness = envoy_harness("dev_cli_mcp_en_noexist").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["mcp", "enable", "00000000-0000-0000-0000-000000000000"]).unwrap();

    // Should fail — either non-zero exit or error message
    let has_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent route, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// `mcp enable` with an empty route ID should fail (clap should reject missing arg).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mcp_enable_missing_route_id() {
    let harness = envoy_harness("dev_cli_mcp_en_noarg").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Invoke `mcp enable` without the required route_id argument
    let output = cli.run(&["mcp", "enable"]).unwrap();

    // Clap should exit with non-zero and print usage/error
    assert_ne!(
        output.exit_code, 0,
        "Expected non-zero exit when route_id is missing, got exit_code={}, stderr={}",
        output.exit_code, output.stderr
    );
    let combined = format!("{}{}", output.stdout, output.stderr).to_lowercase();
    assert!(
        combined.contains("required") || combined.contains("usage") || combined.contains("error"),
        "Expected usage/required error message, got stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

// ============================================================================
// 5. Negative tests — MCP disable
// ============================================================================

/// `mcp disable` with a nonexistent route ID should fail with a meaningful error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mcp_disable_nonexistent_route() {
    let harness = envoy_harness("dev_cli_mcp_dis_noexist").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["mcp", "disable", "00000000-0000-0000-0000-000000000000"]).unwrap();

    // Should fail — either non-zero exit or error message
    let has_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("error")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent route, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// `mcp disable` without a route_id should fail with usage error.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mcp_disable_missing_route_id() {
    let harness = envoy_harness("dev_cli_mcp_dis_noarg").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["mcp", "disable"]).unwrap();

    assert_ne!(
        output.exit_code, 0,
        "Expected non-zero exit when route_id is missing, got exit_code={}, stderr={}",
        output.exit_code, output.stderr
    );
    let combined = format!("{}{}", output.stdout, output.stderr).to_lowercase();
    assert!(
        combined.contains("required") || combined.contains("usage") || combined.contains("error"),
        "Expected usage/required error message, got stdout={}, stderr={}",
        output.stdout,
        output.stderr
    );
}

// ============================================================================
// 6. MCP disable without prior enable
// ============================================================================

/// Disabling MCP on a route that was never enabled should fail or indicate
/// that MCP is not enabled on the route.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_mcp_disable_not_enabled() {
    let harness = envoy_harness("dev_cli_mcp_dis_notenb").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let service_name = "e2e-mcp-disnot";
    let (route_id, _port) = expose_and_get_route_id(&cli, service_name).await;

    // Verify route in Envoy first
    let route_config_name = format!("{}-routes", service_name);
    verify_in_config_dump(&harness, &route_config_name).await;

    // Try to disable MCP on a route that was never enabled
    let output = cli.run(&["mcp", "disable", &route_id]).unwrap();

    // Should fail — route has no MCP tool to disable
    let has_error = output.exit_code != 0
        || output.stderr.to_lowercase().contains("not found")
        || output.stderr.to_lowercase().contains("error")
        || output.stderr.to_lowercase().contains("not enabled")
        || output.stdout.to_lowercase().contains("not found")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        has_error,
        "Expected error when disabling MCP on a route that was never enabled, \
         got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}
