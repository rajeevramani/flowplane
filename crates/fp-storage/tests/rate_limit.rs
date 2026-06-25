#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

//! Black-box, adversarial integration tests for the rate-limit storage layer (bead fpv2-4ht.1).
//!
//! These tests treat `fp_storage::repos::rate_limit` as opaque and exercise it only through its
//! public contract: tenant isolation, optimistic concurrency, soft delete, descriptor-match
//! determinism, override uniqueness, and composite FK/scope integrity. Validation of zero/empty
//! inputs is covered by unit tests elsewhere; here we attack the *storage* invariants.

use std::collections::BTreeMap;

use fp_domain::authz::TeamRef;
use fp_domain::{ErrorCode, RateLimitPolicySpec, RateLimitTeamOverrideSpec, RateLimitUnit};
use fp_storage::repos::{identity, rate_limit};
use fp_storage::scope::TeamScope;
use sqlx::PgPool;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

struct World {
    pool: PgPool,
    team_a: TeamRef,
    team_b: TeamRef,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 16).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&pool, org.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org.id, &unique("team-b"), "")
        .await
        .expect("team b");

    Some(World {
        pool,
        team_a: TeamRef {
            id: team_a.id,
            org_id: org.id,
        },
        team_b: TeamRef {
            id: team_b.id,
            org_id: org.id,
        },
    })
}

/// Build a policy spec from `(key, value)` descriptor pairs in the given order, so tests can
/// prove insertion order does not matter to canonicalization / matching.
fn spec_from(pairs: &[(&str, &str)], rpu: u64, unit: RateLimitUnit) -> RateLimitPolicySpec {
    let mut descriptors = BTreeMap::new();
    for (k, v) in pairs {
        descriptors.insert((*k).to_string(), (*v).to_string());
    }
    RateLimitPolicySpec {
        descriptors,
        requests_per_unit: rpu,
        unit,
    }
}

fn scope(team: TeamRef) -> TeamScope {
    TeamScope::Team(team.id)
}

// ---------------------------------------------------------------------------------------------
// 1. CRUD round-trip
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn crud_round_trip_domain_policy_override_starts_at_version_one() {
    let Some(w) = world().await else { return };

    let dom_name = unique("dom");
    let pol_name = unique("pol");
    let spec = spec_from(
        &[("path", "/v1"), ("method", "GET")],
        100,
        RateLimitUnit::Minute,
    );

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("create domain");
    assert_eq!(domain.version, 1, "new domain version must be 1");
    assert_eq!(domain.name, dom_name);
    assert_eq!(domain.team_id, w.team_a.id);

    let policy = rate_limit::create_policy(&mut tx, w.team_a, domain.id, &pol_name, &spec)
        .await
        .expect("create policy");
    assert_eq!(policy.version, 1, "new policy version must be 1");
    assert_eq!(policy.name, pol_name);
    assert_eq!(policy.domain_id, domain.id);
    assert_eq!(policy.team_id, w.team_a.id);
    assert_eq!(policy.spec.requests_per_unit, 100);
    assert_eq!(policy.spec.unit, RateLimitUnit::Minute);
    assert_eq!(policy.spec.descriptors, spec.descriptors);
    assert!(
        !policy.descriptors_canonical.is_empty(),
        "canonical descriptor string must be populated"
    );

    let override_spec = RateLimitTeamOverrideSpec {
        requests_per_unit: 42,
    };
    let ovr = rate_limit::create_override(&mut tx, w.team_a, policy.id, &override_spec)
        .await
        .expect("create override");
    assert_eq!(ovr.version, 1, "new override version must be 1");
    assert_eq!(ovr.policy_id, policy.id);
    assert_eq!(ovr.team_id, w.team_a.id);
    assert_eq!(ovr.spec.requests_per_unit, 42);
    tx.commit().await.unwrap();

    // get_* round-trips
    let got_dom = rate_limit::get_domain(&w.pool, scope(w.team_a), &dom_name)
        .await
        .expect("get domain")
        .expect("domain present");
    assert_eq!(got_dom.id, domain.id);
    assert_eq!(got_dom.version, 1);

    let got_pol = rate_limit::get_policy(&w.pool, scope(w.team_a), domain.id, &pol_name)
        .await
        .expect("get policy")
        .expect("policy present");
    assert_eq!(got_pol.id, policy.id);
    assert_eq!(got_pol.spec.descriptors, spec.descriptors);
    assert_eq!(got_pol.descriptors_canonical, policy.descriptors_canonical);

    let got_ovr = rate_limit::get_override(&w.pool, scope(w.team_a), policy.id)
        .await
        .expect("get override")
        .expect("override present");
    assert_eq!(got_ovr.id, ovr.id);
    assert_eq!(got_ovr.spec.requests_per_unit, 42);

    // list_* round-trips
    let (domains, dom_total) = rate_limit::list_domains(&w.pool, scope(w.team_a), 100, 0)
        .await
        .expect("list domains");
    assert!(dom_total >= 1);
    assert!(
        domains.iter().any(|d| d.id == domain.id),
        "listed domains must include the created domain"
    );

    let (policies, pol_total) =
        rate_limit::list_policies(&w.pool, scope(w.team_a), domain.id, 100, 0)
            .await
            .expect("list policies");
    assert_eq!(pol_total, 1, "domain should have exactly one policy");
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0].id, policy.id);

    assert_eq!(
        rate_limit::count_policies_for_team(&w.pool, w.team_a.id)
            .await
            .expect("count policies"),
        1
    );
}

// ---------------------------------------------------------------------------------------------
// 2. Tenant isolation (load-bearing)
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn tenant_isolation_domains_policies_overrides_invisible_across_teams() {
    let Some(w) = world().await else { return };

    let dom_name = unique("shared-dom");
    let pol_name = unique("shared-pol");
    // Identical descriptor set AND identical name for both teams — must not collide.
    let spec = spec_from(
        &[("svc", "checkout"), ("tier", "gold")],
        50,
        RateLimitUnit::Second,
    );

    let mut tx = w.pool.begin().await.unwrap();
    let dom_a = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom a");
    let pol_a = rate_limit::create_policy(&mut tx, w.team_a, dom_a.id, &pol_name, &spec)
        .await
        .expect("pol a");
    let _ovr_a = rate_limit::create_override(
        &mut tx,
        w.team_a,
        pol_a.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 9,
        },
    )
    .await
    .expect("ovr a");

    // team_b creates a domain + policy with the SAME name and SAME descriptors.
    let dom_b = rate_limit::create_domain(&mut tx, w.team_b, &dom_name)
        .await
        .expect("team b may reuse the same domain name");
    let pol_b = rate_limit::create_policy(&mut tx, w.team_b, dom_b.id, &pol_name, &spec)
        .await
        .expect("team b may reuse the same policy name + descriptors");
    tx.commit().await.unwrap();

    assert_ne!(dom_a.id, dom_b.id, "distinct teams get distinct domain ids");
    assert_ne!(pol_a.id, pol_b.id, "distinct teams get distinct policy ids");

    // team_b cannot see team_a's domain by name (scoped to team_b's own row).
    let dom_for_b = rate_limit::get_domain(&w.pool, scope(w.team_b), &dom_name)
        .await
        .expect("get domain as b")
        .expect("b sees its own domain");
    assert_eq!(dom_for_b.id, dom_b.id, "b must see ITS domain, not a's");

    // team_b querying team_a's domain_id for the policy must find nothing (cross-tenant id).
    assert!(
        rate_limit::get_policy(&w.pool, scope(w.team_b), dom_a.id, &pol_name)
            .await
            .expect("get policy cross-tenant")
            .is_none(),
        "team_b must not read a policy under team_a's domain_id"
    );

    // team_b cannot read team_a's override via team_a's policy_id.
    assert!(
        rate_limit::get_override(&w.pool, scope(w.team_b), pol_a.id)
            .await
            .expect("get override cross-tenant")
            .is_none(),
        "team_b must not read team_a's override"
    );

    // Listing scoped to team_b excludes team_a's policies (different domain ids anyway, but
    // assert team_b's own domain lists only team_b's policy).
    let (b_policies, b_total) =
        rate_limit::list_policies(&w.pool, scope(w.team_b), dom_b.id, 100, 0)
            .await
            .expect("list b policies");
    assert_eq!(b_total, 1);
    assert!(b_policies.iter().all(|p| p.team_id == w.team_b.id));
    assert!(b_policies.iter().any(|p| p.id == pol_b.id));

    // Listing team_a's domain_id under team_b's scope must be empty.
    let (cross, cross_total) =
        rate_limit::list_policies(&w.pool, scope(w.team_b), dom_a.id, 100, 0)
            .await
            .expect("list cross-tenant policies");
    assert_eq!(
        cross_total, 0,
        "team_b sees zero policies under team_a's domain"
    );
    assert!(cross.is_empty());

    // Counts are per-team.
    assert_eq!(
        rate_limit::count_policies_for_team(&w.pool, w.team_a.id)
            .await
            .expect("count a"),
        1
    );
    assert_eq!(
        rate_limit::count_policies_for_team(&w.pool, w.team_b.id)
            .await
            .expect("count b"),
        1
    );
}

// ---------------------------------------------------------------------------------------------
// 3. Optimistic concurrency
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn optimistic_concurrency_domain_stale_version_and_not_found() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("create domain");
    tx.commit().await.unwrap();
    assert_eq!(domain.version, 1);

    // Correct version succeeds and bumps to 2.
    let renamed = unique("dom-renamed");
    let mut tx = w.pool.begin().await.unwrap();
    let updated = rate_limit::update_domain(&mut tx, w.team_a.id, &dom_name, &renamed, 1)
        .await
        .expect("update with correct version");
    tx.commit().await.unwrap();
    assert_eq!(updated.version, 2, "successful update bumps version to 2");
    assert_eq!(updated.name, renamed);

    // Stale version (1) now fails with RevisionMismatch.
    let mut tx = w.pool.begin().await.unwrap();
    let err = rate_limit::update_domain(&mut tx, w.team_a.id, &renamed, &unique("nope"), 1)
        .await
        .expect_err("stale version must fail");
    assert_eq!(err.code, ErrorCode::RevisionMismatch);
    tx.rollback().await.unwrap();

    // Delete with stale version fails RevisionMismatch.
    let mut tx = w.pool.begin().await.unwrap();
    let err = rate_limit::delete_domain(&mut tx, w.team_a.id, &renamed, 1)
        .await
        .expect_err("stale delete must fail");
    assert_eq!(err.code, ErrorCode::RevisionMismatch);
    tx.rollback().await.unwrap();

    // Update / delete of a non-existent name → NotFound.
    let mut tx = w.pool.begin().await.unwrap();
    let err = rate_limit::update_domain(&mut tx, w.team_a.id, &unique("ghost"), &unique("x"), 1)
        .await
        .expect_err("update missing must fail");
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = rate_limit::delete_domain(&mut tx, w.team_a.id, &unique("ghost"), 1)
        .await
        .expect_err("delete missing must fail");
    assert_eq!(err.code, ErrorCode::NotFound);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn optimistic_concurrency_policy_and_override_versioning() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");
    let pol_name = unique("pol");
    let spec = spec_from(&[("a", "1")], 10, RateLimitUnit::Hour);

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom");
    let policy = rate_limit::create_policy(&mut tx, w.team_a, domain.id, &pol_name, &spec)
        .await
        .expect("pol");
    let ovr = rate_limit::create_override(
        &mut tx,
        w.team_a,
        policy.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 5,
        },
    )
    .await
    .expect("ovr");
    assert_eq!(ovr.version, 1, "new override starts at version 1");
    tx.commit().await.unwrap();

    // Policy: correct version → 2.
    let new_spec = spec_from(&[("a", "1")], 20, RateLimitUnit::Hour);
    let mut tx = w.pool.begin().await.unwrap();
    let updated =
        rate_limit::update_policy(&mut tx, w.team_a.id, domain.id, &pol_name, &new_spec, 1)
            .await
            .expect("policy update correct version");
    tx.commit().await.unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.spec.requests_per_unit, 20);

    // Policy: stale version → RevisionMismatch.
    let mut tx = w.pool.begin().await.unwrap();
    let err = rate_limit::update_policy(&mut tx, w.team_a.id, domain.id, &pol_name, &new_spec, 1)
        .await
        .expect_err("stale policy update");
    assert_eq!(err.code, ErrorCode::RevisionMismatch);
    tx.rollback().await.unwrap();

    // Policy: missing name → NotFound.
    let mut tx = w.pool.begin().await.unwrap();
    let err = rate_limit::update_policy(
        &mut tx,
        w.team_a.id,
        domain.id,
        &unique("ghost"),
        &new_spec,
        1,
    )
    .await
    .expect_err("missing policy update");
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = rate_limit::delete_policy(&mut tx, w.team_a.id, domain.id, &unique("ghost"), 1)
        .await
        .expect_err("missing policy delete");
    assert_eq!(err.code, ErrorCode::NotFound);
    tx.rollback().await.unwrap();

    // Override: correct version → 2.
    let mut tx = w.pool.begin().await.unwrap();
    let updated_ovr = rate_limit::update_override(
        &mut tx,
        w.team_a.id,
        policy.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 7,
        },
        1,
    )
    .await
    .expect("override update correct version");
    tx.commit().await.unwrap();
    assert_eq!(updated_ovr.version, 2);
    assert_eq!(updated_ovr.spec.requests_per_unit, 7);

    // Override: stale version → RevisionMismatch.
    let mut tx = w.pool.begin().await.unwrap();
    let err = rate_limit::update_override(
        &mut tx,
        w.team_a.id,
        policy.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 8,
        },
        1,
    )
    .await
    .expect_err("stale override update");
    assert_eq!(err.code, ErrorCode::RevisionMismatch);
    tx.rollback().await.unwrap();

    // Override: missing (use a fresh policy with no override) → NotFound.
    let other_pol = unique("pol2");
    let mut tx = w.pool.begin().await.unwrap();
    let policy2 = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &other_pol,
        &spec_from(&[("b", "2")], 1, RateLimitUnit::Day),
    )
    .await
    .expect("pol2");
    let err = rate_limit::update_override(
        &mut tx,
        w.team_a.id,
        policy2.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 3,
        },
        1,
    )
    .await
    .expect_err("override update on policy with no override");
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = rate_limit::delete_override(&mut tx, w.team_a.id, policy2.id, 1)
        .await
        .expect_err("override delete on policy with no override");
    assert_eq!(err.code, ErrorCode::NotFound);
    tx.rollback().await.unwrap();
}

// ---------------------------------------------------------------------------------------------
// 4. Soft delete
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn soft_delete_domain_hides_and_allows_name_reuse() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("create");
    let deleted_id = rate_limit::delete_domain(&mut tx, w.team_a.id, &dom_name, domain.version)
        .await
        .expect("delete");
    assert_eq!(deleted_id, domain.id);
    tx.commit().await.unwrap();

    // get returns None, list excludes it.
    assert!(rate_limit::get_domain(&w.pool, scope(w.team_a), &dom_name)
        .await
        .expect("get after delete")
        .is_none());
    let (domains, _) = rate_limit::list_domains(&w.pool, scope(w.team_a), 1000, 0)
        .await
        .expect("list after delete");
    assert!(
        domains.iter().all(|d| d.id != domain.id),
        "soft-deleted domain must not appear in list"
    );

    // Same name can be created again (live-only uniqueness), as a NEW row.
    let mut tx = w.pool.begin().await.unwrap();
    let recreated = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("recreate after soft delete");
    tx.commit().await.unwrap();
    assert_ne!(recreated.id, domain.id, "recreated domain is a new row");
    assert_eq!(
        recreated.version, 1,
        "recreated domain restarts at version 1"
    );
}

#[tokio::test]
async fn soft_delete_policy_hides_and_allows_name_and_descriptor_reuse() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");
    let pol_name = unique("pol");
    let descriptors: &[(&str, &str)] = &[("region", "us"), ("plan", "free")];

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom");
    let policy = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &pol_name,
        &spec_from(descriptors, 100, RateLimitUnit::Minute),
    )
    .await
    .expect("pol");
    let deleted =
        rate_limit::delete_policy(&mut tx, w.team_a.id, domain.id, &pol_name, policy.version)
            .await
            .expect("delete policy");
    assert_eq!(deleted, policy.id);
    tx.commit().await.unwrap();

    assert!(
        rate_limit::get_policy(&w.pool, scope(w.team_a), domain.id, &pol_name)
            .await
            .expect("get after delete")
            .is_none()
    );
    let (policies, total) = rate_limit::list_policies(&w.pool, scope(w.team_a), domain.id, 1000, 0)
        .await
        .expect("list after delete");
    assert_eq!(total, 0, "soft-deleted policy excluded from count");
    assert!(policies.is_empty());
    assert_eq!(
        rate_limit::count_policies_for_team(&w.pool, w.team_a.id)
            .await
            .expect("count after delete"),
        0,
        "count_policies_for_team must exclude soft-deleted policies"
    );

    // Same NAME and same DESCRIPTOR set can be reused after soft delete.
    let mut tx = w.pool.begin().await.unwrap();
    let recreated = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &pol_name,
        &spec_from(descriptors, 100, RateLimitUnit::Minute),
    )
    .await
    .expect("recreate policy reusing name + descriptors after soft delete");
    tx.commit().await.unwrap();
    assert_ne!(recreated.id, policy.id);
    assert_eq!(recreated.version, 1);
}

// ---------------------------------------------------------------------------------------------
// 5. Descriptor-match determinism + name/descriptor uniqueness
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_descriptor_set_same_domain_is_conflict_order_independent() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom");

    // First policy with a 3-key descriptor set. Commit it so it is a live row.
    let first = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &unique("p-first"),
        &spec_from(
            &[("a", "1"), ("b", "2"), ("c", "3")],
            10,
            RateLimitUnit::Second,
        ),
    )
    .await
    .expect("first policy");
    tx.commit().await.unwrap();

    // Second policy: SAME descriptor pairs, DIFFERENT insertion order, DIFFERENT name, even a
    // different limit/unit. Must still conflict on descriptor identity. Run in its own tx so a
    // poisoned transaction cannot bleed into later assertions.
    let mut tx = w.pool.begin().await.unwrap();
    let err = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &unique("p-second"),
        &spec_from(
            &[("c", "3"), ("a", "1"), ("b", "2")],
            999,
            RateLimitUnit::Day,
        ),
    )
    .await
    .expect_err("duplicate descriptor set must conflict regardless of insertion order");
    assert_eq!(err.code, ErrorCode::Conflict);
    let _ = tx.rollback().await;

    // A genuinely DIFFERENT descriptor set in the same domain is allowed.
    let mut tx = w.pool.begin().await.unwrap();
    let different = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &unique("p-diff"),
        &spec_from(&[("a", "1"), ("b", "999")], 10, RateLimitUnit::Second),
    )
    .await
    .expect("different descriptor set is allowed");
    assert_ne!(
        different.descriptors_canonical, first.descriptors_canonical,
        "different descriptor sets must produce different canonical strings"
    );
    tx.commit().await.unwrap();

    // Now prove canonical equality across insertion orders using two separate domains so the
    // uniqueness constraint does not interfere.
    let mut tx = w.pool.begin().await.unwrap();
    let dom_x = rate_limit::create_domain(&mut tx, w.team_a, &unique("dom-x"))
        .await
        .expect("dom x");
    let dom_y = rate_limit::create_domain(&mut tx, w.team_a, &unique("dom-y"))
        .await
        .expect("dom y");
    let px = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        dom_x.id,
        &unique("px"),
        &spec_from(
            &[("x", "1"), ("y", "2"), ("z", "3")],
            5,
            RateLimitUnit::Hour,
        ),
    )
    .await
    .expect("px");
    let py = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        dom_y.id,
        &unique("py"),
        &spec_from(
            &[("z", "3"), ("y", "2"), ("x", "1")],
            5,
            RateLimitUnit::Hour,
        ),
    )
    .await
    .expect("py");
    tx.commit().await.unwrap();
    assert_eq!(
        px.descriptors_canonical, py.descriptors_canonical,
        "same descriptor map in different insertion orders must canonicalize identically"
    );
}

#[tokio::test]
async fn duplicate_policy_name_same_domain_is_conflict() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");
    let pol_name = unique("pol");

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom");
    rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &pol_name,
        &spec_from(&[("k", "v1")], 10, RateLimitUnit::Second),
    )
    .await
    .expect("first policy");

    // Same NAME, DIFFERENT descriptor set → still a name conflict.
    let err = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &pol_name,
        &spec_from(&[("k", "v2")], 10, RateLimitUnit::Second),
    )
    .await
    .expect_err("duplicate policy name in same domain must conflict");
    assert_eq!(err.code, ErrorCode::Conflict);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn duplicate_domain_name_same_team_is_conflict() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");

    let mut tx = w.pool.begin().await.unwrap();
    rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("first domain");
    let err = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect_err("duplicate live domain name for same team must conflict");
    assert_eq!(err.code, ErrorCode::Conflict);
    tx.rollback().await.unwrap();
}

// ---------------------------------------------------------------------------------------------
// 6. Override uniqueness
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn override_uniqueness_and_recreate_after_delete() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");
    let pol_name = unique("pol");

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom");
    let policy = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &pol_name,
        &spec_from(&[("k", "v")], 10, RateLimitUnit::Second),
    )
    .await
    .expect("pol");

    let first = rate_limit::create_override(
        &mut tx,
        w.team_a,
        policy.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 5,
        },
    )
    .await
    .expect("first override");
    tx.commit().await.unwrap();

    // Second override for the SAME policy → Conflict. Own tx so a poison can't bleed forward.
    let mut tx = w.pool.begin().await.unwrap();
    let err = rate_limit::create_override(
        &mut tx,
        w.team_a,
        policy.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 6,
        },
    )
    .await
    .expect_err("second override on same policy must conflict");
    assert_eq!(err.code, ErrorCode::Conflict);
    let _ = tx.rollback().await;

    // Delete the override, then a new one can be created.
    let mut tx = w.pool.begin().await.unwrap();
    let deleted = rate_limit::delete_override(&mut tx, w.team_a.id, policy.id, first.version)
        .await
        .expect("delete override");
    assert_eq!(deleted, first.id);

    let recreated = rate_limit::create_override(
        &mut tx,
        w.team_a,
        policy.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 11,
        },
    )
    .await
    .expect("recreate override after delete");
    tx.commit().await.unwrap();
    assert_ne!(recreated.id, first.id);
    assert_eq!(recreated.version, 1);
    assert_eq!(recreated.spec.requests_per_unit, 11);
}

// ---------------------------------------------------------------------------------------------
// 7. FK / scope integrity (composite FK on (domain_id, team_id) and (policy_id, team_id))
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn create_policy_under_other_teams_domain_fails() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");

    let mut tx = w.pool.begin().await.unwrap();
    let dom_a = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("domain a");
    tx.commit().await.unwrap();

    // team_b tries to create a policy referencing team_a's domain_id. The composite FK
    // (domain_id, team_id) must reject this — there is no (dom_a.id, team_b) domain row.
    let mut tx = w.pool.begin().await.unwrap();
    let result = rate_limit::create_policy(
        &mut tx,
        w.team_b,
        dom_a.id,
        &unique("pol"),
        &spec_from(&[("k", "v")], 10, RateLimitUnit::Second),
    )
    .await;
    // A failure (rollback or returned error) is required; success is the bug.
    assert!(
        result.is_err(),
        "team_b must not create a policy under team_a's domain"
    );
    // Best effort to close the (possibly poisoned) transaction.
    let _ = tx.rollback().await;
}

#[tokio::test]
async fn create_override_on_other_teams_policy_fails() {
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");
    let pol_name = unique("pol");

    let mut tx = w.pool.begin().await.unwrap();
    let dom_a = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("domain a");
    let pol_a = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        dom_a.id,
        &pol_name,
        &spec_from(&[("k", "v")], 10, RateLimitUnit::Second),
    )
    .await
    .expect("policy a");
    tx.commit().await.unwrap();

    // team_b tries to attach an override to team_a's policy_id.
    let mut tx = w.pool.begin().await.unwrap();
    let result = rate_limit::create_override(
        &mut tx,
        w.team_b,
        pol_a.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 9,
        },
    )
    .await;
    assert!(
        result.is_err(),
        "team_b must not attach an override to team_a's policy"
    );
    let _ = tx.rollback().await;
}

// ---------------------------------------------------------------------------------------------
// 8. Soft-delete cascade (children tombstoned with parent; no creation under a deleted parent)
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn cascade_domain_delete_tombstones_policies_and_overrides() {
    // Acceptance: CASCADE on domain delete — every child is gone through the public API.
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");
    let pol_name = unique("pol");

    let before = rate_limit::count_policies_for_team(&w.pool, w.team_a.id)
        .await
        .expect("count before");

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom");
    let policy = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &pol_name,
        &spec_from(
            &[("svc", "pay"), ("tier", "gold")],
            100,
            RateLimitUnit::Minute,
        ),
    )
    .await
    .expect("pol");
    rate_limit::create_override(
        &mut tx,
        w.team_a,
        policy.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 7,
        },
    )
    .await
    .expect("ovr");
    tx.commit().await.unwrap();

    // Sanity: with the live tree, the child count rose by exactly one.
    assert_eq!(
        rate_limit::count_policies_for_team(&w.pool, w.team_a.id)
            .await
            .expect("count after create"),
        before + 1,
        "live policy should be counted before the cascade"
    );

    // Delete the domain — this must cascade-tombstone the policy and its override.
    let mut tx = w.pool.begin().await.unwrap();
    let deleted = rate_limit::delete_domain(&mut tx, w.team_a.id, &dom_name, domain.version)
        .await
        .expect("delete domain");
    assert_eq!(deleted, domain.id);
    tx.commit().await.unwrap();

    // Child policy is gone via every public read path.
    assert!(
        rate_limit::get_policy(&w.pool, scope(w.team_a), domain.id, &pol_name)
            .await
            .expect("get policy after cascade")
            .is_none(),
        "policy must be tombstoned when its domain is deleted"
    );
    let (policies, total) = rate_limit::list_policies(&w.pool, scope(w.team_a), domain.id, 1000, 0)
        .await
        .expect("list policies after cascade");
    assert_eq!(
        total, 0,
        "cascade must leave zero live policies in the domain"
    );
    assert!(policies.is_empty());
    assert_eq!(
        rate_limit::count_policies_for_team(&w.pool, w.team_a.id)
            .await
            .expect("count after cascade"),
        before,
        "cascade must restore the team policy count to its pre-create value"
    );

    // Grandchild override is gone.
    assert!(
        rate_limit::get_override(&w.pool, scope(w.team_a), policy.id)
            .await
            .expect("get override after cascade")
            .is_none(),
        "override must be tombstoned when its domain (via policy) is deleted"
    );
}

#[tokio::test]
async fn cascade_policy_delete_tombstones_override_but_keeps_domain() {
    // Acceptance: CASCADE on policy delete — override gone, policy gone, domain stays live.
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");
    let pol_name = unique("pol");

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom");
    let policy = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &pol_name,
        &spec_from(&[("k", "v")], 10, RateLimitUnit::Second),
    )
    .await
    .expect("pol");
    rate_limit::create_override(
        &mut tx,
        w.team_a,
        policy.id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 3,
        },
    )
    .await
    .expect("ovr");
    tx.commit().await.unwrap();

    let mut tx = w.pool.begin().await.unwrap();
    let deleted =
        rate_limit::delete_policy(&mut tx, w.team_a.id, domain.id, &pol_name, policy.version)
            .await
            .expect("delete policy");
    assert_eq!(deleted, policy.id);
    tx.commit().await.unwrap();

    // Override gone.
    assert!(
        rate_limit::get_override(&w.pool, scope(w.team_a), policy.id)
            .await
            .expect("get override after policy cascade")
            .is_none(),
        "override must be tombstoned when its policy is deleted"
    );
    // Policy gone.
    assert!(
        rate_limit::get_policy(&w.pool, scope(w.team_a), domain.id, &pol_name)
            .await
            .expect("get policy after delete")
            .is_none(),
        "deleted policy must not be readable"
    );
    // Domain stays live.
    let live_domain = rate_limit::get_domain(&w.pool, scope(w.team_a), &dom_name)
        .await
        .expect("get domain after policy delete");
    assert!(
        live_domain.is_some_and(|d| d.id == domain.id),
        "the domain must remain live after only its policy is deleted"
    );
}

#[tokio::test]
async fn create_policy_under_soft_deleted_domain_is_rejected() {
    // Acceptance: CREATE-UNDER-DELETED-DOMAIN rejected (NotFound; pre-check, not a SQL error).
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom");
    let captured_domain_id = domain.id;
    let deleted = rate_limit::delete_domain(&mut tx, w.team_a.id, &dom_name, domain.version)
        .await
        .expect("delete domain");
    assert_eq!(deleted, captured_domain_id);
    tx.commit().await.unwrap();

    // Creating a policy under the now-deleted domain_id must be rejected.
    let mut tx = w.pool.begin().await.unwrap();
    let result = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        captured_domain_id,
        &unique("pol"),
        &spec_from(&[("k", "v")], 10, RateLimitUnit::Second),
    )
    .await;
    assert!(
        result.is_err(),
        "must not create a policy under a soft-deleted domain"
    );
    if let Err(err) = &result {
        assert_eq!(
            err.code,
            ErrorCode::NotFound,
            "creating under a deleted domain should surface NotFound"
        );
    }
    // Pre-check should not poison the tx, but roll back defensively regardless.
    let _ = tx.rollback().await;
}

#[tokio::test]
async fn create_override_under_soft_deleted_policy_is_rejected() {
    // Acceptance: CREATE-UNDER-DELETED-POLICY rejected (NotFound).
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");
    let pol_name = unique("pol");

    let mut tx = w.pool.begin().await.unwrap();
    let domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("dom");
    let policy = rate_limit::create_policy(
        &mut tx,
        w.team_a,
        domain.id,
        &pol_name,
        &spec_from(&[("k", "v")], 10, RateLimitUnit::Second),
    )
    .await
    .expect("pol");
    let captured_policy_id = policy.id;
    let deleted =
        rate_limit::delete_policy(&mut tx, w.team_a.id, domain.id, &pol_name, policy.version)
            .await
            .expect("delete policy");
    assert_eq!(deleted, captured_policy_id);
    tx.commit().await.unwrap();

    // Creating an override under the now-deleted policy_id must be rejected.
    let mut tx = w.pool.begin().await.unwrap();
    let result = rate_limit::create_override(
        &mut tx,
        w.team_a,
        captured_policy_id,
        &RateLimitTeamOverrideSpec {
            requests_per_unit: 5,
        },
    )
    .await;
    assert!(
        result.is_err(),
        "must not create an override under a soft-deleted policy"
    );
    if let Err(err) = &result {
        assert_eq!(
            err.code,
            ErrorCode::NotFound,
            "creating under a deleted policy should surface NotFound"
        );
    }
    let _ = tx.rollback().await;
}

#[tokio::test]
async fn recreated_domain_after_cascade_has_no_resurrected_policies() {
    // Acceptance (regression): recreating a soft-deleted domain by name must not resurface a
    // cascade-tombstoned policy bound to the OLD domain_id under the NEW domain_id.
    let Some(w) = world().await else { return };
    let dom_name = unique("dom");
    let pol_name = unique("pol");

    let mut tx = w.pool.begin().await.unwrap();
    let old_domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("old dom");
    rate_limit::create_policy(
        &mut tx,
        w.team_a,
        old_domain.id,
        &pol_name,
        &spec_from(&[("k", "v")], 10, RateLimitUnit::Second),
    )
    .await
    .expect("old pol");
    rate_limit::delete_domain(&mut tx, w.team_a.id, &dom_name, old_domain.version)
        .await
        .expect("delete old domain (cascade)");
    tx.commit().await.unwrap();

    // Recreate the domain with the same name → a brand-new row / id.
    let mut tx = w.pool.begin().await.unwrap();
    let new_domain = rate_limit::create_domain(&mut tx, w.team_a, &dom_name)
        .await
        .expect("recreate domain");
    tx.commit().await.unwrap();
    assert_ne!(
        new_domain.id, old_domain.id,
        "recreated domain must be a new row with a new id"
    );

    // The cascade-tombstoned policy must NOT resurface under the new domain id.
    let (policies, total) =
        rate_limit::list_policies(&w.pool, scope(w.team_a), new_domain.id, 1000, 0)
            .await
            .expect("list under new domain");
    assert_eq!(
        total, 0,
        "a freshly recreated domain must start with zero policies"
    );
    assert!(policies.is_empty());

    // And the old policy name must not be readable under the new domain either.
    assert!(
        rate_limit::get_policy(&w.pool, scope(w.team_a), new_domain.id, &pol_name)
            .await
            .expect("get old policy under new domain")
            .is_none(),
        "old policy must not be reachable under the recreated domain"
    );
}
