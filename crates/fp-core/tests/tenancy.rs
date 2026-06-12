//! S2 keystone integration test: real PostgreSQL rows → principal loading → the
//! authorization engine → denial audit. Two orgs, adversarial cross-org attempts.
//! Unique names per run keep this parallel-safe against sibling tests sharing the database.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::{check_resource_access, Decision, GrantSet, PrincipalCtx, Reason};
use fp_domain::authz::{Action, Resource};
use fp_domain::{OrgRole, RequestId};
use fp_storage::repos::{audit, identity};
use sqlx::PgPool;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

async fn test_pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn principal_ctx(pool: &PgPool, subject: &str) -> PrincipalCtx {
    let loaded = identity::load_principal(pool, subject)
        .await
        .expect("load principal")
        .expect("principal exists");
    PrincipalCtx::User {
        user_id: loaded.user_id,
        platform_admin: loaded.platform_admin,
        org: loaded.org,
        grants: GrantSet::new(loaded.grants),
    }
}

#[tokio::test]
async fn cross_org_isolation_holds_from_database_to_decision() {
    let Some(pool) = test_pool().await else {
        return;
    };

    // Two real orgs, each with a team and a user.
    let org_a = identity::create_org(&pool, &unique("org-a"), "Org A")
        .await
        .expect("org a");
    let org_b = identity::create_org(&pool, &unique("org-b"), "Org B")
        .await
        .expect("org b");
    let team_a = identity::create_team(&pool, org_a.id, &unique("team-a"), "")
        .await
        .expect("a");
    let team_b = identity::create_team(&pool, org_b.id, &unique("team-b"), "")
        .await
        .expect("b");

    let alice_sub = unique("sub-alice");
    let alice = identity::upsert_user_by_subject(&pool, &alice_sub, "a@a.test", "Alice")
        .await
        .expect("alice");
    identity::add_org_membership(&pool, alice, org_a.id, OrgRole::Admin)
        .await
        .expect("member");

    let alice_ctx = principal_ctx(&pool, &alice_sub).await;

    // Resolve both teams as the middleware would.
    let ref_a = identity::resolve_team_ref(&pool, team_a.id)
        .await
        .expect("q")
        .expect("team a");
    let ref_b = identity::resolve_team_ref(&pool, team_b.id)
        .await
        .expect("q")
        .expect("team b");

    // Own-org team: org-admin implicit access.
    assert!(
        check_resource_access(&alice_ctx, Resource::Clusters, Action::Create, Some(ref_a))
            .is_allowed()
    );

    // Foreign team resolved from the REAL database: denied as cross-org.
    let decision = check_resource_access(&alice_ctx, Resource::Clusters, Action::Read, Some(ref_b));
    assert_eq!(decision, Decision::Deny(Reason::CrossOrg));

    // The denial is auditable end-to-end.
    let rid = RequestId::generate();
    audit::record_best_effort(
        &pool,
        &audit::AuditEntry::denial(
            rid,
            Some(alice),
            audit::Surface::Rest,
            format!("teams/{}", team_b.id),
            decision.reason().as_str(),
        ),
    )
    .await;
    let (outcome,): (String,) =
        sqlx::query_as("SELECT outcome FROM audit_log WHERE request_id = $1")
            .bind(rid.as_uuid())
            .fetch_one(&pool)
            .await
            .expect("audit row");
    assert_eq!(outcome, "denied");
}

#[tokio::test]
async fn grants_load_from_real_rows_and_cross_org_grants_are_unrepresentable() {
    let Some(pool) = test_pool().await else {
        return;
    };

    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let team_a = identity::create_team(&pool, org_a.id, &unique("team"), "")
        .await
        .expect("a");
    let team_b = identity::create_team(&pool, org_b.id, &unique("team"), "")
        .await
        .expect("b");

    let bob_sub = unique("sub-bob");
    let bob = identity::upsert_user_by_subject(&pool, &bob_sub, "b@a.test", "Bob")
        .await
        .expect("bob");
    identity::add_org_membership(&pool, bob, org_a.id, OrgRole::Member)
        .await
        .expect("member");

    // A legitimate grant row in Bob's org...
    identity::add_grant(
        &pool,
        bob,
        org_a.id,
        team_a.id,
        Resource::Secrets,
        Action::Read,
        None,
    )
    .await
    .expect("grant in own org");

    // ...and an attempted cross-org grant (team B paired with org A): the composite FK
    // makes the row unrepresentable at the schema level (spec/08a §2.2.9).
    let err = identity::add_grant(
        &pool,
        bob,
        org_a.id,
        team_b.id,
        Resource::Secrets,
        Action::Read,
        None,
    )
    .await
    .expect_err("cross-org grant must be rejected by the schema");
    assert_eq!(err.code, fp_domain::ErrorCode::ValidationFailed);

    // The loaded principal carries exactly the legitimate grant.
    let ctx = principal_ctx(&pool, &bob_sub).await;
    let ref_a = identity::resolve_team_ref(&pool, team_a.id)
        .await
        .expect("q")
        .expect("a");
    assert_eq!(
        check_resource_access(&ctx, Resource::Secrets, Action::Read, Some(ref_a)),
        Decision::Allow(Reason::GrantMatch)
    );
    assert!(
        !check_resource_access(&ctx, Resource::Secrets, Action::Update, Some(ref_a)).is_allowed()
    );
}

#[tokio::test]
async fn suspended_user_loads_as_absent() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let sub = unique("sub-suspended");
    let user = identity::upsert_user_by_subject(&pool, &sub, "s@s.test", "Sue")
        .await
        .expect("u");
    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(user.as_uuid())
        .execute(&pool)
        .await
        .expect("suspend");
    let loaded = identity::load_principal(&pool, &sub).await.expect("load");
    assert!(
        loaded.is_none(),
        "suspended user must be indistinguishable from absent"
    );
}

#[tokio::test]
async fn duplicate_names_conflict_within_org_but_not_across_orgs() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let org_a = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("a");
    let org_b = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("b");
    let shared_name = unique("payments");

    identity::create_team(&pool, org_a.id, &shared_name, "")
        .await
        .expect("first");
    // Same name in ANOTHER org: fine (per-org namespaces, no cross-tenant name oracle).
    identity::create_team(&pool, org_b.id, &shared_name, "")
        .await
        .expect("other org same name");
    // Same name in the SAME org: conflict with hint.
    let err = identity::create_team(&pool, org_a.id, &shared_name, "")
        .await
        .expect_err("duplicate within org");
    assert_eq!(err.code, fp_domain::ErrorCode::Conflict);
    assert!(err.hint.is_some());
}
