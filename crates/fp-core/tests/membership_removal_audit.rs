//! Acceptance criterion 10 of the grant referential-integrity feature: removing an org member
//! writes its `org.member.remove` audit row in the **same transaction** as the grant cascade.
//!
//! Why this needs its own test rather than riding on the storage-level cascade test: after
//! migration 0033 the cascade revokes authority as a *side effect of a different mutation*.
//! The design decides (and [[FP-DEC-0016]] records) that the single `org.member.remove` audit
//! row is sufficient evidence for that revocation, on the grounds that a grant is authority
//! contingent on the membership — so the audited event entails the consequence. That argument
//! only holds if the audit row and the revocation are genuinely atomic. If a removal could
//! commit its cascade while its audit row was lost, authority would vanish with no record.
//!
//! Two complementary assertions, because neither alone is enough:
//!
//! * `service_removal_...` drives the REAL `services::orgs::remove_org_member`, so it pins the
//!   *implementation property* the design's decision depends on. An earlier version of this file
//!   hand-rolled the transaction and constructed its own audit entry, which meant it would have
//!   stayed green if the service later moved its audit write outside the transaction or dropped
//!   it entirely — it tested PostgreSQL, not Flowplane. Caught at the S2 diff review.
//! * `rollback_...` operates on the repository primitives the service composes, because
//!   observing that both effects are present after a successful call is weak — it is equally
//!   consistent with two independent transactions that both happened to succeed. Rolling back
//!   and observing that *neither* took effect is what demonstrates they share a transaction.
//!   The service cannot be made to fail between its two writes without injecting a fault, so
//!   the atomicity proof necessarily sits one level down, on the exact calls it makes.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::{OrgRole, RequestId};
use fp_storage::repos::{audit, identity};
use sqlx::{PgPool, Row};

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

/// An org with one team, one member, and one grant held by that member.
struct Fixture {
    org_id: fp_domain::OrgId,
    user_id: fp_domain::UserId,
}

async fn fixture(pool: &PgPool) -> Fixture {
    let org = identity::create_org(pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(
        pool,
        &unique("subject"),
        &format!("{}@example.test", unique("user")),
        "Member",
    )
    .await
    .expect("user");
    identity::add_org_membership(pool, user, org.id, OrgRole::Member)
        .await
        .expect("membership");
    identity::add_grant(
        pool,
        user,
        org.id,
        team.id,
        fp_domain::authz::Resource::Clusters,
        fp_domain::authz::Action::Read,
        None,
    )
    .await
    .expect("grant");
    Fixture {
        org_id: org.id,
        user_id: user,
    }
}

async fn grant_count(pool: &PgPool, fx: &Fixture) -> i64 {
    // Scoped to this fixture's own rows — never a global count (parallel-safe by rule).
    sqlx::query_scalar("SELECT count(*) FROM user_grants WHERE user_id = $1 AND org_id = $2")
        .bind(fx.user_id.as_uuid())
        .bind(fx.org_id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("count grants")
}

async fn audit_count(pool: &PgPool, fx: &Fixture, request_id: RequestId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'org.member.remove' \
         AND org_id = $1 AND request_id = $2",
    )
    .bind(fx.org_id.as_uuid())
    .bind(request_id.as_uuid())
    .fetch_one(pool)
    .await
    .expect("count audit")
}

/// Build the same audit entry shape the org service records for a member removal.
fn removal_entry(fx: &Fixture, request_id: RequestId) -> audit::AuditEntry {
    audit::AuditEntry {
        request_id: Some(request_id),
        actor_type: audit::ActorType::User,
        actor_id: Some(fx.user_id.as_uuid()),
        actor_label: "test-admin".to_string(),
        surface: audit::Surface::Rest,
        action: "org.member.remove".to_string(),
        resource: format!("users/{}", fx.user_id),
        org_id: Some(fx.org_id),
        team_id: None,
        outcome: audit::Outcome::Success,
        detail: serde_json::json!({}),
    }
}

/// The REAL service call cascades the grant away AND records its audit row. This pins the
/// implementation property FP-DEC-0016 rests on: if `remove_org_member` ever stopped writing
/// `org.member.remove`, the cascade would still revoke authority but the evidence licensing the
/// no-per-grant-audit decision would be gone, and this test fails.
#[tokio::test]
async fn service_removal_cascades_grants_and_records_its_audit_row() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let fx = fixture(&pool).await;
    let request_id = RequestId::generate();

    assert_eq!(grant_count(&pool, &fx).await, 1, "fixture seeds one grant");
    assert_eq!(audit_count(&pool, &fx, request_id).await, 0);

    // An owner of the same org performs the removal, through the service's own authorization.
    let admin = identity::upsert_user_by_subject(
        &pool,
        &unique("admin-subject"),
        &format!("{}@example.test", unique("admin")),
        "Owner",
    )
    .await
    .expect("admin user");
    identity::add_org_membership(&pool, admin, fx.org_id, OrgRole::Owner)
        .await
        .expect("admin membership");
    let ctx = PrincipalCtx::User {
        user_id: admin,
        platform_admin: false,
        org: Some((fx.org_id, OrgRole::Owner)),
        org_selector_required: false,
        grants: GrantSet::default(),
    };

    fp_core::services::orgs::remove_org_member(&pool, &ctx, fx.org_id, fx.user_id, request_id)
        .await
        .expect("service removal");

    assert_eq!(
        grant_count(&pool, &fx).await,
        0,
        "the service removal must cascade the member's grants for that org away"
    );
    assert_eq!(
        audit_count(&pool, &fx, request_id).await,
        1,
        "the service must record exactly one org.member.remove audit row for this request"
    );
}

/// The load-bearing case. Rolling the transaction back must undo BOTH the revocation and the
/// audit row. If the cascade could survive a rollback that discarded the audit row, authority
/// would disappear with no record of why — which is exactly what the design's decision to
/// treat one membership-removal row as sufficient evidence would then be resting on.
#[tokio::test]
async fn rollback_undoes_the_cascade_and_the_audit_row_together() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let fx = fixture(&pool).await;
    let request_id = RequestId::generate();

    assert_eq!(grant_count(&pool, &fx).await, 1);

    let mut tx = pool.begin().await.expect("begin");
    identity::remove_org_membership_in_tx(&mut tx, fx.user_id, fx.org_id)
        .await
        .expect("remove membership");
    audit::record_in_tx(&mut tx, &removal_entry(&fx, request_id))
        .await
        .expect("audit");

    // Both writes are staged; prove they are visible INSIDE the transaction, so the rollback
    // assertions below cannot pass merely because the writes never happened.
    let staged_grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_grants WHERE user_id = $1 AND org_id = $2")
            .bind(fx.user_id.as_uuid())
            .bind(fx.org_id.as_uuid())
            .fetch_one(&mut *tx)
            .await
            .expect("staged grant count");
    assert_eq!(
        staged_grants, 0,
        "inside the transaction the cascade has already taken effect"
    );

    tx.rollback().await.expect("rollback");

    assert_eq!(
        grant_count(&pool, &fx).await,
        1,
        "a rolled-back removal must not revoke the grant"
    );
    assert_eq!(
        audit_count(&pool, &fx, request_id).await,
        0,
        "a rolled-back removal must not leave an audit row"
    );

    // Membership survives too, so the fixture is genuinely back to its starting state.
    let memberships: i64 =
        sqlx::query("SELECT count(*) AS c FROM org_memberships WHERE user_id = $1 AND org_id = $2")
            .bind(fx.user_id.as_uuid())
            .bind(fx.org_id.as_uuid())
            .fetch_one(&pool)
            .await
            .expect("membership count")
            .get("c");
    assert_eq!(memberships, 1, "rollback must restore the membership");
}
