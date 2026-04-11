//! Negative / error-handling tests for CLI commands that lack them elsewhere.
//!
//! Each test exercises ONE error path per command — the most impactful one.
//! All tests use `dev_harness()` (no Envoy needed for error handling).
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_neg_ -- --ignored --nocapture
//! ```

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;
use crate::common::test_helpers::write_temp_file;

// ============================================================================
// GET nonexistent resource
// ============================================================================

/// cluster get with a nonexistent cluster name should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_cluster_get_nonexistent() {
    let harness = dev_harness("neg_clus_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["cluster", "get", "no-such-cluster-xyz-999"]).unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error =
        output.exit_code != 0 || combined.contains("not found") || combined.contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent cluster, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// listener get with a nonexistent listener name should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_listener_get_nonexistent() {
    let harness = dev_harness("neg_lst_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["listener", "get", "no-such-listener-xyz-999"]).unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error =
        output.exit_code != 0 || combined.contains("not found") || combined.contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent listener, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// route get with a nonexistent route name should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_route_get_nonexistent() {
    let harness = dev_harness("neg_rte_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["route", "get", "no-such-route-xyz-999"]).unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error =
        output.exit_code != 0 || combined.contains("not found") || combined.contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent route, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

/// filter get with a nonexistent filter name should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_filter_get_nonexistent() {
    let harness = dev_harness("neg_flt_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["filter", "get", "no-such-filter-xyz-999"]).unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error =
        output.exit_code != 0 || combined.contains("not found") || combined.contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent filter, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// filter attach — nonexistent filter and listener
// ============================================================================

/// filter attach with a nonexistent filter name should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_filter_attach_nonexistent() {
    let harness = dev_harness("neg_flt_att").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli
        .run(&["filter", "attach", "ghost-filter-xyz-999", "--listener", "ghost-listener-xyz-999"])
        .unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error =
        output.exit_code != 0 || combined.contains("not found") || combined.contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent filter attach, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// scaffold — invalid type (only filter scaffold has a required type arg)
// ============================================================================

/// filter scaffold with an invalid filter type should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_filter_scaffold_invalid_type() {
    let harness = dev_harness("neg_flt_scaf").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["filter", "scaffold", "totally_bogus_filter_type_xyz"]).unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error = output.exit_code != 0
        || combined.contains("unknown")
        || combined.contains("unsupported")
        || combined.contains("invalid")
        || combined.contains("error");
    assert!(
        has_error,
        "Expected error for invalid filter scaffold type, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// create -f with malformed YAML (filter create — not yet covered)
// ============================================================================

/// filter create with malformed YAML should fail without panicking.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_filter_create_malformed_yaml() {
    let harness = dev_harness("neg_flt_crt").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let bad_yaml = "name: [unterminated\n  broken: {yaml: content";
    let file = write_temp_file(bad_yaml, ".yaml");

    let output = cli.run(&["filter", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Expected non-zero exit for malformed YAML, got stdout={}, stderr={}",
        output.stdout, output.stderr
    );
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    assert!(!combined.contains("panicked at"), "CLI panicked on malformed input: {}", combined);
}

// ============================================================================
// expose / unexpose
// ============================================================================

/// expose with no upstream argument should fail (clap validation).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_expose_missing_upstream() {
    let harness = dev_harness("neg_expose").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["expose"]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Expected non-zero exit for expose with no upstream, got stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

/// unexpose with a nonexistent service name should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_unexpose_nonexistent() {
    let harness = dev_harness("neg_unexpose").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["unexpose", "no-such-service-xyz-999"]).unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error =
        output.exit_code != 0 || combined.contains("not found") || combined.contains("error");
    assert!(
        has_error,
        "Expected error for nonexistent unexpose, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// validate — bad config file
// ============================================================================

/// validate with a nonexistent config file via -f should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_validate_bad_config() {
    let harness = dev_harness("neg_validate").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // validate without -f just validates current server config — it may pass or
    // fail depending on state. With -f pointing to garbage, it should fail.
    let bad_yaml = "this is not: [valid flowplane config\n  broken: {";
    let file = write_temp_file(bad_yaml, ".yaml");

    let output = cli.run(&["validate", "-f", file.path().to_str().unwrap()]).unwrap();

    // validate -f may not exist as a flag — in that case clap rejects it
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error = output.exit_code != 0
        || combined.contains("error")
        || combined.contains("invalid")
        || combined.contains("unexpected");
    assert!(
        has_error,
        "Expected error for validate with bad config, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// xds nacks — invalid --type value
// ============================================================================

/// xds nacks with an invalid resource type should fail (clap value_parser).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_xds_nacks_invalid_type() {
    let harness = dev_harness("neg_xds_nack").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["xds", "nacks", "--type", "BOGUS_TYPE"]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Expected non-zero exit for invalid xds nacks type, got stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

// ============================================================================
// audit list — invalid --action value
// ============================================================================

/// audit list with an invalid action should fail (clap value_parser).
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_audit_list_invalid_action() {
    let harness = dev_harness("neg_audit").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["audit", "list", "--action", "not_a_real_action"]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Expected non-zero exit for invalid audit action, got stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

// ============================================================================
// route create -f — malformed YAML (not yet covered unlike cluster/listener/route)
// ============================================================================

/// route create with bad file extension should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_route_create_bad_extension() {
    let harness = dev_harness("neg_rte_ext").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("name: test-route", ".txt");

    let output = cli.run(&["route", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error = output.exit_code != 0
        || combined.contains("extension")
        || combined.contains("unsupported")
        || combined.contains("yaml")
        || combined.contains("json")
        || combined.contains("error");
    assert!(
        has_error,
        "Expected error for .txt extension, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// listener create -f — bad extension (malformed yaml already tested in test_cli_scaffold)
// ============================================================================

/// listener create with bad file extension should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_listener_create_bad_extension() {
    let harness = dev_harness("neg_lst_ext").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("name: test-listener", ".txt");

    let output = cli.run(&["listener", "create", "-f", file.path().to_str().unwrap()]).unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error = output.exit_code != 0
        || combined.contains("extension")
        || combined.contains("unsupported")
        || combined.contains("yaml")
        || combined.contains("json")
        || combined.contains("error");
    assert!(
        has_error,
        "Expected error for .txt extension, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}

// ============================================================================
// filter create -f — nonexistent file
// ============================================================================

/// filter create with a nonexistent file should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_filter_create_nonexistent_file() {
    let harness = dev_harness("neg_flt_nofile").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output =
        cli.run(&["filter", "create", "-f", "/tmp/this-file-does-not-exist-xyz.yaml"]).unwrap();
    assert_ne!(
        output.exit_code, 0,
        "Expected non-zero exit for nonexistent file, got stdout={}, stderr={}",
        output.stdout, output.stderr
    );
}

// ============================================================================
// apply -f — bad file extension (malformed/missing-name/nonexistent covered in test_cli_scaffold)
// ============================================================================

/// apply with bad file extension should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_neg_apply_bad_extension() {
    let harness = dev_harness("neg_apply_ext").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("kind: Cluster\nname: test", ".txt");

    let output = cli.run(&["apply", "-f", file.path().to_str().unwrap()]).unwrap();
    let combined = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    let has_error = output.exit_code != 0
        || combined.contains("extension")
        || combined.contains("unsupported")
        || combined.contains("yaml")
        || combined.contains("json")
        || combined.contains("error");
    assert!(
        has_error,
        "Expected error for .txt extension, got exit_code={}, stdout={}, stderr={}",
        output.exit_code, output.stdout, output.stderr
    );
}
