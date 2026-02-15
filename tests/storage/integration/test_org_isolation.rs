#![cfg(feature = "postgres_tests")]

//! Integration tests for organization-level data isolation.
//!
//! Verifies that organizations provide proper tenant boundaries: teams, members,
//! and resources belonging to one org are not visible to another.

use flowplane::auth::hashing;
use flowplane::auth::organization::{CreateOrganizationRequest, OrgRole};
use flowplane::auth::team::CreateTeamRequest;
use flowplane::auth::user::{NewUser, UserStatus};
use flowplane::domain::{OrgId, UserId};
use flowplane::storage::repositories::{
    OrgMembershipRepository, OrganizationRepository, SqlxOrgMembershipRepository,
    SqlxOrganizationRepository, SqlxTeamRepository, SqlxUserRepository, TeamRepository,
    UserRepository,
};
use flowplane::storage::DbPool;

#[path = "../../common/mod.rs"]
mod common;
use common::test_db::TestDatabase;

/// Helper: create an org via the repository and return its OrgId.
async fn create_org(pool: &DbPool, name: &str) -> OrgId {
    let repo = SqlxOrganizationRepository::new(pool.clone());
    let org = repo
        .create_organization(CreateOrganizationRequest {
            name: name.to_string(),
            display_name: format!("Org {}", name),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .unwrap_or_else(|e| panic!("Failed to create org '{}': {}", name, e));
    org.id
}

/// Helper: create a user belonging to an org.
async fn create_user(pool: &DbPool, email: &str, org_id: OrgId) -> UserId {
    let user_repo = SqlxUserRepository::new(pool.clone());
    let user_id = UserId::new();
    let password_hash = hashing::hash_password("TestPass123!").expect("hash password");
    user_repo
        .create_user(NewUser {
            id: user_id.clone(),
            email: email.to_string(),
            password_hash,
            name: email.split('@').next().unwrap_or("user").to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id,
        })
        .await
        .unwrap_or_else(|e| panic!("Failed to create user '{}': {}", email, e));
    user_id
}

// ---------------------------------------------------------------------------
// Test: Org A's teams are not visible when listing Org B's teams
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_teams_are_isolated_between_orgs() {
    let test_db = TestDatabase::new("org_team_isolation").await;
    let pool = test_db.pool().clone();

    let org_a_id = create_org(&pool, "org-alpha").await;
    let org_b_id = create_org(&pool, "org-beta").await;

    let team_repo = SqlxTeamRepository::new(pool.clone());

    // Create teams assigned to Org A
    team_repo
        .create_team(CreateTeamRequest {
            name: "alpha-team-1".to_string(),
            display_name: "Alpha Team 1".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a_id.clone(),
            settings: None,
        })
        .await
        .expect("create alpha-team-1");

    team_repo
        .create_team(CreateTeamRequest {
            name: "alpha-team-2".to_string(),
            display_name: "Alpha Team 2".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a_id.clone(),
            settings: None,
        })
        .await
        .expect("create alpha-team-2");

    // Create team assigned to Org B
    team_repo
        .create_team(CreateTeamRequest {
            name: "beta-team-1".to_string(),
            display_name: "Beta Team 1".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b_id.clone(),
            settings: None,
        })
        .await
        .expect("create beta-team-1");

    // Verify: listing teams by Org A returns only A's teams
    let org_a_teams = team_repo.list_teams_by_org(&org_a_id).await.expect("list org A teams");
    assert_eq!(org_a_teams.len(), 2, "Org A should have exactly 2 teams");
    let team_names: Vec<&str> = org_a_teams.iter().map(|t| t.name.as_str()).collect();
    assert!(team_names.contains(&"alpha-team-1"));
    assert!(team_names.contains(&"alpha-team-2"));
    assert!(!team_names.contains(&"beta-team-1"), "Org A should NOT see Org B's teams");

    // Verify: listing teams by Org B returns only B's teams
    let org_b_teams = team_repo.list_teams_by_org(&org_b_id).await.expect("list org B teams");
    assert_eq!(org_b_teams.len(), 1, "Org B should have exactly 1 team");
    assert_eq!(org_b_teams[0].name, "beta-team-1");
}

// ---------------------------------------------------------------------------
// Test: Cross-org team creation blocked by FK constraint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cross_org_team_creation_blocked_by_fk() {
    let test_db = TestDatabase::new("org_cross_team_fk").await;
    let pool = test_db.pool().clone();

    let team_repo = SqlxTeamRepository::new(pool.clone());

    // Attempt to create a team with a non-existent org ID
    let bogus_org_id = OrgId::new();
    let result = team_repo
        .create_team(CreateTeamRequest {
            name: "rogue-team".to_string(),
            display_name: "Rogue Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: bogus_org_id,
            settings: None,
        })
        .await;

    assert!(result.is_err(), "Creating a team with a non-existent org_id should fail");
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.contains("foreign key")
            || err_msg.contains("violates")
            || err_msg.contains("Failed to create team"),
        "Error should be a database error from FK violation, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// Test: Org deletion with active teams is rejected with friendly message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_deletion_with_teams_rejected() {
    let test_db = TestDatabase::new("org_delete_with_teams").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "org-with-teams").await;

    // Create a team belonging to this org
    let team_repo = SqlxTeamRepository::new(pool.clone());
    team_repo
        .create_team(CreateTeamRequest {
            name: "child-team".to_string(),
            display_name: "Child Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_id.clone(),
            settings: None,
        })
        .await
        .expect("create child-team");

    // Attempt to delete the org - should fail with friendly message
    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let result = org_repo.delete_organization(&org_id).await;

    assert!(result.is_err(), "Deleting org with active teams should fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Cannot delete organization") || err_msg.contains("active teams"),
        "Error should be the friendly FK violation message, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// Test: Org member isolation - Org A's members not visible via Org B query
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_member_isolation() {
    let test_db = TestDatabase::new("org_member_isolation").await;
    let pool = test_db.pool().clone();

    let org_a_id = create_org(&pool, "org-members-a").await;
    let org_b_id = create_org(&pool, "org-members-b").await;

    // Create users
    let user_a = create_user(&pool, "alice@org-a.com", org_a_id.clone()).await;
    let user_b = create_user(&pool, "bob@org-b.com", org_b_id.clone()).await;

    // Create memberships
    let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
    membership_repo
        .create_membership(&user_a, &org_a_id, OrgRole::Admin)
        .await
        .expect("create membership for user_a in org A");
    membership_repo
        .create_membership(&user_b, &org_b_id, OrgRole::Member)
        .await
        .expect("create membership for user_b in org B");

    // Verify: Org A members only show user_a
    let org_a_members =
        membership_repo.list_org_members(&org_a_id).await.expect("list org A members");
    assert_eq!(org_a_members.len(), 1);
    assert_eq!(org_a_members[0].user_id, user_a);

    // Verify: Org B members only show user_b
    let org_b_members =
        membership_repo.list_org_members(&org_b_id).await.expect("list org B members");
    assert_eq!(org_b_members.len(), 1);
    assert_eq!(org_b_members[0].user_id, user_b);

    // Verify: Cross-org membership lookup returns None
    let cross =
        membership_repo.get_membership(&user_a, &org_b_id).await.expect("get cross-org membership");
    assert!(cross.is_none(), "User A should NOT have membership in Org B");
}

// ---------------------------------------------------------------------------
// Test: User org_id assignment via update_user_org
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_org_assignment() {
    let test_db = TestDatabase::new("user_org_assignment").await;
    let pool = test_db.pool().clone();

    let org_id_initial = create_org(&pool, "org-assign-initial").await;
    let org_id_target = create_org(&pool, "org-assign-target").await;

    // Create user with initial org
    let user_repo = SqlxUserRepository::new(pool.clone());
    let user_id = UserId::new();
    let password_hash = hashing::hash_password("TestPass123!").expect("hash password");
    let user = user_repo
        .create_user(NewUser {
            id: user_id.clone(),
            email: "reassign@test.com".to_string(),
            password_hash,
            name: "Reassign User".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: org_id_initial.clone(),
        })
        .await
        .expect("create user");

    assert_eq!(user.org_id.as_str(), org_id_initial.as_str(), "User should start in initial org");

    // Reassign user to target org
    user_repo.update_user_org(&user_id, &org_id_target).await.expect("reassign user to org");

    // Verify the assignment persists
    let fetched = user_repo.get_user(&user_id).await.expect("get user").expect("user exists");
    assert_eq!(
        fetched.org_id.as_str(),
        org_id_target.as_str(),
        "User org reassignment should persist"
    );
}

// ---------------------------------------------------------------------------
// Test: Org with members cannot be deleted (FK on memberships)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_deletion_with_members_rejected() {
    let test_db = TestDatabase::new("org_delete_with_members").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "org-with-members").await;

    // Create a user and membership
    let user_id = create_user(&pool, "member@org.com", org_id.clone()).await;
    let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
    membership_repo
        .create_membership(&user_id, &org_id, OrgRole::Member)
        .await
        .expect("create membership");

    // Attempt to delete the org - should fail
    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let result = org_repo.delete_organization(&org_id).await;

    assert!(result.is_err(), "Deleting org with active members should fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Cannot delete organization") || err_msg.contains("members"),
        "Error should mention members constraint, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// Test: Duplicate org name is rejected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_org_name_rejected() {
    let test_db = TestDatabase::new("org_dup_name").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());

    let request = CreateOrganizationRequest {
        name: "unique-org-name".to_string(),
        display_name: "Unique Org".to_string(),
        description: None,
        owner_user_id: None,
        settings: None,
    };

    org_repo.create_organization(request.clone()).await.expect("first create");
    let result = org_repo.create_organization(request).await;
    assert!(result.is_err(), "Duplicate org name should be rejected");
}

// ---------------------------------------------------------------------------
// Test: User memberships across multiple orgs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_memberships_listed_correctly() {
    let test_db = TestDatabase::new("user_multi_org_membership").await;
    let pool = test_db.pool().clone();

    let org_a_id = create_org(&pool, "multi-org-a").await;
    let org_b_id = create_org(&pool, "multi-org-b").await;

    let user_id = create_user(&pool, "multi-org-user@test.com", org_a_id.clone()).await;

    let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());

    membership_repo
        .create_membership(&user_id, &org_a_id, OrgRole::Admin)
        .await
        .expect("create membership in org A");
    membership_repo
        .create_membership(&user_id, &org_b_id, OrgRole::Viewer)
        .await
        .expect("create membership in org B");

    let memberships =
        membership_repo.list_user_memberships(&user_id).await.expect("list user memberships");
    assert_eq!(memberships.len(), 2);

    let org_ids: Vec<&str> = memberships.iter().map(|m| m.org_id.as_str()).collect();
    assert!(org_ids.contains(&org_a_id.as_str()));
    assert!(org_ids.contains(&org_b_id.as_str()));
}

// ---------------------------------------------------------------------------
// Test: Last owner protection on membership deletion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cannot_delete_last_owner() {
    let test_db = TestDatabase::new("last_owner_delete").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "owner-protect-org").await;
    let user_id = create_user(&pool, "sole-owner@test.com", org_id.clone()).await;

    let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
    membership_repo
        .create_membership(&user_id, &org_id, OrgRole::Owner)
        .await
        .expect("create owner membership");

    // Attempt to delete the sole owner
    let result = membership_repo.delete_membership(&user_id, &org_id).await;
    assert!(result.is_err(), "Should not be able to delete the last owner");
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("last owner"), "Error should mention last owner, got: {}", err_msg);
}

// ---------------------------------------------------------------------------
// Test: Resources in org-alpha's team are not visible via org-beta's team
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_resource_isolation_via_team_scoping() {
    let test_db = TestDatabase::new("org_resource_isolation").await;
    let pool = test_db.pool().clone();

    let org_a_id = create_org(&pool, "res-org-alpha").await;
    let org_b_id = create_org(&pool, "res-org-beta").await;

    let team_repo = SqlxTeamRepository::new(pool.clone());

    // Create teams in each org
    let team_alpha = team_repo
        .create_team(CreateTeamRequest {
            name: "alpha-infra".to_string(),
            display_name: "Alpha Infra".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a_id.clone(),
            settings: None,
        })
        .await
        .expect("create alpha-infra");

    let team_beta = team_repo
        .create_team(CreateTeamRequest {
            name: "beta-infra".to_string(),
            display_name: "Beta Infra".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b_id.clone(),
            settings: None,
        })
        .await
        .expect("create beta-infra");

    let alpha_team_id = team_alpha.id.to_string();
    let beta_team_id = team_beta.id.to_string();

    // Create clusters in each org's team
    let cluster_repo = flowplane::storage::ClusterRepository::new(pool.clone());

    cluster_repo
        .create(flowplane::storage::CreateClusterRequest {
            name: "alpha-cluster".to_string(),
            service_name: "alpha-svc".to_string(),
            configuration: serde_json::json!({
                "endpoints": [{"Address": {"host": "10.0.0.1", "port": 8080}}],
                "connect_timeout_seconds": 5
            }),
            team: Some(alpha_team_id.clone()),
            import_id: None,
        })
        .await
        .expect("create alpha cluster");

    cluster_repo
        .create(flowplane::storage::CreateClusterRequest {
            name: "beta-cluster".to_string(),
            service_name: "beta-svc".to_string(),
            configuration: serde_json::json!({
                "endpoints": [{"Address": {"host": "10.0.0.2", "port": 8080}}],
                "connect_timeout_seconds": 5
            }),
            team: Some(beta_team_id.clone()),
            import_id: None,
        })
        .await
        .expect("create beta cluster");

    // User with org-alpha team scope sees only alpha resources
    let alpha_results = cluster_repo
        .list_by_teams(std::slice::from_ref(&alpha_team_id), false, None, None)
        .await
        .expect("list alpha clusters");
    assert_eq!(alpha_results.len(), 1, "Alpha team should see 1 cluster");
    assert_eq!(alpha_results[0].name, "alpha-cluster");

    // User with org-beta team scope sees only beta resources
    let beta_results = cluster_repo
        .list_by_teams(std::slice::from_ref(&beta_team_id), false, None, None)
        .await
        .expect("list beta clusters");
    assert_eq!(beta_results.len(), 1, "Beta team should see 1 cluster");
    assert_eq!(beta_results[0].name, "beta-cluster");

    // User with alpha scope gets EMPTY for beta resources
    let cross_results = cluster_repo
        .list_by_teams(std::slice::from_ref(&alpha_team_id), false, None, None)
        .await
        .expect("list with alpha scope");
    let cross_names: Vec<&str> = cross_results.iter().map(|c| c.name.as_str()).collect();
    assert!(!cross_names.contains(&"beta-cluster"), "Alpha user should NOT see beta-cluster");

    // Empty team scopes returns no results (security hardening)
    let empty_results =
        cluster_repo.list_by_teams(&[], false, None, None).await.expect("empty teams list");
    assert_eq!(empty_results.len(), 0);
}

// ---------------------------------------------------------------------------
// Test: Listener isolation between orgs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_listener_isolation_via_team_scoping() {
    let test_db = TestDatabase::new("org_listener_isolation").await;
    let pool = test_db.pool().clone();

    let org_a_id = create_org(&pool, "listener-org-a").await;
    let org_b_id = create_org(&pool, "listener-org-b").await;

    let team_repo = SqlxTeamRepository::new(pool.clone());

    let team_a = team_repo
        .create_team(CreateTeamRequest {
            name: "listener-alpha".to_string(),
            display_name: "Listener Alpha".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a_id,
            settings: None,
        })
        .await
        .expect("create listener-alpha team");

    let team_b = team_repo
        .create_team(CreateTeamRequest {
            name: "listener-beta".to_string(),
            display_name: "Listener Beta".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b_id,
            settings: None,
        })
        .await
        .expect("create listener-beta team");

    let team_a_id = team_a.id.to_string();
    let team_b_id = team_b.id.to_string();

    let listener_repo = flowplane::storage::ListenerRepository::new(pool.clone());

    listener_repo
        .create(flowplane::storage::CreateListenerRequest {
            name: "alpha-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(9090),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({
                "name": "alpha-listener",
                "address": "0.0.0.0",
                "port": 9090,
                "filter_chains": []
            }),
            team: Some(team_a_id.clone()),
            import_id: None,
            dataplane_id: None,
        })
        .await
        .expect("create alpha listener");

    listener_repo
        .create(flowplane::storage::CreateListenerRequest {
            name: "beta-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(9091),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({
                "name": "beta-listener",
                "address": "0.0.0.0",
                "port": 9091,
                "filter_chains": []
            }),
            team: Some(team_b_id.clone()),
            import_id: None,
            dataplane_id: None,
        })
        .await
        .expect("create beta listener");

    // Org A team sees only their listener
    let a_listeners = listener_repo
        .list_by_teams(std::slice::from_ref(&team_a_id), false, None, None)
        .await
        .expect("list alpha listeners");
    assert_eq!(a_listeners.len(), 1);
    assert_eq!(a_listeners[0].name, "alpha-listener");

    // Org B team sees only their listener
    let b_listeners = listener_repo
        .list_by_teams(std::slice::from_ref(&team_b_id), false, None, None)
        .await
        .expect("list beta listeners");
    assert_eq!(b_listeners.len(), 1);
    assert_eq!(b_listeners[0].name, "beta-listener");

    // Cross-org check: alpha scope does NOT include beta
    let cross_check = listener_repo
        .list_by_teams(&[team_a_id], false, None, None)
        .await
        .expect("cross-org check");
    let names: Vec<&str> = cross_check.iter().map(|l| l.name.as_str()).collect();
    assert!(!names.contains(&"beta-listener"));
}

// ---------------------------------------------------------------------------
// Test: Org admin can list teams within their org only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_admin_cannot_list_other_org_teams() {
    let test_db = TestDatabase::new("org_admin_team_list").await;
    let pool = test_db.pool().clone();

    let org_a_id = create_org(&pool, "admin-list-org-a").await;
    let org_b_id = create_org(&pool, "admin-list-org-b").await;

    let team_repo = SqlxTeamRepository::new(pool.clone());

    team_repo
        .create_team(CreateTeamRequest {
            name: "admin-a-team".to_string(),
            display_name: "Admin A Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a_id.clone(),
            settings: None,
        })
        .await
        .expect("create admin-a-team");

    team_repo
        .create_team(CreateTeamRequest {
            name: "admin-b-team".to_string(),
            display_name: "Admin B Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b_id.clone(),
            settings: None,
        })
        .await
        .expect("create admin-b-team");

    // Org admin for A queries teams by their org -- sees only A's teams
    let a_teams = team_repo.list_teams_by_org(&org_a_id).await.expect("list org A teams");
    assert_eq!(a_teams.len(), 1);
    assert_eq!(a_teams[0].name, "admin-a-team");

    // Querying B's org returns only B's teams
    let b_teams = team_repo.list_teams_by_org(&org_b_id).await.expect("list org B teams");
    assert_eq!(b_teams.len(), 1);
    assert_eq!(b_teams[0].name, "admin-b-team");
}

// ---------------------------------------------------------------------------
// Test: Last owner protection on role downgrade
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cannot_downgrade_last_owner() {
    let test_db = TestDatabase::new("last_owner_downgrade").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "owner-downgrade-org").await;
    let user_id = create_user(&pool, "sole-owner-dg@test.com", org_id.clone()).await;

    let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());
    membership_repo
        .create_membership(&user_id, &org_id, OrgRole::Owner)
        .await
        .expect("create owner membership");

    // Attempt to downgrade the sole owner to member
    let result = membership_repo.update_membership_role(&user_id, &org_id, OrgRole::Member).await;
    assert!(result.is_err(), "Should not be able to downgrade the last owner");
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("last owner"), "Error should mention last owner, got: {}", err_msg);
}

// ---------------------------------------------------------------------------
// Test: Cross-team access within same org is denied when user lacks scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cross_team_access_within_same_org_denied() {
    use flowplane::auth::authorization::check_resource_access;
    use flowplane::auth::models::AuthContext;
    use flowplane::domain::TokenId;

    let test_db = TestDatabase::new("cross_team_same_org").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "single-org").await;

    let team_repo = SqlxTeamRepository::new(pool.clone());

    // Create two teams in the same org
    let team_x = team_repo
        .create_team(flowplane::auth::team::CreateTeamRequest {
            name: "team-x".to_string(),
            display_name: "Team X".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_id.clone(),
            settings: None,
        })
        .await
        .expect("create team-x");

    let team_y = team_repo
        .create_team(flowplane::auth::team::CreateTeamRequest {
            name: "team-y".to_string(),
            display_name: "Team Y".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_id.clone(),
            settings: None,
        })
        .await
        .expect("create team-y");

    let team_x_id = team_x.id.to_string();
    let team_y_id = team_y.id.to_string();

    // Create clusters in each team
    let cluster_repo = flowplane::storage::ClusterRepository::new(pool.clone());

    cluster_repo
        .create(flowplane::storage::CreateClusterRequest {
            name: "x-cluster".to_string(),
            service_name: "x-svc".to_string(),
            configuration: serde_json::json!({
                "endpoints": [{"Address": {"host": "10.0.0.10", "port": 8080}}],
                "connect_timeout_seconds": 5
            }),
            team: Some(team_x_id.clone()),
            import_id: None,
        })
        .await
        .expect("create x-cluster");

    cluster_repo
        .create(flowplane::storage::CreateClusterRequest {
            name: "y-cluster".to_string(),
            service_name: "y-svc".to_string(),
            configuration: serde_json::json!({
                "endpoints": [{"Address": {"host": "10.0.0.11", "port": 8080}}],
                "connect_timeout_seconds": 5
            }),
            team: Some(team_y_id.clone()),
            import_id: None,
        })
        .await
        .expect("create y-cluster");

    // User with team-x scope sees only team-x resources
    let x_results = cluster_repo
        .list_by_teams(std::slice::from_ref(&team_x_id), false, None, None)
        .await
        .expect("list team-x clusters");
    assert_eq!(x_results.len(), 1, "Team-x should see 1 cluster");
    assert_eq!(x_results[0].name, "x-cluster");

    // Team-x scope does NOT include team-y (same org, different team)
    let x_names: Vec<&str> = x_results.iter().map(|c| c.name.as_str()).collect();
    assert!(!x_names.contains(&"y-cluster"), "Team-x should NOT see team-y cluster in same org");

    // Authorization check: user with team-x scope cannot access team-y resources
    let team_x_user = AuthContext::new(
        TokenId::from_str_unchecked("team-x-user"),
        "team-x-user".into(),
        vec!["team:team-x:clusters:read".into()],
    )
    .with_org(org_id.clone(), "single-org".to_string());

    assert!(
        check_resource_access(&team_x_user, "clusters", "read", Some("team-x")),
        "User should access their own team"
    );
    assert!(
        !check_resource_access(&team_x_user, "clusters", "read", Some("team-y")),
        "User should NOT access other team within same org"
    );
}

// ---------------------------------------------------------------------------
// Test: Multiple resource types isolated between orgs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_clusters_isolated_between_orgs() {
    let test_db = TestDatabase::new("multi_cluster_org_isolation").await;
    let pool = test_db.pool().clone();

    let org_a_id = create_org(&pool, "multi-org-a").await;
    let org_b_id = create_org(&pool, "multi-org-b").await;

    let team_repo = SqlxTeamRepository::new(pool.clone());

    let team_a = team_repo
        .create_team(flowplane::auth::team::CreateTeamRequest {
            name: "multi-alpha".to_string(),
            display_name: "Multi Alpha".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a_id,
            settings: None,
        })
        .await
        .expect("create multi-alpha team");

    let team_b = team_repo
        .create_team(flowplane::auth::team::CreateTeamRequest {
            name: "multi-beta".to_string(),
            display_name: "Multi Beta".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b_id,
            settings: None,
        })
        .await
        .expect("create multi-beta team");

    let team_a_id = team_a.id.to_string();
    let team_b_id = team_b.id.to_string();

    let cluster_repo = flowplane::storage::ClusterRepository::new(pool.clone());

    // Create multiple clusters in each org's team
    for i in 1..=3 {
        cluster_repo
            .create(flowplane::storage::CreateClusterRequest {
                name: format!("alpha-svc-{}", i),
                service_name: format!("alpha-svc-{}", i),
                configuration: serde_json::json!({
                    "endpoints": [{"Address": {"host": format!("10.0.0.{}", i), "port": 8080}}],
                    "connect_timeout_seconds": 5
                }),
                team: Some(team_a_id.clone()),
                import_id: None,
            })
            .await
            .unwrap_or_else(|e| panic!("Failed to create alpha-svc-{}: {}", i, e));
    }

    for i in 1..=2 {
        cluster_repo
            .create(flowplane::storage::CreateClusterRequest {
                name: format!("beta-svc-{}", i),
                service_name: format!("beta-svc-{}", i),
                configuration: serde_json::json!({
                    "endpoints": [{"Address": {"host": format!("10.1.0.{}", i), "port": 8080}}],
                    "connect_timeout_seconds": 5
                }),
                team: Some(team_b_id.clone()),
                import_id: None,
            })
            .await
            .unwrap_or_else(|e| panic!("Failed to create beta-svc-{}: {}", i, e));
    }

    // Org A sees exactly 3 clusters
    let a_results = cluster_repo
        .list_by_teams(std::slice::from_ref(&team_a_id), false, None, None)
        .await
        .expect("list alpha clusters");
    assert_eq!(a_results.len(), 3, "Org A should see exactly 3 clusters");

    // Org B sees exactly 2 clusters
    let b_results = cluster_repo
        .list_by_teams(std::slice::from_ref(&team_b_id), false, None, None)
        .await
        .expect("list beta clusters");
    assert_eq!(b_results.len(), 2, "Org B should see exactly 2 clusters");

    // No cross-contamination
    let a_names: Vec<&str> = a_results.iter().map(|c| c.name.as_str()).collect();
    let b_names: Vec<&str> = b_results.iter().map(|c| c.name.as_str()).collect();

    for name in &a_names {
        assert!(!b_names.contains(name), "Org B should NOT see org A's cluster '{}'", name);
    }
    for name in &b_names {
        assert!(!a_names.contains(name), "Org A should NOT see org B's cluster '{}'", name);
    }
}

// ---------------------------------------------------------------------------
// Test: Org empty after team deletion allows org deletion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_deletion_succeeds_after_removing_all_teams() {
    let test_db = TestDatabase::new("org_delete_clean").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "deletable-org").await;

    let team_repo = SqlxTeamRepository::new(pool.clone());

    let team = team_repo
        .create_team(flowplane::auth::team::CreateTeamRequest {
            name: "temp-team".to_string(),
            display_name: "Temp Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_id.clone(),
            settings: None,
        })
        .await
        .expect("create temp-team");

    // Verify deletion fails while team exists
    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let blocked = org_repo.delete_organization(&org_id).await;
    assert!(blocked.is_err(), "Should not delete org with active teams");

    // Remove the team
    team_repo.delete_team(&team.id).await.expect("delete team");

    // Now org deletion should succeed
    let result = org_repo.delete_organization(&org_id).await;
    assert!(
        result.is_ok(),
        "Org deletion should succeed after removing all teams: {:?}",
        result.err()
    );

    // Verify org is gone
    let fetched = org_repo.get_organization_by_id(&org_id).await.expect("get org");
    assert!(fetched.is_none(), "Org should be deleted");
}
