//! Revoking a grant by id must revoke it in BOTH grant tables.
//!
//! Since migration 0033 a grant id names a row in either `user_grants` or `agent_grants`, and
//! the REST surface still takes a single opaque id. `user_grants.id` and `agent_grants.id` are
//! independent primary keys, so PostgreSQL permits the SAME uuid in both tables. An
//! implementation that stops at the first table it finds a match in would delete the user row,
//! report success, and leave an identically-identified agent grant still conferring authority.
//!
//! Random uuids make that collision astronomically unlikely in practice, which is exactly why
//! it needs a deliberate test: a revocation path must be correct by construction, not by
//! probability. This test constructs the collision on purpose.
//!
//! (Regression test for a real defect found at the S2 diff review, where `remove_grant` used a
//! short-circuiting `||` and a comment asserting a uniqueness guarantee the schema never made.)

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::{OrgRole, RequestId};
use fp_storage::repos::identity;
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

async fn row_exists(pool: &PgPool, table: &str, id: uuid::Uuid) -> bool {
    // `table` is a literal from this test, never user input.
    let sql = format!("SELECT count(*) FROM {table} WHERE id = $1");
    let count: i64 = sqlx::query_scalar(&sql)
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("count");
    count > 0
}

#[tokio::test]
async fn revoking_a_grant_id_present_in_both_tables_removes_both() {
    let Some(pool) = test_pool().await else {
        return;
    };

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");

    // A member holding a user grant on the team.
    let member = identity::upsert_user_by_subject(
        &pool,
        &unique("subject"),
        &format!("{}@example.test", unique("user")),
        "Member",
    )
    .await
    .expect("user");
    identity::add_org_membership(&pool, member, org.id, OrgRole::Member)
        .await
        .expect("membership");

    // An agent in the same org holding an agent grant on the same team.
    let agent_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, name, kind, token_hash) \
         VALUES ($1, $2, $3, 'cp-tool', $4)",
    )
    .bind(agent_id)
    .bind(org.id.as_uuid())
    .bind(unique("agent"))
    .bind(unique("token-hash"))
    .execute(&pool)
    .await
    .expect("agent");

    // THE POINT: one id, two tables. Both rows are legal — the ids are independent keys.
    let shared_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO user_grants (id, user_id, org_id, team_id, resource, action) \
         VALUES ($1, $2, $3, $4, 'clusters', 'read')",
    )
    .bind(shared_id)
    .bind(member.as_uuid())
    .bind(org.id.as_uuid())
    .bind(team.id.as_uuid())
    .execute(&pool)
    .await
    .expect("user grant");
    sqlx::query(
        "INSERT INTO agent_grants (id, agent_id, org_id, team_id, resource, action) \
         VALUES ($1, $2, $3, $4, 'clusters', 'read')",
    )
    .bind(shared_id)
    .bind(agent_id)
    .bind(org.id.as_uuid())
    .bind(team.id.as_uuid())
    .execute(&pool)
    .await
    .expect("agent grant");

    assert!(row_exists(&pool, "user_grants", shared_id).await);
    assert!(row_exists(&pool, "agent_grants", shared_id).await);

    let admin = identity::upsert_user_by_subject(
        &pool,
        &unique("admin-subject"),
        &format!("{}@example.test", unique("admin")),
        "Owner",
    )
    .await
    .expect("admin");
    identity::add_org_membership(&pool, admin, org.id, OrgRole::Owner)
        .await
        .expect("admin membership");
    let ctx = PrincipalCtx::User {
        user_id: admin,
        platform_admin: false,
        org: Some((org.id, OrgRole::Owner)),
        org_selector_required: false,
        grants: GrantSet::default(),
    };

    let request_id = RequestId::generate();
    fp_core::services::teams::remove_grant(
        &pool,
        &ctx,
        TeamRef {
            id: team.id,
            org_id: org.id,
        },
        shared_id,
        request_id,
    )
    .await
    .expect("revoke");

    assert!(
        !row_exists(&pool, "user_grants", shared_id).await,
        "the user grant must be revoked"
    );
    assert!(
        !row_exists(&pool, "agent_grants", shared_id).await,
        "the agent grant sharing the id must ALSO be revoked — stopping at the first match \
         would report success while leaving authority in force"
    );

    // Revoking two rows is still ONE revocation from the operator's point of view, so it must
    // produce exactly one audit row — not one per table, and not zero.
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'grant.remove' AND request_id = $1",
    )
    .bind(request_id.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("count audit");
    assert_eq!(
        audited, 1,
        "a single revocation must record exactly one grant.remove audit row"
    );
}
