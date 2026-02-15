#![cfg(feature = "postgres_tests")]

//! Cross-org isolation integration tests.
//!
//! Verifies that organization boundaries are correctly enforced:
//! - Team name resolution is org-scoped (same name in different orgs returns correct UUID)
//! - Org admin scopes cannot cross org boundaries
//! - list_teams_by_org returns only teams from the specified org
//! - Cross-org invitation revocation is blocked

use chrono::{Duration, Utc};
use flowplane::{
    auth::{
        authorization::check_resource_access, invitation::InvitationStatus, models::AuthContext,
        organization::OrgRole,
    },
    domain::{InvitationId, OrgId, TokenId},
    storage::repositories::{
        InvitationRepository, SqlxInvitationRepository, SqlxTeamRepository, TeamRepository,
    },
};

#[path = "common/mod.rs"]
mod common;
use common::test_db::{TestDatabase, TEST_ORG_ID};

use flowplane::auth::invitation_service::InvitationService;
use flowplane::auth::team::CreateTeamRequest;

/// Helper: create a second organization in the test DB.
async fn create_org(pool: &flowplane::storage::DbPool, id: &str, name: &str) {
    sqlx::query(
        "INSERT INTO organizations (id, name, display_name, status, created_at, updated_at) \
         VALUES ($1, $2, $3, 'active', NOW(), NOW()) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(id)
    .bind(name)
    .bind(format!("Org {}", name))
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to create org '{}': {}", name, e));
}

// ---------------------------------------------------------------------------
// Test 1: Cross-org team name collision — resolve_team_ids is org-scoped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_team_ids_returns_only_correct_org_team_on_name_collision() {
    let db = TestDatabase::new("org_iso_resolve_collision").await;
    let pool = db.pool().clone();
    let repo = SqlxTeamRepository::new(pool.clone());

    // Org A is the default test-org seeded by TestDatabase
    let org_a = OrgId::from_str_unchecked(TEST_ORG_ID);

    // Create Org B
    let org_b_id = "00000000-0000-0000-0000-0000000000b1";
    create_org(&pool, org_b_id, "org-b").await;
    let org_b = OrgId::from_str_unchecked(org_b_id);

    // Create team "engineering" in Org A
    let team_a = repo
        .create_team(CreateTeamRequest {
            name: "engineering".to_string(),
            display_name: "Engineering (Org A)".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a.clone(),
            settings: None,
        })
        .await
        .expect("create engineering in org A");

    // Create team "engineering" in Org B (same name, different org)
    let team_b = repo
        .create_team(CreateTeamRequest {
            name: "engineering".to_string(),
            display_name: "Engineering (Org B)".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b.clone(),
            settings: None,
        })
        .await
        .expect("create engineering in org B");

    // Resolve "engineering" with org A context → must get org A's team UUID only
    let ids_a = repo
        .resolve_team_ids(Some(&org_a), &["engineering".to_string()])
        .await
        .expect("resolve with org A");
    assert_eq!(ids_a.len(), 1);
    assert_eq!(ids_a[0], team_a.id.as_str());

    // Resolve "engineering" with org B context → must get org B's team UUID only
    let ids_b = repo
        .resolve_team_ids(Some(&org_b), &["engineering".to_string()])
        .await
        .expect("resolve with org B");
    assert_eq!(ids_b.len(), 1);
    assert_eq!(ids_b[0], team_b.id.as_str());

    // UUIDs must differ
    assert_ne!(team_a.id, team_b.id, "Same-name teams in different orgs must have distinct IDs");
}

// ---------------------------------------------------------------------------
// Test 2: Org admin cross-org access denied
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_admin_cannot_access_team_in_different_org() {
    // Create an AuthContext with org:acme:admin scope, org_name = "acme"
    let acme_org_id = OrgId::new();
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("acme-admin-token"),
        "acme-admin".into(),
        vec!["org:acme:admin".into()],
    )
    .with_org(acme_org_id, "acme".into());

    // Access to a team within acme should be granted (org admin implicit access)
    assert!(
        check_resource_access(&ctx, "routes", "read", Some("acme-engineering")),
        "Org admin should have implicit access to teams in their own org"
    );

    // Now create a context where the scope says "acme" but the user's org is "globex"
    // This simulates a corrupted scope trying to access cross-org resources
    let globex_org_id = OrgId::new();
    let cross_ctx = AuthContext::new(
        TokenId::from_str_unchecked("cross-admin-token"),
        "cross-admin".into(),
        vec!["org:acme:admin".into()],
    )
    .with_org(globex_org_id, "globex".into());

    // Should be DENIED — scope says acme but user's actual org is globex
    assert!(
        !check_resource_access(&cross_ctx, "routes", "read", Some("globex-team")),
        "Org admin scope from different org must be denied"
    );
    assert!(
        !check_resource_access(&cross_ctx, "clusters", "write", Some("globex-team")),
        "Org admin scope from different org must be denied for write"
    );
}

// ---------------------------------------------------------------------------
// Test 3: resolve_team_ids org-scoped — two orgs, same team name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_team_ids_org_scoped_isolates_results() {
    let db = TestDatabase::new("org_iso_resolve_scoped").await;
    let pool = db.pool().clone();
    let repo = SqlxTeamRepository::new(pool.clone());

    let org_a = OrgId::from_str_unchecked(TEST_ORG_ID);

    let org_b_id = "00000000-0000-0000-0000-0000000000b2";
    create_org(&pool, org_b_id, "org-b-scoped").await;
    let org_b = OrgId::from_str_unchecked(org_b_id);

    // Create "backend" in both orgs
    let team_a = repo
        .create_team(CreateTeamRequest {
            name: "backend".to_string(),
            display_name: "Backend A".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a.clone(),
            settings: None,
        })
        .await
        .expect("create backend in org A");

    let team_b = repo
        .create_team(CreateTeamRequest {
            name: "backend".to_string(),
            display_name: "Backend B".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b.clone(),
            settings: None,
        })
        .await
        .expect("create backend in org B");

    // Org A resolve
    let ids_a =
        repo.resolve_team_ids(Some(&org_a), &["backend".to_string()]).await.expect("resolve org A");
    assert_eq!(ids_a.len(), 1);
    assert_eq!(ids_a[0], team_a.id.as_str());

    // Org B resolve
    let ids_b =
        repo.resolve_team_ids(Some(&org_b), &["backend".to_string()]).await.expect("resolve org B");
    assert_eq!(ids_b.len(), 1);
    assert_eq!(ids_b[0], team_b.id.as_str());

    // Cross-check: org A should NOT resolve org B's team
    assert_ne!(ids_a[0], ids_b[0]);

    // Resolving a team that only exists in org B with org A context should error
    // First create a unique team only in org B
    repo.create_team(CreateTeamRequest {
        name: "org-b-only".to_string(),
        display_name: "Org B Only".to_string(),
        description: None,
        owner_user_id: None,
        org_id: org_b.clone(),
        settings: None,
    })
    .await
    .expect("create org-b-only team");

    let result = repo.resolve_team_ids(Some(&org_a), &["org-b-only".to_string()]).await;
    assert!(result.is_err(), "Resolving org B's team with org A context should fail");
    assert!(
        result.unwrap_err().to_string().contains("org-b-only"),
        "Error should mention the missing team name"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Org admin list_teams scoped — only see teams from their org
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_teams_by_org_returns_only_org_teams() {
    let db = TestDatabase::new("org_iso_list_teams").await;
    let pool = db.pool().clone();
    let repo = SqlxTeamRepository::new(pool.clone());

    let org_a = OrgId::from_str_unchecked(TEST_ORG_ID);

    let org_b_id = "00000000-0000-0000-0000-0000000000b3";
    create_org(&pool, org_b_id, "org-b-list").await;
    let org_b = OrgId::from_str_unchecked(org_b_id);

    // Create teams in Org A (in addition to seed teams)
    repo.create_team(CreateTeamRequest {
        name: "alpha-team".to_string(),
        display_name: "Alpha".to_string(),
        description: None,
        owner_user_id: None,
        org_id: org_a.clone(),
        settings: None,
    })
    .await
    .expect("create alpha");

    // Create teams in Org B
    repo.create_team(CreateTeamRequest {
        name: "beta-team".to_string(),
        display_name: "Beta".to_string(),
        description: None,
        owner_user_id: None,
        org_id: org_b.clone(),
        settings: None,
    })
    .await
    .expect("create beta");

    repo.create_team(CreateTeamRequest {
        name: "gamma-team".to_string(),
        display_name: "Gamma".to_string(),
        description: None,
        owner_user_id: None,
        org_id: org_b.clone(),
        settings: None,
    })
    .await
    .expect("create gamma");

    // List teams for org A
    let teams_a = repo.list_teams_by_org(&org_a).await.expect("list org A teams");
    // Should include seed teams (test-team, team-a, team-b, platform) + alpha-team
    assert!(teams_a.iter().all(|t| t.org_id == org_a), "All listed teams must belong to org A");
    assert!(
        teams_a.iter().any(|t| t.name == "alpha-team"),
        "Org A's alpha-team should be in the list"
    );
    assert!(
        !teams_a.iter().any(|t| t.name == "beta-team"),
        "Org B's beta-team must NOT appear in org A's list"
    );
    assert!(
        !teams_a.iter().any(|t| t.name == "gamma-team"),
        "Org B's gamma-team must NOT appear in org A's list"
    );

    // List teams for org B
    let teams_b = repo.list_teams_by_org(&org_b).await.expect("list org B teams");
    assert!(teams_b.iter().all(|t| t.org_id == org_b), "All listed teams must belong to org B");
    assert_eq!(teams_b.len(), 2, "Org B should have exactly 2 teams");
    assert!(teams_b.iter().any(|t| t.name == "beta-team"));
    assert!(teams_b.iter().any(|t| t.name == "gamma-team"));
    assert!(
        !teams_b.iter().any(|t| t.name == "alpha-team"),
        "Org A's alpha-team must NOT appear in org B's list"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Cross-org invitation revocation blocked
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cross_org_invitation_revocation_blocked() {
    let db = TestDatabase::new("org_iso_invite_revoke").await;
    let pool = db.pool().clone();

    // Create two orgs
    let org_a = OrgId::from_str_unchecked(TEST_ORG_ID); // already seeded
    let org_b_id = "00000000-0000-0000-0000-0000000000b4";
    create_org(&pool, org_b_id, "org-b-invite").await;
    let org_b = OrgId::from_str_unchecked(org_b_id);

    let invitation_repo = SqlxInvitationRepository::new(pool.clone());

    // Create an invitation in Org A
    let invitation_id = InvitationId::new();
    let token_hash = "test_hash_placeholder_value";
    let expires_at = Utc::now() + Duration::hours(48);

    let invitation = invitation_repo
        .create_invitation(
            &invitation_id,
            &org_a,
            "user@org-a.com",
            &OrgRole::Member,
            token_hash,
            None,
            expires_at,
        )
        .await
        .expect("create invitation in org A");

    assert_eq!(invitation.org_id, org_a);
    assert_eq!(invitation.status, InvitationStatus::Pending);

    // Now try to revoke this invitation using the InvitationService as org B admin
    let invitation_service =
        InvitationService::with_sqlx(pool.clone(), 48, "https://test.local".into());

    let org_b_admin_ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-b-admin-token"),
        "org-b-admin".into(),
        vec!["org:org-b-invite:admin".into()],
    )
    .with_org(org_b.clone(), "org-b-invite".into());

    // Attempt revocation with org B's context but passing org B's ID
    // The service should deny because the invitation belongs to org A
    let result = invitation_service
        .revoke_invitation(&org_b_admin_ctx, &invitation_id, &org_b, None, None)
        .await;

    assert!(result.is_err(), "Cross-org invitation revocation must be denied");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Not authorized") || err_msg.contains("not found"),
        "Error should indicate authorization failure or not-found, got: {}",
        err_msg
    );

    // Verify the invitation is still pending (not revoked)
    let still_pending = invitation_repo
        .get_invitation_by_id(&invitation_id)
        .await
        .expect("fetch invitation")
        .expect("invitation should still exist");

    assert_eq!(
        still_pending.status,
        InvitationStatus::Pending,
        "Invitation must remain pending after blocked cross-org revocation"
    );

    // Verify that org A admin CAN revoke their own invitation
    let org_a_admin_ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-a-admin-token"),
        "org-a-admin".into(),
        vec!["org:test-org:admin".into()],
    )
    .with_org(org_a.clone(), "test-org".into());

    let revoke_result = invitation_service
        .revoke_invitation(&org_a_admin_ctx, &invitation_id, &org_a, None, None)
        .await;

    assert!(
        revoke_result.is_ok(),
        "Org A admin should be able to revoke their own invitation: {:?}",
        revoke_result.err()
    );

    // Verify invitation is now revoked
    let revoked = invitation_repo
        .get_invitation_by_id(&invitation_id)
        .await
        .expect("fetch invitation after revoke")
        .expect("invitation should still exist");
    assert_eq!(
        revoked.status,
        InvitationStatus::Revoked,
        "Invitation should be revoked after org A admin revokes it"
    );
}

// ---------------------------------------------------------------------------
// Test: resolve_team_names is also org-scoped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_team_names_org_scoped_isolates_results() {
    let db = TestDatabase::new("org_iso_resolve_names").await;
    let pool = db.pool().clone();
    let repo = SqlxTeamRepository::new(pool.clone());

    let org_a = OrgId::from_str_unchecked(TEST_ORG_ID);

    let org_b_id = "00000000-0000-0000-0000-0000000000b5";
    create_org(&pool, org_b_id, "org-b-names").await;
    let org_b = OrgId::from_str_unchecked(org_b_id);

    // Create "frontend" in both orgs
    let team_a = repo
        .create_team(CreateTeamRequest {
            name: "frontend".to_string(),
            display_name: "Frontend A".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_a.clone(),
            settings: None,
        })
        .await
        .expect("create frontend in org A");

    let team_b = repo
        .create_team(CreateTeamRequest {
            name: "frontend".to_string(),
            display_name: "Frontend B".to_string(),
            description: None,
            owner_user_id: None,
            org_id: org_b.clone(),
            settings: None,
        })
        .await
        .expect("create frontend in org B");

    // Resolve team A's ID with org A scope → should return "frontend"
    let names_a = repo
        .resolve_team_names(Some(&org_a), &[team_a.id.as_str().to_string()])
        .await
        .expect("resolve names org A");
    assert_eq!(names_a, vec!["frontend"]);

    // Resolve team B's ID with org B scope → should return "frontend"
    let names_b = repo
        .resolve_team_names(Some(&org_b), &[team_b.id.as_str().to_string()])
        .await
        .expect("resolve names org B");
    assert_eq!(names_b, vec!["frontend"]);

    // Cross-org: resolve team B's ID with org A scope → should FAIL
    let cross_result =
        repo.resolve_team_names(Some(&org_a), &[team_b.id.as_str().to_string()]).await;
    assert!(cross_result.is_err(), "Resolving org B's team ID with org A context should fail");
}

// ---------------------------------------------------------------------------
// Test: verify_org_boundary blocks cross-org access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn verify_org_boundary_blocks_cross_org_resource_access() {
    use flowplane::auth::authorization::verify_org_boundary;

    let org_a = OrgId::new();
    let org_b = OrgId::new();

    // User belongs to org A
    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-a-user"),
        "org-a-user".into(),
        vec!["team:eng:routes:read".into()],
    )
    .with_org(org_a.clone(), "acme".into());

    // Accessing resource in same org → allowed
    assert!(
        verify_org_boundary(&ctx, &Some(org_a.clone())).is_ok(),
        "Same-org access should be allowed"
    );

    // Accessing resource in different org → blocked with NotFound
    let result = verify_org_boundary(&ctx, &Some(org_b));
    assert!(result.is_err(), "Cross-org access must be denied");

    // Admin without org context cannot bypass org boundary (governance-only)
    let admin_ctx = AuthContext::new(
        TokenId::from_str_unchecked("admin"),
        "admin".into(),
        vec!["admin:all".into()],
    );
    assert!(
        verify_org_boundary(&admin_ctx, &Some(OrgId::new())).is_err(),
        "Admin without org context should not bypass org boundary"
    );
    // But admin can access global resources (no org)
    assert!(verify_org_boundary(&admin_ctx, &None).is_ok(), "Admin can access global resources");
}
