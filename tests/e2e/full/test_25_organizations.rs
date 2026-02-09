//! Organization E2E Tests (test_25)
//!
//! Tests organization CRUD, membership management, and org-scoped operations:
//! - Organization creation, listing, updating
//! - Organization membership management
//! - Org-scoped team creation and listing
//! - Bootstrap default org verification
//! - Organization deletion
//! - Cross-org isolation

use serde_json::json;

use crate::common::{
    api_client::{ApiClient, TEST_EMAIL, TEST_NAME, TEST_PASSWORD},
    harness::{TestHarness, TestHarnessConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Test that bootstrap creates a default organization and login returns org context
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2500_default_org_from_bootstrap() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_2500_default_org_from_bootstrap").isolated().without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap
    let bootstrap = with_timeout(TestTimeout::default_with_label("Bootstrap"), async {
        api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME).await
    })
    .await
    .expect("Bootstrap should succeed");
    assert!(bootstrap.setup_token.starts_with("fp_setup_"));

    // Login and verify org context
    let (session, login_resp) = with_timeout(TestTimeout::default_with_label("Login"), async {
        api.login_full(TEST_EMAIL, TEST_PASSWORD).await
    })
    .await
    .expect("Login should succeed");

    assert!(login_resp.org_id.is_some(), "Login should include org_id");
    assert_eq!(login_resp.org_name.as_deref(), Some("default"));
    println!("ok Login includes org context: org={:?}", login_resp.org_name);

    // Create admin token for API calls
    let token_resp = with_timeout(TestTimeout::default_with_label("Create admin token"), async {
        api.create_token(&session, "org-test-token", vec!["admin:all".to_string()]).await
    })
    .await
    .expect("Token creation should succeed");

    // List organizations - should see default org
    let orgs = with_timeout(TestTimeout::default_with_label("List organizations"), async {
        api.list_organizations(&token_resp.token).await
    })
    .await
    .expect("List organizations should succeed");

    assert!(orgs.total >= 1, "Should have at least 1 org");
    let default_org =
        orgs.items.iter().find(|o| o.name == "default").expect("Default org should exist");
    println!("ok Default org found: id={}", default_org.id);

    // Get current org
    let current = with_timeout(TestTimeout::default_with_label("Get current org"), async {
        api.get_current_org(&session).await
    })
    .await
    .expect("Get current org should succeed");

    assert_eq!(current.organization.name, "default");
    assert_eq!(current.role, "owner");
    println!("ok Current org: {} (role={})", current.organization.name, current.role);
}

/// Test creating a new organization
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2501_create_organization() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_2501_create_organization").isolated().without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap and login
    with_timeout(TestTimeout::default_with_label("Bootstrap"), async {
        api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME).await
    })
    .await
    .expect("Bootstrap should succeed");

    let session = with_timeout(TestTimeout::default_with_label("Login"), async {
        api.login(TEST_EMAIL, TEST_PASSWORD).await
    })
    .await
    .expect("Login should succeed");

    let token_resp = with_timeout(TestTimeout::default_with_label("Create admin token"), async {
        api.create_token(&session, "org-create-token", vec!["admin:all".to_string()]).await
    })
    .await
    .expect("Token creation should succeed");

    let admin_token = &token_resp.token;

    // Create a new organization
    let new_org = with_timeout(TestTimeout::default_with_label("Create organization"), async {
        api.create_organization(
            admin_token,
            "test-org-alpha",
            "Test Org Alpha",
            Some("An organization for E2E testing"),
        )
        .await
    })
    .await
    .expect("Create organization should succeed");

    assert_eq!(new_org.name, "test-org-alpha");
    assert_eq!(new_org.display_name, "Test Org Alpha");
    assert!(!new_org.id.is_empty(), "Org should have a valid ID");
    println!("ok Organization created: {} (id={})", new_org.name, new_org.id);

    // List organizations - should see both default and test-org-alpha
    let orgs = with_timeout(TestTimeout::default_with_label("List organizations"), async {
        api.list_organizations(admin_token).await
    })
    .await
    .expect("List organizations should succeed");

    assert!(orgs.total >= 2, "Should have at least 2 orgs, got {}", orgs.total);
    let org_names: Vec<&str> = orgs.items.iter().map(|o| o.name.as_str()).collect();
    assert!(org_names.contains(&"default"), "Should contain default org");
    assert!(org_names.contains(&"test-org-alpha"), "Should contain test-org-alpha");
    println!("ok Listed {} organizations: {:?}", orgs.total, org_names);
}

/// Test getting and updating an organization
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2502_get_and_update_organization() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_2502_get_and_update_organization").isolated().without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap and login
    with_timeout(TestTimeout::default_with_label("Bootstrap"), async {
        api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME).await
    })
    .await
    .expect("Bootstrap should succeed");

    let session = with_timeout(TestTimeout::default_with_label("Login"), async {
        api.login(TEST_EMAIL, TEST_PASSWORD).await
    })
    .await
    .expect("Login should succeed");

    let token_resp = with_timeout(TestTimeout::default_with_label("Create admin token"), async {
        api.create_token(&session, "org-update-token", vec!["admin:all".to_string()]).await
    })
    .await
    .expect("Token creation should succeed");

    let admin_token = &token_resp.token;

    // Create a test org
    let created_org = with_timeout(TestTimeout::default_with_label("Create organization"), async {
        api.create_organization(
            admin_token,
            "test-org-beta",
            "Test Org Beta",
            Some("Original description"),
        )
        .await
    })
    .await
    .expect("Create organization should succeed");

    println!("ok Created org: {} (id={})", created_org.name, created_org.id);

    // Get org by ID
    let fetched_org = with_timeout(TestTimeout::default_with_label("Get organization"), async {
        api.get_organization(admin_token, &created_org.id).await
    })
    .await
    .expect("Get organization should succeed");

    assert_eq!(fetched_org.id, created_org.id);
    assert_eq!(fetched_org.name, "test-org-beta");
    assert_eq!(fetched_org.display_name, "Test Org Beta");
    println!("ok Fetched org: {} (display={})", fetched_org.name, fetched_org.display_name);

    // Update org
    let updated_org = with_timeout(TestTimeout::default_with_label("Update organization"), async {
        api.update_organization(
            admin_token,
            &created_org.id,
            Some("Updated Beta Org"),
            Some("Updated description for E2E"),
        )
        .await
    })
    .await
    .expect("Update organization should succeed");

    assert_eq!(updated_org.display_name, "Updated Beta Org");
    println!("ok Updated org display_name: {}", updated_org.display_name);

    // Verify update persisted
    let verified_org = with_timeout(TestTimeout::default_with_label("Verify update"), async {
        api.get_organization(admin_token, &created_org.id).await
    })
    .await
    .expect("Get organization after update should succeed");

    assert_eq!(verified_org.display_name, "Updated Beta Org");
    assert_eq!(verified_org.description.as_deref(), Some("Updated description for E2E"));
    println!(
        "ok Update verified: display={}, desc={:?}",
        verified_org.display_name, verified_org.description
    );
}

/// Test organization membership lifecycle: add, list, update role, remove
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2503_org_membership_lifecycle() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_2503_org_membership_lifecycle").isolated().without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap and login
    with_timeout(TestTimeout::default_with_label("Bootstrap"), async {
        api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME).await
    })
    .await
    .expect("Bootstrap should succeed");

    let session = with_timeout(TestTimeout::default_with_label("Login"), async {
        api.login(TEST_EMAIL, TEST_PASSWORD).await
    })
    .await
    .expect("Login should succeed");

    let token_resp = with_timeout(TestTimeout::default_with_label("Create admin token"), async {
        api.create_token(&session, "org-membership-token", vec!["admin:all".to_string()]).await
    })
    .await
    .expect("Token creation should succeed");

    let admin_token = &token_resp.token;

    // Create a test org
    let test_org = with_timeout(TestTimeout::default_with_label("Create organization"), async {
        api.create_organization(admin_token, "test-org-members", "Test Org Members", None).await
    })
    .await
    .expect("Create organization should succeed");

    println!("ok Created org: {} (id={})", test_org.name, test_org.id);

    // List initial members - may have the admin as owner
    let initial_members =
        with_timeout(TestTimeout::default_with_label("List initial members"), async {
            api.list_org_members(admin_token, &test_org.id).await
        })
        .await
        .expect("List org members should succeed");

    println!("ok Initial members count: {}", initial_members.members.len());

    // Create a new user via POST /api/v1/users (requires orgId)
    let (create_status, create_body) =
        with_timeout(TestTimeout::default_with_label("Create test user"), async {
            api.post(
                admin_token,
                "/api/v1/users",
                json!({
                    "email": "member-test@e2e.test",
                    "password": "MemberTest123!",
                    "name": "Member Test User",
                    "isAdmin": false,
                    "orgId": test_org.id
                }),
            )
            .await
        })
        .await
        .expect("Create user should succeed");

    assert!(
        create_status.is_success(),
        "Create user should return 2xx, got {} - {:?}",
        create_status,
        create_body
    );
    let new_user_id = create_body["id"].as_str().expect("User response should have id").to_string();
    println!("ok Created user: id={}", new_user_id);

    // Add user to org as member
    let member = with_timeout(TestTimeout::default_with_label("Add org member"), async {
        api.add_org_member(admin_token, &test_org.id, &new_user_id, "member").await
    })
    .await
    .expect("Add org member should succeed");

    assert_eq!(member.user_id, new_user_id);
    assert_eq!(member.role, "member");
    println!("ok Added member: user_id={}, role={}", member.user_id, member.role);

    // List members - should include the new member
    let members_after_add =
        with_timeout(TestTimeout::default_with_label("List members after add"), async {
            api.list_org_members(admin_token, &test_org.id).await
        })
        .await
        .expect("List org members should succeed");

    let found_member = members_after_add.members.iter().find(|m| m.user_id == new_user_id);
    assert!(found_member.is_some(), "New member should appear in member list");
    assert_eq!(found_member.unwrap().role, "member");
    println!("ok Member list contains user, total members: {}", members_after_add.members.len());

    // Update member role to admin
    let updated_member =
        with_timeout(TestTimeout::default_with_label("Update member role"), async {
            api.update_org_member_role(admin_token, &test_org.id, &new_user_id, "admin").await
        })
        .await
        .expect("Update org member role should succeed");

    assert_eq!(updated_member.role, "admin");
    println!("ok Updated member role to: {}", updated_member.role);

    // Remove member from org
    with_timeout(TestTimeout::default_with_label("Remove org member"), async {
        api.remove_org_member(admin_token, &test_org.id, &new_user_id).await
    })
    .await
    .expect("Remove org member should succeed");

    // Verify member removed
    let members_after_remove =
        with_timeout(TestTimeout::default_with_label("List members after remove"), async {
            api.list_org_members(admin_token, &test_org.id).await
        })
        .await
        .expect("List org members should succeed");

    let removed_member = members_after_remove.members.iter().find(|m| m.user_id == new_user_id);
    assert!(removed_member.is_none(), "Removed member should not appear in member list");
    println!("ok Member removed, remaining members: {}", members_after_remove.members.len());
}

/// Test org-scoped team creation and listing
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2504_org_scoped_teams() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_2504_org_scoped_teams").isolated().without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap and login
    with_timeout(TestTimeout::default_with_label("Bootstrap"), async {
        api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME).await
    })
    .await
    .expect("Bootstrap should succeed");

    let (session, login_resp) = with_timeout(TestTimeout::default_with_label("Login"), async {
        api.login_full(TEST_EMAIL, TEST_PASSWORD).await
    })
    .await
    .expect("Login should succeed");

    let default_org_name = login_resp.org_name.as_deref().unwrap_or("default");
    let default_org_id = login_resp.org_id.as_deref().expect("Login should return org_id");
    println!("ok Logged in, default org: {} (id={})", default_org_name, default_org_id);

    let token_resp = with_timeout(TestTimeout::default_with_label("Create admin token"), async {
        api.create_token(&session, "org-teams-token", vec!["admin:all".to_string()]).await
    })
    .await
    .expect("Token creation should succeed");

    let admin_token = &token_resp.token;

    // Create a team within the default org
    let team = with_timeout(TestTimeout::default_with_label("Create team"), async {
        api.create_team(admin_token, "org-scoped-team", Some("Org Scoped Team"), default_org_id)
            .await
    })
    .await
    .expect("Team creation should succeed");

    println!("ok Created team: {} (id={})", team.name, team.id);

    // List org teams
    let org_teams = with_timeout(TestTimeout::default_with_label("List org teams"), async {
        api.list_org_teams(admin_token, default_org_name).await
    })
    .await
    .expect("List org teams should succeed");

    let found_team = org_teams.teams.iter().find(|t| t.name == "org-scoped-team");
    assert!(
        found_team.is_some(),
        "Team should appear in org teams list. Teams: {:?}",
        org_teams.teams.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    println!("ok Org teams list contains team, total teams: {}", org_teams.teams.len());

    // Verify team has org_id
    if let Some(team_org_id) = &found_team.unwrap().org_id {
        if let Some(login_org_id) = &login_resp.org_id {
            assert_eq!(team_org_id, login_org_id, "Team org_id should match default org id");
            println!("ok Team org_id matches default org: {}", team_org_id);
        }
    }
}

/// Test deleting an organization
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2505_delete_organization() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_2505_delete_organization").isolated().without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap and login
    with_timeout(TestTimeout::default_with_label("Bootstrap"), async {
        api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME).await
    })
    .await
    .expect("Bootstrap should succeed");

    let session = with_timeout(TestTimeout::default_with_label("Login"), async {
        api.login(TEST_EMAIL, TEST_PASSWORD).await
    })
    .await
    .expect("Login should succeed");

    let token_resp = with_timeout(TestTimeout::default_with_label("Create admin token"), async {
        api.create_token(&session, "org-delete-token", vec!["admin:all".to_string()]).await
    })
    .await
    .expect("Token creation should succeed");

    let admin_token = &token_resp.token;

    // Create a disposable org
    let disposable_org =
        with_timeout(TestTimeout::default_with_label("Create disposable org"), async {
            api.create_organization(
                admin_token,
                "disposable-org",
                "Disposable Org",
                Some("This org will be deleted"),
            )
            .await
        })
        .await
        .expect("Create organization should succeed");

    println!("ok Created disposable org: {} (id={})", disposable_org.name, disposable_org.id);

    // Verify it exists in list
    let orgs_before =
        with_timeout(TestTimeout::default_with_label("List orgs before delete"), async {
            api.list_organizations(admin_token).await
        })
        .await
        .expect("List organizations should succeed");

    let org_names_before: Vec<&str> = orgs_before.items.iter().map(|o| o.name.as_str()).collect();
    assert!(
        org_names_before.contains(&"disposable-org"),
        "Disposable org should exist before deletion"
    );
    println!("ok Org exists before delete: {:?}", org_names_before);

    // Delete the org
    with_timeout(TestTimeout::default_with_label("Delete organization"), async {
        api.delete_organization(admin_token, &disposable_org.id).await
    })
    .await
    .expect("Delete organization should succeed");

    println!("ok Organization deleted: {}", disposable_org.id);

    // Verify it no longer appears in list
    let orgs_after =
        with_timeout(TestTimeout::default_with_label("List orgs after delete"), async {
            api.list_organizations(admin_token).await
        })
        .await
        .expect("List organizations should succeed");

    let org_names_after: Vec<&str> = orgs_after.items.iter().map(|o| o.name.as_str()).collect();
    assert!(
        !org_names_after.contains(&"disposable-org"),
        "Disposable org should not exist after deletion, but found: {:?}",
        org_names_after
    );
    println!("ok Org no longer in list after delete: {:?}", org_names_after);
}

/// Test cross-org isolation: user in org A cannot access org B resources
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2506_cross_org_isolation() {
    let harness = TestHarness::start(
        TestHarnessConfig::new("test_2506_cross_org_isolation").isolated().without_envoy(),
    )
    .await
    .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());

    // Bootstrap and login as admin
    with_timeout(TestTimeout::default_with_label("Bootstrap"), async {
        api.bootstrap(TEST_EMAIL, TEST_PASSWORD, TEST_NAME).await
    })
    .await
    .expect("Bootstrap should succeed");

    let session = with_timeout(TestTimeout::default_with_label("Login"), async {
        api.login(TEST_EMAIL, TEST_PASSWORD).await
    })
    .await
    .expect("Login should succeed");

    let token_resp = with_timeout(TestTimeout::default_with_label("Create admin token"), async {
        api.create_token(&session, "cross-org-token", vec!["admin:all".to_string()]).await
    })
    .await
    .expect("Token creation should succeed");

    let admin_token = &token_resp.token;

    // Create two separate organizations
    let org_a = with_timeout(TestTimeout::default_with_label("Create org A"), async {
        api.create_organization(admin_token, "org-alpha", "Org Alpha", None).await
    })
    .await
    .expect("Create org A should succeed");

    let org_b = with_timeout(TestTimeout::default_with_label("Create org B"), async {
        api.create_organization(admin_token, "org-beta", "Org Beta", None).await
    })
    .await
    .expect("Create org B should succeed");

    println!("ok Created orgs: {} (id={}), {} (id={})", org_a.name, org_a.id, org_b.name, org_b.id);

    // Create a user in org A
    let (create_status, create_body) =
        with_timeout(TestTimeout::default_with_label("Create user for org A"), async {
            api.post(
                admin_token,
                "/api/v1/users",
                json!({
                    "email": "user-alpha@e2e.test",
                    "password": "AlphaUser123!",
                    "name": "Alpha User",
                    "isAdmin": false,
                    "orgId": org_a.id
                }),
            )
            .await
        })
        .await
        .expect("Create user should succeed");

    assert!(
        create_status.is_success(),
        "Create user should return 2xx, got {} - {:?}",
        create_status,
        create_body
    );
    let user_a_id = create_body["id"].as_str().expect("User response should have id").to_string();
    println!("ok Created user for org A: id={}", user_a_id);

    // Add user to org A
    with_timeout(TestTimeout::default_with_label("Add user to org A"), async {
        api.add_org_member(admin_token, &org_a.id, &user_a_id, "member").await
    })
    .await
    .expect("Add member to org A should succeed");

    println!("ok User added to org A as member");

    // Verify user appears in org A members
    let org_a_members =
        with_timeout(TestTimeout::default_with_label("List org A members"), async {
            api.list_org_members(admin_token, &org_a.id).await
        })
        .await
        .expect("List org A members should succeed");

    let in_org_a = org_a_members.members.iter().any(|m| m.user_id == user_a_id);
    assert!(in_org_a, "User should be member of org A");
    println!("ok User is member of org A");

    // Verify user does NOT appear in org B members
    let org_b_members =
        with_timeout(TestTimeout::default_with_label("List org B members"), async {
            api.list_org_members(admin_token, &org_b.id).await
        })
        .await
        .expect("List org B members should succeed");

    let in_org_b = org_b_members.members.iter().any(|m| m.user_id == user_a_id);
    assert!(!in_org_b, "User should NOT be member of org B");
    println!("ok User is NOT member of org B - cross-org isolation verified");

    // Create a team in org A, verify it does not appear under org B teams
    let team_in_a =
        with_timeout(TestTimeout::default_with_label("Create team in org A context"), async {
            api.create_team(admin_token, "alpha-team", Some("Alpha Team"), &org_a.id).await
        })
        .await
        .expect("Team creation should succeed");

    println!("ok Created team in org context: {} (id={})", team_in_a.name, team_in_a.id);

    // List teams under org B - the alpha-team should not appear
    let org_b_teams = with_timeout(TestTimeout::default_with_label("List org B teams"), async {
        api.list_org_teams(admin_token, &org_b.name).await
    })
    .await
    .expect("List org B teams should succeed");

    let team_in_b = org_b_teams.teams.iter().any(|t| t.name == "alpha-team");
    assert!(
        !team_in_b,
        "Team from org A should NOT appear in org B teams. Org B teams: {:?}",
        org_b_teams.teams.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    println!("ok Cross-org team isolation verified - alpha-team not in org B");
}
