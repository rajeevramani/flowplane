//! Integration tests for cross-organization access control scenarios.
//!
//! Tests the full matrix of org-scoped operations: admin listing orgs,
//! org-scoped team listing, cross-org scope detection, and bootstrap org
//! creation patterns.

use flowplane::auth::authorization::{
    extract_org_scopes, has_admin_bypass, has_org_admin, has_org_membership,
};
use flowplane::auth::hashing;
use flowplane::auth::models::AuthContext;
use flowplane::auth::organization::{CreateOrganizationRequest, OrgRole, OrgStatus};
use flowplane::auth::team::CreateTeamRequest;
use flowplane::auth::user::{NewUser, UserStatus};
use flowplane::domain::{TokenId, UserId};
use flowplane::storage::repositories::{
    OrgMembershipRepository, OrganizationRepository, SqlxOrgMembershipRepository,
    SqlxOrganizationRepository, SqlxTeamRepository, SqlxUserRepository, TeamRepository,
    UserRepository,
};

#[path = "../../common/mod.rs"]
mod common;
use common::test_db::TestDatabase;

// ---------------------------------------------------------------------------
// Test: System admin can list all orgs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn system_admin_lists_all_orgs() {
    let test_db = TestDatabase::new("admin_list_orgs").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());

    // Create multiple orgs
    for name in &["alpha", "beta", "gamma"] {
        org_repo
            .create_organization(CreateOrganizationRequest {
                name: name.to_string(),
                display_name: format!("Org {}", name),
                description: None,
                owner_user_id: None,
                settings: None,
            })
            .await
            .unwrap_or_else(|e| panic!("Failed to create org '{}': {}", name, e));
    }

    // System admin context has admin:all
    let admin_ctx = AuthContext::new(
        TokenId::from_str_unchecked("admin-token"),
        "admin".into(),
        vec!["admin:all".into()],
    );

    assert!(has_admin_bypass(&admin_ctx));

    // Admin can list all orgs (3 created + 1 seeded test-org)
    let orgs = org_repo.list_organizations(100, 0).await.expect("list orgs");
    assert_eq!(orgs.len(), 4, "Admin should see all orgs (3 created + 1 seeded)");

    let count = org_repo.count_organizations().await.expect("count orgs");
    assert_eq!(count, 4);
}

// ---------------------------------------------------------------------------
// Test: Org admin lists only their org's teams (filtered at repo level)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_admin_sees_only_own_org_teams() {
    let test_db = TestDatabase::new("org_admin_teams").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let team_repo = SqlxTeamRepository::new(pool.clone());

    // Create two orgs
    let org_a = org_repo
        .create_organization(CreateOrganizationRequest {
            name: "org-alpha".to_string(),
            display_name: "Org Alpha".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org alpha");

    let org_b = org_repo
        .create_organization(CreateOrganizationRequest {
            name: "org-beta".to_string(),
            display_name: "Org Beta".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org beta");

    // Create teams in each org
    team_repo
        .create_team(CreateTeamRequest {
            name: "alpha-engineering".to_string(),
            display_name: "Alpha Engineering".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a.id.clone(),
            settings: None,
        })
        .await
        .expect("create alpha-engineering");

    team_repo
        .create_team(CreateTeamRequest {
            name: "alpha-platform".to_string(),
            display_name: "Alpha Platform".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a.id.clone(),
            settings: None,
        })
        .await
        .expect("create alpha-platform");

    team_repo
        .create_team(CreateTeamRequest {
            name: "beta-frontend".to_string(),
            display_name: "Beta Frontend".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b.id.clone(),
            settings: None,
        })
        .await
        .expect("create beta-frontend");

    // Org admin for alpha can only see alpha's teams via list_teams_by_org
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-admin"),
        "org-admin".into(),
        vec!["org:org-alpha:admin".into()],
    );
    assert!(has_org_admin(&ctx, "org-alpha"));
    assert!(!has_org_admin(&ctx, "org-beta"));

    let alpha_teams = team_repo.list_teams_by_org(&org_a.id).await.expect("list alpha teams");
    assert_eq!(alpha_teams.len(), 2);
    let names: Vec<&str> = alpha_teams.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"alpha-engineering"));
    assert!(names.contains(&"alpha-platform"));
    assert!(!names.contains(&"beta-frontend"));

    let beta_teams = team_repo.list_teams_by_org(&org_b.id).await.expect("list beta teams");
    assert_eq!(beta_teams.len(), 1);
    assert_eq!(beta_teams[0].name, "beta-frontend");
}

// ---------------------------------------------------------------------------
// Test: Cross-org scope detection rejects tokens with scopes from multiple orgs
// ---------------------------------------------------------------------------

#[test]
fn cross_org_scopes_are_detected() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("cross-org-token"),
        "cross-org".into(),
        vec![
            "org:acme:admin".into(),
            "org:globex:admin".into(), // Different org!
        ],
    );

    let org_scopes = extract_org_scopes(&ctx);
    assert_eq!(org_scopes.len(), 2, "Should detect scopes from two orgs");

    // Application code can detect this situation and reject:
    let unique_orgs: std::collections::HashSet<&str> =
        org_scopes.iter().map(|(name, _)| name.as_str()).collect();
    assert!(unique_orgs.len() > 1, "Cross-org scopes should be detectable for rejection");
}

// ---------------------------------------------------------------------------
// Test: Org deletion with active teams is rejected with friendly error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_deletion_blocked_when_teams_exist() {
    let test_db = TestDatabase::new("org_delete_teams").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let team_repo = SqlxTeamRepository::new(pool.clone());

    let org = org_repo
        .create_organization(CreateOrganizationRequest {
            name: "delete-test-org".to_string(),
            display_name: "Delete Test Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org");

    team_repo
        .create_team(CreateTeamRequest {
            name: "blocking-team".to_string(),
            display_name: "Blocking Team".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org.id.clone(),
            settings: None,
        })
        .await
        .expect("create team");

    let result = org_repo.delete_organization(&org.id).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Cannot delete organization"),
        "Should get friendly FK violation message, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// Test: Bootstrap-style org creation (create org, verify it exists)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bootstrap_creates_default_org_pattern() {
    let test_db = TestDatabase::new("bootstrap_default_org").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());

    // Simulate bootstrap: check if default org exists, create if not
    let default_org_name = "default";
    let existing =
        org_repo.get_organization_by_name(default_org_name).await.expect("check existing");
    assert!(existing.is_none(), "Default org should not exist initially");

    let created = org_repo
        .create_organization(CreateOrganizationRequest {
            name: default_org_name.to_string(),
            display_name: "Default Organization".to_string(),
            description: Some("Auto-created during bootstrap".to_string()),
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create default org");

    assert_eq!(created.name, "default");
    assert_eq!(created.status, OrgStatus::Active);

    // Verify it's retrievable by name
    let fetched = org_repo
        .get_organization_by_name(default_org_name)
        .await
        .expect("get by name")
        .expect("org should exist");
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.display_name, "Default Organization");
}

// ---------------------------------------------------------------------------
// Test: Org name availability check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_name_availability() {
    let test_db = TestDatabase::new("org_name_avail").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());

    assert!(org_repo.is_name_available("new-org").await.expect("check"));

    org_repo
        .create_organization(CreateOrganizationRequest {
            name: "new-org".to_string(),
            display_name: "New Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org");

    assert!(!org_repo.is_name_available("new-org").await.expect("check taken"));
}

// ---------------------------------------------------------------------------
// Test: Org update preserves immutable name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_update_preserves_name() {
    let test_db = TestDatabase::new("org_update_name").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());

    let org = org_repo
        .create_organization(CreateOrganizationRequest {
            name: "immutable-name".to_string(),
            display_name: "Original Display".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org");

    let updated = org_repo
        .update_organization(
            &org.id,
            flowplane::auth::organization::UpdateOrganizationRequest {
                display_name: Some("Updated Display".to_string()),
                description: Some("New description".to_string()),
                owner_user_id: None,
                settings: None,
                status: None,
            },
        )
        .await
        .expect("update org");

    assert_eq!(updated.name, "immutable-name", "Name should be immutable");
    assert_eq!(updated.display_name, "Updated Display");
    assert_eq!(updated.description.as_deref(), Some("New description"));
}

// ---------------------------------------------------------------------------
// Test: Org status transitions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_status_transitions() {
    let test_db = TestDatabase::new("org_status_transitions").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());

    let org = org_repo
        .create_organization(CreateOrganizationRequest {
            name: "status-org".to_string(),
            display_name: "Status Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org");

    assert_eq!(org.status, OrgStatus::Active);

    // Suspend the org
    let suspended = org_repo
        .update_organization(
            &org.id,
            flowplane::auth::organization::UpdateOrganizationRequest {
                display_name: None,
                description: None,
                owner_user_id: None,
                settings: None,
                status: Some(OrgStatus::Suspended),
            },
        )
        .await
        .expect("suspend org");
    assert_eq!(suspended.status, OrgStatus::Suspended);

    // Archive the org
    let archived = org_repo
        .update_organization(
            &org.id,
            flowplane::auth::organization::UpdateOrganizationRequest {
                display_name: None,
                description: None,
                owner_user_id: None,
                settings: None,
                status: Some(OrgStatus::Archived),
            },
        )
        .await
        .expect("archive org");
    assert_eq!(archived.status, OrgStatus::Archived);
}

// ---------------------------------------------------------------------------
// Test: Membership role upgrade and downgrade
// ---------------------------------------------------------------------------

#[tokio::test]
async fn membership_role_upgrade_and_downgrade() {
    let test_db = TestDatabase::new("membership_roles").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let membership_repo = SqlxOrgMembershipRepository::new(pool.clone());

    let org = org_repo
        .create_organization(CreateOrganizationRequest {
            name: "role-test-org".to_string(),
            display_name: "Role Test Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org");

    // Create two users, both as owners (so we can downgrade one)
    let user_repo = SqlxUserRepository::new(pool.clone());
    let password_hash = hashing::hash_password("TestPass123!").expect("hash");

    let user1_id = UserId::new();
    user_repo
        .create_user(NewUser {
            id: user1_id.clone(),
            email: "user1@roles.com".to_string(),
            password_hash: password_hash.clone(),
            name: "User 1".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: org.id.clone(),
        })
        .await
        .expect("create user1");

    let user2_id = UserId::new();
    user_repo
        .create_user(NewUser {
            id: user2_id.clone(),
            email: "user2@roles.com".to_string(),
            password_hash,
            name: "User 2".to_string(),
            status: UserStatus::Active,
            is_admin: false,
            org_id: org.id.clone(),
        })
        .await
        .expect("create user2");

    // Make both owners
    membership_repo.create_membership(&user1_id, &org.id, OrgRole::Owner).await.expect("owner1");
    membership_repo.create_membership(&user2_id, &org.id, OrgRole::Owner).await.expect("owner2");

    // Downgrade user1 to member (allowed because user2 is still an owner)
    let downgraded = membership_repo
        .update_membership_role(&user1_id, &org.id, OrgRole::Member)
        .await
        .expect("downgrade user1");
    assert_eq!(downgraded.role, OrgRole::Member);

    // Upgrade user1 to admin
    let upgraded = membership_repo
        .update_membership_role(&user1_id, &org.id, OrgRole::Admin)
        .await
        .expect("upgrade user1");
    assert_eq!(upgraded.role, OrgRole::Admin);

    // Now try to downgrade user2 (the last owner) - should fail
    let result = membership_repo.update_membership_role(&user2_id, &org.id, OrgRole::Member).await;
    assert!(result.is_err(), "Downgrading the last owner should fail");
}

// ---------------------------------------------------------------------------
// Test: org_membership scope check with mixed scopes
// ---------------------------------------------------------------------------

#[test]
fn has_org_membership_with_mixed_scopes() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("mixed-scope-token"),
        "mixed".into(),
        vec!["org:acme:admin".into(), "team:engineering:routes:read".into(), "routes:read".into()],
    );

    assert!(has_org_membership(&ctx, "acme"));
    assert!(!has_org_membership(&ctx, "other-org"));
    assert!(has_org_admin(&ctx, "acme"));
    assert!(!has_admin_bypass(&ctx));
}

// ---------------------------------------------------------------------------
// Test: check_resource_access with global scope + non-admin = denied
// ---------------------------------------------------------------------------

#[test]
fn check_resource_access_global_scope_non_admin_denied() {
    use flowplane::auth::authorization::check_resource_access;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("global-user"),
        "global-user".into(),
        vec!["clusters:read".into(), "routes:write".into()],
    );

    // Non-admin with global scopes should be DENIED (security fix)
    assert!(!check_resource_access(&ctx, "clusters", "read", None));
    assert!(!check_resource_access(&ctx, "routes", "write", None));
    assert!(!check_resource_access(&ctx, "clusters", "read", Some("any-team")));
    assert!(!check_resource_access(&ctx, "routes", "write", Some("any-team")));
}

// ---------------------------------------------------------------------------
// Test: check_resource_access with global scope + admin = allowed
// ---------------------------------------------------------------------------

#[test]
fn check_resource_access_global_scope_admin_allowed() {
    use flowplane::auth::authorization::check_resource_access;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("admin-token"),
        "admin".into(),
        vec!["admin:all".into()],
    );

    // Admin always has access
    assert!(check_resource_access(&ctx, "clusters", "read", None));
    assert!(check_resource_access(&ctx, "routes", "write", Some("any-team")));
    assert!(check_resource_access(&ctx, "listeners", "delete", Some("engineering")));
}

// ---------------------------------------------------------------------------
// Test: check_resource_access with correct team scope = allowed
// ---------------------------------------------------------------------------

#[test]
fn check_resource_access_correct_team_scope_allowed() {
    use flowplane::auth::authorization::check_resource_access;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("team-user"),
        "team-user".into(),
        vec!["team:engineering:clusters:read".into(), "team:engineering:routes:write".into()],
    );

    assert!(check_resource_access(&ctx, "clusters", "read", Some("engineering")));
    assert!(check_resource_access(&ctx, "routes", "write", Some("engineering")));
}

// ---------------------------------------------------------------------------
// Test: check_resource_access with wrong team scope = denied
// ---------------------------------------------------------------------------

#[test]
fn check_resource_access_wrong_team_scope_denied() {
    use flowplane::auth::authorization::check_resource_access;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("team-user"),
        "team-user".into(),
        vec!["team:engineering:clusters:read".into()],
    );

    // Wrong team
    assert!(!check_resource_access(&ctx, "clusters", "read", Some("platform")));
    // Wrong action
    assert!(!check_resource_access(&ctx, "clusters", "write", Some("engineering")));
    // Wrong resource
    assert!(!check_resource_access(&ctx, "routes", "read", Some("engineering")));
}

// ---------------------------------------------------------------------------
// Test: verify_org_boundary cross-org returns NotFound (not Forbidden)
// ---------------------------------------------------------------------------

#[test]
fn verify_org_boundary_cross_org_returns_not_found() {
    use flowplane::api::error::ApiError;
    use flowplane::auth::authorization::verify_org_boundary;
    use flowplane::domain::OrgId;

    let user_org = OrgId::new();
    let team_org = OrgId::new();

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-user"),
        "org-user".into(),
        vec!["team:eng:routes:read".into()],
    )
    .with_org(user_org, "acme".into());

    let result = verify_org_boundary(&ctx, &Some(team_org));
    assert!(result.is_err(), "Cross-org access should be denied");

    // Returns 404 (not 403) to prevent enumeration
    if let Err(ApiError::NotFound(msg)) = result {
        assert_eq!(msg, "Resource not found");
    } else {
        panic!("Expected NotFound error, got: {:?}", result);
    }
}

// ---------------------------------------------------------------------------
// Test: verify_org_boundary same-org returns Ok
// ---------------------------------------------------------------------------

#[test]
fn verify_org_boundary_same_org_returns_ok() {
    use flowplane::auth::authorization::verify_org_boundary;
    use flowplane::domain::OrgId;

    let org = OrgId::new();

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-user"),
        "org-user".into(),
        vec!["team:eng:routes:read".into()],
    )
    .with_org(org.clone(), "acme".into());

    assert!(verify_org_boundary(&ctx, &Some(org)).is_ok());
}

// ---------------------------------------------------------------------------
// Test: verify_org_boundary admin bypasses check
// ---------------------------------------------------------------------------

#[test]
fn verify_org_boundary_admin_bypasses() {
    use flowplane::auth::authorization::verify_org_boundary;
    use flowplane::domain::OrgId;

    let org_a = OrgId::new();
    let org_b = OrgId::new();

    let admin_ctx = AuthContext::new(
        TokenId::from_str_unchecked("admin"),
        "admin".into(),
        vec!["admin:all".into()],
    );

    // Admin can cross org boundaries
    assert!(verify_org_boundary(&admin_ctx, &Some(org_a)).is_ok());
    assert!(verify_org_boundary(&admin_ctx, &Some(org_b)).is_ok());
    assert!(verify_org_boundary(&admin_ctx, &None).is_ok());
}

// ---------------------------------------------------------------------------
// Test: Global resource scopes classified correctly for security enforcement
// ---------------------------------------------------------------------------

#[test]
fn global_resource_scopes_identified_for_restriction() {
    use flowplane::auth::authorization::is_global_resource_scope;

    // Scopes that SHOULD be flagged as global (restricted to platform admins)
    for scope in &["clusters:read", "routes:write", "listeners:read", "secrets:write"] {
        assert!(
            is_global_resource_scope(scope),
            "Scope '{}' should be classified as global resource scope",
            scope
        );
    }

    // Scopes that should NOT be flagged as global
    for scope in &["admin:all", "team:eng:routes:read", "org:acme:admin"] {
        assert!(
            !is_global_resource_scope(scope),
            "Scope '{}' should NOT be classified as global resource scope",
            scope
        );
    }
}

// ---------------------------------------------------------------------------
// Test: Non-admin with global scopes is denied resource access
// ---------------------------------------------------------------------------

#[test]
fn non_admin_global_scopes_denied_resource_access() {
    use flowplane::auth::authorization::{check_resource_access, require_resource_access};

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("escalation-attempt"),
        "attacker".into(),
        vec!["clusters:read".into(), "routes:write".into()],
    );

    // Security: non-admin with global scopes denied for all teams
    assert!(!check_resource_access(&ctx, "clusters", "read", Some("engineering")));
    assert!(!check_resource_access(&ctx, "routes", "write", Some("platform")));
    assert!(!check_resource_access(&ctx, "clusters", "read", None));

    // require_resource_access returns Forbidden
    let err = require_resource_access(&ctx, "clusters", "read", None).unwrap_err();
    assert!(
        matches!(err, flowplane::auth::models::AuthError::Forbidden),
        "Expected Forbidden error"
    );
}

// ---------------------------------------------------------------------------
// Test: Cross-org scope detection identifies tokens with multiple org scopes
// ---------------------------------------------------------------------------

#[test]
fn cross_org_scope_detection_multiple_orgs() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("cross-org-attempt"),
        "cross-org".into(),
        vec!["org:acme:admin".into(), "org:globex:member".into()],
    );

    let org_scopes = extract_org_scopes(&ctx);
    let unique_orgs: std::collections::HashSet<&str> =
        org_scopes.iter().map(|(name, _)| name.as_str()).collect();

    assert!(unique_orgs.len() > 1, "Should detect scopes from multiple orgs for rejection");
    assert!(unique_orgs.contains("acme"));
    assert!(unique_orgs.contains("globex"));
}

// ---------------------------------------------------------------------------
// Test: get_effective_team_scopes returns empty for non-admin with no team scopes
// ---------------------------------------------------------------------------

#[test]
fn get_effective_team_scopes_empty_for_no_team_scopes() {
    use flowplane::api::handlers::team_access::get_effective_team_scopes;

    // Non-admin user with only global scopes
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("global-only"),
        "global".into(),
        vec!["routes:read".into(), "clusters:read".into()],
    );

    let scopes = get_effective_team_scopes(&ctx);
    assert!(
        scopes.is_empty(),
        "Non-admin with no team scopes should get empty team list, got {:?}",
        scopes
    );
}

// ---------------------------------------------------------------------------
// Test: get_effective_team_scopes returns empty for admin (bypass)
// ---------------------------------------------------------------------------

#[test]
fn get_effective_team_scopes_empty_for_admin() {
    use flowplane::api::handlers::team_access::get_effective_team_scopes;

    let admin_ctx = AuthContext::new(
        TokenId::from_str_unchecked("admin"),
        "admin".into(),
        vec!["admin:all".into()],
    );

    let scopes = get_effective_team_scopes(&admin_ctx);
    assert!(scopes.is_empty(), "Admin should get empty team scopes (bypass), got {:?}", scopes);
}

// ---------------------------------------------------------------------------
// Test: get_effective_team_scopes returns team names for team-scoped user
// ---------------------------------------------------------------------------

#[test]
fn get_effective_team_scopes_returns_teams_for_user() {
    use flowplane::api::handlers::team_access::get_effective_team_scopes;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("team-user"),
        "team-user".into(),
        vec![
            "team:engineering:routes:read".into(),
            "team:engineering:clusters:write".into(),
            "team:platform:routes:read".into(),
        ],
    );

    let scopes = get_effective_team_scopes(&ctx);
    assert_eq!(scopes.len(), 2, "Should have 2 unique teams");
    assert!(scopes.contains(&"engineering".to_string()));
    assert!(scopes.contains(&"platform".to_string()));
}

// ---------------------------------------------------------------------------
// Test: is_global_resource_scope classification
// ---------------------------------------------------------------------------

#[test]
fn is_global_resource_scope_classification() {
    use flowplane::auth::authorization::is_global_resource_scope;

    // These ARE global resource scopes (dangerous for non-admins)
    assert!(is_global_resource_scope("clusters:read"));
    assert!(is_global_resource_scope("routes:write"));
    assert!(is_global_resource_scope("listeners:read"));
    assert!(is_global_resource_scope("openapi-import:write"));
    assert!(is_global_resource_scope("secrets:read"));

    // These are NOT global resource scopes
    assert!(!is_global_resource_scope("admin:all")); // admin bypass
    assert!(!is_global_resource_scope("team:eng:routes:read")); // team-prefixed
    assert!(!is_global_resource_scope("org:acme:admin")); // org scope
    assert!(!is_global_resource_scope("team:platform:*:*")); // wildcard
}

// ---------------------------------------------------------------------------
// Test: Org admin scope authorizes team creation in own org
// ---------------------------------------------------------------------------

#[test]
fn org_admin_scope_authorizes_own_org_team_creation() {
    use flowplane::auth::authorization::{has_org_admin, require_org_admin};

    // Org admin for acme
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-admin-acme"),
        "acme-admin".into(),
        vec!["org:acme:admin".into()],
    );

    // Can manage teams in acme
    assert!(has_org_admin(&ctx, "acme"));
    assert!(require_org_admin(&ctx, "acme").is_ok());

    // CANNOT create teams in another org
    assert!(!has_org_admin(&ctx, "globex"));
    assert!(require_org_admin(&ctx, "globex").is_err());
}

// ---------------------------------------------------------------------------
// Test: Org member cannot create teams (requires admin)
// ---------------------------------------------------------------------------

#[test]
fn org_member_cannot_create_teams() {
    use flowplane::auth::authorization::require_org_admin;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-member-acme"),
        "acme-member".into(),
        vec!["org:acme:member".into()],
    );

    // Member should NOT be able to manage teams (403)
    let result = require_org_admin(&ctx, "acme");
    assert!(result.is_err(), "Org member should not have admin privileges");
}

// ---------------------------------------------------------------------------
// Test: Token with scopes from two orgs detected for rejection
// ---------------------------------------------------------------------------

#[test]
fn token_with_multi_org_scopes_detected_for_rejection() {
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("multi-org-token"),
        "multi-org".into(),
        vec!["org:acme:admin".into(), "org:globex:admin".into(), "team:eng:routes:read".into()],
    );

    let org_scopes = extract_org_scopes(&ctx);

    // Detect scopes from multiple orgs
    let unique_orgs: std::collections::BTreeSet<&str> =
        org_scopes.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(unique_orgs.len(), 2, "Should detect scopes from two different orgs");
    assert!(unique_orgs.contains("acme"));
    assert!(unique_orgs.contains("globex"));

    // Application should reject tokens with multi-org scopes
    let has_cross_org = unique_orgs.len() > 1;
    assert!(has_cross_org, "Cross-org tokens must be detected and can be rejected");
}

// ---------------------------------------------------------------------------
// Test: verify_org_boundary with user in org and team in different org returns NotFound
// ---------------------------------------------------------------------------

#[test]
fn verify_org_boundary_cross_org_consistent_with_team_scoping() {
    use flowplane::api::error::ApiError;
    use flowplane::auth::authorization::verify_org_boundary;
    use flowplane::domain::OrgId;

    let org_acme = OrgId::new();
    let org_globex = OrgId::new();

    // User in org acme
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("acme-user"),
        "acme-user".into(),
        vec!["org:acme:admin".into(), "team:eng:clusters:read".into()],
    )
    .with_org(org_acme.clone(), "acme".into());

    // Same org -- allowed
    assert!(verify_org_boundary(&ctx, &Some(org_acme.clone())).is_ok());

    // Different org -- returns NotFound (not Forbidden) to prevent enumeration
    let result = verify_org_boundary(&ctx, &Some(org_globex));
    assert!(result.is_err());
    match result {
        Err(ApiError::NotFound(_)) => {} // expected
        other => panic!("Expected NotFound, got: {:?}", other),
    }

    // No org on team (global) -- allowed
    assert!(verify_org_boundary(&ctx, &None).is_ok());
}

// ---------------------------------------------------------------------------
// Test: Org admin for one org cannot list teams for a different org
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_admin_for_one_org_cannot_access_other_org_teams_via_repo() {
    let test_db = TestDatabase::new("org_admin_cross_list").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let team_repo = SqlxTeamRepository::new(pool.clone());

    // Create two orgs
    let org_a = org_repo
        .create_organization(CreateOrganizationRequest {
            name: "admin-test-a".to_string(),
            display_name: "Admin Test A".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org A");

    let org_b = org_repo
        .create_organization(CreateOrganizationRequest {
            name: "admin-test-b".to_string(),
            display_name: "Admin Test B".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org B");

    // Create teams
    team_repo
        .create_team(flowplane::auth::team::CreateTeamRequest {
            name: "team-in-a".to_string(),
            display_name: "Team in A".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a.id.clone(),
            settings: None,
        })
        .await
        .expect("create team-in-a");

    team_repo
        .create_team(flowplane::auth::team::CreateTeamRequest {
            name: "team-in-b".to_string(),
            display_name: "Team in B".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b.id.clone(),
            settings: None,
        })
        .await
        .expect("create team-in-b");

    // Verify: if someone queries with org_a's ID, they get only org_a teams
    let a_teams = team_repo.list_teams_by_org(&org_a.id).await.expect("list org A teams");
    assert_eq!(a_teams.len(), 1);
    assert_eq!(a_teams[0].name, "team-in-a");

    // Org A's list does NOT contain org B's teams
    let a_names: Vec<&str> = a_teams.iter().map(|t| t.name.as_str()).collect();
    assert!(!a_names.contains(&"team-in-b"), "Org A should NOT see Org B's teams");
}

// ---------------------------------------------------------------------------
// Test: Org admin can create and then list teams within their org (full flow)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_admin_create_and_list_teams_flow() {
    let test_db = TestDatabase::new("org_admin_full_flow").await;
    let pool = test_db.pool().clone();

    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let team_repo = SqlxTeamRepository::new(pool.clone());

    // Create an org
    let org = org_repo
        .create_organization(CreateOrganizationRequest {
            name: "flow-test-org".to_string(),
            display_name: "Flow Test Org".to_string(),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create org");

    // Org admin context
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("flow-admin"),
        "flow-admin".into(),
        vec!["org:flow-test-org:admin".into()],
    );

    // Verify admin privileges
    assert!(has_org_admin(&ctx, "flow-test-org"));

    // Create teams via repo (simulating what the handler does after auth check)
    for name in &["flow-eng", "flow-platform", "flow-data"] {
        team_repo
            .create_team(flowplane::auth::team::CreateTeamRequest {
                name: name.to_string(),
                display_name: format!("Flow {}", name),
                description: None,
                owner_user_id: None,
                org_id: org.id.clone(),
                settings: None,
            })
            .await
            .unwrap_or_else(|e| panic!("Failed to create team '{}': {}", name, e));
    }

    // List teams
    let teams = team_repo.list_teams_by_org(&org.id).await.expect("list teams");
    assert_eq!(teams.len(), 3, "Should have 3 teams");

    let names: Vec<&str> = teams.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"flow-eng"));
    assert!(names.contains(&"flow-platform"));
    assert!(names.contains(&"flow-data"));
}
