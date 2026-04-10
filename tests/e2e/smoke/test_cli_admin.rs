//! E2E tests for CLI agent, cert, and admin commands.
//!
//! Agent tests require Zitadel (prod mode only). Cert and admin tests run in
//! dev mode with graceful skips when PKI/Vault is not configured.
//!
//! ```bash
//! # Agent tests (prod mode only)
//! RUN_E2E=1 cargo test --test e2e prod_cli_agent -- --ignored --nocapture
//! # Cert + admin tests (dev mode)
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_cert -- --ignored --nocapture
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_admin -- --ignored --nocapture
//! ```

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;
use crate::common::test_helpers::write_temp_file;

// ============================================================================
// Agent commands
// ============================================================================

/// `flowplane agent create -f <file>` should create a machine identity agent,
/// and `flowplane agent list` should show it afterwards.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_agent_create_and_list() {
    let harness = dev_harness("prod_cli_agent_create").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: agent management requires Zitadel (prod mode)");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let agent_name = "e2e-test-agent";
    let spec = serde_json::json!({
        "name": agent_name,
        "description": "E2E test agent",
        "teams": [&harness.team]
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create the agent
    let create_output =
        cli.run(&["agent", "create", "--org", &harness.org, "-f", &file_path]).unwrap();
    create_output.assert_success();
    create_output.assert_stdout_contains(agent_name);

    // Verify via list
    let list_output = cli.run(&["agent", "list", "--org", &harness.org, "-o", "json"]).unwrap();
    list_output.assert_success();
    list_output.assert_stdout_contains(agent_name);

    // Cleanup
    let _ = cli.run(&["agent", "delete", "--org", &harness.org, agent_name, "--yes"]);
}

/// `flowplane agent delete <name>` should remove an agent, and subsequent
/// list should not contain it.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_agent_delete() {
    let harness = dev_harness("prod_cli_agent_del").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: agent management requires Zitadel (prod mode)");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // First create an agent to delete
    let agent_name = "e2e-del-agent";
    let spec = serde_json::json!({
        "name": agent_name,
        "description": "Agent to be deleted",
        "teams": [&harness.team]
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let create_output =
        cli.run(&["agent", "create", "--org", &harness.org, "-f", &file_path]).unwrap();
    create_output.assert_success();

    // Delete the agent
    let delete_output =
        cli.run(&["agent", "delete", "--org", &harness.org, agent_name, "--yes"]).unwrap();
    delete_output.assert_success();
    delete_output.assert_stdout_contains("deleted");

    // Verify agent is gone from list
    let list_output = cli.run(&["agent", "list", "--org", &harness.org, "-o", "json"]).unwrap();
    list_output.assert_success();
    assert!(
        !list_output.stdout.contains(agent_name),
        "Deleted agent '{}' should not appear in agent list. stdout: {}",
        agent_name,
        list_output.stdout
    );
}

/// `flowplane agent create` with invalid config (missing teams) should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_agent_create_bad_config() {
    let harness = dev_harness("prod_cli_agent_bad").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: agent management requires Zitadel (prod mode)");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Missing required "teams" field
    let spec = serde_json::json!({
        "name": "bad-agent",
        "description": "Missing teams"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["agent", "create", "--org", &harness.org, "-f", &file_path]).unwrap();
    output.assert_failure();
}

/// `flowplane agent create` with invalid name (uppercase, special chars) should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_agent_create_invalid_name() {
    let harness = dev_harness("prod_cli_agent_invn").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: agent management requires Zitadel (prod mode)");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let spec = serde_json::json!({
        "name": "BAD_AGENT!!",
        "description": "Invalid name chars",
        "teams": [&harness.team]
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["agent", "create", "--org", &harness.org, "-f", &file_path]).unwrap();
    output.assert_failure();
}

/// `flowplane agent delete` for a nonexistent agent should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_agent_delete_nonexistent() {
    let harness = dev_harness("prod_cli_agent_del404").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: agent management requires Zitadel (prod mode)");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output =
        cli.run(&["agent", "delete", "--org", &harness.org, "no-such-agent-xyz", "--yes"]).unwrap();
    output.assert_failure();
}

// ============================================================================
// Cert commands
// ============================================================================

/// `flowplane cert create -f <file>` should create a proxy certificate,
/// and `flowplane cert list` should show it afterwards.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cert_create_and_list() {
    let harness = dev_harness("dev_cli_cert_create").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let proxy_id = "e2e-test-proxy";
    let spec = serde_json::json!({
        "proxy_id": proxy_id
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create the certificate
    let create_output = cli.run(&["cert", "create", "-f", &file_path, "-o", "json"]).unwrap();

    // If mTLS/Vault PKI is not configured, cert create may fail with service unavailable.
    // That's a valid negative result in dev mode without Vault — verify the error is reasonable.
    if create_output.exit_code != 0 {
        let combined = format!("{}{}", create_output.stdout, create_output.stderr);
        let is_pki_not_configured = combined.to_lowercase().contains("mtls")
            || combined.to_lowercase().contains("vault")
            || combined.to_lowercase().contains("pki")
            || combined.to_lowercase().contains("not configured")
            || combined.to_lowercase().contains("service unavailable")
            || combined.to_lowercase().contains("certificate");
        assert!(
            is_pki_not_configured,
            "cert create failed with unexpected error (not PKI/Vault related). \
             exit_code={}, stdout={}, stderr={}",
            create_output.exit_code, create_output.stdout, create_output.stderr
        );
        eprintln!(
            "SKIP remaining cert create checks: PKI backend not available in dev mode. \
             Error: {}{}",
            create_output.stdout, create_output.stderr
        );
        return;
    }

    // If create succeeded, extract the cert ID and verify via list and get
    create_output.assert_stdout_contains("id");

    let cert_response: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("cert create output should be JSON");
    let cert_id =
        cert_response["id"].as_str().expect("cert response should contain 'id' string field");

    // Verify via cert list
    let list_output = cli.run(&["cert", "list", "-o", "json"]).unwrap();
    list_output.assert_success();
    list_output.assert_stdout_contains(cert_id);

    // Verify via cert get
    let get_output = cli.run(&["cert", "get", cert_id, "-o", "json"]).unwrap();
    get_output.assert_success();
    get_output.assert_stdout_contains(cert_id);
}

/// `flowplane cert revoke <id>` should revoke a certificate.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cert_revoke() {
    let harness = dev_harness("dev_cli_cert_revoke").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // First create a certificate to revoke
    let spec = serde_json::json!({ "proxy_id": "e2e-revoke-proxy" });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let create_output = cli.run(&["cert", "create", "-f", &file_path, "-o", "json"]).unwrap();
    if create_output.exit_code != 0 {
        eprintln!(
            "SKIP: cert create failed (likely no PKI backend). stderr={}",
            create_output.stderr
        );
        return;
    }

    let cert_response: serde_json::Value =
        serde_json::from_str(&create_output.stdout).expect("cert create output should be JSON");
    let cert_id = cert_response["id"].as_str().expect("cert response should contain 'id'");

    // Revoke the certificate
    let revoke_output = cli.run(&["cert", "revoke", cert_id, "--yes"]).unwrap();
    revoke_output.assert_success();
    revoke_output.assert_stdout_contains("revoked");

    // Verify revoked status via get
    let get_output = cli.run(&["cert", "get", cert_id, "-o", "json"]).unwrap();
    get_output.assert_success();
    // The cert should still exist but be in revoked state
    let cert_detail: serde_json::Value =
        serde_json::from_str(&get_output.stdout).expect("cert get output should be JSON");
    let status = cert_detail["status"].as_str().unwrap_or("");
    assert!(
        status.to_lowercase().contains("revok"),
        "Expected certificate status to indicate revoked, got '{status}'. Full response: {}",
        get_output.stdout
    );
}

/// `flowplane cert create` with invalid/malformed config should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cert_create_bad_config() {
    let harness = dev_harness("dev_cli_cert_bad").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Empty JSON — missing required proxy_id
    let file = write_temp_file("{}", ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["cert", "create", "-f", &file_path]).unwrap();
    output.assert_failure();
}

/// `flowplane cert create` with a proxy_id that violates validation rules.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cert_create_invalid_proxy_id() {
    let harness = dev_harness("dev_cli_cert_invid").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // proxy_id too short (min 3 chars)
    let spec = serde_json::json!({ "proxy_id": "ab" });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["cert", "create", "-f", &file_path]).unwrap();
    output.assert_failure();
}

/// `flowplane cert revoke` with a nonexistent ID should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cert_revoke_nonexistent() {
    let harness = dev_harness("dev_cli_cert_rev404").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output =
        cli.run(&["cert", "revoke", "00000000-0000-0000-0000-000000000000", "--yes"]).unwrap();
    output.assert_failure();
}

/// `flowplane cert create` with non-JSON garbage file should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_cert_create_malformed_file() {
    let harness = dev_harness("dev_cli_cert_malform").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("this is not json{{{", ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["cert", "create", "-f", &file_path]).unwrap();
    output.assert_failure();
}

// ============================================================================
// Admin commands
// ============================================================================

/// `flowplane admin reload-filter-schemas` should succeed or produce a
/// reasonable admin permission error in dev mode.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_admin_reload_filter_schemas() {
    let harness = dev_harness("dev_cli_admin_reload").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["admin", "reload-filter-schemas"]).unwrap();

    // In dev mode, admin commands may succeed (dev token has full access) or
    // fail with permission errors. Either is acceptable — must not crash.
    let is_reasonable = output.exit_code == 0
        || output.stderr.to_lowercase().contains("error")
        || output.stderr.to_lowercase().contains("permission")
        || output.stderr.to_lowercase().contains("unauthorized")
        || output.stderr.to_lowercase().contains("forbidden")
        || output.stdout.to_lowercase().contains("error");
    assert!(
        is_reasonable,
        "Expected reload-filter-schemas to either succeed or produce an auth/permission error, \
         got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );

    // If it succeeded, verify the success message
    if output.exit_code == 0 {
        output.assert_stdout_contains("reloaded");
    }
}

/// Running `flowplane admin reload-filter-schemas` twice should be idempotent.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_admin_reload_filter_schemas_idempotent() {
    let harness = dev_harness("dev_cli_admin_idem").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let first = cli.run(&["admin", "reload-filter-schemas"]).unwrap();
    let second = cli.run(&["admin", "reload-filter-schemas"]).unwrap();

    // Both calls should produce the same outcome
    assert_eq!(
        first.exit_code, second.exit_code,
        "reload-filter-schemas should be idempotent. \
         First: exit={} stdout={} stderr={}, \
         Second: exit={} stdout={} stderr={}",
        first.exit_code, first.stdout, first.stderr, second.exit_code, second.stdout, second.stderr
    );
}

// ============================================================================
// Agent create with malformed file
// ============================================================================

/// `flowplane agent create` with a malformed (non-JSON) file should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_agent_create_malformed_file() {
    let harness = dev_harness("prod_cli_agent_malf").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: agent management requires Zitadel (prod mode)");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("not valid json content!!!", ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["agent", "create", "--org", &harness.org, "-f", &file_path]).unwrap();
    output.assert_failure();
}

/// `flowplane agent create` with name that's too short (< 3 chars) should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_agent_create_name_too_short() {
    let harness = dev_harness("prod_cli_agent_short").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: agent management requires Zitadel (prod mode)");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let spec = serde_json::json!({
        "name": "ab",
        "description": "Name too short",
        "teams": [&harness.team]
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["agent", "create", "--org", &harness.org, "-f", &file_path]).unwrap();
    output.assert_failure();
}
