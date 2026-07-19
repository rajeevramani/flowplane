//! AI trace-event cursor pagination integration tests (slice fpv2-0t4.3).
//!
//! Spec under test: `list_trace_events` returns rows in total order
//! `created_at DESC, id DESC` (id is the tiebreaker), and `AiTraceQuery.before =
//! Some((ts, id))` returns ONLY rows strictly before that position, i.e.
//! `(created_at, id) < (ts, id)` in row-value comparison. Paging by repeatedly
//! passing the last row of each page as the next cursor must yield every row
//! exactly once — no duplicates, no gaps — even across created_at ties spanning
//! page boundaries and retention deletes between page fetches.
//!
//! Parallel-safe: every test creates its own uniquely named org/team, seeds rows
//! with fresh UUID ids/request ids, keeps expires_at in the future (so concurrent
//! retention sweeps in other tests cannot touch these rows), and asserts only on
//! its own teams' rows.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};
use fp_domain::{AiTraceEvent, OrgId, TeamId};
use fp_storage::repos::{ai_trace, identity};
use sqlx::PgPool;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

struct World {
    pool: PgPool,
    #[allow(dead_code)]
    org: OrgId,
    team_a: TeamId,
    team_b: TeamId,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    let org = identity::create_org(&pool, &unique("org-cursor"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&pool, org.id, &unique("team-cursor-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org.id, &unique("team-cursor-b"), "")
        .await
        .expect("team b");
    Some(World {
        pool,
        org: org.id,
        team_a: team_a.id,
        team_b: team_b.id,
    })
}

/// Insert one trace row with an explicit created_at (schema per migration 0031:
/// hops JSONB defaults to [], expires_at NOT NULL — kept in the future so no
/// concurrent retention sweep can delete fixture rows mid-test). Returns the row id.
async fn seed_row(pool: &PgPool, team: TeamId, created_at: DateTime<Utc>) -> Uuid {
    let id = Uuid::now_v7();
    let request_id = Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO ai_trace_events \
           (id, team_id, request_id, route_config_id, hops, created_at, expires_at) \
         VALUES ($1, $2, $3, $4, '[]'::jsonb, $5, now() + interval '60 days')",
    )
    .bind(id)
    .bind(team.as_uuid())
    .bind(&request_id)
    .bind(Uuid::now_v7())
    .bind(created_at)
    .execute(pool)
    .await
    .expect("seed trace row");
    id
}

/// The spec's total order: created_at DESC, id DESC.
fn sort_desc(keys: &mut [(DateTime<Utc>, Uuid)]) {
    keys.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
}

fn keys_of(rows: &[AiTraceEvent]) -> Vec<(DateTime<Utc>, Uuid)> {
    rows.iter().map(|r| (r.created_at, r.id)).collect()
}

async fn list_page(
    pool: &PgPool,
    team: TeamId,
    before: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> Vec<AiTraceEvent> {
    ai_trace::list_trace_events(
        pool,
        team,
        ai_trace::AiTraceQuery {
            request_id: None,
            trace_id: None,
            before,
            limit,
        },
    )
    .await
    .expect("list_trace_events")
}

/// Page to exhaustion using the last row of each page as the next cursor.
/// Panics if paging fails to terminate or a page exceeds the limit.
async fn page_to_end(pool: &PgPool, team: TeamId, limit: i64) -> Vec<Vec<AiTraceEvent>> {
    let mut pages = Vec::new();
    let mut before = None;
    for _ in 0..50 {
        let page = list_page(pool, team, before, limit).await;
        if page.is_empty() {
            return pages;
        }
        assert!(
            page.len() as i64 <= limit,
            "page of {} rows exceeds limit {limit}",
            page.len()
        );
        let last = page.last().unwrap();
        before = Some((last.created_at, last.id));
        pages.push(page);
    }
    panic!("cursor paging did not terminate within 50 pages — cursor is not advancing");
}

/// Assert the concatenation of pages is strictly decreasing in (created_at, id)
/// row-value order — the mandated total order with id as tiebreaker.
fn assert_strictly_ordered(rows: &[AiTraceEvent]) {
    for pair in rows.windows(2) {
        let a = (pair[0].created_at, pair[0].id);
        let b = (pair[1].created_at, pair[1].id);
        assert!(
            b < a,
            "rows not in strict created_at DESC, id DESC order: {:?} then {:?}",
            a,
            b
        );
    }
}

// --- Test 1: full-order + tie ordering assertion (mandatory) -------------------------

#[tokio::test]
async fn results_are_ordered_created_at_desc_id_desc_over_tie_heavy_fixture() {
    let Some(w) = world().await else { return };
    let base = Utc::now() - Duration::minutes(10);

    // Tie-heavy fixture: 5 rows at t2 (newest), 4 at t1, 3 at t0 — whole-second
    // offsets so Postgres microsecond truncation cannot merge or reorder groups.
    let mut expected: Vec<(DateTime<Utc>, Uuid)> = Vec::new();
    for (offset_secs, count) in [(2, 5usize), (1, 4), (0, 3)] {
        let ts = base + Duration::seconds(offset_secs);
        for _ in 0..count {
            expected.push((ts, seed_row(&w.pool, w.team_a, ts).await));
        }
    }
    sort_desc(&mut expected);

    let rows = list_page(&w.pool, w.team_a, None, 100).await;
    assert_eq!(rows.len(), expected.len(), "all fixture rows returned");
    assert_eq!(
        keys_of(&rows),
        expected,
        "order must be exactly created_at DESC, id DESC (id breaks timestamp ties)"
    );
    assert_strictly_ordered(&rows);
}

// --- Test 2: exact-once paging with ties spanning a page boundary --------------------

#[tokio::test]
async fn cursor_paging_yields_every_row_exactly_once_with_ties_at_page_boundary() {
    let Some(w) = world().await else { return };
    let base = Utc::now() - Duration::minutes(10);

    // 12 rows, limit 4: 3 at the newest timestamp, then 6 sharing ONE identical
    // created_at (the tie group spans the page-1/page-2 boundary and fills page 2),
    // then 3 at the oldest.
    let mut fixture: BTreeSet<Uuid> = BTreeSet::new();
    let mut expected: Vec<(DateTime<Utc>, Uuid)> = Vec::new();
    for (offset_secs, count) in [(2, 3usize), (1, 6), (0, 3)] {
        let ts = base + Duration::seconds(offset_secs);
        for _ in 0..count {
            let id = seed_row(&w.pool, w.team_a, ts).await;
            fixture.insert(id);
            expected.push((ts, id));
        }
    }
    sort_desc(&mut expected);

    let pages = page_to_end(&w.pool, w.team_a, 4).await;
    assert_eq!(
        pages.len(),
        3,
        "12 rows at limit 4 pages in exactly 3 fetches"
    );

    let all: Vec<AiTraceEvent> = pages.into_iter().flatten().collect();
    let seen_ids: Vec<Uuid> = all.iter().map(|r| r.id).collect();
    let seen_set: BTreeSet<Uuid> = seen_ids.iter().copied().collect();
    assert_eq!(
        seen_set.len(),
        seen_ids.len(),
        "no row may appear on more than one page"
    );
    assert_eq!(
        seen_set, fixture,
        "union of pages must equal the full fixture — no gaps"
    );
    assert_eq!(
        keys_of(&all),
        expected,
        "concatenated pages reproduce the full created_at DESC, id DESC order"
    );
    assert_strictly_ordered(&all);
}

// --- Test 3: retention delete of already-seen rows between page fetches --------------

#[tokio::test]
async fn paging_survives_deletion_of_already_seen_rows_between_fetches() {
    let Some(w) = world().await else { return };
    let base = Utc::now() - Duration::minutes(10);

    // 12 rows with a 5-row tie group in the middle; limit 4.
    let mut fixture: BTreeSet<Uuid> = BTreeSet::new();
    for (offset_secs, count) in [(2, 4usize), (1, 5), (0, 3)] {
        let ts = base + Duration::seconds(offset_secs);
        for _ in 0..count {
            fixture.insert(seed_row(&w.pool, w.team_a, ts).await);
        }
    }

    let limit = 4i64;
    let page1 = list_page(&w.pool, w.team_a, None, limit).await;
    assert_eq!(page1.len(), 4);
    let last = page1.last().unwrap();
    let mut before = Some((last.created_at, last.id));

    // Retention delete mid-pagination: remove the already-seen page-1 rows
    // (including the row the cursor points at). Every one of them already
    // appeared, so the union of all pages must still equal the full fixture.
    for row in &page1 {
        let deleted = sqlx::query("DELETE FROM ai_trace_events WHERE id = $1 AND team_id = $2")
            .bind(row.id)
            .bind(w.team_a.as_uuid())
            .execute(&w.pool)
            .await
            .expect("delete seen row")
            .rows_affected();
        assert_eq!(deleted, 1, "seen row {} must exist to delete", row.id);
    }

    let mut all: Vec<AiTraceEvent> = page1;
    for _ in 0..50 {
        let page = list_page(&w.pool, w.team_a, before, limit).await;
        if page.is_empty() {
            break;
        }
        let last = page.last().unwrap();
        before = Some((last.created_at, last.id));
        all.extend(page);
    }

    let seen_ids: Vec<Uuid> = all.iter().map(|r| r.id).collect();
    let seen_set: BTreeSet<Uuid> = seen_ids.iter().copied().collect();
    assert_eq!(
        seen_set.len(),
        seen_ids.len(),
        "deleting seen rows must not make any row reappear (duplicate)"
    );
    assert_eq!(
        seen_set, fixture,
        "deleting already-seen rows must not skip any unseen row (gap)"
    );
    assert_strictly_ordered(&all);
}

// --- Test 4: cross-team rows at identical timestamps never leak ----------------------

#[tokio::test]
async fn cross_team_rows_at_identical_timestamps_never_appear_in_any_page() {
    let Some(w) = world().await else { return };
    let base = Utc::now() - Duration::minutes(10);

    // Interleave team B rows at IDENTICAL created_at values as team A's, including
    // inside team A's tie groups, so a team-unscoped cursor would splice them in.
    let mut fixture_a: BTreeSet<Uuid> = BTreeSet::new();
    let mut fixture_b: BTreeSet<Uuid> = BTreeSet::new();
    for (offset_secs, count) in [(2, 3usize), (1, 4), (0, 2)] {
        let ts = base + Duration::seconds(offset_secs);
        for _ in 0..count {
            fixture_a.insert(seed_row(&w.pool, w.team_a, ts).await);
            fixture_b.insert(seed_row(&w.pool, w.team_b, ts).await);
        }
    }

    let pages = page_to_end(&w.pool, w.team_a, 3).await;
    let all: Vec<AiTraceEvent> = pages.into_iter().flatten().collect();
    for row in &all {
        assert_eq!(
            row.team_id, w.team_a,
            "page contained a row from another team: {}",
            row.id
        );
        assert!(
            !fixture_b.contains(&row.id),
            "team B row {} leaked into team A's pages",
            row.id
        );
    }
    let seen: BTreeSet<Uuid> = all.iter().map(|r| r.id).collect();
    assert_eq!(
        seen, fixture_a,
        "team A paging returns exactly team A's fixture, exactly once"
    );
    assert_strictly_ordered(&all);
}

// --- Test 5: before composes with the request_id filter ------------------------------

#[tokio::test]
async fn before_cursor_composes_with_request_id_filter() {
    let Some(w) = world().await else { return };
    let base = Utc::now() - Duration::minutes(10);

    // Three rows at the SAME created_at so composition is exercised on the id
    // tiebreaker, plus one strictly newer row.
    let ts = base + Duration::seconds(1);
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(seed_row(&w.pool, w.team_a, ts).await);
    }
    let newest = seed_row(&w.pool, w.team_a, base + Duration::seconds(2)).await;

    let all = list_page(&w.pool, w.team_a, None, 10).await;
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].id, newest);
    let target = &all[2]; // middle of the tie group
    let target_request_id = target.request_id.clone();

    // Cursor strictly after (newer than) the target: the filtered query still finds it.
    let found = ai_trace::list_trace_events(
        &w.pool,
        w.team_a,
        ai_trace::AiTraceQuery {
            request_id: Some(&target_request_id),
            trace_id: None,
            before: Some((all[1].created_at, all[1].id)),
            limit: 10,
        },
    )
    .await
    .expect("request_id filter with newer cursor");
    assert_eq!(found.len(), 1, "target row is strictly before the cursor");
    assert_eq!(found[0].id, target.id);

    // Cursor AT the target's own position: strictly-before excludes the row itself.
    let excluded = ai_trace::list_trace_events(
        &w.pool,
        w.team_a,
        ai_trace::AiTraceQuery {
            request_id: Some(&target_request_id),
            trace_id: None,
            before: Some((target.created_at, target.id)),
            limit: 10,
        },
    )
    .await
    .expect("request_id filter with self cursor");
    assert!(
        excluded.is_empty(),
        "a row is never at a position strictly before itself"
    );
}
