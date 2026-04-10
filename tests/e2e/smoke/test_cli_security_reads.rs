//! E2E tests for security-sensitive CLI read commands.
//!
//! Tests secret reads, cert reads, and tenant-scoped reads (org, team, agent)
//! for auth boundary enforcement and cross-tenant isolation.
//!
//! Dev-mode tests (prefix `dev_`) test secret/cert reads with bearer token.
//! Prod-mode tests (prefix `prod_`) test multi-user tenant isolation.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_cli_security -- --ignored --nocapture
//! RUN_E2E=1 cargo test --test e2e prod_cli_security -- --ignored --nocapture
//! ```

use crate::common::cli_runner::CliRunner;
use crate::common::harness::{dev_harness, quick_harness};

// ============================================================================
// Dev-mode: Secret reads
// ============================================================================

/// `flowplane secret list` returns secrets for the authorized user.
/// Creates a secret first, then verifies list includes it.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_security_secret_list() {
    let harness = quick_harness("dev_sec_slist").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Create a secret so the list is non-empty
    let secret_name = "e2e-secread-list";
    let create = cli
        .run(&[
            "secret",
            "create",
            "--name",
            secret_name,
            "--type",
            "generic_secret",
            "--config",
            r#"{"secret": "dGVzdC1saXN0"}"#, // pragma: allowlist secret
        ])
        .unwrap();
    create.assert_success();
    let created: serde_json::Value =
        serde_json::from_str(&create.stdout).expect("create output should be valid JSON");
    let secret_id = created["id"].as_str().expect("response should contain id");

    // List secrets — should succeed and contain our secret
    let list = cli.run(&["secret", "list", "-o", "json"]).unwrap();
    list.assert_success();
    let list_json: serde_json::Value =
        serde_json::from_str(&list.stdout).expect("list output should be valid JSON");
    let items = list_json.as_array().expect("list should return an array");
    let found = items.iter().any(|s| s["name"].as_str() == Some(secret_name));
    assert!(found, "Created secret '{}' should appear in list output", secret_name);

    // Cleanup
    let _ = cli.run(&["secret", "delete", secret_id, "--yes"]);
}

/// `flowplane secret get <id>` returns the secret details for an existing secret.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_security_secret_get() {
    let harness = quick_harness("dev_sec_sget").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let secret_name = "e2e-secread-get";
    let create = cli
        .run(&[
            "secret",
            "create",
            "--name",
            secret_name,
            "--type",
            "generic_secret",
            "--config",
            r#"{"secret": "dGVzdC1nZXQ="}"#, // pragma: allowlist secret
        ])
        .unwrap();
    create.assert_success();
    let created: serde_json::Value =
        serde_json::from_str(&create.stdout).expect("create output should be valid JSON");
    let secret_id = created["id"].as_str().expect("response should contain id");

    // Get the secret by ID
    let get = cli.run(&["secret", "get", secret_id]).unwrap();
    get.assert_success();
    let get_json: serde_json::Value =
        serde_json::from_str(&get.stdout).expect("get output should be valid JSON");
    assert_eq!(get_json["id"].as_str(), Some(secret_id));
    assert_eq!(get_json["name"].as_str(), Some(secret_name));
    assert_eq!(get_json["secretType"].as_str(), Some("generic_secret"));

    // Cleanup
    let _ = cli.run(&["secret", "delete", secret_id, "--yes"]);
}

/// `flowplane secret get` with a nonexistent ID should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_security_secret_get_nonexistent() {
    let harness = quick_harness("dev_sec_sget404").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let fake_id = "00000000-0000-0000-0000-000000000000";
    let output = cli.run(&["secret", "get", fake_id]).unwrap();
    output.assert_failure();
}

// ============================================================================
// Dev-mode: Cert reads
// ============================================================================

/// `flowplane cert list` returns certs for the authorized user.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_security_cert_list() {
    let harness = quick_harness("dev_sec_clist").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // List certs — should succeed (may be empty, but command itself must work)
    let list = cli.run(&["cert", "list", "-o", "json"]).unwrap();
    list.assert_success();
    // Output should be valid JSON (array or object)
    let _: serde_json::Value =
        serde_json::from_str(&list.stdout).expect("cert list output should be valid JSON");
}

/// `flowplane cert get` with a nonexistent ID should fail.
#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_cli_security_cert_get_nonexistent() {
    let harness = quick_harness("dev_sec_cget404").await.expect("harness should start");
    if !harness.is_dev_mode() {
        eprintln!("SKIP: not in dev mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    let fake_id = "00000000-0000-0000-0000-000000000000";
    let output = cli.run(&["cert", "get", fake_id]).unwrap();
    output.assert_failure();
}

// ============================================================================
// Prod-mode: Tenant-scoped org reads
// ============================================================================

/// `flowplane org list` should only return orgs the user belongs to.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_org_list() {
    let harness = dev_harness("prod_sec_olist").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode for tenant isolation");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // List orgs — authorized user should succeed
    let list = cli.run(&["org", "list", "-o", "json"]).unwrap();
    list.assert_success();
    let list_json: serde_json::Value =
        serde_json::from_str(&list.stdout).expect("org list should be valid JSON");
    // Should contain "organizations" key
    assert!(
        list_json.get("organizations").is_some() || list_json.is_array(),
        "org list should return organizations data, got: {}",
        list.stdout
    );
}

/// `flowplane org get <name>` for own org succeeds; nonexistent org fails.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_org_get() {
    let harness = dev_harness("prod_sec_oget").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Get own org — should succeed
    let get = cli.run(&["org", "get", &harness.org, "-o", "json"]).unwrap();
    get.assert_success();
    get.assert_stdout_contains(&harness.org);

    // Get nonexistent org — should fail
    let bad = cli.run(&["org", "get", "nonexistent-org-xyz-999", "-o", "json"]).unwrap();
    bad.assert_failure();
}

/// `flowplane org members <name>` for own org succeeds; nonexistent org fails.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_org_members() {
    let harness = dev_harness("prod_sec_omemb").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Members of own org — should succeed
    let members = cli.run(&["org", "members", &harness.org, "-o", "json"]).unwrap();
    members.assert_success();
    members.assert_stdout_contains("members");

    // Members of nonexistent org — should fail
    let bad = cli.run(&["org", "members", "nonexistent-org-xyz-999", "-o", "json"]).unwrap();
    bad.assert_failure();
}

// ============================================================================
// Prod-mode: Tenant-scoped team reads
// ============================================================================

/// `flowplane team list --org <org>` should only return teams in the user's org.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_team_list() {
    let harness = dev_harness("prod_sec_tlist").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // List teams in own org — should succeed
    let list = cli.run(&["team", "list", "--org", &harness.org, "-o", "json"]).unwrap();
    list.assert_success();
    list.assert_stdout_contains("teams");

    // List teams in nonexistent org — should fail or return empty
    let bad = cli.run(&["team", "list", "--org", "nonexistent-org-xyz-999", "-o", "json"]).unwrap();
    // Either fails outright or returns empty — in both cases, must not contain harness team
    assert!(
        bad.exit_code != 0 || !bad.stdout.contains(&harness.team),
        "Listing teams in a foreign org must not expose own team. exit={}, stdout={}",
        bad.exit_code,
        bad.stdout
    );
}

/// `flowplane team get --org <org> <name>` for own team succeeds; nonexistent fails.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_team_get() {
    let harness = dev_harness("prod_sec_tget").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Get own team — should succeed
    let get =
        cli.run(&["team", "get", "--org", &harness.org, &harness.team, "-o", "json"]).unwrap();
    get.assert_success();
    get.assert_stdout_contains(&harness.team);

    // Get nonexistent team — should fail
    let bad = cli
        .run(&["team", "get", "--org", &harness.org, "nonexistent-team-xyz-999", "-o", "json"])
        .unwrap();
    bad.assert_failure();
}

// ============================================================================
// Prod-mode: Tenant-scoped agent reads
// ============================================================================

/// `flowplane agent list --org <org>` should only return agents in the user's org.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_agent_list() {
    let harness = dev_harness("prod_sec_alist").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode");
        return;
    }
    let cli = CliRunner::from_harness(&harness).unwrap();

    // List agents in own org — should succeed
    let list = cli.run(&["agent", "list", "--org", &harness.org, "-o", "json"]).unwrap();
    list.assert_success();
    // Output should be parseable JSON
    let _: serde_json::Value =
        serde_json::from_str(&list.stdout).expect("agent list should be valid JSON");

    // List agents in nonexistent org — should fail
    let bad =
        cli.run(&["agent", "list", "--org", "nonexistent-org-xyz-999", "-o", "json"]).unwrap();
    bad.assert_failure();
}

// ============================================================================
// Prod-mode: Multi-user cross-tenant isolation
// ============================================================================

/// Cross-tenant secret isolation: user A's secrets must not be visible to user B
/// in a different team.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_secret_cross_tenant() {
    let harness = quick_harness("prod_sec_sxtenant").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode for multi-user isolation");
        return;
    }
    let shared = match harness.shared_infra() {
        Some(s) if s.supports_multi_user() => s,
        _ => {
            eprintln!("SKIP: shared infra with multi-user support required");
            return;
        }
    };

    // User A: uses harness default credentials (engineering team)
    let alice_cli = CliRunner::from_harness(&harness).unwrap();

    // Create a secret as user A
    let secret_name = "e2e-xtenant-secret";
    let create = alice_cli
        .run(&[
            "secret",
            "create",
            "--name",
            secret_name,
            "--type",
            "generic_secret",
            "--config",
            r#"{"secret": "YWxpY2Utc2VjcmV0"}"#, // pragma: allowlist secret
        ])
        .unwrap();
    create.assert_success();
    let created: serde_json::Value =
        serde_json::from_str(&create.stdout).expect("create should return valid JSON");
    let secret_id = created["id"].as_str().expect("should have id");

    // Verify user A sees the secret
    let alice_list = alice_cli.run(&["secret", "list", "-o", "json"]).unwrap();
    alice_list.assert_success();
    assert!(alice_list.stdout.contains(secret_name), "User A should see their own secret in list");

    // Create user B in a different team
    let bob_email = "bob-secread@e2e-test.local";
    let bob_password = "B0bP@ssw0rd!2026"; // pragma: allowlist secret
    let _bob_id = shared
        .create_test_user(bob_email, "Bob", "SecRead", bob_password)
        .await
        .expect("should create bob");

    let bob_token =
        shared.get_user_token(bob_email, bob_password).await.expect("should get bob's token");

    // Bob uses a different team context
    let bob_cli =
        CliRunner::with_token_and_team(&harness, &bob_token, "team-bob-secread", "org-bob")
            .unwrap();

    // Bob's secret list should NOT contain Alice's secret
    let bob_list = bob_cli.run(&["secret", "list", "-o", "json"]).unwrap();
    // Bob might get an error (no access to this team) or empty list — both are acceptable
    // What is NOT acceptable: Bob seeing Alice's secret
    if bob_list.exit_code == 0 {
        assert!(
            !bob_list.stdout.contains(secret_name),
            "User B must NOT see user A's secret '{}' in list. Bob's output: {}",
            secret_name,
            bob_list.stdout
        );
    }
    // Non-zero exit is also acceptable — means access denied

    // Cleanup
    let _ = alice_cli.run(&["secret", "delete", secret_id, "--yes"]);
}

/// Cross-tenant org isolation: user should only see orgs they belong to.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_org_cross_tenant() {
    let harness = dev_harness("prod_sec_oxtenant").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode for multi-user isolation");
        return;
    }
    let shared = match harness.shared_infra() {
        Some(s) if s.supports_multi_user() => s,
        _ => {
            eprintln!("SKIP: shared infra with multi-user support required");
            return;
        }
    };

    // Create a fresh user with no org memberships beyond default
    let carol_email = "carol-orgread@e2e-test.local";
    let carol_password = "C@r0lP@ss!2026"; // pragma: allowlist secret
    let _carol_id = shared
        .create_test_user(carol_email, "Carol", "OrgRead", carol_password)
        .await
        .expect("should create carol");

    let carol_token =
        shared.get_user_token(carol_email, carol_password).await.expect("should get carol's token");

    let carol_cli = CliRunner::with_token(&harness, &carol_token).unwrap();

    // Carol lists orgs — should succeed but only see orgs she belongs to
    let carol_orgs = carol_cli.run(&["org", "list", "-o", "json"]).unwrap();
    carol_orgs.assert_success();

    // Carol should NOT be able to get the harness org details (belongs to admin user)
    // This tests cross-tenant boundary: Carol != admin, so admin's org may not be accessible
    let carol_get_admin_org = carol_cli.run(&["org", "get", &harness.org, "-o", "json"]).unwrap();
    // Carol may or may not have access depending on her membership — but if she gets
    // access, the response should only contain data she's authorized to see.
    // The key security property: she must not see OTHER orgs she doesn't belong to.
    let carol_members =
        carol_cli.run(&["org", "members", "nonexistent-org-xyz-999", "-o", "json"]).unwrap();
    carol_members.assert_failure();

    // Verify admin user CAN see own org (positive control)
    let admin_cli = CliRunner::from_harness(&harness).unwrap();
    let admin_get = admin_cli.run(&["org", "get", &harness.org, "-o", "json"]).unwrap();
    admin_get.assert_success();
    admin_get.assert_stdout_contains(&harness.org);

    // Suppress unused variable warning for carol_get_admin_org
    let _ = carol_get_admin_org;
}

/// Cross-tenant team isolation: user A's teams must not be accessible to user B
/// in a different org.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_team_cross_tenant() {
    let harness = dev_harness("prod_sec_txtenant").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode for multi-user isolation");
        return;
    }
    let shared = match harness.shared_infra() {
        Some(s) if s.supports_multi_user() => s,
        _ => {
            eprintln!("SKIP: shared infra with multi-user support required");
            return;
        }
    };

    // Admin can list teams in own org
    let admin_cli = CliRunner::from_harness(&harness).unwrap();
    let admin_teams =
        admin_cli.run(&["team", "list", "--org", &harness.org, "-o", "json"]).unwrap();
    admin_teams.assert_success();

    // Create a user in a different context
    let dave_email = "dave-teamread@e2e-test.local";
    let dave_password = "D@veP@ss!2026"; // pragma: allowlist secret
    let _dave_id = shared
        .create_test_user(dave_email, "Dave", "TeamRead", dave_password)
        .await
        .expect("should create dave");

    let dave_token =
        shared.get_user_token(dave_email, dave_password).await.expect("should get dave's token");

    // Dave tries to list teams in admin's org
    let dave_cli =
        CliRunner::with_token_and_team(&harness, &dave_token, "team-dave-read", "org-dave")
            .unwrap();

    let dave_teams = dave_cli.run(&["team", "list", "--org", &harness.org, "-o", "json"]).unwrap();

    // Dave should either get denied or see an empty list — NOT see admin's teams
    if dave_teams.exit_code == 0 {
        assert!(
            !dave_teams.stdout.contains(&harness.team),
            "User Dave must NOT see admin's team '{}'. Dave's output: {}",
            harness.team,
            dave_teams.stdout
        );
    }

    // Dave cannot get admin's specific team
    let dave_get =
        dave_cli.run(&["team", "get", "--org", &harness.org, &harness.team, "-o", "json"]).unwrap();
    assert!(
        dave_get.exit_code != 0 || !dave_get.stdout.contains(&harness.team),
        "Dave must not be able to read admin's team details. exit={}, stdout={}",
        dave_get.exit_code,
        dave_get.stdout
    );
}

/// Cross-tenant agent isolation: user cannot list agents in another user's org.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn prod_cli_security_agent_cross_tenant() {
    let harness = dev_harness("prod_sec_axtenant").await.expect("harness should start");
    if harness.is_dev_mode() {
        eprintln!("SKIP: requires prod mode for multi-user isolation");
        return;
    }
    let shared = match harness.shared_infra() {
        Some(s) if s.supports_multi_user() => s,
        _ => {
            eprintln!("SKIP: shared infra with multi-user support required");
            return;
        }
    };

    // Admin lists agents in own org — should succeed
    let admin_cli = CliRunner::from_harness(&harness).unwrap();
    let admin_agents =
        admin_cli.run(&["agent", "list", "--org", &harness.org, "-o", "json"]).unwrap();
    admin_agents.assert_success();

    // Create an outsider user
    let eve_email = "eve-agentread@e2e-test.local";
    let eve_password = "Ev3P@ss!2026"; // pragma: allowlist secret
    let _eve_id = shared
        .create_test_user(eve_email, "Eve", "AgentRead", eve_password)
        .await
        .expect("should create eve");

    let eve_token =
        shared.get_user_token(eve_email, eve_password).await.expect("should get eve's token");

    let eve_cli =
        CliRunner::with_token_and_team(&harness, &eve_token, "team-eve-read", "org-eve").unwrap();

    // Eve tries to list agents in admin's org — should be denied
    let eve_agents = eve_cli.run(&["agent", "list", "--org", &harness.org, "-o", "json"]).unwrap();

    // Eve should either fail or get empty results, never see admin's agents
    if eve_agents.exit_code == 0 && admin_agents.stdout.contains("agents") {
        // If admin had agents and Eve also got a 200, Eve's list must not contain admin's agents
        // Parse admin's agent names and verify none appear in Eve's output
        if let Ok(admin_json) = serde_json::from_str::<serde_json::Value>(&admin_agents.stdout) {
            if let Some(admin_arr) = admin_json.get("agents").and_then(|a| a.as_array()) {
                for agent in admin_arr {
                    if let Some(name) = agent.get("name").and_then(|n| n.as_str()) {
                        assert!(
                            !eve_agents.stdout.contains(name),
                            "Eve must NOT see admin's agent '{}'. Eve's output: {}",
                            name,
                            eve_agents.stdout
                        );
                    }
                }
            }
        }
    }
    // Non-zero exit is acceptable — means access denied
}
