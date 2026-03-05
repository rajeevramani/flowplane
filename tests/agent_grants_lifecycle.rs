// Integration tests for agent grant lifecycle — Phase E unified grants model.
//
// These tests cover DB-level grant operations and the auth/permission layer.
// They do NOT require a running Zitadel instance or gateway executor.
//
// Run with: cargo test --features postgres_tests -- agent_grants_lifecycle
#![cfg(feature = "postgres_tests")]

mod common;

use common::test_db::{TestDatabase, TEST_ORG_ID, TEST_TEAM_ID};
use flowplane::auth::authorization::{check_resource_access, require_org_admin_only};
use flowplane::auth::models::{AgentContext, AuthContext, CpGrant};
use flowplane::config::SimpleXdsConfig;
use flowplane::domain::RouteMatchType;
use flowplane::domain::{RouteId, TokenId, UserId, VirtualHostId};
use flowplane::internal_api::{auth::InternalAuthContext, RouteOperations};
use flowplane::storage::repositories::route::{
    CreateRouteRequest, RouteRepository, UpdateRouteRequest,
};
use flowplane::xds::XdsState;
use std::collections::HashSet;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Insert a minimal user row directly (no Zitadel required).
async fn insert_test_user(
    pool: &flowplane::storage::DbPool,
    user_id: &str,
    zitadel_sub: &str,
    name: &str,
    agent_context: &str,
) {
    let email = format!("{}@machine.local", zitadel_sub);
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
    .bind(zitadel_sub)
    .bind(agent_context)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert test user '{}': {}", name, e));
}

/// Insert an org membership for a user.
async fn insert_org_membership(pool: &flowplane::storage::DbPool, user_id: &str, org_id: &str) {
    let id = format!("om_{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO organization_memberships (id, user_id, org_id, role, created_at) \
         VALUES ($1, $2, $3, 'member', NOW()) \
         ON CONFLICT (user_id, org_id) DO NOTHING",
    )
    .bind(&id)
    .bind(user_id)
    .bind(org_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert org membership for user '{}': {}", user_id, e));
}

/// Insert a team membership for a user.
async fn insert_team_membership(pool: &flowplane::storage::DbPool, user_id: &str, team_id: &str) {
    let id = format!("utm_{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO user_team_memberships (id, user_id, team, scopes, created_at) \
         VALUES ($1, $2, $3, '[]', NOW()) \
         ON CONFLICT (user_id, team) DO NOTHING",
    )
    .bind(&id)
    .bind(user_id)
    .bind(team_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| {
        panic!(
            "Failed to insert team membership for user '{}' in team '{}': {}",
            user_id, team_id, e
        )
    });
}

/// Insert a grant row directly in the DB (bypasses API-level checks for test setup).
#[allow(clippy::too_many_arguments)]
async fn insert_grant(
    pool: &flowplane::storage::DbPool,
    grant_id: &str,
    agent_id: &str,
    org_id: &str,
    team_id: &str,
    grant_type: &str,
    resource_type: Option<&str>,
    action: Option<&str>,
    route_id: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO agent_grants \
         (id, agent_id, org_id, team, grant_type, resource_type, action, route_id, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $2)",
    )
    .bind(grant_id)
    .bind(agent_id)
    .bind(org_id)
    .bind(team_id)
    .bind(grant_type)
    .bind(resource_type)
    .bind(action)
    .bind(route_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert grant '{}': {}", grant_id, e));
}

/// Create an org-admin AuthContext.
fn org_admin_context(org_name: &str) -> AuthContext {
    let mut ctx = AuthContext::new(
        TokenId::from_str_unchecked("org-admin-token"),
        "org-admin".into(),
        vec![format!("org:{}:admin", org_name)],
    );
    ctx.user_id = Some(UserId::from_str_unchecked("admin-user-id"));
    ctx
}

/// Create a cp-tool agent AuthContext with the given grants.
fn cp_agent_context(user_id: &str, grants: Vec<CpGrant>) -> AuthContext {
    AuthContext::with_user(
        TokenId::from_str_unchecked("cp-agent-token"),
        "cp-agent".into(),
        UserId::from_str_unchecked(user_id),
        format!("{}@machine.local", user_id),
        vec![],
    )
    .with_agent_data(Some(AgentContext::CpTool), grants)
}

/// Create a gateway-tool agent AuthContext (no CP grants).
fn gateway_agent_context(user_id: &str) -> AuthContext {
    AuthContext::with_user(
        TokenId::from_str_unchecked("gw-agent-token"),
        "gw-agent".into(),
        UserId::from_str_unchecked(user_id),
        format!("{}@machine.local", user_id),
        vec![],
    )
    .with_agent_data(Some(AgentContext::GatewayTool), vec![])
}

/// Create an api-consumer agent AuthContext (no grants).
fn api_consumer_context(user_id: &str) -> AuthContext {
    AuthContext::with_user(
        TokenId::from_str_unchecked("consumer-token"),
        "consumer".into(),
        UserId::from_str_unchecked(user_id),
        format!("{}@machine.local", user_id),
        vec![],
    )
    .with_agent_data(Some(AgentContext::ApiConsumer), vec![])
}

/// Create an external route via the RouteRepository (requires virtual_host).
///
/// Inserts a cluster + route_config + virtual_host chain directly, then returns the RouteId.
async fn create_external_route(
    pool: &flowplane::storage::DbPool,
    team_id: &str,
    route_name: &str,
) -> RouteId {
    let cluster_name = format!("cluster-{}", uuid::Uuid::new_v4().simple());
    let rc_id = format!("rc-{}", uuid::Uuid::new_v4().simple());
    let vh_id = format!("vh-{}", uuid::Uuid::new_v4().simple());

    // Insert cluster (required by route_configs FK)
    let cluster_config = serde_json::json!({
        "cluster_name": &cluster_name,
        "type": "STRICT_DNS",
        "connect_timeout": "0.25s",
        "load_assignment": {}
    });
    sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, version, team) \
         VALUES ($1, $2, $2, $3, 1, $4)",
    )
    .bind(format!("c-{}", uuid::Uuid::new_v4().simple()))
    .bind(&cluster_name)
    .bind(cluster_config.to_string())
    .bind(team_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert cluster for route '{}': {}", route_name, e));

    // Insert route_config
    let rc_config = serde_json::json!({ "cluster_name": &cluster_name });
    sqlx::query(
        "INSERT INTO route_configs (id, name, path_prefix, cluster_name, configuration, version, team) \
         VALUES ($1, $2, '/test', $3, $4, 1, $5)",
    )
    .bind(&rc_id)
    .bind(format!("rc-{}", route_name))
    .bind(&cluster_name)
    .bind(rc_config.to_string())
    .bind(team_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert route_config: {}", e));

    // Insert virtual_host
    sqlx::query(
        "INSERT INTO virtual_hosts (id, route_config_id, name, domains, rule_order) \
         VALUES ($1, $2, $3, '[\"*\"]', 0)",
    )
    .bind(&vh_id)
    .bind(&rc_id)
    .bind(format!("vh-{}", route_name))
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to insert virtual_host: {}", e));

    // Create route via repository (defaults to 'internal')
    let repo = RouteRepository::new(pool.clone());
    let route = repo
        .create(CreateRouteRequest {
            virtual_host_id: VirtualHostId::from_string(vh_id),
            name: route_name.to_string(),
            path_pattern: format!("/{}", route_name),
            match_type: RouteMatchType::Prefix,
            rule_order: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("Failed to create route '{}': {}", route_name, e));

    // Set exposure to 'external'
    repo.update(
        &route.id,
        UpdateRouteRequest {
            path_pattern: None,
            match_type: None,
            rule_order: None,
            exposure: Some("external".to_string()),
        },
    )
    .await
    .unwrap_or_else(|e| panic!("Failed to set route '{}' to external: {}", route_name, e));

    route.id
}

// ---------------------------------------------------------------------------
// 1.1 — Cross-Context Isolation
// ---------------------------------------------------------------------------

/// A cp-tool agent with a clusters:read grant can access clusters but not routes.
#[tokio::test]
async fn test_cp_agent_grant_permits_correct_resource() {
    let _db = TestDatabase::new("cp_grant_correct_resource").await;

    let grants = vec![CpGrant {
        resource_type: "clusters".to_string(),
        action: "read".to_string(),
        team: "test-team".to_string(),
    }];
    let ctx = cp_agent_context("cp-agent-1", grants);

    // Has clusters:read
    assert!(
        check_resource_access(&ctx, "clusters", "read", Some("test-team")),
        "cp-tool agent with clusters:read grant must have access"
    );

    // Does NOT have clusters:create
    assert!(
        !check_resource_access(&ctx, "clusters", "create", Some("test-team")),
        "cp-tool agent without clusters:create grant must be denied"
    );

    // Does NOT have routes:read (different resource)
    assert!(
        !check_resource_access(&ctx, "routes", "read", Some("test-team")),
        "cp-tool agent without routes:read grant must be denied"
    );
}

/// A cp-tool agent with grants cannot access resources in a different team.
#[tokio::test]
async fn test_cp_agent_grant_is_team_scoped() {
    let _db = TestDatabase::new("cp_grant_team_scoped").await;

    let grants = vec![CpGrant {
        resource_type: "clusters".to_string(),
        action: "read".to_string(),
        team: "test-team".to_string(),
    }];
    let ctx = cp_agent_context("cp-agent-2", grants);

    assert!(
        check_resource_access(&ctx, "clusters", "read", Some("test-team")),
        "grant for test-team must work"
    );
    assert!(
        !check_resource_access(&ctx, "clusters", "read", Some("team-a")),
        "grant for test-team must not work for team-a"
    );
}

/// A cp-tool agent with zero grants has no access to any resource.
#[tokio::test]
async fn test_cp_agent_zero_grants_has_no_access() {
    let _db = TestDatabase::new("cp_agent_zero_grants").await;

    let ctx = cp_agent_context("cp-agent-zero", vec![]);

    assert!(
        !check_resource_access(&ctx, "clusters", "read", None),
        "cp-tool agent with zero grants must have no cluster access"
    );
    assert!(
        !check_resource_access(&ctx, "routes", "read", None),
        "cp-tool agent with zero grants must have no route access"
    );
    assert!(
        !check_resource_access(&ctx, "listeners", "create", None),
        "cp-tool agent with zero grants must have no listener access"
    );
}

/// A gateway-tool agent cannot access any CP resources regardless of any scopes.
#[tokio::test]
async fn test_gateway_agent_cannot_access_cp_resources() {
    let _db = TestDatabase::new("gateway_no_cp_access").await;

    let ctx = gateway_agent_context("gw-agent-1");

    assert!(
        !check_resource_access(&ctx, "clusters", "read", None),
        "gateway-tool agent must not have cluster access"
    );
    assert!(
        !check_resource_access(&ctx, "routes", "create", Some("test-team")),
        "gateway-tool agent must not have route create access"
    );
    assert!(
        !check_resource_access(&ctx, "listeners", "read", None),
        "gateway-tool agent must not have listener access"
    );
}

/// An api-consumer agent cannot access any CP resources.
#[tokio::test]
async fn test_api_consumer_cannot_access_cp_resources() {
    let _db = TestDatabase::new("api_consumer_no_cp_access").await;

    let ctx = api_consumer_context("consumer-1");

    assert!(
        !check_resource_access(&ctx, "clusters", "read", None),
        "api-consumer agent must not have cluster access"
    );
    assert!(
        !check_resource_access(&ctx, "routes", "read", None),
        "api-consumer agent must not have route access"
    );
}

/// A cp-tool agent with multiple grants sees each resource independently.
#[tokio::test]
async fn test_cp_agent_multiple_grants() {
    let _db = TestDatabase::new("cp_multiple_grants").await;

    let grants = vec![
        CpGrant {
            resource_type: "clusters".to_string(),
            action: "read".to_string(),
            team: "test-team".to_string(),
        },
        CpGrant {
            resource_type: "routes".to_string(),
            action: "read".to_string(),
            team: "test-team".to_string(),
        },
        CpGrant {
            resource_type: "routes".to_string(),
            action: "create".to_string(),
            team: "test-team".to_string(),
        },
    ];
    let ctx = cp_agent_context("cp-agent-multi", grants);

    // Granted resources
    assert!(check_resource_access(&ctx, "clusters", "read", Some("test-team")));
    assert!(check_resource_access(&ctx, "routes", "read", Some("test-team")));
    assert!(check_resource_access(&ctx, "routes", "create", Some("test-team")));

    // NOT granted resources
    assert!(!check_resource_access(&ctx, "clusters", "create", Some("test-team")));
    assert!(!check_resource_access(&ctx, "listeners", "read", Some("test-team")));
    assert!(!check_resource_access(&ctx, "filters", "read", Some("test-team")));
}

// ---------------------------------------------------------------------------
// 1.2 — Grant API CRUD (DB-level tests)
// ---------------------------------------------------------------------------

/// Create a grant row, list it back, then delete it.
#[tokio::test]
async fn test_grant_crud_create_list_delete() {
    let db = TestDatabase::new("grant_crud").await;
    let pool = &db.pool;

    // Insert a test agent user
    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_test_user(pool, &agent_id, &format!("sub-{}", agent_id), "test-bot", "cp-tool").await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID).await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    // Create grant directly
    let grant_id = format!("grant-{}", uuid::Uuid::new_v4().simple());
    insert_grant(
        pool,
        &grant_id,
        &agent_id,
        TEST_ORG_ID,
        "test-team",
        "cp-tool",
        Some("clusters"),
        Some("read"),
        None,
    )
    .await;

    // List grants for agent
    type GrantRow = (String, String, String, Option<String>, Option<String>);
    #[allow(clippy::type_complexity)]
    let grants: Vec<GrantRow> = sqlx::query_as(
        "SELECT id, grant_type, team, resource_type, action \
         FROM agent_grants WHERE agent_id = $1",
    )
    .bind(&agent_id)
    .fetch_all(pool)
    .await
    .expect("Failed to list grants");

    assert_eq!(grants.len(), 1, "Should have exactly 1 grant");
    assert_eq!(grants[0].0, grant_id);
    assert_eq!(grants[0].1, "cp-tool");
    assert_eq!(grants[0].2, "test-team");
    assert_eq!(grants[0].3.as_deref(), Some("clusters"));
    assert_eq!(grants[0].4.as_deref(), Some("read"));

    // Delete grant
    sqlx::query("DELETE FROM agent_grants WHERE id = $1")
        .bind(&grant_id)
        .execute(pool)
        .await
        .expect("Failed to delete grant");

    // Verify gone
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_grants WHERE agent_id = $1")
        .bind(&agent_id)
        .fetch_one(pool)
        .await
        .expect("Failed to count grants");
    assert_eq!(count.0, 0, "Grant should be gone after deletion");
}

/// Duplicate cp-tool grants are rejected by the unique index.
#[tokio::test]
async fn test_duplicate_cp_tool_grant_rejected() {
    let db = TestDatabase::new("grant_duplicate").await;
    let pool = &db.pool;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_test_user(pool, &agent_id, &format!("sub-{}", agent_id), "dup-test", "cp-tool").await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID).await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    // First grant
    let grant_id1 = format!("grant-{}", uuid::Uuid::new_v4().simple());
    insert_grant(
        pool,
        &grant_id1,
        &agent_id,
        TEST_ORG_ID,
        "test-team",
        "cp-tool",
        Some("clusters"),
        Some("read"),
        None,
    )
    .await;

    // Second identical grant — must fail with unique constraint violation
    let grant_id2 = format!("grant-{}", uuid::Uuid::new_v4().simple());
    let result = sqlx::query(
        "INSERT INTO agent_grants \
         (id, agent_id, org_id, team, grant_type, resource_type, action, created_by) \
         VALUES ($1, $2, $3, 'test-team', 'cp-tool', 'clusters', 'read', $2)",
    )
    .bind(&grant_id2)
    .bind(&agent_id)
    .bind(TEST_ORG_ID)
    .execute(pool)
    .await;

    assert!(result.is_err(), "Duplicate cp-tool grant must be rejected");
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("23505") || err_str.contains("unique"),
        "Error must be a unique constraint violation, got: {}",
        err_str
    );
}

/// Duplicate gateway-tool grants are rejected by the unique index.
#[tokio::test]
async fn test_duplicate_gateway_tool_grant_rejected() {
    let db = TestDatabase::new("grant_gw_duplicate").await;
    let pool = &db.pool;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_test_user(pool, &agent_id, &format!("sub-{}", agent_id), "gw-dup-test", "gateway-tool")
        .await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID).await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    let route_id = create_external_route(pool, TEST_TEAM_ID, "gw-dup-route").await;

    // First grant
    insert_grant(
        pool,
        &format!("g1-{}", uuid::Uuid::new_v4().simple()),
        &agent_id,
        TEST_ORG_ID,
        "test-team",
        "gateway-tool",
        None,
        None,
        Some(route_id.as_str()),
    )
    .await;

    // Second identical grant — must fail
    let result = sqlx::query(
        "INSERT INTO agent_grants \
         (id, agent_id, org_id, team, grant_type, route_id, created_by) \
         VALUES ($1, $2, $3, 'test-team', 'gateway-tool', $4, $2)",
    )
    .bind(format!("g2-{}", uuid::Uuid::new_v4().simple()))
    .bind(&agent_id)
    .bind(TEST_ORG_ID)
    .bind(route_id.as_str())
    .execute(pool)
    .await;

    assert!(result.is_err(), "Duplicate gateway-tool grant must be rejected");
}

/// Agent deletion cascades to agent_grants rows.
#[tokio::test]
async fn test_agent_deletion_cascades_to_grants() {
    let db = TestDatabase::new("grant_cascade_delete").await;
    let pool = &db.pool;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_test_user(pool, &agent_id, &format!("sub-{}", agent_id), "cascade-test", "cp-tool")
        .await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID).await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    let grant_id = format!("grant-{}", uuid::Uuid::new_v4().simple());
    insert_grant(
        pool,
        &grant_id,
        &agent_id,
        TEST_ORG_ID,
        "test-team",
        "cp-tool",
        Some("routes"),
        Some("read"),
        None,
    )
    .await;

    // Verify grant exists
    let count_before: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM agent_grants WHERE agent_id = $1")
            .bind(&agent_id)
            .fetch_one(pool)
            .await
            .expect("Count query failed");
    assert_eq!(count_before.0, 1);

    // Delete user (ON DELETE CASCADE on agent_grants.agent_id FK)
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(&agent_id)
        .execute(pool)
        .await
        .expect("Failed to delete user");

    // Grants should be gone
    let count_after: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM agent_grants WHERE agent_id = $1")
            .bind(&agent_id)
            .fetch_one(pool)
            .await
            .expect("Count query failed");
    assert_eq!(count_after.0, 0, "Grants must cascade-delete when agent is deleted");
}

/// Multiple agents can hold grants for the same route.
#[tokio::test]
async fn test_multiple_agents_same_route_grant() {
    let db = TestDatabase::new("multi_agent_route").await;
    let pool = &db.pool;

    let route_id = create_external_route(pool, TEST_TEAM_ID, "shared-route").await;

    let agent_id1 = format!("user-{}", uuid::Uuid::new_v4().simple());
    let agent_id2 = format!("user-{}", uuid::Uuid::new_v4().simple());

    for (id, name) in [(&agent_id1, "agent-one"), (&agent_id2, "agent-two")] {
        insert_test_user(pool, id, &format!("sub-{}", id), name, "gateway-tool").await;
        insert_org_membership(pool, id, TEST_ORG_ID).await;
        insert_team_membership(pool, id, TEST_TEAM_ID).await;
    }

    // Both agents get grants on the same route
    insert_grant(
        pool,
        &format!("g1-{}", uuid::Uuid::new_v4().simple()),
        &agent_id1,
        TEST_ORG_ID,
        "test-team",
        "gateway-tool",
        None,
        None,
        Some(route_id.as_str()),
    )
    .await;

    insert_grant(
        pool,
        &format!("g2-{}", uuid::Uuid::new_v4().simple()),
        &agent_id2,
        TEST_ORG_ID,
        "test-team",
        "gateway-tool",
        None,
        None,
        Some(route_id.as_str()),
    )
    .await;

    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM agent_grants WHERE route_id = $1 AND grant_type = 'gateway-tool'",
    )
    .bind(route_id.as_str())
    .fetch_one(pool)
    .await
    .expect("Count query failed");

    assert_eq!(count.0, 2, "Both agents should have grants on the same route");
}

// ---------------------------------------------------------------------------
// 1.3 — Route Exposure + Grant Blocking
// ---------------------------------------------------------------------------

/// Internal route starts with exposure='internal' by default.
///
/// Uses the route DB directly (via RouteRepository) to verify the default.
#[tokio::test]
async fn test_new_route_defaults_to_internal() {
    let db = TestDatabase::new("route_default_internal").await;
    let pool = &db.pool;

    // create_external_route internally creates the route (starts internal) then sets external.
    // We verify internal state by querying DB after route creation but before the update.
    let cluster_name = format!("cluster-{}", uuid::Uuid::new_v4().simple());
    let rc_id = format!("rc-{}", uuid::Uuid::new_v4().simple());
    let vh_id = format!("vh-{}", uuid::Uuid::new_v4().simple());

    sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, version, team) \
         VALUES ($1, $2, $2, $3, 1, $4)",
    )
    .bind(format!("c-{}", uuid::Uuid::new_v4().simple()))
    .bind(&cluster_name)
    .bind(serde_json::json!({"type":"STRICT_DNS"}).to_string())
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await
    .expect("cluster insert failed");

    sqlx::query(
        "INSERT INTO route_configs (id, name, path_prefix, cluster_name, configuration, version, team) \
         VALUES ($1, $2, '/def', $3, $4, 1, $5)",
    )
    .bind(&rc_id)
    .bind(format!("rc-def-{}", uuid::Uuid::new_v4().simple()))
    .bind(&cluster_name)
    .bind(serde_json::json!({"cluster_name":&cluster_name}).to_string())
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await
    .expect("rc insert failed");

    sqlx::query(
        "INSERT INTO virtual_hosts (id, route_config_id, name, domains, rule_order) \
         VALUES ($1, $2, 'def-vh', '[\"*\"]', 0)",
    )
    .bind(&vh_id)
    .bind(&rc_id)
    .execute(pool)
    .await
    .expect("vh insert failed");

    let repo = RouteRepository::new(pool.clone());
    let route = repo
        .create(CreateRouteRequest {
            virtual_host_id: VirtualHostId::from_string(vh_id),
            name: "default-route".to_string(),
            path_pattern: "/default".to_string(),
            match_type: RouteMatchType::Prefix,
            rule_order: 0,
        })
        .await
        .expect("create route failed");

    assert_eq!(route.exposure, "internal", "New route must default to 'internal'");
}

/// Can change a route from internal to external.
#[tokio::test]
async fn test_route_can_be_marked_external() {
    let db = TestDatabase::new("route_mark_external").await;
    let pool = &db.pool;

    let cluster_name = format!("cluster-{}", uuid::Uuid::new_v4().simple());
    let rc_id = format!("rc-{}", uuid::Uuid::new_v4().simple());
    let vh_id = format!("vh-{}", uuid::Uuid::new_v4().simple());

    sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, version, team) \
         VALUES ($1, $2, $2, $3, 1, $4)",
    )
    .bind(format!("c-{}", uuid::Uuid::new_v4().simple()))
    .bind(&cluster_name)
    .bind(serde_json::json!({"type":"STRICT_DNS"}).to_string())
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await
    .expect("cluster insert failed");

    sqlx::query(
        "INSERT INTO route_configs (id, name, path_prefix, cluster_name, configuration, version, team) \
         VALUES ($1, $2, '/ext', $3, $4, 1, $5)",
    )
    .bind(&rc_id)
    .bind(format!("rc-ext-{}", uuid::Uuid::new_v4().simple()))
    .bind(&cluster_name)
    .bind(serde_json::json!({"cluster_name":&cluster_name}).to_string())
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await
    .expect("rc insert failed");

    sqlx::query(
        "INSERT INTO virtual_hosts (id, route_config_id, name, domains, rule_order) \
         VALUES ($1, $2, 'ext-vh', '[\"*\"]', 0)",
    )
    .bind(&vh_id)
    .bind(&rc_id)
    .execute(pool)
    .await
    .expect("vh insert failed");

    let repo = RouteRepository::new(pool.clone());
    let route = repo
        .create(CreateRouteRequest {
            virtual_host_id: VirtualHostId::from_string(vh_id),
            name: "ext-route".to_string(),
            path_pattern: "/ext".to_string(),
            match_type: RouteMatchType::Prefix,
            rule_order: 0,
        })
        .await
        .expect("create route failed");

    // Mark external
    let updated = repo
        .update(
            &route.id,
            UpdateRouteRequest {
                path_pattern: None,
                match_type: None,
                rule_order: None,
                exposure: Some("external".to_string()),
            },
        )
        .await
        .expect("update route failed");

    assert_eq!(updated.exposure, "external", "Route should now be external");
}

/// Changing an external route back to internal is blocked when active grants exist.
#[tokio::test]
async fn test_exposure_rollback_blocked_by_active_grants() {
    let db = TestDatabase::new("exposure_rollback_blocked").await;
    let pool = &db.pool;

    let route_id = create_external_route(pool, TEST_TEAM_ID, "blocked-route").await;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_test_user(
        pool,
        &agent_id,
        &format!("sub-{}", agent_id),
        "exposure-test",
        "gateway-tool",
    )
    .await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID).await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    // Create active grant on this route
    insert_grant(
        pool,
        &format!("g-{}", uuid::Uuid::new_v4().simple()),
        &agent_id,
        TEST_ORG_ID,
        "test-team",
        "gateway-tool",
        None,
        None,
        Some(route_id.as_str()),
    )
    .await;

    // Try to roll back to internal — must fail
    let repo = RouteRepository::new(pool.clone());
    let result = repo
        .update(
            &route_id,
            UpdateRouteRequest {
                path_pattern: None,
                match_type: None,
                rule_order: None,
                exposure: Some("internal".to_string()),
            },
        )
        .await;

    assert!(result.is_err(), "Exposure rollback must be rejected when active grants exist");
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("active agent grants") || err_str.contains("grant"),
        "Error must mention grants, got: {}",
        err_str
    );
}

/// Rolling back exposure to internal succeeds after revoking all grants.
#[tokio::test]
async fn test_exposure_rollback_allowed_after_grant_revocation() {
    let db = TestDatabase::new("exposure_rollback_after_revoke").await;
    let pool = &db.pool;

    let route_id = create_external_route(pool, TEST_TEAM_ID, "revoke-route").await;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_test_user(pool, &agent_id, &format!("sub-{}", agent_id), "revoke-test", "gateway-tool")
        .await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID).await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    let grant_id = format!("g-{}", uuid::Uuid::new_v4().simple());
    insert_grant(
        pool,
        &grant_id,
        &agent_id,
        TEST_ORG_ID,
        "test-team",
        "gateway-tool",
        None,
        None,
        Some(route_id.as_str()),
    )
    .await;

    // Revoke grant
    sqlx::query("DELETE FROM agent_grants WHERE id = $1")
        .bind(&grant_id)
        .execute(pool)
        .await
        .expect("Failed to revoke grant");

    // Now rollback to internal should succeed
    let repo = RouteRepository::new(pool.clone());
    let result = repo
        .update(
            &route_id,
            UpdateRouteRequest {
                path_pattern: None,
                match_type: None,
                rule_order: None,
                exposure: Some("internal".to_string()),
            },
        )
        .await;

    assert!(
        result.is_ok(),
        "Exposure rollback should succeed after all grants are revoked: {:?}",
        result
    );
    assert_eq!(result.unwrap().exposure, "internal");
}

/// Route deletion cascades to agent_grants rows.
#[tokio::test]
async fn test_route_deletion_cascades_to_grants() {
    let db = TestDatabase::new("route_cascade_grants").await;
    let pool = &db.pool;

    let route_id = create_external_route(pool, TEST_TEAM_ID, "cascade-route").await;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_test_user(
        pool,
        &agent_id,
        &format!("sub-{}", agent_id),
        "cascade-route-test",
        "gateway-tool",
    )
    .await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID).await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    insert_grant(
        pool,
        &format!("g-{}", uuid::Uuid::new_v4().simple()),
        &agent_id,
        TEST_ORG_ID,
        "test-team",
        "gateway-tool",
        None,
        None,
        Some(route_id.as_str()),
    )
    .await;

    // Delete route (cascade should remove grants)
    sqlx::query("DELETE FROM routes WHERE id = $1")
        .bind(route_id.as_str())
        .execute(pool)
        .await
        .expect("Failed to delete route");

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_grants WHERE route_id = $1")
        .bind(route_id.as_str())
        .fetch_one(pool)
        .await
        .expect("Count query failed");
    assert_eq!(count.0, 0, "Grants must cascade-delete when route is deleted");
}

// ---------------------------------------------------------------------------
// 1.4 — Grant Org Isolation (AuthContext level)
// ---------------------------------------------------------------------------

/// An org admin cannot create grants in a different org.
#[tokio::test]
async fn test_grant_org_isolation() {
    let _db = TestDatabase::new("grant_org_isolation").await;

    // Org admin for "acme-corp"
    let acme_admin = org_admin_context("acme-corp");

    // Can manage their own org
    assert!(
        require_org_admin_only(&acme_admin, "acme-corp").is_ok(),
        "Org admin must be able to manage their own org"
    );

    // Cannot manage "globex-corp"
    assert!(
        require_org_admin_only(&acme_admin, "globex-corp").is_err(),
        "Org admin must not be able to manage a different org"
    );
}

/// Platform admin cannot manage org grants (org-admin-only enforces this boundary).
#[tokio::test]
async fn test_platform_admin_cannot_manage_org_grants() {
    let _db = TestDatabase::new("platform_admin_grant_isolation").await;

    let platform_admin = AuthContext::new(
        TokenId::from_str_unchecked("platform-token"),
        "platform-admin".into(),
        vec!["admin:all".to_string()],
    );

    // Platform admin cannot use the org-admin-only grant endpoint
    assert!(
        require_org_admin_only(&platform_admin, "acme-corp").is_err(),
        "Platform admin must not be able to manage org grants"
    );
}

// ---------------------------------------------------------------------------
// 1.5 — Grant constraint validation
// ---------------------------------------------------------------------------

/// cp-tool grants require resource_type and action (DB constraint).
#[tokio::test]
async fn test_cp_tool_grant_requires_resource_type_and_action() {
    let db = TestDatabase::new("cp_grant_constraint").await;
    let pool = &db.pool;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_test_user(pool, &agent_id, &format!("sub-{}", agent_id), "cp-constraint", "cp-tool")
        .await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID).await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    // cp-tool grant without resource_type — must fail DB constraint
    let result = sqlx::query(
        "INSERT INTO agent_grants \
         (id, agent_id, org_id, team, grant_type, created_by) \
         VALUES ($1, $2, $3, 'test-team', 'cp-tool', $2)",
    )
    .bind(format!("g-{}", uuid::Uuid::new_v4().simple()))
    .bind(&agent_id)
    .bind(TEST_ORG_ID)
    .execute(pool)
    .await;

    assert!(
        result.is_err(),
        "cp-tool grant without resource_type must be rejected by DB constraint"
    );
}

/// gateway-tool grants require route_id (DB constraint).
#[tokio::test]
async fn test_gateway_tool_grant_requires_route_id() {
    let db = TestDatabase::new("gw_grant_constraint").await;
    let pool = &db.pool;

    let agent_id = format!("user-{}", uuid::Uuid::new_v4().simple());
    insert_test_user(
        pool,
        &agent_id,
        &format!("sub-{}", agent_id),
        "gw-constraint",
        "gateway-tool",
    )
    .await;
    insert_org_membership(pool, &agent_id, TEST_ORG_ID).await;
    insert_team_membership(pool, &agent_id, TEST_TEAM_ID).await;

    // gateway-tool grant without route_id — must fail DB constraint
    let result = sqlx::query(
        "INSERT INTO agent_grants \
         (id, agent_id, org_id, team, grant_type, created_by) \
         VALUES ($1, $2, $3, 'test-team', 'gateway-tool', $2)",
    )
    .bind(format!("g-{}", uuid::Uuid::new_v4().simple()))
    .bind(&agent_id)
    .bind(TEST_ORG_ID)
    .execute(pool)
    .await;

    assert!(
        result.is_err(),
        "gateway-tool grant without route_id must be rejected by DB constraint"
    );
}

// ---------------------------------------------------------------------------
// 1.6 — CP grant tools list filtering
// ---------------------------------------------------------------------------

/// The `check_resource_access` logic correctly derives the set of accessible
/// resources from a list of grants (simulating what tools/list filtering does).
#[tokio::test]
async fn test_cp_grants_produce_correct_tool_access_set() {
    let _db = TestDatabase::new("tool_access_set").await;

    let grants = vec![
        CpGrant {
            resource_type: "clusters".to_string(),
            action: "read".to_string(),
            team: "test-team".to_string(),
        },
        CpGrant {
            resource_type: "listeners".to_string(),
            action: "read".to_string(),
            team: "test-team".to_string(),
        },
    ];
    let ctx = cp_agent_context("cp-set-agent", grants);

    // Build expected access set by checking each resource:action pair
    let resources = [
        ("clusters", "read"),
        ("clusters", "create"),
        ("routes", "read"),
        ("routes", "create"),
        ("listeners", "read"),
        ("listeners", "create"),
        ("filters", "read"),
        ("filters", "create"),
    ];

    let mut granted_set: HashSet<(&str, &str)> = HashSet::new();
    for (resource, action) in &resources {
        if check_resource_access(&ctx, resource, action, Some("test-team")) {
            granted_set.insert((resource, action));
        }
    }

    assert!(granted_set.contains(&("clusters", "read")), "clusters:read must be in granted set");
    assert!(granted_set.contains(&("listeners", "read")), "listeners:read must be in granted set");
    assert!(
        !granted_set.contains(&("clusters", "create")),
        "clusters:create must NOT be in granted set"
    );
    assert!(!granted_set.contains(&("routes", "read")), "routes:read must NOT be in granted set");
    assert_eq!(granted_set.len(), 2, "Exactly 2 resource:action pairs must be granted");
}

/// An agent with no grants has an empty accessible resource set.
#[tokio::test]
async fn test_zero_grants_empty_tool_access_set() {
    let _db = TestDatabase::new("zero_grants_empty_set").await;

    let ctx = cp_agent_context("zero-grants-agent", vec![]);

    let resources = [
        ("clusters", "read"),
        ("clusters", "create"),
        ("routes", "read"),
        ("routes", "create"),
        ("listeners", "read"),
        ("listeners", "create"),
    ];

    for (resource, action) in &resources {
        assert!(
            !check_resource_access(&ctx, resource, action, Some("test-team")),
            "Agent with zero grants must have no access to {}:{}",
            resource,
            action
        );
    }
}

// ---------------------------------------------------------------------------
// 1.7 — Internal API exposure toggle (regression test for E.4 bug fix)
// ---------------------------------------------------------------------------

/// The internal API layer (RouteOperations) passes exposure through to the storage layer.
///
/// This is a regression test: the internal API previously hardcoded `exposure: None`,
/// which meant the MCP cp_update_route tool could never toggle route exposure.
#[tokio::test]
async fn test_internal_api_exposure_toggle() {
    let db = TestDatabase::new("internal_api_exposure").await;
    let pool = &db.pool;

    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));
    let route_ops = RouteOperations::new(state.clone());
    let auth = InternalAuthContext::for_team("test-team");

    // Setup: cluster → route_config → virtual_host chain
    let cluster_repo = state.cluster_repository.as_ref().expect("cluster repo");
    let cluster_req = flowplane::storage::repositories::cluster::CreateClusterRequest {
        name: "exposure-cluster".to_string(),
        service_name: "exposure-svc".to_string(),
        configuration: serde_json::json!({}),
        team: None,
        import_id: None,
    };
    let _ = cluster_repo.create(cluster_req).await;

    let rc_repo = state.route_config_repository.as_ref().expect("route config repo");
    let rc_req = flowplane::storage::repositories::route_config::CreateRouteConfigRequest {
        name: "exposure-rc".to_string(),
        path_prefix: "/".to_string(),
        cluster_name: "exposure-cluster".to_string(),
        configuration: serde_json::json!({}),
        team: None,
        import_id: None,
        route_order: None,
        headers: None,
    };
    let rc = rc_repo.create(rc_req).await.expect("create route config");

    let vh_repo = state.virtual_host_repository.as_ref().expect("virtual host repo");
    let vh_req = flowplane::storage::CreateVirtualHostRequest {
        route_config_id: rc.id.clone(),
        name: "exposure-vh".to_string(),
        domains: vec!["*".to_string()],
        rule_order: 0,
    };
    vh_repo.create(vh_req).await.expect("create virtual host");

    // Create route via internal API — defaults to internal
    let create_req = flowplane::internal_api::CreateRouteRequest {
        route_config: "exposure-rc".to_string(),
        virtual_host: "exposure-vh".to_string(),
        name: "toggle-route".to_string(),
        path_pattern: "/api/toggle".to_string(),
        match_type: "prefix".to_string(),
        rule_order: Some(0),
        action: serde_json::json!({"Cluster": {"name": "exposure-cluster"}}),
    };
    let created = route_ops.create(create_req, &auth).await.expect("create route");
    assert_eq!(created.data.exposure, "internal", "New route must default to internal");

    // Toggle to external via internal API (this was the bug — exposure was hardcoded to None)
    let update_req = flowplane::internal_api::UpdateRouteRequest {
        path_pattern: None,
        match_type: None,
        rule_order: None,
        action: None,
        exposure: Some("external".to_string()),
    };
    let updated = route_ops
        .update("exposure-rc", "exposure-vh", "toggle-route", update_req, &auth)
        .await
        .expect("update route exposure to external");
    assert_eq!(
        updated.data.exposure, "external",
        "Internal API must pass exposure through to storage layer"
    );

    // Toggle back to internal
    let update_req = flowplane::internal_api::UpdateRouteRequest {
        path_pattern: None,
        match_type: None,
        rule_order: None,
        action: None,
        exposure: Some("internal".to_string()),
    };
    let updated = route_ops
        .update("exposure-rc", "exposure-vh", "toggle-route", update_req, &auth)
        .await
        .expect("update route exposure back to internal");
    assert_eq!(updated.data.exposure, "internal", "Route must be back to internal");
}
