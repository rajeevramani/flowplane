//! Multi-org "any team" authorization: real PostgreSQL rows → `load_principal` →
//! the authorization engine, for team-less checks (`team: None`).
//!
//! A grant is authority *inside the org that issued it*. A user who belongs to
//! several orgs carries every org's grants in one loaded grant set, so a team-less
//! "can I see any of these at all?" check must be answered against the ACTIVE org
//! only — otherwise authority leaks sideways between tenants of the same user.
//! These tests exercise that from real rows, not from hand-built grant sets, so a
//! regression in the loading path is caught alongside a regression in the engine.
//! Unique names per run keep this parallel-safe against sibling tests sharing the database.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::{check_resource_access, GrantSet, PrincipalCtx};
use fp_domain::authz::{Action, Resource};
use fp_domain::{OrgId, OrgRole};
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

/// Load a principal from real rows and pin the active org explicitly.
///
/// Unlike `tenancy.rs`'s inference helper, the active org is the variable under
/// test here: the same loaded grants must decide differently depending on which
/// org the caller is acting as, so the caller of this helper chooses it.
async fn ctx_with_active_org(
    pool: &PgPool,
    subject: &str,
    org: Option<(OrgId, OrgRole)>,
    org_selector_required: bool,
) -> PrincipalCtx {
    let loaded = identity::load_principal(pool, subject)
        .await
        .expect("load principal")
        .expect("principal exists");
    PrincipalCtx::User {
        user_id: loaded.user_id,
        platform_admin: loaded.platform_admin,
        org_selector_required,
        org,
        grants: GrantSet::new(loaded.grants),
    }
}

#[tokio::test]
async fn grant_in_another_org_does_not_authorize_a_team_less_read() {
    let Some(pool) = test_pool().await else {
        return;
    };

    let org_a = identity::create_org(&pool, &unique("org-a"), "Org A")
        .await
        .expect("org a");
    let org_b = identity::create_org(&pool, &unique("org-b"), "Org B")
        .await
        .expect("org b");
    let _team_a = identity::create_team(&pool, org_a.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org_b.id, &unique("team-b"), "")
        .await
        .expect("team b");

    // One human, two tenants. This is the shape that makes the bug reachable:
    // both orgs' grants arrive in a single loaded grant set.
    let sub = unique("sub-multi");
    let user = identity::upsert_user_by_subject(&pool, &sub, "m@m.test", "Mallory")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org_a.id, OrgRole::Member)
        .await
        .expect("member of a");
    identity::add_org_membership(&pool, user, org_b.id, OrgRole::Member)
        .await
        .expect("member of b");

    // The ONLY grant lives in org B.
    identity::add_grant(
        &pool,
        user,
        org_b.id,
        team_b.id,
        Resource::Agents,
        Action::Read,
        None,
    )
    .await
    .expect("grant in org b");

    // Acting as org A: org B's grant must not answer for org A. A team-less read is
    // exactly the "list everything I can see" question, and in org A the answer is
    // "nothing" — the user is a plain member there with no grants at all.
    let ctx_a = ctx_with_active_org(&pool, &sub, Some((org_a.id, OrgRole::Member)), false).await;
    let denied = check_resource_access(&ctx_a, Resource::Agents, Action::Read, None);
    assert!(
        !denied.is_allowed(),
        "a grant held in org B must not authorize a team-less read while org A is active, got {denied:?}"
    );

    // Same user, same loaded grants, only the active org differs: now it must pass.
    // This is the contrast that proves the deny above is org scoping and not the
    // grant simply failing to load.
    let ctx_b = ctx_with_active_org(&pool, &sub, Some((org_b.id, OrgRole::Member)), false).await;
    let allowed = check_resource_access(&ctx_b, Resource::Agents, Action::Read, None);
    assert!(
        allowed.is_allowed(),
        "the grant is real and must authorize the same team-less read with org B active, got {allowed:?}"
    );
}

#[tokio::test]
async fn no_resolved_active_org_denies_team_less_reads() {
    let Some(pool) = test_pool().await else {
        return;
    };

    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let team_a = identity::create_team(&pool, org_a.id, &unique("team-a"), "")
        .await
        .expect("team a");

    let sub = unique("sub-noselector");
    let user = identity::upsert_user_by_subject(&pool, &sub, "n@n.test", "Nora")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org_a.id, OrgRole::Member)
        .await
        .expect("member of a");
    identity::add_org_membership(&pool, user, org_b.id, OrgRole::Member)
        .await
        .expect("member of b");
    identity::add_grant(
        &pool,
        user,
        org_a.id,
        team_a.id,
        Resource::Agents,
        Action::Read,
        None,
    )
    .await
    .expect("grant in org a");

    // A multi-org caller who sent no org selector: nothing is active, so no grant
    // can be attributed to an org and the answer must be "no" rather than
    // "whatever you hold somewhere". Ambiguity must fail closed.
    let ctx = ctx_with_active_org(&pool, &sub, None, true).await;

    // Deny even for the pair they genuinely hold — holding it in *some* org is not
    // enough when no org is active.
    let held = check_resource_access(&ctx, Resource::Agents, Action::Read, None);
    assert!(
        !held.is_allowed(),
        "unresolved active org must deny even a genuinely held (agents, read), got {held:?}"
    );

    // And of course for a pair they hold nowhere.
    let unheld = check_resource_access(&ctx, Resource::Clusters, Action::Read, None);
    assert!(
        !unheld.is_allowed(),
        "unresolved active org must deny an unheld (clusters, read), got {unheld:?}"
    );
}

#[tokio::test]
async fn org_admin_default_still_works_for_team_less_reads() {
    let Some(pool) = test_pool().await else {
        return;
    };

    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let sub = unique("sub-orgadmin");
    let user = identity::upsert_user_by_subject(&pool, &sub, "o@o.test", "Olga")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org_a.id, OrgRole::Admin)
        .await
        .expect("admin of a");

    // No grant rows at all: an org admin's tenant-wide read comes from their org
    // role, not from grants. Scoping the any-team check to the active org must not
    // have collapsed that path into "needs a grant".
    let ctx = ctx_with_active_org(&pool, &sub, Some((org_a.id, OrgRole::Admin)), false).await;
    let decision = check_resource_access(&ctx, Resource::Clusters, Action::Read, None);
    assert!(
        decision.is_allowed(),
        "org admin must keep implicit team-less read in their own org, got {decision:?}"
    );
}

#[tokio::test]
async fn same_org_grant_still_authorizes() {
    let Some(pool) = test_pool().await else {
        return;
    };

    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let team_a = identity::create_team(&pool, org_a.id, &unique("team-a"), "")
        .await
        .expect("team a");

    let sub = unique("sub-member");
    let user = identity::upsert_user_by_subject(&pool, &sub, "p@p.test", "Pat")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org_a.id, OrgRole::Member)
        .await
        .expect("member of a");
    identity::add_grant(
        &pool,
        user,
        org_a.id,
        team_a.id,
        Resource::Agents,
        Action::Read,
        None,
    )
    .await
    .expect("grant in org a");

    // The everyday case: org scoping must not have made grants in the active org
    // stop counting.
    let ctx = ctx_with_active_org(&pool, &sub, Some((org_a.id, OrgRole::Member)), false).await;
    let decision = check_resource_access(&ctx, Resource::Agents, Action::Read, None);
    assert!(
        decision.is_allowed(),
        "a grant in the active org must authorize a team-less read, got {decision:?}"
    );
}
