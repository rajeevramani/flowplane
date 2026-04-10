//! E2E tests for CLI team and org CRUD commands (prod + dev mode).
//!
//! Tests `flowplane team create|list|get|update|delete` and
//! `flowplane org create|list|get|delete|members` via the CLI binary.
//! All mutations verified via subsequent CLI reads.
//!
//! ```bash
//! RUN_E2E=1 cargo test --test e2e cli_team_org -- --ignored --nocapture
//! ```

use crate::common::cli_runner::CliRunner;
use crate::common::harness::dev_harness;
use crate::common::test_helpers::write_temp_file;

// ============================================================================
// Team commands — prod mode
// ============================================================================

/// `flowplane team create --org <org> -f <file>` creates a team,
/// `flowplane team list --org <org>` shows it, and
/// `flowplane team get --org <org> <name>` returns its details.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_create_list_get() {
    let harness = dev_harness("prod_cli_team_clg").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();
    let org = &harness.org;

    let team_name = "e2e-team-clg";
    let spec = serde_json::json!({
        "name": team_name,
        "displayName": "E2E Team CLG",
        "description": "Team for create-list-get test"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create team
    let create_out = cli.run(&["team", "create", "--org", org, "-f", &file_path]).unwrap();
    create_out.assert_success();
    create_out.assert_stdout_contains(team_name);

    // List teams — should contain the new team
    let list_out = cli.run(&["team", "list", "--org", org, "-o", "json"]).unwrap();
    list_out.assert_success();
    list_out.assert_stdout_contains(team_name);

    // Get team
    let get_out = cli.run(&["team", "get", "--org", org, team_name, "-o", "json"]).unwrap();
    get_out.assert_success();
    get_out.assert_stdout_contains(team_name);
    get_out.assert_stdout_contains("E2E Team CLG");

    // Cleanup
    let _ = cli.run(&["team", "delete", "--org", org, team_name, "--yes"]);
}

/// `flowplane team update --org <org> <name> -f <file>` updates a team's fields.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_update() {
    let harness = dev_harness("prod_cli_team_upd").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();
    let org = &harness.org;

    let team_name = "e2e-team-upd";
    let spec = serde_json::json!({
        "name": team_name,
        "displayName": "Before Update",
        "description": "Original description"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create team
    let create_out = cli.run(&["team", "create", "--org", org, "-f", &file_path]).unwrap();
    create_out.assert_success();

    // Update team
    let update_spec = serde_json::json!({
        "displayName": "After Update",
        "description": "Updated description"
    });
    let update_file =
        write_temp_file(&serde_json::to_string_pretty(&update_spec).unwrap(), ".json");
    let update_path = update_file.path().to_str().unwrap().to_string();

    let update_out =
        cli.run(&["team", "update", "--org", org, team_name, "-f", &update_path]).unwrap();
    update_out.assert_success();
    update_out.assert_stdout_contains("After Update");

    // Verify via get
    let get_out = cli.run(&["team", "get", "--org", org, team_name, "-o", "json"]).unwrap();
    get_out.assert_success();
    get_out.assert_stdout_contains("After Update");

    // Cleanup
    let _ = cli.run(&["team", "delete", "--org", org, team_name, "--yes"]);
}

/// `flowplane team delete --org <org> <name> --yes` deletes a team.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_delete() {
    let harness = dev_harness("prod_cli_team_del").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();
    let org = &harness.org;

    let team_name = "e2e-team-del";
    let spec = serde_json::json!({
        "name": team_name,
        "displayName": "Team To Delete",
        "description": "Will be deleted"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create team
    let create_out = cli.run(&["team", "create", "--org", org, "-f", &file_path]).unwrap();
    create_out.assert_success();

    // Delete team
    let delete_out = cli.run(&["team", "delete", "--org", org, team_name, "--yes"]).unwrap();
    delete_out.assert_success();
    delete_out.assert_stdout_contains("deleted");

    // Verify team is gone — get should fail
    let get_out = cli.run(&["team", "get", "--org", org, team_name, "-o", "json"]).unwrap();
    get_out.assert_failure();
}

// ============================================================================
// Team negative tests — prod mode
// ============================================================================

/// Creating a team with a duplicate name should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_create_duplicate() {
    let harness = dev_harness("prod_cli_team_dup").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();
    let org = &harness.org;

    let team_name = "e2e-team-dup";
    let spec = serde_json::json!({
        "name": team_name,
        "displayName": "Duplicate Test",
        "description": "First creation"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create first
    let first = cli.run(&["team", "create", "--org", org, "-f", &file_path]).unwrap();
    first.assert_success();

    // Create duplicate — should fail
    let second = cli.run(&["team", "create", "--org", org, "-f", &file_path]).unwrap();
    second.assert_failure();

    // Cleanup
    let _ = cli.run(&["team", "delete", "--org", org, team_name, "--yes"]);
}

/// Deleting a nonexistent team should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_delete_nonexistent() {
    let harness = dev_harness("prod_cli_team_del404").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output =
        cli.run(&["team", "delete", "--org", &harness.org, "no-such-team-xyz", "--yes"]).unwrap();
    output.assert_failure();
}

/// Updating a nonexistent team should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_update_nonexistent() {
    let harness = dev_harness("prod_cli_team_upd404").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let update_spec = serde_json::json!({
        "displayName": "Ghost Team"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&update_spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli
        .run(&["team", "update", "--org", &harness.org, "no-such-team-xyz", "-f", &file_path])
        .unwrap();
    output.assert_failure();
}

/// Getting a nonexistent team should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_get_nonexistent() {
    let harness = dev_harness("prod_cli_team_get404").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output =
        cli.run(&["team", "get", "--org", &harness.org, "no-such-team-xyz", "-o", "json"]).unwrap();
    output.assert_failure();
}

/// Creating a team with malformed JSON file should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_create_malformed_file() {
    let harness = dev_harness("prod_cli_team_malf").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("this is not valid json{{{", ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["team", "create", "--org", &harness.org, "-f", &file_path]).unwrap();
    output.assert_failure();
}

/// Creating a team with an empty JSON body should fail (missing required name).
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_create_empty_body() {
    let harness = dev_harness("prod_cli_team_empty").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("{}", ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["team", "create", "--org", &harness.org, "-f", &file_path]).unwrap();
    output.assert_failure();
}

// ============================================================================
// Team commands — admin endpoint (list all teams)
// ============================================================================

/// `flowplane team list --admin` should list teams across all orgs.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_team_list_admin() {
    let harness = dev_harness("prod_cli_team_ladm").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["team", "list", "--admin", "-o", "json"]).unwrap();
    output.assert_success();
    // Should contain at least the "teams" key in JSON response
    output.assert_stdout_contains("teams");
}

// ============================================================================
// Org commands — prod mode
// ============================================================================

/// `flowplane org create -f <file>` creates an org,
/// `flowplane org list` shows it, and `flowplane org get <name>` returns details.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_create_list_get() {
    let harness = dev_harness("prod_cli_org_clg").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let org_name = "e2e-org-clg";
    let spec = serde_json::json!({
        "name": org_name,
        "displayName": "E2E Org CLG",
        "description": "Org for create-list-get test"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create org
    let create_out = cli.run(&["org", "create", "-f", &file_path]).unwrap();
    create_out.assert_success();
    create_out.assert_stdout_contains(org_name);

    // List orgs — should contain the new org
    let list_out = cli.run(&["org", "list", "-o", "json"]).unwrap();
    list_out.assert_success();
    list_out.assert_stdout_contains(org_name);

    // Get org
    let get_out = cli.run(&["org", "get", org_name, "-o", "json"]).unwrap();
    get_out.assert_success();
    get_out.assert_stdout_contains(org_name);

    // Cleanup
    let _ = cli.run(&["org", "delete", org_name, "--yes"]);
}

/// `flowplane org delete <name> --yes` deletes an org.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_delete() {
    let harness = dev_harness("prod_cli_org_del").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let org_name = "e2e-org-del";
    let spec = serde_json::json!({
        "name": org_name,
        "displayName": "Org To Delete"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create org
    let create_out = cli.run(&["org", "create", "-f", &file_path]).unwrap();
    create_out.assert_success();

    // Delete org
    let delete_out = cli.run(&["org", "delete", org_name, "--yes"]).unwrap();
    delete_out.assert_success();
    delete_out.assert_stdout_contains("deleted");

    // Verify org is gone — get should fail
    let get_out = cli.run(&["org", "get", org_name, "-o", "json"]).unwrap();
    get_out.assert_failure();
}

/// `flowplane org members <name>` lists members of an org.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_members() {
    let harness = dev_harness("prod_cli_org_memb").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Use the harness org (platform) which should have members
    let output = cli.run(&["org", "members", &harness.org, "-o", "json"]).unwrap();
    output.assert_success();
    // Should contain the "members" key in JSON response
    output.assert_stdout_contains("members");
}

// ============================================================================
// Org negative tests — prod mode
// ============================================================================

/// Creating a duplicate org should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_create_duplicate() {
    let harness = dev_harness("prod_cli_org_dup").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let org_name = "e2e-org-dup";
    let spec = serde_json::json!({
        "name": org_name,
        "displayName": "Duplicate Org"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create first
    let first = cli.run(&["org", "create", "-f", &file_path]).unwrap();
    first.assert_success();

    // Create duplicate — should fail
    let second = cli.run(&["org", "create", "-f", &file_path]).unwrap();
    second.assert_failure();

    // Cleanup
    let _ = cli.run(&["org", "delete", org_name, "--yes"]);
}

/// Deleting a nonexistent org should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_delete_nonexistent() {
    let harness = dev_harness("prod_cli_org_del404").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["org", "delete", "no-such-org-xyz", "--yes"]).unwrap();
    output.assert_failure();
}

/// Getting a nonexistent org should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_get_nonexistent() {
    let harness = dev_harness("prod_cli_org_get404").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["org", "get", "no-such-org-xyz", "-o", "json"]).unwrap();
    output.assert_failure();
}

/// Org members for a nonexistent org should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_members_nonexistent() {
    let harness = dev_harness("prod_cli_org_mem404").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["org", "members", "no-such-org-xyz", "-o", "json"]).unwrap();
    output.assert_failure();
}

/// Creating an org with malformed JSON file should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_create_malformed_file() {
    let harness = dev_harness("prod_cli_org_malf").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("not valid json!!!", ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["org", "create", "-f", &file_path]).unwrap();
    output.assert_failure();
}

/// Creating an org with an empty JSON body should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_create_empty_body() {
    let harness = dev_harness("prod_cli_org_empty").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let file = write_temp_file("{}", ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    let output = cli.run(&["org", "create", "-f", &file_path]).unwrap();
    output.assert_failure();
}

// ============================================================================
// Org + team lifecycle — prod mode
// ============================================================================

/// Full lifecycle: create org → create team in that org → list teams → delete
/// team → delete org. Tests the full org/team dependency chain.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_org_team_lifecycle() {
    let harness = dev_harness("prod_cli_ot_life").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let org_name = "e2e-lifecycle-org";
    let team_name = "e2e-lifecycle-team";

    // Create org
    let org_spec = serde_json::json!({
        "name": org_name,
        "displayName": "Lifecycle Org"
    });
    let org_file = write_temp_file(&serde_json::to_string_pretty(&org_spec).unwrap(), ".json");
    let org_file_path = org_file.path().to_str().unwrap().to_string();

    let create_org = cli.run(&["org", "create", "-f", &org_file_path]).unwrap();
    create_org.assert_success();

    // Create team in that org
    let team_spec = serde_json::json!({
        "name": team_name,
        "displayName": "Lifecycle Team",
        "description": "Team in lifecycle org"
    });
    let team_file = write_temp_file(&serde_json::to_string_pretty(&team_spec).unwrap(), ".json");
    let team_file_path = team_file.path().to_str().unwrap().to_string();

    let create_team =
        cli.run(&["team", "create", "--org", org_name, "-f", &team_file_path]).unwrap();
    create_team.assert_success();

    // List teams in the org
    let list_teams = cli.run(&["team", "list", "--org", org_name, "-o", "json"]).unwrap();
    list_teams.assert_success();
    list_teams.assert_stdout_contains(team_name);

    // Delete team first (org with teams may refuse deletion)
    let del_team = cli.run(&["team", "delete", "--org", org_name, team_name, "--yes"]).unwrap();
    del_team.assert_success();

    // Verify team is gone
    let list_after = cli.run(&["team", "list", "--org", org_name, "-o", "json"]).unwrap();
    list_after.assert_success();
    assert!(
        !list_after.stdout.contains(team_name),
        "Deleted team '{}' should not appear in team list. stdout: {}",
        team_name,
        list_after.stdout
    );

    // Delete org
    let del_org = cli.run(&["org", "delete", org_name, "--yes"]).unwrap();
    del_org.assert_success();

    // Verify org is gone
    let get_org = cli.run(&["org", "get", org_name, "-o", "json"]).unwrap();
    get_org.assert_failure();
}

// ============================================================================
// Dev-mode team/org tests
// ============================================================================

/// Dev mode: `flowplane team list --org <org>` should work with bearer token.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_team_list() {
    let harness = dev_harness("dev_cli_team_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["team", "list", "--org", &harness.org, "-o", "json"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("teams");
}

/// Dev mode: `flowplane team list --admin` should work.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_team_list_admin() {
    let harness = dev_harness("dev_cli_team_ladm").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["team", "list", "--admin", "-o", "json"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("teams");
}

/// Dev mode: `flowplane team create + get + delete` lifecycle.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_team_create_get_delete() {
    let harness = dev_harness("dev_cli_team_cgd").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();
    let org = &harness.org;

    let team_name = "e2e-dev-team";
    let spec = serde_json::json!({
        "name": team_name,
        "displayName": "Dev Team",
        "description": "Team in dev mode"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create
    let create_out = cli.run(&["team", "create", "--org", org, "-f", &file_path]).unwrap();
    create_out.assert_success();
    create_out.assert_stdout_contains(team_name);

    // Get
    let get_out = cli.run(&["team", "get", "--org", org, team_name, "-o", "json"]).unwrap();
    get_out.assert_success();
    get_out.assert_stdout_contains(team_name);

    // Delete
    let del_out = cli.run(&["team", "delete", "--org", org, team_name, "--yes"]).unwrap();
    del_out.assert_success();

    // Verify gone
    let get_after = cli.run(&["team", "get", "--org", org, team_name, "-o", "json"]).unwrap();
    get_after.assert_failure();
}

/// Dev mode: `flowplane org list` should work.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_org_list() {
    let harness = dev_harness("dev_cli_org_list").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["org", "list", "-o", "json"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("organizations");
}

/// Dev mode: `flowplane org get <org>` should return the dev org.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_org_get() {
    let harness = dev_harness("dev_cli_org_get").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["org", "get", &harness.org, "-o", "json"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains(&harness.org);
}

/// Dev mode: `flowplane org create + delete` lifecycle.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_org_create_delete() {
    let harness = dev_harness("dev_cli_org_cd").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let org_name = "e2e-dev-org";
    let spec = serde_json::json!({
        "name": org_name,
        "displayName": "Dev Org"
    });
    let file = write_temp_file(&serde_json::to_string_pretty(&spec).unwrap(), ".json");
    let file_path = file.path().to_str().unwrap().to_string();

    // Create
    let create_out = cli.run(&["org", "create", "-f", &file_path]).unwrap();
    create_out.assert_success();
    create_out.assert_stdout_contains(org_name);

    // Verify via list
    let list_out = cli.run(&["org", "list", "-o", "json"]).unwrap();
    list_out.assert_success();
    list_out.assert_stdout_contains(org_name);

    // Delete
    let del_out = cli.run(&["org", "delete", org_name, "--yes"]).unwrap();
    del_out.assert_success();

    // Verify gone
    let get_out = cli.run(&["org", "get", org_name, "-o", "json"]).unwrap();
    get_out.assert_failure();
}

/// Dev mode: `flowplane org members <org>` should work.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_org_members() {
    let harness = dev_harness("dev_cli_org_memb").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["org", "members", &harness.org, "-o", "json"]).unwrap();
    output.assert_success();
    output.assert_stdout_contains("members");
}
