//! Spec-driven service tests for `fp_core::services::xds_status::list_nack_window` (S5.5).
//! Treats the service as a black box: the `check_resource_access(Stats, Read, team)` gate,
//! `window_total` independence from `limit`, `next_cursor` limit+1 semantics, the default page
//! size, and the empty `since > until` window. Rows with controlled timestamps are inserted
//! directly via SQL. Unique org/team names per run keep this parallel-safe (invariant 18).

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use chrono::{DateTime, Duration, Timelike, Utc};
use fp_core::services::xds_status::{list_nack_window, NackQuery, DEFAULT_NACK_LIMIT};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::identity;
use sqlx::PgPool;
use uuid::Uuid;

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
    org_id: fp_domain::OrgId,
    team: TeamRef,
}

async fn fixture() -> Option<Fixture> {
    let pool = test_pool().await?;
    let org = identity::create_org(&pool, &unique("org-nack"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team-nack"), "")
        .await
        .expect("team");
    let team_ref = identity::resolve_team_ref(&pool, team.id)
        .await
        .expect("resolve")
        .expect("team ref");
    Some(Fixture {
        pool,
        org_id: org.id,
        team: team_ref,
    })
}

/// A member holding (Stats, Read) on the team — passes `check_resource_access`.
async fn stats_reader_ctx(
    pool: &PgPool,
    org_id: fp_domain::OrgId,
    team_id: fp_domain::TeamId,
) -> PrincipalCtx {
    let subject = unique("sub-stats-reader");
    let user = identity::upsert_user_by_subject(pool, &subject, "reader@a.test", "Reader")
        .await
        .expect("user");
    identity::add_org_membership(pool, user, org_id, OrgRole::Member)
        .await
        .expect("member");
    identity::add_grant(
        pool,
        user,
        org_id,
        team_id,
        Resource::Stats,
        Action::Read,
        None,
    )
    .await
    .expect("grant stats read");
    principal_ctx(pool, &subject).await
}

fn base_time() -> DateTime<Utc> {
    Utc::now().with_nanosecond(0).unwrap()
}

async fn insert_nack(pool: &PgPool, team: &TeamRef, node_id: &str, created_at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO xds_nack_events \
           (id, team_id, org_id, node_id, type_url, version_rejected, error_message, \
            quarantined_resources, created_at) \
         VALUES ($1, $2, $3, $4, \
            'type.googleapis.com/envoy.config.listener.v3.Listener', '1', 'boom', \
            '[]'::jsonb, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(node_id)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("insert nack");
}

// AC4 + AC5: window_total counts every matching row regardless of the page limit; a smaller
// limit yields exactly `limit` events and a Some next_cursor (more remain).
#[tokio::test]
async fn window_total_is_independent_of_limit_and_next_cursor_signals_more() {
    let Some(f) = fixture().await else { return };
    let base = base_time();
    for i in 0..5i64 {
        insert_nack(
            &f.pool,
            &f.team,
            &format!("n-{i}"),
            base - Duration::seconds(i),
        )
        .await;
    }
    let ctx = stats_reader_ctx(&f.pool, f.org_id, f.team.id).await;

    let page = list_nack_window(
        &f.pool,
        &ctx,
        f.team,
        NackQuery {
            since: None,
            until: None,
            before: None,
            limit: Some(2),
        },
        RequestId::generate(),
    )
    .await
    .expect("authorized read");

    assert_eq!(page.events.len(), 2, "limit=2 returns two events");
    assert_eq!(
        page.window_total, 5,
        "window_total is the full matching count, not the page size"
    );
    assert!(
        page.next_cursor.is_some(),
        "a further page remains -> next_cursor is Some"
    );
    let cursor = page.next_cursor.unwrap();
    assert_eq!(
        cursor,
        (page.events[1].created_at, page.events[1].id),
        "next_cursor is (created_at, id) of the last returned row"
    );
}

// AC5: next_cursor is None once the last page is reached (limit+1 probe finds nothing extra).
#[tokio::test]
async fn next_cursor_is_none_on_the_last_page() {
    let Some(f) = fixture().await else { return };
    let base = base_time();
    insert_nack(&f.pool, &f.team, "n-0", base).await;
    insert_nack(&f.pool, &f.team, "n-1", base - Duration::seconds(1)).await;
    let ctx = stats_reader_ctx(&f.pool, f.org_id, f.team.id).await;

    let page = list_nack_window(
        &f.pool,
        &ctx,
        f.team,
        NackQuery {
            since: None,
            until: None,
            before: None,
            limit: Some(5),
        },
        RequestId::generate(),
    )
    .await
    .expect("read");

    assert_eq!(page.events.len(), 2);
    assert_eq!(page.window_total, 2);
    assert!(
        page.next_cursor.is_none(),
        "all rows fit on one page -> next_cursor is None"
    );
}

// AC5: an unfiltered query (no since/until, no limit) returns at most DEFAULT_NACK_LIMIT rows,
// newest first, and signals more remain.
#[tokio::test]
async fn unfiltered_returns_latest_default_limit_newest_first() {
    let Some(f) = fixture().await else { return };
    let base = base_time();
    let total = DEFAULT_NACK_LIMIT + 1; // 51 -> one page overflows the default
    for i in 0..total {
        insert_nack(
            &f.pool,
            &f.team,
            &format!("n-{i}"),
            base - Duration::seconds(i),
        )
        .await;
    }
    let ctx = stats_reader_ctx(&f.pool, f.org_id, f.team.id).await;

    let page = list_nack_window(
        &f.pool,
        &ctx,
        f.team,
        NackQuery::default(),
        RequestId::generate(),
    )
    .await
    .expect("read");

    assert_eq!(
        page.events.len() as i64,
        DEFAULT_NACK_LIMIT,
        "default page size caps at 50"
    );
    assert_eq!(
        page.window_total, total,
        "window_total reflects all matching rows"
    );
    assert!(page.next_cursor.is_some(), "more than a page exists");
    // Newest first: created_at is non-increasing across the page.
    for w in page.events.windows(2) {
        assert!(
            w[0].created_at >= w[1].created_at,
            "events ordered newest-first"
        );
    }
    // The newest row (i == 0, created_at == base) is at the head.
    assert_eq!(
        page.events[0].created_at, base,
        "the newest row leads the page"
    );
}

// AC6: since > until yields an empty window, not an error.
#[tokio::test]
async fn since_after_until_is_an_empty_window_not_an_error() {
    let Some(f) = fixture().await else { return };
    let base = base_time();
    for i in 0..3i64 {
        insert_nack(
            &f.pool,
            &f.team,
            &format!("n-{i}"),
            base - Duration::seconds(i),
        )
        .await;
    }
    let ctx = stats_reader_ctx(&f.pool, f.org_id, f.team.id).await;

    let page = list_nack_window(
        &f.pool,
        &ctx,
        f.team,
        NackQuery {
            since: Some(base + Duration::seconds(100)),
            until: Some(base),
            before: None,
            limit: None,
        },
        RequestId::generate(),
    )
    .await
    .expect("empty window is Ok, not Err");

    assert!(page.events.is_empty(), "no rows in an inverted window");
    assert_eq!(page.window_total, 0, "window_total is zero");
    assert!(page.next_cursor.is_none(), "no next page");
}

// AC8: a principal WITHOUT a (Stats, Read) grant for the team is denied.
#[tokio::test]
async fn read_without_stats_read_grant_is_forbidden() {
    let Some(f) = fixture().await else { return };
    insert_nack(&f.pool, &f.team, "n-0", base_time()).await;

    // Member of the org with an UNRELATED grant on the team, but no (Stats, Read).
    let subject = unique("sub-no-stats");
    let user = identity::upsert_user_by_subject(&f.pool, &subject, "other@a.test", "Other")
        .await
        .expect("user");
    identity::add_org_membership(&f.pool, user, f.org_id, OrgRole::Member)
        .await
        .expect("member");
    identity::add_grant(
        &f.pool,
        user,
        f.org_id,
        f.team.id,
        Resource::AiProviders,
        Action::Read,
        None,
    )
    .await
    .expect("unrelated grant");
    let ctx = principal_ctx(&f.pool, &subject).await;

    let err = list_nack_window(
        &f.pool,
        &ctx,
        f.team,
        NackQuery::default(),
        RequestId::generate(),
    )
    .await
    .expect_err("missing (stats, read) grant must deny");

    assert_eq!(err.code, ErrorCode::Forbidden);
}
