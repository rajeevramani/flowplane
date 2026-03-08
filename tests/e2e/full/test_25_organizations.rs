//! Organization E2E Tests (test_25)
//!
//! Tests organization CRUD, membership management, and org-scoped operations:
//! - Organization creation, listing, updating
//! - Organization membership management
//! - Org-scoped team creation and listing
//! - Bootstrap platform org verification
//! - Organization deletion
//! - Cross-org isolation
//!
//! All tests authenticate via Zitadel JWT (no PATs).

use crate::common::{
    api_client::ApiClient,
    shared_infra::SharedInfrastructure,
    timeout::{with_timeout, TestTimeout},
    zitadel,
};

/// Helper: obtain superadmin JWT and return (api, token)
async fn setup_org_test() -> (ApiClient, String) {
    let infra = SharedInfrastructure::get_or_init()
        .await
        .expect("Failed to initialize shared infrastructure");

    let api = ApiClient::new(infra.api_url());

    let token = with_timeout(TestTimeout::default_with_label("Obtain superadmin JWT"), async {
        zitadel::obtain_human_token(
            &infra.zitadel_config,
            zitadel::SUPERADMIN_EMAIL,
            zitadel::SUPERADMIN_PASSWORD,
        )
        .await
    })
    .await
    .expect("JWT acquisition should succeed");

    (api, token)
}

/// Test that bootstrap creates a platform organization
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2500_default_org_from_bootstrap() {
    let (api, admin_token) = setup_org_test().await;

    // List organizations - should see platform org
    let orgs = with_timeout(TestTimeout::default_with_label("List organizations"), async {
        api.list_organizations(&admin_token).await
    })
    .await
    .expect("List organizations should succeed");

    assert!(orgs.total >= 1, "Should have at least 1 org");
    let default_org =
        orgs.items.iter().find(|o| o.name == "platform").expect("Platform org should exist");
    println!("ok Platform org found: id={}", default_org.id);

    // Get current org
    let current = with_timeout(TestTimeout::default_with_label("Get current org"), async {
        api.get_current_org(&admin_token).await
    })
    .await
    .expect("Get current org should succeed");

    assert_eq!(current.organization.name, "platform");
    assert_eq!(current.role, "owner");
    println!("ok Current org: {} (role={})", current.organization.name, current.role);
}

/// Test creating a new organization
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2501_create_organization() {
    let (api, admin_token) = setup_org_test().await;

    // Create a new organization
    let new_org = with_timeout(TestTimeout::default_with_label("Create organization"), async {
        api.create_organization(
            &admin_token,
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

    // List organizations - should see both platform and test-org-alpha
    let orgs = with_timeout(TestTimeout::default_with_label("List organizations"), async {
        api.list_organizations(&admin_token).await
    })
    .await
    .expect("List organizations should succeed");

    assert!(orgs.total >= 2, "Should have at least 2 orgs, got {}", orgs.total);
    let org_names: Vec<&str> = orgs.items.iter().map(|o| o.name.as_str()).collect();
    assert!(org_names.contains(&"platform"), "Should contain platform org");
    assert!(org_names.contains(&"test-org-alpha"), "Should contain test-org-alpha");
    println!("ok Listed {} organizations: {:?}", orgs.total, org_names);
}

/// Test getting and updating an organization
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2502_get_and_update_organization() {
    let (api, admin_token) = setup_org_test().await;

    // Create a test org
    let created_org = with_timeout(TestTimeout::default_with_label("Create organization"), async {
        api.create_organization(
            &admin_token,
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
        api.get_organization(&admin_token, &created_org.id).await
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
            &admin_token,
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
        api.get_organization(&admin_token, &created_org.id).await
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
    let (api, admin_token) = setup_org_test().await;

    // Create a test org
    let test_org = with_timeout(TestTimeout::default_with_label("Create organization"), async {
        api.create_organization(&admin_token, "test-org-members", "Test Org Members", None).await
    })
    .await
    .expect("Create organization should succeed");

    println!("ok Created org: {} (id={})", test_org.name, test_org.id);

    // List initial members
    let initial_members =
        with_timeout(TestTimeout::default_with_label("List initial members"), async {
            api.list_org_members(&admin_token, &test_org.id).await
        })
        .await
        .expect("List org members should succeed");

    println!("ok Initial members count: {}", initial_members.members.len());

    // Create user in Zitadel and JIT-provision via authentication
    let infra = SharedInfrastructure::get_or_init().await.expect("Failed to get shared infra");

    let member_email = "member-test@e2e.test";
    let member_password = "MemberTest123!";
    with_timeout(TestTimeout::default_with_label("Create Zitadel user"), async {
        zitadel::create_human_user(
            &infra.zitadel_config.base_url,
            &infra.zitadel_config.admin_pat,
            member_email,
            "Member",
            "Test User",
            member_password,
        )
        .await
    })
    .await
    .expect("Create Zitadel user should succeed");

    // Authenticate to trigger JIT provisioning in CP
    let member_token =
        with_timeout(TestTimeout::default_with_label("Authenticate test user"), async {
            zitadel::obtain_human_token(&infra.zitadel_config, member_email, member_password).await
        })
        .await
        .expect("User authentication should succeed");

    let member_session =
        api.get_auth_session(&member_token).await.expect("User auth session should succeed");
    let new_user_id = member_session.user_id;
    println!("ok Created user: id={}", new_user_id);

    // Add user to org as member
    let member = with_timeout(TestTimeout::default_with_label("Add org member"), async {
        api.add_org_member(&admin_token, &test_org.id, &new_user_id, "member").await
    })
    .await
    .expect("Add org member should succeed");

    assert_eq!(member.user_id, new_user_id);
    assert_eq!(member.role, "member");
    println!("ok Added member: user_id={}, role={}", member.user_id, member.role);

    // List members - should include the new member
    let members_after_add =
        with_timeout(TestTimeout::default_with_label("List members after add"), async {
            api.list_org_members(&admin_token, &test_org.id).await
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
            api.update_org_member_role(&admin_token, &test_org.id, &new_user_id, "admin").await
        })
        .await
        .expect("Update org member role should succeed");

    assert_eq!(updated_member.role, "admin");
    println!("ok Updated member role to: {}", updated_member.role);

    // Remove member from org
    with_timeout(TestTimeout::default_with_label("Remove org member"), async {
        api.remove_org_member(&admin_token, &test_org.id, &new_user_id).await
    })
    .await
    .expect("Remove org member should succeed");

    // Verify member removed
    let members_after_remove =
        with_timeout(TestTimeout::default_with_label("List members after remove"), async {
            api.list_org_members(&admin_token, &test_org.id).await
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
    let (api, admin_token) = setup_org_test().await;

    // Create a tenant org (platform org is governance-only, no teams allowed)
    let tenant_org = with_timeout(TestTimeout::default_with_label("Create tenant org"), async {
        api.create_organization(
            &admin_token,
            "test-org-2504",
            "Test Org 2504",
            Some("Tenant org for org-scoped team tests"),
        )
        .await
    })
    .await
    .expect("Create tenant org should succeed");

    println!("ok Created tenant org: {} (id={})", tenant_org.name, tenant_org.id);

    // Create a team within the tenant org
    let team = with_timeout(TestTimeout::default_with_label("Create team"), async {
        api.create_team(&admin_token, "org-scoped-team", Some("Org Scoped Team"), &tenant_org.id)
            .await
    })
    .await
    .expect("Team creation should succeed");

    println!("ok Created team: {} (id={})", team.name, team.id);

    // List org teams for the tenant org
    let org_teams = with_timeout(TestTimeout::default_with_label("List org teams"), async {
        api.list_org_teams(&admin_token, &tenant_org.name).await
    })
    .await
    .expect("List org teams should succeed");

    let found_team = org_teams.teams.iter().find(|t| t.name == "org-scoped-team");
    assert!(
        found_team.is_some(),
        "Team should appear in tenant org teams list. Teams: {:?}",
        org_teams.teams.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    println!("ok Org teams list contains team, total teams: {}", org_teams.teams.len());

    // Verify team's org_id matches the tenant org
    let found = found_team.unwrap();
    assert_eq!(
        found.org_id.as_deref(),
        Some(tenant_org.id.as_str()),
        "Team org_id should match tenant org id"
    );
    println!("ok Team org_id matches tenant org: {}", tenant_org.id);
}

/// Test deleting an organization
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2505_delete_organization() {
    let (api, admin_token) = setup_org_test().await;

    // Create a disposable org
    let disposable_org =
        with_timeout(TestTimeout::default_with_label("Create disposable org"), async {
            api.create_organization(
                &admin_token,
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
            api.list_organizations(&admin_token).await
        })
        .await
        .expect("List organizations should succeed");

    let org_names_before: Vec<&str> = orgs_before.items.iter().map(|o| o.name.as_str()).collect();
    assert!(
        org_names_before.contains(&"disposable-org"),
        "Disposable org should exist before deletion"
    );
    println!("ok Org exists before delete: {:?}", org_names_before);

    // Clean up auto-created default team before deleting the org (FK constraint)
    let teams = with_timeout(TestTimeout::default_with_label("List teams"), async {
        api.list_teams(&admin_token).await
    })
    .await
    .expect("List teams should succeed");

    for team in &teams {
        if team.org_id.as_deref() == Some(&disposable_org.id) {
            with_timeout(TestTimeout::default_with_label("Delete org team"), async {
                api.delete_team(&admin_token, &team.id).await
            })
            .await
            .expect("Delete org team should succeed");
            println!("ok Deleted team '{}' (id={}) from disposable org", team.name, team.id);
        }
    }

    // Delete the org
    with_timeout(TestTimeout::default_with_label("Delete organization"), async {
        api.delete_organization(&admin_token, &disposable_org.id).await
    })
    .await
    .expect("Delete organization should succeed");

    println!("ok Organization deleted: {}", disposable_org.id);

    // Verify it no longer appears in list
    let orgs_after =
        with_timeout(TestTimeout::default_with_label("List orgs after delete"), async {
            api.list_organizations(&admin_token).await
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

/// Test cross-org isolation: teams in org A don't appear in org B
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2506_cross_org_isolation() {
    let (api, admin_token) = setup_org_test().await;

    // Create two separate organizations
    let org_a = with_timeout(TestTimeout::default_with_label("Create org A"), async {
        api.create_organization(&admin_token, "org-alpha", "Org Alpha", None).await
    })
    .await
    .expect("Create org A should succeed");

    let org_b = with_timeout(TestTimeout::default_with_label("Create org B"), async {
        api.create_organization(&admin_token, "org-beta", "Org Beta", None).await
    })
    .await
    .expect("Create org B should succeed");

    println!("ok Created orgs: {} (id={}), {} (id={})", org_a.name, org_a.id, org_b.name, org_b.id);

    // Create a user and JIT-provision via authentication
    let infra = SharedInfrastructure::get_or_init().await.expect("Failed to get shared infra");

    let alpha_email = "user-alpha@e2e.test";
    let alpha_password = "AlphaUser123!";
    with_timeout(TestTimeout::default_with_label("Create Zitadel user"), async {
        zitadel::create_human_user(
            &infra.zitadel_config.base_url,
            &infra.zitadel_config.admin_pat,
            alpha_email,
            "Alpha",
            "User",
            alpha_password,
        )
        .await
    })
    .await
    .expect("Create Zitadel user should succeed");

    // Authenticate to trigger JIT provisioning
    let alpha_token =
        with_timeout(TestTimeout::default_with_label("Authenticate alpha user"), async {
            zitadel::obtain_human_token(&infra.zitadel_config, alpha_email, alpha_password).await
        })
        .await
        .expect("User authentication should succeed");

    let alpha_session =
        api.get_auth_session(&alpha_token).await.expect("User auth session should succeed");
    let user_a_id = alpha_session.user_id;
    println!("ok Created user for org A: id={}", user_a_id);

    // Add user to org A
    with_timeout(TestTimeout::default_with_label("Add user to org A"), async {
        api.add_org_member(&admin_token, &org_a.id, &user_a_id, "member").await
    })
    .await
    .expect("Add member to org A should succeed");

    println!("ok User added to org A as member");

    // Verify user appears in org A members
    let org_a_members =
        with_timeout(TestTimeout::default_with_label("List org A members"), async {
            api.list_org_members(&admin_token, &org_a.id).await
        })
        .await
        .expect("List org A members should succeed");

    let in_org_a = org_a_members.members.iter().any(|m| m.user_id == user_a_id);
    assert!(in_org_a, "User should be member of org A");
    println!("ok User is member of org A");

    // Verify user does NOT appear in org B members
    let org_b_members =
        with_timeout(TestTimeout::default_with_label("List org B members"), async {
            api.list_org_members(&admin_token, &org_b.id).await
        })
        .await
        .expect("List org B members should succeed");

    let in_org_b = org_b_members.members.iter().any(|m| m.user_id == user_a_id);
    assert!(!in_org_b, "User should NOT be member of org B");
    println!("ok User is NOT member of org B - cross-org isolation verified");

    // Create a team in org A, verify it does not appear under org B teams
    let team_in_a =
        with_timeout(TestTimeout::default_with_label("Create team in org A context"), async {
            api.create_team(&admin_token, "alpha-team", Some("Alpha Team"), &org_a.id).await
        })
        .await
        .expect("Team creation should succeed");

    println!("ok Created team in org context: {} (id={})", team_in_a.name, team_in_a.id);

    // List teams under org B - the alpha-team should not appear
    let org_b_teams = with_timeout(TestTimeout::default_with_label("List org B teams"), async {
        api.list_org_teams(&admin_token, &org_b.name).await
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
