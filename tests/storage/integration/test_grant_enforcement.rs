#![cfg(feature = "postgres_tests")]

//! Integration tests for grant-based access control enforcement.
//!
//! Tests the full DB → permission loading → enforcement chain:
//! seed grants into the `grants` table, load them via `load_permissions()`,
//! build an `AuthContext`, and verify `check_resource_access()` results.

use flowplane::auth::authorization::check_resource_access;
use flowplane::auth::models::AuthContext;
use flowplane::auth::organization::{CreateOrganizationRequest, OrgRole};
use flowplane::auth::permissions::load_permissions;
use flowplane::auth::team::CreateTeamRequest;
use flowplane::auth::user::{NewUser, NewUserTeamMembership, UserStatus};
use flowplane::domain::{OrgId, TokenId, UserId};
use flowplane::storage::repositories::{
    OrgMembershipRepository, OrganizationRepository, SqlxOrgMembershipRepository,
    SqlxOrganizationRepository, SqlxTeamMembershipRepository, SqlxTeamRepository,
    SqlxUserRepository, TeamMembershipRepository, TeamRepository, UserRepository,
};
use flowplane::storage::DbPool;

#[path = "../../common/mod.rs"]
mod common;
use common::test_db::TestDatabase;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

async fn create_user(pool: &DbPool, email: &str) -> UserId {
    let user_repo = SqlxUserRepository::new(pool.clone());
    let user_id = UserId::new();
    user_repo
        .create_user(NewUser {
            id: user_id.clone(),
            email: email.to_string(),
            password_hash: "dummy-hash".to_string(),
            name: email.split('@').next().unwrap_or("user").to_string(),
            status: UserStatus::Active,
            is_admin: false,
        })
        .await
        .unwrap_or_else(|e| panic!("Failed to create user '{}': {}", email, e));
    user_id
}

async fn create_team(pool: &DbPool, name: &str, org_id: &OrgId) -> String {
    let repo = SqlxTeamRepository::new(pool.clone());
    let team = repo
        .create_team(CreateTeamRequest {
            name: name.to_string(),
            display_name: format!("Team {}", name),
            description: None,
            owner_user_id: None,
            org_id: org_id.clone(),
            settings: None,
        })
        .await
        .unwrap_or_else(|e| panic!("Failed to create team '{}': {}", name, e));
    team.id.to_string()
}

async fn add_org_membership(pool: &DbPool, user_id: &UserId, org_id: &OrgId, role: OrgRole) {
    let repo = SqlxOrgMembershipRepository::new(pool.clone());
    repo.create_membership(user_id, org_id, role)
        .await
        .unwrap_or_else(|e| panic!("Failed to create org membership: {}", e));
}

async fn add_team_membership(pool: &DbPool, user_id: &UserId, team_id: &str) {
    let repo = SqlxTeamMembershipRepository::new(pool.clone());
    repo.create_membership(NewUserTeamMembership {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: user_id.clone(),
        team: team_id.to_string(),
    })
    .await
    .unwrap_or_else(|e| panic!("Failed to create team membership: {}", e));
}

/// Insert a resource grant directly into the `grants` table.
async fn insert_grant(
    pool: &DbPool,
    user_id: &UserId,
    org_id: &OrgId,
    team_id: &str,
    resource_type: &str,
    action: &str,
    expires_at: Option<&str>,
) {
    let id = uuid::Uuid::new_v4().to_string();
    let query = if let Some(exp) = expires_at {
        sqlx::query(
            "INSERT INTO grants (id, principal_id, org_id, team_id, grant_type, resource_type, action, created_by, expires_at) \
             VALUES ($1, $2, $3, $4, 'resource', $5, $6, $2, $7::timestamptz)",
        )
        .bind(&id)
        .bind(user_id.as_str())
        .bind(org_id.as_str())
        .bind(team_id)
        .bind(resource_type)
        .bind(action)
        .bind(exp)
    } else {
        sqlx::query(
            "INSERT INTO grants (id, principal_id, org_id, team_id, grant_type, resource_type, action, created_by) \
             VALUES ($1, $2, $3, $4, 'resource', $5, $6, $2)",
        )
        .bind(&id)
        .bind(user_id.as_str())
        .bind(org_id.as_str())
        .bind(team_id)
        .bind(resource_type)
        .bind(action)
    };
    query.execute(pool).await.unwrap_or_else(|e| panic!("Failed to insert grant: {}", e));
}

/// Load permissions for a user and build an AuthContext with grants populated.
async fn build_auth_context(pool: &DbPool, user_id: &UserId, token_name: &str) -> AuthContext {
    let perms = load_permissions(pool, user_id)
        .await
        .unwrap_or_else(|e| panic!("Failed to load permissions: {}", e));

    let org_scopes: Vec<String> = perms.org_scopes.into_iter().collect();
    let mut ctx = AuthContext::new(
        TokenId::from_str_unchecked(token_name),
        token_name.to_string(),
        org_scopes,
    );
    ctx.grants = perms.grants;

    if let (Some(org_id), Some(org_name)) = (perms.org_id, perms.org_name) {
        ctx = ctx.with_org(org_id, org_name);
    }

    ctx
}

// ---------------------------------------------------------------------------
// Test 1: Grant allows matching access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grant_allows_matching_access() {
    let test_db = TestDatabase::new("grant_allows_match").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "grant-match-org").await;
    let user_id = create_user(&pool, "alice@grant-match.com").await;
    let team_id = create_team(&pool, "team-alpha", &org_id).await;

    add_org_membership(&pool, &user_id, &org_id, OrgRole::Member).await;
    add_team_membership(&pool, &user_id, &team_id).await;
    insert_grant(&pool, &user_id, &org_id, &team_id, "clusters", "read", None).await;

    let ctx = build_auth_context(&pool, &user_id, "alice-token").await;

    assert!(
        check_resource_access(&ctx, "clusters", "read", Some("team-alpha")),
        "User with clusters:read grant should have access to read clusters on team-alpha"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Grant denies wrong team
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grant_denies_wrong_team() {
    let test_db = TestDatabase::new("grant_wrong_team").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "wrong-team-org").await;
    let user_id = create_user(&pool, "bob@wrong-team.com").await;
    let team_x_id = create_team(&pool, "team-x", &org_id).await;
    let _team_y_id = create_team(&pool, "team-y", &org_id).await;

    add_org_membership(&pool, &user_id, &org_id, OrgRole::Member).await;
    add_team_membership(&pool, &user_id, &team_x_id).await;
    insert_grant(&pool, &user_id, &org_id, &team_x_id, "clusters", "read", None).await;

    let ctx = build_auth_context(&pool, &user_id, "bob-token").await;

    // Has access to team-x
    assert!(
        check_resource_access(&ctx, "clusters", "read", Some("team-x")),
        "User should access their own team"
    );
    // Denied access to team-y (no grant for that team)
    assert!(
        !check_resource_access(&ctx, "clusters", "read", Some("team-y")),
        "User with team-x grant should NOT access team-y"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Grant denies wrong action
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grant_denies_wrong_action() {
    let test_db = TestDatabase::new("grant_wrong_action").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "wrong-action-org").await;
    let user_id = create_user(&pool, "carol@wrong-action.com").await;
    let team_id = create_team(&pool, "team-gamma", &org_id).await;

    add_org_membership(&pool, &user_id, &org_id, OrgRole::Member).await;
    add_team_membership(&pool, &user_id, &team_id).await;
    insert_grant(&pool, &user_id, &org_id, &team_id, "clusters", "read", None).await;

    let ctx = build_auth_context(&pool, &user_id, "carol-token").await;

    // read is allowed
    assert!(check_resource_access(&ctx, "clusters", "read", Some("team-gamma")));
    // create is denied (only read was granted)
    assert!(
        !check_resource_access(&ctx, "clusters", "create", Some("team-gamma")),
        "User with clusters:read should NOT have clusters:create"
    );
    // delete is denied
    assert!(!check_resource_access(&ctx, "clusters", "delete", Some("team-gamma")));
}

// ---------------------------------------------------------------------------
// Test 4: Org admin gets implicit access (no explicit grants needed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_org_admin_implicit_access() {
    let test_db = TestDatabase::new("org_admin_implicit").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "admin-implicit-org").await;
    let user_id = create_user(&pool, "admin@implicit.com").await;
    let _team_id = create_team(&pool, "team-delta", &org_id).await;

    // Make user an org admin — no explicit resource grants
    add_org_membership(&pool, &user_id, &org_id, OrgRole::Admin).await;

    let ctx = build_auth_context(&pool, &user_id, "admin-token").await;

    // Org admin should have implicit access to any team in their org
    assert!(
        check_resource_access(&ctx, "clusters", "read", Some("team-delta")),
        "Org admin should have implicit access to team resources"
    );
    assert!(
        check_resource_access(&ctx, "routes", "create", Some("team-delta")),
        "Org admin should have implicit write access"
    );
    assert!(
        check_resource_access(&ctx, "listeners", "delete", Some("team-delta")),
        "Org admin should have implicit delete access"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Expired grant is denied
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_expired_grant_denied() {
    let test_db = TestDatabase::new("grant_expired").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "expired-grant-org").await;
    let user_id = create_user(&pool, "dave@expired.com").await;
    let team_id = create_team(&pool, "team-epsilon", &org_id).await;

    add_org_membership(&pool, &user_id, &org_id, OrgRole::Member).await;
    add_team_membership(&pool, &user_id, &team_id).await;

    // Insert a grant that expired yesterday
    insert_grant(
        &pool,
        &user_id,
        &org_id,
        &team_id,
        "clusters",
        "read",
        Some("2020-01-01 00:00:00+00"),
    )
    .await;

    let ctx = build_auth_context(&pool, &user_id, "dave-token").await;

    // Expired grant should not be loaded by load_permissions (filtered by NOW())
    assert!(
        !check_resource_access(&ctx, "clusters", "read", Some("team-epsilon")),
        "Expired grant should be denied"
    );
    // Verify no grants were loaded
    assert!(ctx.grants.is_empty(), "Expired grants should not be loaded");
}

// ---------------------------------------------------------------------------
// Test 6: Multi-org grant isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multi_org_grant_isolation() {
    let test_db = TestDatabase::new("multi_org_grant").await;
    let pool = test_db.pool().clone();

    let org_a_id = create_org(&pool, "grant-org-a").await;
    let org_b_id = create_org(&pool, "grant-org-b").await;

    let user_id = create_user(&pool, "eve@multi-org.com").await;
    let team_a_id = create_team(&pool, "team-in-a", &org_a_id).await;
    let _team_b_id = create_team(&pool, "team-in-b", &org_b_id).await;

    // User is member of org A only
    add_org_membership(&pool, &user_id, &org_a_id, OrgRole::Member).await;
    add_team_membership(&pool, &user_id, &team_a_id).await;
    insert_grant(&pool, &user_id, &org_a_id, &team_a_id, "clusters", "read", None).await;

    let ctx = build_auth_context(&pool, &user_id, "eve-token").await;

    // Access in org A is allowed
    assert!(
        check_resource_access(&ctx, "clusters", "read", Some("team-in-a")),
        "Grant in org A should allow access to org A's team"
    );
    // Access in org B is denied (no grant for team-in-b)
    assert!(
        !check_resource_access(&ctx, "clusters", "read", Some("team-in-b")),
        "Grant in org A should NOT allow access to org B's team"
    );
}

// ---------------------------------------------------------------------------
// Test 7: Viewer role with read-only grants — write denied
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_viewer_role_reduced_grants() {
    let test_db = TestDatabase::new("viewer_reduced").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "viewer-org").await;
    let user_id = create_user(&pool, "viewer@reduced.com").await;
    let team_id = create_team(&pool, "team-zeta", &org_id).await;

    add_org_membership(&pool, &user_id, &org_id, OrgRole::Viewer).await;
    add_team_membership(&pool, &user_id, &team_id).await;

    // Viewer gets only read grants
    insert_grant(&pool, &user_id, &org_id, &team_id, "clusters", "read", None).await;
    insert_grant(&pool, &user_id, &org_id, &team_id, "routes", "read", None).await;
    insert_grant(&pool, &user_id, &org_id, &team_id, "listeners", "read", None).await;

    let ctx = build_auth_context(&pool, &user_id, "viewer-token").await;

    // Read access is allowed
    assert!(check_resource_access(&ctx, "clusters", "read", Some("team-zeta")));
    assert!(check_resource_access(&ctx, "routes", "read", Some("team-zeta")));
    assert!(check_resource_access(&ctx, "listeners", "read", Some("team-zeta")));

    // Write access is denied
    assert!(
        !check_resource_access(&ctx, "clusters", "create", Some("team-zeta")),
        "Viewer with read-only grants should NOT create clusters"
    );
    assert!(
        !check_resource_access(&ctx, "routes", "create", Some("team-zeta")),
        "Viewer should NOT create routes"
    );
    assert!(
        !check_resource_access(&ctx, "listeners", "delete", Some("team-zeta")),
        "Viewer should NOT delete listeners"
    );
}

// ---------------------------------------------------------------------------
// Test 8: No grants denies all access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_no_grants_denies_all() {
    let test_db = TestDatabase::new("no_grants").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "no-grants-org").await;
    let user_id = create_user(&pool, "zero@nogrants.com").await;
    let team_id = create_team(&pool, "team-theta", &org_id).await;

    // User has org + team membership but ZERO grants
    add_org_membership(&pool, &user_id, &org_id, OrgRole::Member).await;
    add_team_membership(&pool, &user_id, &team_id).await;

    let ctx = build_auth_context(&pool, &user_id, "zero-token").await;

    assert!(ctx.grants.is_empty(), "User should have no grants loaded");

    // All resource access should be denied
    assert!(!check_resource_access(&ctx, "clusters", "read", Some("team-theta")));
    assert!(!check_resource_access(&ctx, "clusters", "create", Some("team-theta")));
    assert!(!check_resource_access(&ctx, "routes", "read", Some("team-theta")));
    assert!(!check_resource_access(&ctx, "listeners", "delete", Some("team-theta")));

    // No team specified — allowed because user has org membership (org_scopes non-empty).
    // The handler will filter by the user's empty grant list, returning no results.
    // check_resource_access with team=None checks org membership, not grants.
    assert!(check_resource_access(&ctx, "clusters", "read", None));
}

// ---------------------------------------------------------------------------
// Test 9: Multiple grants for same team, different resources
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multiple_grants_same_team() {
    let test_db = TestDatabase::new("multi_grants_same_team").await;
    let pool = test_db.pool().clone();

    let org_id = create_org(&pool, "multi-grant-org").await;
    let user_id = create_user(&pool, "multi@grants.com").await;
    let team_id = create_team(&pool, "team-iota", &org_id).await;

    add_org_membership(&pool, &user_id, &org_id, OrgRole::Member).await;
    add_team_membership(&pool, &user_id, &team_id).await;

    // Grant read + create for clusters, read for routes
    insert_grant(&pool, &user_id, &org_id, &team_id, "clusters", "read", None).await;
    insert_grant(&pool, &user_id, &org_id, &team_id, "clusters", "create", None).await;
    insert_grant(&pool, &user_id, &org_id, &team_id, "routes", "read", None).await;

    let ctx = build_auth_context(&pool, &user_id, "multi-token").await;

    assert_eq!(ctx.grants.len(), 3, "Should load exactly 3 grants");

    assert!(check_resource_access(&ctx, "clusters", "read", Some("team-iota")));
    assert!(check_resource_access(&ctx, "clusters", "create", Some("team-iota")));
    assert!(check_resource_access(&ctx, "routes", "read", Some("team-iota")));

    // Not granted
    assert!(!check_resource_access(&ctx, "routes", "create", Some("team-iota")));
    assert!(!check_resource_access(&ctx, "listeners", "read", Some("team-iota")));
}
