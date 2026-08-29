//! Regression for fpv2-vgt: `fp_core::services::xds_status::status` must decide dataplane
//! liveness on the same clock authority and the same 60-second inclusive boundary as
//! `fp_storage::repos::dataplanes::stats_overview` (PostgreSQL `clock_timestamp()` against
//! the PostgreSQL-stamped `last_heartbeat_at`), so `ops xds status` and `stats overview`
//! never disagree about the same dataplane.
//!
//! Honesty note on the reproduction seam: a real host/DB clock offset cannot be induced on a
//! single synchronized host (the same limitation fpv2-ejw recorded). What CAN be shown
//! deterministically is the observable symptom — the two reads disagreeing for one row — via the
//! sub-second boundary: a heartbeat aged 60 s + 500 ms is stale under the SQL `interval`
//! comparison but was reported live by the pre-fix host-clock `num_seconds()` truncation. After
//! the fix both reads share PostgreSQL as the clock and use an exact 60-second comparison, so this
//! test is deterministically green; the 59 s / 61 s cases pin the unchanged threshold.
//!
//! Heartbeats are seeded relative to PostgreSQL's own clock so the seed and the evaluation live in
//! one clock domain. Unique org/team/dataplane names per run keep this parallel-safe
//! (constitution invariant 18). Authorization is real: reads go through a principal holding an
//! (Stats, Read) grant on the team.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::xds_status;
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{DataplaneId, OrgRole, RequestId};
use fp_storage::repos::{dataplanes, identity};
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

/// Mirror the auth middleware's D-014 resolution for single-org test users.
async fn principal_ctx(pool: &PgPool, subject: &str) -> PrincipalCtx {
    let loaded = identity::load_principal(pool, subject)
        .await
        .expect("load principal")
        .expect("principal exists");
    let candidates: Vec<_> = loaded
        .memberships
        .iter()
        .copied()
        .filter(|(org_id, _)| Some(*org_id) != loaded.platform_org_id)
        .collect();
    let (org, org_selector_required) = match candidates.as_slice() {
        [one] => (Some(*one), false),
        [] => (None, false),
        _ => (None, true),
    };
    PrincipalCtx::User {
        user_id: loaded.user_id,
        platform_admin: loaded.platform_admin,
        org_selector_required,
        org,
        grants: GrantSet::new(loaded.grants),
    }
}

struct Fixture {
    pool: PgPool,
    team: TeamRef,
    ctx: PrincipalCtx,
    dataplane: DataplaneId,
}

async fn fixture() -> Option<Fixture> {
    let pool = test_pool().await?;
    let org = identity::create_org(&pool, &unique("org-xds-live"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team-xds-live"), "")
        .await
        .expect("team");
    let team = identity::resolve_team_ref(&pool, team.id)
        .await
        .expect("resolve")
        .expect("team ref");

    let subject = unique("sub-xds-live-reader");
    let user = identity::upsert_user_by_subject(&pool, &subject, "reader@a.test", "Reader")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Member)
        .await
        .expect("member");
    identity::add_grant(
        &pool,
        user,
        org.id,
        team.id,
        Resource::Stats,
        Action::Read,
        None,
    )
    .await
    .expect("grant stats read");
    let ctx = principal_ctx(&pool, &subject).await;

    let mut tx = pool.begin().await.expect("begin");
    let dataplane = dataplanes::create_dataplane(&mut tx, team, &unique("dp-xds-live"), "")
        .await
        .expect("dataplane");
    tx.commit().await.expect("commit");

    Some(Fixture {
        pool,
        team,
        ctx,
        dataplane: dataplane.id,
    })
}

/// Seed `last_heartbeat_at` relative to PostgreSQL's own clock (the clock that stamps real
/// heartbeats), so the age is exact in the database clock domain.
async fn seed_heartbeat_age(pool: &PgPool, dataplane: DataplaneId, age: &str) {
    sqlx::query(&format!(
        "UPDATE dataplanes SET last_heartbeat_at = clock_timestamp() - interval '{age}' WHERE id = $1"
    ))
    .bind(dataplane.as_uuid())
    .execute(pool)
    .await
    .expect("seed heartbeat age");
}

/// Read both surfaces and return `(overview.live_dataplanes, xds.live_dataplanes, per-row live)`.
async fn read_both(f: &Fixture) -> (i64, i64, bool) {
    let overview = dataplanes::stats_overview(&f.pool, f.team.id)
        .await
        .expect("stats overview");
    let status = xds_status::status(&f.pool, &f.ctx, f.team, RequestId::generate())
        .await
        .expect("xds status");
    assert_eq!(
        status.total_dataplanes, 1,
        "fixture owns exactly one dataplane"
    );
    assert_eq!(
        status.live_dataplanes + status.stale_dataplanes,
        status.total_dataplanes,
        "live + stale must partition total"
    );
    let row = status
        .dataplanes
        .iter()
        .find(|d| d.dataplane.id == f.dataplane)
        .expect("fixture dataplane listed");
    (overview.live_dataplanes, status.live_dataplanes, row.live)
}

#[tokio::test]
async fn xds_status_liveness_agrees_with_stats_overview_on_the_database_clock() {
    let Some(f) = fixture().await else { return };

    // Clearly inside the window: both live (threshold unchanged).
    seed_heartbeat_age(&f.pool, f.dataplane, "59 seconds").await;
    let (overview, xds, row) = read_both(&f).await;
    assert_eq!(overview, 1, "59-second heartbeat is live in stats overview");
    assert_eq!(xds, 1, "59-second heartbeat is live in xds status");
    assert!(row, "59-second heartbeat row is live in xds status");

    // Clearly outside the window: both stale (threshold unchanged).
    seed_heartbeat_age(&f.pool, f.dataplane, "61 seconds").await;
    let (overview, xds, row) = read_both(&f).await;
    assert_eq!(
        overview, 0,
        "61-second heartbeat is stale in stats overview"
    );
    assert_eq!(xds, 0, "61-second heartbeat is stale in xds status");
    assert!(!row, "61-second heartbeat row is stale in xds status");

    // Just past the boundary in the database clock domain: stats overview says stale; xds
    // status must say the same. Pre-fix, host-clock `num_seconds()` truncation reported live.
    seed_heartbeat_age(&f.pool, f.dataplane, "60 seconds 500 milliseconds").await;
    let (overview, xds, row) = read_both(&f).await;
    assert_eq!(
        overview, 0,
        "60.5-second heartbeat is stale in stats overview"
    );
    assert_eq!(
        xds, overview,
        "xds status live_dataplanes must agree with stats overview for a 60.5-second heartbeat"
    );
    assert!(
        !row,
        "xds status per-dataplane live flag must agree with stats overview for a 60.5-second heartbeat"
    );

    // Heartbeat stamped in the future relative to the DB clock (DB-behind-writer skew shape):
    // both surfaces must still agree (live) and no arithmetic may panic or underflow.
    seed_heartbeat_age(&f.pool, f.dataplane, "-5 seconds").await;
    let (overview, xds, row) = read_both(&f).await;
    assert_eq!(
        overview, 1,
        "future-stamped heartbeat is live in stats overview"
    );
    assert_eq!(xds, 1, "future-stamped heartbeat is live in xds status");
    assert!(row, "future-stamped heartbeat row is live in xds status");
}

#[tokio::test]
async fn xds_status_never_reports_a_heartbeat_less_dataplane_as_live() {
    let Some(f) = fixture().await else { return };
    let (overview, xds, row) = read_both(&f).await;
    assert_eq!(overview, 0);
    assert_eq!(xds, 0);
    assert!(!row, "a dataplane that never reported is stale");
}
