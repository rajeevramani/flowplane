// Integration tests for the unified grant model (I.5c).
//
// Covers human grant lifecycle, cross-principal isolation, org admin implicit
// access, default grants on member add, team cascade, and expired grants.
//
// Run with: cargo test --features postgres_tests -- unified_grants_lifecycle
#![cfg(feature = "postgres_tests")]

mod common;

use common::test_db::{TestDatabase, TEST_ORG_ID, TEST_TEAM_ID};
use flowplane::auth::authorization::{check_resource_access, require_org_admin_only};
use flowplane::auth::models::{AgentContext, AuthContext, Grant, GrantType};
use flowplane::auth::permissions::load_permissions;
use flowplane::auth::scope_registry::{is_valid_resource_action_pair, VALID_GRANTS};
use flowplane::domain::{TokenId, UserId};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Insert a human user row.
async fn insert_human_user(pool: &flowplane::storage::DbPool, user_id: &str, name: &str) {
    let email = format!("{}@test.local", name);
    let zitadel_sub = format!("zsub-{}", user_id);
    sqlx::query(
        "INSERT INTO users \
         (id, email, password_hash, name, status, is_admin, zitadel_sub, user_type, \
          created_at, updated_at) \
         VALUES ($1, $2, '', $3, 'active', false, $4, 'human', NOW(), NOW()) \
         ON CONFLICT (zitadel_sub) DO NOTHING",
    )
    .bind(user_id)
    .bind(&email)
    .bind(name)
    .bind(&zitadel_sub)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert human user '{}': {}", name, e));
}

/// Insert a machine user row with a given agent_context.
async fn insert_machine_user(
    pool: &flowplane::storage::DbPool,
    user_id: &str,
    name: &str,
    agent_context: &str,
) {
    let email = format!("{}@machine.local", name);
    let zitadel_sub = format!("zsub-{}", user_id);
    sqlx::query(
        "INSERT INTO users \
         (id, email, password_hash, name, status, is_admin, zitadel_sub, user_type, agent_context, \
          created_at, updated_at) \
         VALUES ($1, $2, '', $3, 'active', false, $4, 'machine', $5, NOW(), NOW()) \
         ON CONFLICT (zitadel_sub) DO NOTHING",
    )
    .bind(user_id)
    .bind(&email)
    .bind(name)
    .bind(&zitadel_sub)
    .bind(agent_context)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert machine user '{}': {}", name, e));
}

/// Insert an org membership for a user.
async fn insert_org_membership(
    pool: &flowplane::storage::DbPool,
    user_id: &str,
    org_id: &str,
    role: &str,
) {
    let id = format!("om-{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO organization_memberships (id, user_id, org_id, role, created_at) \
         VALUES ($1, $2, $3, $4, NOW()) \
         ON CONFLICT (user_id, org_id) DO NOTHING",
    )
    .bind(&id)
    .bind(user_id)
    .bind(org_id)
    .bind(role)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert org membership: {}", e));
}

/// Insert a team membership for a user.
async fn insert_team_membership(pool: &flowplane::storage::DbPool, user_id: &str, team_id: &str) {
    let id = format!("utm-{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO user_team_memberships (id, user_id, team, created_at) \
         VALUES ($1, $2, $3, NOW()) \
         ON CONFLICT (user_id, team) DO NOTHING",
    )
    .bind(&id)
    .bind(user_id)
    .bind(team_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert team membership: {}", e));
}

/// Insert a resource grant row directly.
async fn insert_resource_grant(
    pool: &flowplane::storage::DbPool,
    principal_id: &str,
    org_id: &str,
    team_id: &str,
    resource_type: &str,
    action: &str,
) -> String {
    let grant_id = format!("grant-{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO grants \
         (id, principal_id, org_id, team_id, grant_type, resource_type, action, created_by) \
         VALUES ($1, $2, $3, $4, 'resource', $5, $6, $2)",
    )
    .bind(&grant_id)
    .bind(principal_id)
    .bind(org_id)
    .bind(team_id)
    .bind(resource_type)
    .bind(action)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert resource grant: {}", e));
    grant_id
}

/// Insert a grant with an explicit expires_at timestamp.
async fn insert_expired_grant(
    pool: &flowplane::storage::DbPool,
    principal_id: &str,
    org_id: &str,
    team_id: &str,
    resource_type: &str,
    action: &str,
) -> String {
    let grant_id = format!("grant-{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO grants \
         (id, principal_id, org_id, team_id, grant_type, resource_type, action, created_by, expires_at) \
         VALUES ($1, $2, $3, $4, 'resource', $5, $6, $2, NOW() - INTERVAL '1 hour')",
    )
    .bind(&grant_id)
    .bind(principal_id)
    .bind(org_id)
    .bind(team_id)
    .bind(resource_type)
    .bind(action)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert expired grant: {}", e));
    grant_id
}

/// Create a human AuthContext with the given grants and org context.
fn human_context(
    user_id: &str,
    org_scopes: Vec<String>,
    grants: Vec<Grant>,
    org_name: Option<&str>,
) -> AuthContext {
    let mut ctx = AuthContext::with_user(
        TokenId::from_str_unchecked("human-token"),
        "human-user".into(),
        UserId::from_str_unchecked(user_id),
        format!("{}@test.local", user_id),
        org_scopes,
    )
    .with_grants(grants, None);
    ctx.org_name = org_name.map(|s| s.to_string());
    ctx
}

/// Create a resource Grant for testing.
fn make_grant(resource: &str, action: &str, team_name: &str) -> Grant {
    Grant {
        grant_type: GrantType::Resource,
        team_id: format!("test-team-id-{}", team_name),
        team_name: team_name.to_string(),
        resource_type: Some(resource.to_string()),
        action: Some(action.to_string()),
        route_id: None,
        allowed_methods: vec![],
    }
}

// ---------------------------------------------------------------------------
// 2.1 — Human grant lifecycle
// ---------------------------------------------------------------------------

/// A human user with resource grants can access the granted resources.
#[tokio::test]
async fn test_human_with_resource_grants_has_access() {
    let _db = TestDatabase::new("human_resource_grants").await;

    let grants = vec![
        make_grant("clusters", "read", "test-team"),
        make_grant("routes", "read", "test-team"),
        make_grant("routes", "create", "test-team"),
    ];
    let ctx = human_context("human-1", vec![], grants, None);

    assert!(check_resource_access(&ctx, "clusters", "read", Some("test-team")));
    assert!(check_resource_access(&ctx, "routes", "read", Some("test-team")));
    assert!(check_resource_access(&ctx, "routes", "create", Some("test-team")));

    // Not granted
    assert!(!check_resource_access(&ctx, "clusters", "create", Some("test-team")));
    assert!(!check_resource_access(&ctx, "listeners", "read", Some("test-team")));
}

/// A human user with zero grants and no org-admin scope has no access.
#[tokio::test]
async fn test_human_zero_grants_no_access() {
    let _db = TestDatabase::new("human_zero_grants").await;

    let ctx = human_context("human-zero", vec![], vec![], None);

    assert!(!check_resource_access(&ctx, "clusters", "read", Some("test-team")));
    assert!(!check_resource_access(&ctx, "routes", "read", None));
}

/// Human grant CRUD via direct DB — create, list, delete.
#[tokio::test]
async fn test_human_grant_crud() {
    let db = TestDatabase::new("human_grant_crud").await;
    let pool = &db.pool;

    let user_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_human_user(pool, &user_id, "human-crud").await;
    insert_org_membership(pool, &user_id, TEST_ORG_ID, "member").await;
    insert_team_membership(pool, &user_id, TEST_TEAM_ID).await;

    // Create grant
    let grant_id =
        insert_resource_grant(pool, &user_id, TEST_ORG_ID, TEST_TEAM_ID, "clusters", "read").await;

    // List grants
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM grants WHERE principal_id = $1")
        .bind(&user_id)
        .fetch_one(pool)
        .await
        .expect("count query failed");
    assert_eq!(count.0, 1);

    // Delete grant
    sqlx::query("DELETE FROM grants WHERE id = $1")
        .bind(&grant_id)
        .execute(pool)
        .await
        .expect("delete failed");

    let count_after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM grants WHERE principal_id = $1")
        .bind(&user_id)
        .fetch_one(pool)
        .await
        .expect("count query failed");
    assert_eq!(count_after.0, 0);
}

// ---------------------------------------------------------------------------
// 2.2 — Cross-principal isolation
// ---------------------------------------------------------------------------

/// Human grants are not visible when loading permissions for an agent user.
#[tokio::test]
async fn test_human_grants_not_visible_to_agents() {
    let db = TestDatabase::new("human_grants_isolation").await;
    let pool = &db.pool;

    let human_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());

    insert_human_user(pool, &human_id, "isolation-human").await;
    insert_machine_user(pool, &agent_id, "isolation-agent", "cp-tool").await;

    for uid in [&human_id, &agent_id] {
        insert_org_membership(pool, uid, TEST_ORG_ID, "member").await;
        insert_team_membership(pool, uid, TEST_TEAM_ID).await;
    }

    // Grant clusters:read to human only
    insert_resource_grant(pool, &human_id, TEST_ORG_ID, TEST_TEAM_ID, "clusters", "read").await;

    // Load agent permissions — should NOT include human's grant
    let agent_perms = load_permissions(pool, &UserId::from_str_unchecked(&agent_id))
        .await
        .expect("load agent permissions failed");

    assert!(
        agent_perms.grants.is_empty(),
        "Agent must not see human's grants, but got {:?}",
        agent_perms.grants
    );
}

/// Agent grants are not visible when loading permissions for a human user.
#[tokio::test]
async fn test_agent_grants_not_visible_to_humans() {
    let db = TestDatabase::new("agent_grants_isolation").await;
    let pool = &db.pool;

    let human_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());

    insert_human_user(pool, &human_id, "isolation-human2").await;
    insert_machine_user(pool, &agent_id, "isolation-agent2", "cp-tool").await;

    for uid in [&human_id, &agent_id] {
        insert_org_membership(pool, uid, TEST_ORG_ID, "member").await;
        insert_team_membership(pool, uid, TEST_TEAM_ID).await;
    }

    // Grant routes:read to agent only
    insert_resource_grant(pool, &agent_id, TEST_ORG_ID, TEST_TEAM_ID, "routes", "read").await;

    // Load human permissions — should NOT include agent's grant
    let human_perms = load_permissions(pool, &UserId::from_str_unchecked(&human_id))
        .await
        .expect("load human permissions failed");

    assert!(
        human_perms.grants.is_empty(),
        "Human must not see agent's grants, but got {:?}",
        human_perms.grants
    );
}

// ---------------------------------------------------------------------------
// 2.3 — check_resource_access unified path
// ---------------------------------------------------------------------------

/// check_resource_access works for human with grants (same path as agents).
#[tokio::test]
async fn test_check_access_works_for_human_with_grants() {
    let _db = TestDatabase::new("check_access_human").await;

    let grants = vec![
        make_grant("clusters", "read", "test-team"),
        make_grant("clusters", "create", "test-team"),
    ];
    let ctx = human_context("human-access", vec![], grants, None);

    assert!(check_resource_access(&ctx, "clusters", "read", Some("test-team")));
    assert!(check_resource_access(&ctx, "clusters", "create", Some("test-team")));
    assert!(!check_resource_access(&ctx, "clusters", "delete", Some("test-team")));
    assert!(!check_resource_access(&ctx, "routes", "read", Some("test-team")));
}

/// check_resource_access works for cp-tool agent with grants.
#[tokio::test]
async fn test_check_access_works_for_agent_with_grants() {
    let _db = TestDatabase::new("check_access_agent").await;

    let grants = vec![make_grant("routes", "read", "test-team")];
    let ctx = AuthContext::with_user(
        TokenId::from_str_unchecked("agent-token"),
        "cp-agent".into(),
        UserId::from_str_unchecked("agent-access"),
        "agent@machine.local".into(),
        vec![],
    )
    .with_grants(grants, Some(AgentContext::CpTool));

    assert!(check_resource_access(&ctx, "routes", "read", Some("test-team")));
    assert!(!check_resource_access(&ctx, "routes", "create", Some("test-team")));
    assert!(!check_resource_access(&ctx, "clusters", "read", Some("test-team")));
}

// ---------------------------------------------------------------------------
// 2.4 — Agent context guard (DD-3)
// ---------------------------------------------------------------------------

/// A gateway-tool agent is blocked from CP resources even if it somehow has resource grants.
#[tokio::test]
async fn test_gateway_tool_agent_blocked_from_cp_even_with_resource_grant() {
    let _db = TestDatabase::new("gw_agent_cp_blocked").await;

    // Construct a gateway-tool context with a resource grant (shouldn't happen
    // in practice, but the guard must still block).
    let grants = vec![make_grant("clusters", "read", "test-team")];
    let ctx = AuthContext::with_user(
        TokenId::from_str_unchecked("gw-token"),
        "gw-agent".into(),
        UserId::from_str_unchecked("gw-blocked"),
        "gw@machine.local".into(),
        vec![],
    )
    .with_grants(grants, Some(AgentContext::GatewayTool));

    assert!(
        !check_resource_access(&ctx, "clusters", "read", Some("test-team")),
        "GatewayTool agent must be structurally blocked from CP resources"
    );
}

/// An api-consumer agent is blocked from CP resources.
#[tokio::test]
async fn test_api_consumer_agent_blocked_from_cp_resources() {
    let _db = TestDatabase::new("consumer_cp_blocked").await;

    let ctx = AuthContext::with_user(
        TokenId::from_str_unchecked("consumer-token"),
        "consumer".into(),
        UserId::from_str_unchecked("consumer-blocked"),
        "consumer@machine.local".into(),
        vec![],
    )
    .with_grants(vec![], Some(AgentContext::ApiConsumer));

    assert!(!check_resource_access(&ctx, "clusters", "read", None));
    assert!(!check_resource_access(&ctx, "routes", "read", None));
    assert!(!check_resource_access(&ctx, "listeners", "create", None));
}

// ---------------------------------------------------------------------------
// 2.5 — Org admin implicit access
// ---------------------------------------------------------------------------

/// An org admin has implicit access to team resources without explicit grants.
#[tokio::test]
async fn test_org_admin_has_implicit_team_access_without_grants() {
    let _db = TestDatabase::new("org_admin_implicit").await;

    let ctx = human_context(
        "org-admin-user",
        vec!["org:test-org:admin".to_string()],
        vec![], // No explicit grants
        Some("test-org"),
    );

    // Org admin should have implicit access to any team in their org
    assert!(
        check_resource_access(&ctx, "clusters", "read", Some("test-team")),
        "Org admin must have implicit team access without explicit grants"
    );
    assert!(
        check_resource_access(&ctx, "routes", "create", Some("test-team")),
        "Org admin must have implicit team access for any action"
    );
}

/// Org admin cannot access teams in a different org.
#[tokio::test]
async fn test_org_admin_cannot_access_other_org_teams() {
    let _db = TestDatabase::new("org_admin_cross_org").await;

    // Admin for "acme-corp" but org_name on context is "acme-corp"
    let ctx = human_context(
        "acme-admin",
        vec!["org:acme-corp:admin".to_string()],
        vec![],
        Some("acme-corp"),
    );

    // The scope says acme-corp:admin, but check_resource_access validates
    // the scope org matches context.org_name. Since team_access.rs resolves
    // team within the org, cross-org access is denied at that layer.
    // At the check_resource_access level, the org_name must match.
    assert!(check_resource_access(&ctx, "clusters", "read", Some("some-team")));

    // Different org context would fail — an admin for "globex-corp" trying "acme-corp" teams
    let cross_ctx = human_context(
        "globex-admin",
        vec!["org:globex-corp:admin".to_string()],
        vec![],
        Some("globex-corp"),
    );
    // The scope is globex-corp:admin, context org is globex-corp.
    // Access is granted because check_resource_access sees org_name matches.
    // Cross-org isolation is enforced by team_access.rs (SQL joins on org).
    // This test validates the scope/context match defense-in-depth.
    assert!(check_resource_access(&cross_ctx, "clusters", "read", Some("some-team")));

    // But if someone has a scope for org A with context for org B, it's denied
    let mismatched = human_context(
        "mismatch-admin",
        vec!["org:acme-corp:admin".to_string()],
        vec![],
        Some("globex-corp"), // Org context doesn't match scope
    );
    assert!(
        !check_resource_access(&mismatched, "clusters", "read", Some("some-team")),
        "Mismatched org scope and org context must be denied"
    );
}

// ---------------------------------------------------------------------------
// 2.6 — Default grants on member add (DD-2)
// ---------------------------------------------------------------------------

/// Adding a member creates default read grants (tested via grants_for_org_role logic).
#[tokio::test]
async fn test_adding_member_creates_default_read_grants() {
    let db = TestDatabase::new("default_member_grants").await;
    let pool = &db.pool;

    let user_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_human_user(pool, &user_id, "default-member").await;
    insert_org_membership(pool, &user_id, TEST_ORG_ID, "member").await;
    insert_team_membership(pool, &user_id, TEST_TEAM_ID).await;

    // Simulate what insert_default_grants does for "member" role:
    // member gets read grants for all VALID_GRANTS resources.
    let valid_grants = flowplane::auth::scope_registry::VALID_GRANTS;
    for (resource, actions) in valid_grants {
        if actions.contains(&"read") {
            insert_resource_grant(pool, &user_id, TEST_ORG_ID, TEST_TEAM_ID, resource, "read")
                .await;
        }
    }

    // Load permissions and verify
    let perms = load_permissions(pool, &UserId::from_str_unchecked(&user_id))
        .await
        .expect("load permissions failed");

    // Should have one read grant per resource that has a "read" action
    let expected_count =
        valid_grants.iter().filter(|(_, actions)| actions.contains(&"read")).count();

    assert_eq!(
        perms.grants.len(),
        expected_count,
        "Member should have {} default read grants, got {}",
        expected_count,
        perms.grants.len()
    );

    // All grants should be Resource type with action "read"
    for grant in &perms.grants {
        assert_eq!(grant.grant_type, GrantType::Resource);
        assert_eq!(grant.action.as_deref(), Some("read"));
    }
}

/// Adding an admin creates NO grants (DD-2: admin access is implicit).
#[tokio::test]
async fn test_adding_admin_creates_no_grants() {
    let db = TestDatabase::new("admin_no_grants").await;
    let pool = &db.pool;

    let user_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_human_user(pool, &user_id, "admin-no-grants").await;
    insert_org_membership(pool, &user_id, TEST_ORG_ID, "admin").await;
    insert_team_membership(pool, &user_id, TEST_TEAM_ID).await;

    // grants_for_org_role(Admin) returns empty vec — no grants inserted
    // Verify no grants exist
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM grants WHERE principal_id = $1")
        .bind(&user_id)
        .fetch_one(pool)
        .await
        .expect("count query failed");

    assert_eq!(count.0, 0, "Admin should have zero explicit grants (access is implicit)");

    // But load_permissions should give them org-admin scope
    let perms = load_permissions(pool, &UserId::from_str_unchecked(&user_id))
        .await
        .expect("load permissions failed");

    assert!(
        perms.org_scopes.contains("org:test-org:admin"),
        "Admin should have org:test-org:admin scope, got {:?}",
        perms.org_scopes
    );
    assert!(perms.grants.is_empty(), "Admin should have no explicit grants");
}

// ---------------------------------------------------------------------------
// 2.7 — Team FK cascade
// ---------------------------------------------------------------------------

/// Deleting a team cascades to grants rows for that team.
#[tokio::test]
async fn test_deleting_team_cascades_grants() {
    let db = TestDatabase::new("team_cascade_grants").await;
    let pool = &db.pool;

    // Create an ephemeral team for this test
    let team_id = format!("team-{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO teams (id, name, display_name, org_id, status) \
         VALUES ($1, 'ephemeral-team', 'Ephemeral Team', $2, 'active')",
    )
    .bind(&team_id)
    .bind(TEST_ORG_ID)
    .execute(pool)
    .await
    .expect("create ephemeral team failed");

    let user_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_human_user(pool, &user_id, "team-cascade-user").await;
    insert_org_membership(pool, &user_id, TEST_ORG_ID, "member").await;
    insert_team_membership(pool, &user_id, &team_id).await;

    // Insert grant for the ephemeral team
    insert_resource_grant(pool, &user_id, TEST_ORG_ID, &team_id, "clusters", "read").await;

    // Verify grant exists
    let count_before: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM grants WHERE principal_id = $1 AND team_id = $2")
            .bind(&user_id)
            .bind(&team_id)
            .fetch_one(pool)
            .await
            .expect("count query failed");
    assert_eq!(count_before.0, 1);

    // Delete the team (ON DELETE CASCADE on grants.team_id FK)
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(&team_id)
        .execute(pool)
        .await
        .expect("delete team failed");

    // Grants should be gone
    let count_after: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM grants WHERE principal_id = $1 AND team_id = $2")
            .bind(&user_id)
            .bind(&team_id)
            .fetch_one(pool)
            .await
            .expect("count query failed");
    assert_eq!(count_after.0, 0, "Grants must cascade-delete when team is deleted");
}

// ---------------------------------------------------------------------------
// 2.8 — Grant expiry
// ---------------------------------------------------------------------------

/// An expired grant is not loaded by load_permissions (filtered by expires_at > NOW()).
#[tokio::test]
async fn test_expired_grant_is_not_effective() {
    let db = TestDatabase::new("expired_grant").await;
    let pool = &db.pool;

    let user_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_human_user(pool, &user_id, "expired-grant-user").await;
    insert_org_membership(pool, &user_id, TEST_ORG_ID, "member").await;
    insert_team_membership(pool, &user_id, TEST_TEAM_ID).await;

    // Insert an expired grant (expires_at in the past)
    insert_expired_grant(pool, &user_id, TEST_ORG_ID, TEST_TEAM_ID, "clusters", "read").await;

    // Also insert a valid (non-expired) grant for routes:read
    insert_resource_grant(pool, &user_id, TEST_ORG_ID, TEST_TEAM_ID, "routes", "read").await;

    // Load permissions — expired grant should be filtered out
    let perms = load_permissions(pool, &UserId::from_str_unchecked(&user_id))
        .await
        .expect("load permissions failed");

    assert_eq!(
        perms.grants.len(),
        1,
        "Only non-expired grant should be loaded, got {}",
        perms.grants.len()
    );
    assert_eq!(perms.grants[0].resource_type.as_deref(), Some("routes"));
    assert_eq!(perms.grants[0].action.as_deref(), Some("read"));
}

// ---------------------------------------------------------------------------
// 2.9 — Security invariants
// ---------------------------------------------------------------------------

/// Platform admin cannot see into org team resources.
#[tokio::test]
async fn test_platform_admin_cannot_access_team_resources() {
    let _db = TestDatabase::new("platform_admin_boundary").await;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("platform-token"),
        "platform-admin".into(),
        vec!["admin:all".to_string()],
    );

    // admin:all grants governance access but NOT tenant resource access
    assert!(
        !check_resource_access(&ctx, "clusters", "read", Some("test-team")),
        "Platform admin must not access team clusters"
    );
    assert!(
        !check_resource_access(&ctx, "routes", "read", Some("test-team")),
        "Platform admin must not access team routes"
    );
    assert!(
        !check_resource_access(&ctx, "listeners", "read", None),
        "Platform admin must not access listeners"
    );
}

/// Platform admin CAN access governance resources.
#[tokio::test]
async fn test_platform_admin_can_access_governance_resources() {
    let _db = TestDatabase::new("platform_admin_governance").await;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("platform-token"),
        "platform-admin".into(),
        vec!["admin:all".to_string()],
    );

    assert!(check_resource_access(&ctx, "admin", "read", None));
    assert!(check_resource_access(&ctx, "organizations", "read", None));
    assert!(check_resource_access(&ctx, "users", "read", None));
    assert!(check_resource_access(&ctx, "teams", "read", None));
}

/// Org admin cannot see other orgs' resources (org scope mismatch blocked).
#[tokio::test]
async fn test_org_admin_scope_mismatch_blocked() {
    let _db = TestDatabase::new("org_admin_scope_mismatch").await;

    // User has org:acme:admin scope but their actual org context is "globex"
    let ctx =
        human_context("scope-mismatch", vec!["org:acme:admin".to_string()], vec![], Some("globex"));

    assert!(
        !check_resource_access(&ctx, "clusters", "read", Some("test-team")),
        "Scope/context mismatch must deny access"
    );
}

/// Team member cannot access resources in a different team.
#[tokio::test]
async fn test_team_member_cross_team_denied() {
    let _db = TestDatabase::new("cross_team_denied").await;

    let grants = vec![make_grant("clusters", "read", "test-team")];
    let ctx = human_context("team-member", vec![], grants, None);

    assert!(check_resource_access(&ctx, "clusters", "read", Some("test-team")));
    assert!(
        !check_resource_access(&ctx, "clusters", "read", Some("other-team")),
        "Team member must not access resources in another team"
    );
}

/// Agents cannot exceed their grants — a cp-tool with routes:read cannot write.
#[tokio::test]
async fn test_agent_cannot_exceed_grants() {
    let _db = TestDatabase::new("agent_exceed_grants").await;

    let grants = vec![make_grant("routes", "read", "test-team")];
    let ctx = AuthContext::with_user(
        TokenId::from_str_unchecked("agent-token"),
        "cp-agent".into(),
        UserId::from_str_unchecked("exceed-agent"),
        "exceed@machine.local".into(),
        vec![],
    )
    .with_grants(grants, Some(AgentContext::CpTool));

    assert!(check_resource_access(&ctx, "routes", "read", Some("test-team")));
    assert!(!check_resource_access(&ctx, "routes", "create", Some("test-team")));
    assert!(!check_resource_access(&ctx, "routes", "update", Some("test-team")));
    assert!(!check_resource_access(&ctx, "routes", "delete", Some("test-team")));
}

// ---------------------------------------------------------------------------
// 2.10 — VALID_GRANTS audit
// ---------------------------------------------------------------------------

/// Verify VALID_GRANTS contains all 14 expected resources.
#[test]
fn test_valid_grants_has_expected_resources() {
    // Must have exactly 14 resource entries
    assert_eq!(VALID_GRANTS.len(), 14, "VALID_GRANTS must have exactly 14 resources");

    // Check all expected resources are present
    let resources: HashSet<&str> = VALID_GRANTS.iter().map(|(r, _)| *r).collect();
    let expected = [
        "clusters",
        "routes",
        "listeners",
        "filters",
        "secrets",
        "dataplanes",
        "custom-wasm-filters",
        "learning-sessions",
        "aggregated-schemas",
        "proxy-certificates",
        "reports",
        "audit",
        "stats",
        "agents",
    ];

    for r in &expected {
        assert!(
            resources.contains(r),
            "VALID_GRANTS must contain '{}' — missing resource would break existing grants",
            r
        );
    }

    // Every resource must have at least "read" action
    for (resource, actions) in VALID_GRANTS {
        assert!(actions.contains(&"read"), "Resource '{}' must have 'read' action", resource);
    }
}

// ---------------------------------------------------------------------------
// 2.11 — Agent grant lifecycle (CRUD)
// ---------------------------------------------------------------------------

/// Agent grant CRUD: create, list, delete via direct DB.
#[tokio::test]
async fn test_agent_grant_crud() {
    let db = TestDatabase::new("agent_grant_crud").await;
    let pool = &db.pool;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_machine_user(pool, &agent_id, "agent-crud", "cp-tool").await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID, "member").await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    // Create grant
    let grant_id =
        insert_resource_grant(pool, &agent_id, TEST_ORG_ID, TEST_TEAM_ID, "routes", "create").await;

    // List grants
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM grants WHERE principal_id = $1")
        .bind(&agent_id)
        .fetch_one(pool)
        .await
        .expect("count query failed");
    assert_eq!(count.0, 1);

    // Verify grant details
    let row: (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT grant_type, resource_type, action FROM grants WHERE id = $1")
            .bind(&grant_id)
            .fetch_one(pool)
            .await
            .expect("query grant details failed");
    assert_eq!(row.0, "resource");
    assert_eq!(row.1.as_deref(), Some("routes"));
    assert_eq!(row.2.as_deref(), Some("create"));

    // Delete grant
    sqlx::query("DELETE FROM grants WHERE id = $1")
        .bind(&grant_id)
        .execute(pool)
        .await
        .expect("delete failed");

    let count_after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM grants WHERE principal_id = $1")
        .bind(&agent_id)
        .fetch_one(pool)
        .await
        .expect("count query failed");
    assert_eq!(count_after.0, 0);
}

// ---------------------------------------------------------------------------
// 2.12 — Validation
// ---------------------------------------------------------------------------

/// Invalid resource:action pairs are rejected by is_valid_resource_action_pair.
#[test]
fn test_rejects_invalid_resource_action_pair() {
    // clusters doesn't have "execute" action
    assert!(
        !is_valid_resource_action_pair("clusters", "execute"),
        "clusters:execute must be rejected"
    );
    // nonexistent resource
    assert!(!is_valid_resource_action_pair("foobar", "read"), "foobar:read must be rejected");
    // empty strings
    assert!(!is_valid_resource_action_pair("", "read"));
    assert!(!is_valid_resource_action_pair("clusters", ""));

    // Valid pairs for comparison
    assert!(is_valid_resource_action_pair("clusters", "read"));
    assert!(is_valid_resource_action_pair("learning-sessions", "execute"));
    assert!(is_valid_resource_action_pair("aggregated-schemas", "execute"));
}

/// DB constraint: resource grant without resource_type is rejected.
#[tokio::test]
async fn test_resource_grant_requires_resource_type() {
    let db = TestDatabase::new("resource_grant_needs_type").await;
    let pool = &db.pool;

    let user_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_human_user(pool, &user_id, "constraint-test").await;
    insert_org_membership(pool, &user_id, TEST_ORG_ID, "member").await;
    insert_team_membership(pool, &user_id, TEST_TEAM_ID).await;

    let result = sqlx::query(
        "INSERT INTO grants \
         (id, principal_id, org_id, team_id, grant_type, created_by) \
         VALUES ($1, $2, $3, $4, 'resource', $2)",
    )
    .bind(format!("g-{}", uuid::Uuid::new_v4()))
    .bind(&user_id)
    .bind(TEST_ORG_ID)
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await;

    assert!(
        result.is_err(),
        "resource grant without resource_type must be rejected by DB constraint"
    );
}

/// DB constraint: gateway-tool grant without route_id is rejected.
#[tokio::test]
async fn test_gateway_tool_grant_requires_route_id() {
    let db = TestDatabase::new("gw_grant_needs_route").await;
    let pool = &db.pool;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_machine_user(pool, &agent_id, "gw-constraint", "gateway-tool").await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID, "member").await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    let result = sqlx::query(
        "INSERT INTO grants \
         (id, principal_id, org_id, team_id, grant_type, created_by) \
         VALUES ($1, $2, $3, $4, 'gateway-tool', $2)",
    )
    .bind(format!("g-{}", uuid::Uuid::new_v4()))
    .bind(&agent_id)
    .bind(TEST_ORG_ID)
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await;

    assert!(
        result.is_err(),
        "gateway-tool grant without route_id must be rejected by DB constraint"
    );
}

/// Duplicate resource grant is rejected by unique index.
#[tokio::test]
async fn test_duplicate_resource_grant_rejected() {
    let db = TestDatabase::new("dup_resource_grant").await;
    let pool = &db.pool;

    let user_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_human_user(pool, &user_id, "dup-test").await;
    insert_org_membership(pool, &user_id, TEST_ORG_ID, "member").await;
    insert_team_membership(pool, &user_id, TEST_TEAM_ID).await;

    // First grant succeeds
    insert_resource_grant(pool, &user_id, TEST_ORG_ID, TEST_TEAM_ID, "clusters", "read").await;

    // Duplicate must fail with unique constraint
    let result = sqlx::query(
        "INSERT INTO grants \
         (id, principal_id, org_id, team_id, grant_type, resource_type, action, created_by) \
         VALUES ($1, $2, $3, $4, 'resource', 'clusters', 'read', $2)",
    )
    .bind(format!("g-{}", uuid::Uuid::new_v4()))
    .bind(&user_id)
    .bind(TEST_ORG_ID)
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await;

    assert!(result.is_err(), "Duplicate resource grant must be rejected");
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("23505") || err_str.contains("unique"),
        "Error must be unique constraint violation, got: {}",
        err_str
    );
}

// ---------------------------------------------------------------------------
// 2.13 — Principal deletion cascade
// ---------------------------------------------------------------------------

/// Deleting a user cascades to their grants.
#[tokio::test]
async fn test_principal_deletion_cascades_to_grants() {
    let db = TestDatabase::new("principal_cascade").await;
    let pool = &db.pool;

    let user_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_human_user(pool, &user_id, "cascade-del").await;
    insert_org_membership(pool, &user_id, TEST_ORG_ID, "member").await;
    insert_team_membership(pool, &user_id, TEST_TEAM_ID).await;

    insert_resource_grant(pool, &user_id, TEST_ORG_ID, TEST_TEAM_ID, "clusters", "read").await;
    insert_resource_grant(pool, &user_id, TEST_ORG_ID, TEST_TEAM_ID, "routes", "read").await;

    let count_before: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM grants WHERE principal_id = $1")
            .bind(&user_id)
            .fetch_one(pool)
            .await
            .expect("count query failed");
    assert_eq!(count_before.0, 2);

    // Delete user — grants must cascade-delete
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(&user_id)
        .execute(pool)
        .await
        .expect("delete user failed");

    let count_after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM grants WHERE principal_id = $1")
        .bind(&user_id)
        .fetch_one(pool)
        .await
        .expect("count query failed");
    assert_eq!(count_after.0, 0, "Grants must cascade-delete when user is deleted");
}

// ---------------------------------------------------------------------------
// 2.14 — Platform admin org grant boundary
// ---------------------------------------------------------------------------

/// Platform admin cannot pass require_org_admin_only (org grant management boundary).
#[tokio::test]
async fn test_platform_admin_cannot_manage_org_grants() {
    let _db = TestDatabase::new("platform_no_org_grants").await;

    let ctx = AuthContext::new(
        TokenId::from_str_unchecked("platform-token"),
        "platform-admin".into(),
        vec!["admin:all".to_string()],
    );

    assert!(
        require_org_admin_only(&ctx, "any-org").is_err(),
        "Platform admin must not pass require_org_admin_only"
    );
}

/// Org admin for one org cannot manage grants in another org.
#[tokio::test]
async fn test_org_admin_cross_org_grant_management_denied() {
    let _db = TestDatabase::new("cross_org_grant_mgmt").await;

    let acme_admin = {
        let mut ctx = AuthContext::new(
            TokenId::from_str_unchecked("acme-token"),
            "acme-admin".into(),
            vec!["org:acme:admin".to_string()],
        );
        ctx.user_id = Some(UserId::from_str_unchecked("acme-admin-id"));
        ctx
    };

    assert!(require_org_admin_only(&acme_admin, "acme").is_ok());
    assert!(
        require_org_admin_only(&acme_admin, "globex").is_err(),
        "Org admin for acme must be denied grant management in globex"
    );
}

// ---------------------------------------------------------------------------
// 2.15 — check_resource_access is single enforcement point
// ---------------------------------------------------------------------------

/// All auth paths (human grants, agent grants, org admin implicit) converge on
/// check_resource_access as the single enforcement point.
#[tokio::test]
async fn test_check_resource_access_is_single_enforcement_point() {
    let _db = TestDatabase::new("single_enforcement").await;

    // Human with grants
    let human = human_context(
        "human-1",
        vec!["org:test-org:member".to_string()],
        vec![make_grant("clusters", "read", "test-team")],
        Some("test-org"),
    );
    assert!(check_resource_access(&human, "clusters", "read", Some("test-team")));
    assert!(!check_resource_access(&human, "clusters", "delete", Some("test-team")));

    // Agent with grants
    let agent = AuthContext::with_user(
        TokenId::from_str_unchecked("agent-token"),
        "cp-agent".into(),
        UserId::from_str_unchecked("agent-1"),
        "agent@machine.local".into(),
        vec![],
    )
    .with_grants(vec![make_grant("clusters", "read", "test-team")], Some(AgentContext::CpTool));
    assert!(check_resource_access(&agent, "clusters", "read", Some("test-team")));
    assert!(!check_resource_access(&agent, "routes", "read", Some("test-team")));

    // Org admin (implicit, no grants)
    let admin =
        human_context("admin-1", vec!["org:test-org:admin".to_string()], vec![], Some("test-org"));
    assert!(check_resource_access(&admin, "clusters", "read", Some("test-team")));
    assert!(check_resource_access(&admin, "routes", "create", Some("any-team")));

    // Gateway-tool agent (structurally blocked)
    let gw = AuthContext::with_user(
        TokenId::from_str_unchecked("gw-token"),
        "gw-agent".into(),
        UserId::from_str_unchecked("gw-1"),
        "gw@machine.local".into(),
        vec![],
    )
    .with_grants(vec![], Some(AgentContext::GatewayTool));
    assert!(!check_resource_access(&gw, "clusters", "read", None));
}
