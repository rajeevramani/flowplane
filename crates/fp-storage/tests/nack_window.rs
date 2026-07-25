//! Spec-driven storage tests for the NACK history window (S5.5).
//! Exercises `xds_nacks::list_window` / `count_window` as black boxes against the real DB:
//! since/until boundary semantics, cursor paging over identical timestamps and under
//! interleaved deletes, window counts, and team isolation. Rows are inserted directly via SQL
//! so timestamps and tie-breaking ids are fully controlled (`record` stamps now()/UUIDv7).
//! Unique org/team names per run keep this parallel-safe; every assertion is scoped to a
//! freshly-created team (invariant 18).

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use chrono::{DateTime, Duration, Timelike, Utc};
use fp_domain::authz::TeamRef;
use fp_storage::repos::identity;
use fp_storage::repos::xds_nacks::{self, NackWindowQuery};
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
    team_a: TeamRef,
    team_b: TeamRef,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let team_a = identity::create_team(&pool, org_a.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org_b.id, &unique("team-b"), "")
        .await
        .expect("team b");

    Some(World {
        pool,
        team_a: TeamRef {
            id: team_a.id,
            org_id: org_a.id,
        },
        team_b: TeamRef {
            id: team_b.id,
            org_id: org_b.id,
        },
    })
}

/// Whole-second `now()` so inserted values round-trip through TIMESTAMPTZ (microsecond
/// precision) exactly — boundary equality tests depend on it.
fn base_time() -> DateTime<Utc> {
    Utc::now().with_nanosecond(0).unwrap()
}

/// Insert one NACK row with a fully-controlled id / node_id / created_at.
async fn insert_nack(
    pool: &PgPool,
    team: &TeamRef,
    id: Uuid,
    node_id: &str,
    created_at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO xds_nack_events \
           (id, team_id, org_id, node_id, type_url, version_rejected, error_message, \
            quarantined_resources, created_at) \
         VALUES ($1, $2, $3, $4, \
            'type.googleapis.com/envoy.config.listener.v3.Listener', '1', 'boom', \
            '[]'::jsonb, $5)",
    )
    .bind(id)
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(node_id)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("insert nack");
}

// AC1: since INCLUSIVE, until EXCLUSIVE. A row exactly AT `since` is included; a row exactly
// AT `until` is excluded. window_total matches the exact set.
#[tokio::test]
async fn since_until_boundaries_are_inclusive_and_exclusive() {
    let Some(w) = world().await else { return };
    let base = base_time();
    let since = base;
    let until = base + Duration::seconds(100);

    // node -> created_at, straddling both boundaries.
    insert_nack(
        &w.pool,
        &w.team_a,
        Uuid::now_v7(),
        "before-since",
        base - Duration::seconds(10),
    )
    .await; // excluded (< since)
    insert_nack(&w.pool, &w.team_a, Uuid::now_v7(), "at-since", since).await; // included (== since)
    insert_nack(
        &w.pool,
        &w.team_a,
        Uuid::now_v7(),
        "mid",
        base + Duration::seconds(50),
    )
    .await; // included
    insert_nack(&w.pool, &w.team_a, Uuid::now_v7(), "at-until", until).await; // excluded (== until)
    insert_nack(
        &w.pool,
        &w.team_a,
        Uuid::now_v7(),
        "after-until",
        base + Duration::seconds(200),
    )
    .await; // excluded

    let events = xds_nacks::list_window(
        &w.pool,
        &NackWindowQuery {
            team_id: w.team_a.id,
            since: Some(since),
            until: Some(until),
            before: None,
            limit: 100,
        },
    )
    .await
    .expect("list window");

    let nodes: Vec<&str> = events.iter().map(|e| e.node_id.as_str()).collect();
    // ORDER BY created_at DESC: mid (base+50) then at-since (base).
    assert_eq!(
        nodes,
        vec!["mid", "at-since"],
        "exactly the [since, until) rows, newest first"
    );

    let count = xds_nacks::count_window(&w.pool, w.team_a.id, Some(since), Some(until))
        .await
        .expect("count window");
    assert_eq!(count, 2, "window_total counts only the [since, until) rows");
}

// AC2: page through a >limit fixture where EVERY row shares the same created_at. The union of
// pages must equal the full set with no repeats and no gaps.
#[tokio::test]
async fn cursor_paging_over_identical_created_at_covers_every_row_once() {
    let Some(w) = world().await else { return };
    let ts = base_time();
    let n = 7usize;
    let limit = 3i64;

    let mut inserted: Vec<Uuid> = Vec::new();
    for i in 0..n {
        let id = Uuid::now_v7();
        inserted.push(id);
        insert_nack(&w.pool, &w.team_a, id, &format!("node-{i}"), ts).await;
    }

    let mut seen: Vec<Uuid> = Vec::new();
    let mut before: Option<(DateTime<Utc>, Uuid)> = None;
    loop {
        let page = xds_nacks::list_window(
            &w.pool,
            &NackWindowQuery {
                team_id: w.team_a.id,
                since: None,
                until: None,
                before,
                limit,
            },
        )
        .await
        .expect("list page");
        if page.is_empty() {
            break;
        }
        assert!(page.len() as i64 <= limit, "a page never exceeds the limit");
        let last = page.last().unwrap();
        before = Some((last.created_at, last.id));
        seen.extend(page.iter().map(|e| e.id));
        if (page.len() as i64) < limit {
            break;
        }
    }

    assert_eq!(
        seen.len(),
        n,
        "no gaps and no repeats: every row returned exactly once"
    );
    let mut deduped = seen.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(deduped.len(), n, "no duplicate rows across pages");
    let mut expected = inserted.clone();
    expected.sort();
    assert_eq!(
        deduped, expected,
        "union of pages equals the full inserted set"
    );
}

// AC3: cursor paging is stable when not-yet-returned older rows are deleted mid-walk.
#[tokio::test]
async fn cursor_paging_is_stable_under_interleaved_deletes() {
    let Some(w) = world().await else { return };
    let base = base_time();
    let limit = 3i64;

    // 7 rows, strictly descending created_at: row 0 newest, row 6 oldest.
    let mut ids: Vec<Uuid> = Vec::new();
    for i in 0..7i64 {
        let id = Uuid::now_v7();
        ids.push(id);
        insert_nack(
            &w.pool,
            &w.team_a,
            id,
            &format!("node-{i}"),
            base - Duration::seconds(i),
        )
        .await;
    }

    // Page 1 (newest 3): rows 0,1,2.
    let page1 = xds_nacks::list_window(
        &w.pool,
        &NackWindowQuery {
            team_id: w.team_a.id,
            since: None,
            until: None,
            before: None,
            limit,
        },
    )
    .await
    .expect("page1");
    assert_eq!(page1.len(), 3);
    let mut seen: Vec<Uuid> = page1.iter().map(|e| e.id).collect();
    let last = page1.last().unwrap();
    let mut before = Some((last.created_at, last.id));

    // Delete two OLDER rows not yet returned (rows 5 and 6).
    for id in [ids[5], ids[6]] {
        sqlx::query("DELETE FROM xds_nack_events WHERE id = $1")
            .bind(id)
            .execute(&w.pool)
            .await
            .expect("delete older row");
    }

    // Continue paging to exhaustion.
    loop {
        let page = xds_nacks::list_window(
            &w.pool,
            &NackWindowQuery {
                team_id: w.team_a.id,
                since: None,
                until: None,
                before,
                limit,
            },
        )
        .await
        .expect("continue paging");
        if page.is_empty() {
            break;
        }
        let l = page.last().unwrap();
        before = Some((l.created_at, l.id));
        seen.extend(page.iter().map(|e| e.id));
        if (page.len() as i64) < limit {
            break;
        }
    }

    // Rows 3 and 4 survive; 5 and 6 were deleted before being reached.
    let mut deduped = seen.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        seen.len(),
        "no row returned twice across the interleaved delete"
    );
    let mut expected = vec![ids[0], ids[1], ids[2], ids[3], ids[4]];
    expected.sort();
    assert_eq!(
        deduped, expected,
        "walk yields the surviving rows exactly once, no crash"
    );
}

// AC4 (storage): count_window equals a direct COUNT over the window and is independent of any
// list limit.
#[tokio::test]
async fn count_window_matches_direct_count_and_is_independent_of_limit() {
    let Some(w) = world().await else { return };
    let base = base_time();
    let since = base;
    let until = base + Duration::seconds(100);

    for i in 0..5i64 {
        insert_nack(
            &w.pool,
            &w.team_a,
            Uuid::now_v7(),
            &format!("n-{i}"),
            base + Duration::seconds(i * 10),
        )
        .await;
    }

    let count = xds_nacks::count_window(&w.pool, w.team_a.id, Some(since), Some(until))
        .await
        .expect("count");
    let direct: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM xds_nack_events \
         WHERE team_id = $1 AND created_at >= $2 AND created_at < $3",
    )
    .bind(w.team_a.id.as_uuid())
    .bind(since)
    .bind(until)
    .fetch_one(&w.pool)
    .await
    .expect("direct count");
    assert_eq!(count, direct);
    assert_eq!(count, 5);

    let limited = xds_nacks::list_window(
        &w.pool,
        &NackWindowQuery {
            team_id: w.team_a.id,
            since: Some(since),
            until: Some(until),
            before: None,
            limit: 2,
        },
    )
    .await
    .expect("limited list");
    assert_eq!(
        limited.len(),
        2,
        "list respects the limit while count is unaffected"
    );
}

// AC7: rows for team B never appear in team A's window (and vice versa).
#[tokio::test]
async fn list_window_is_team_scoped() {
    let Some(w) = world().await else { return };
    let base = base_time();

    let a1 = Uuid::now_v7();
    let a2 = Uuid::now_v7();
    let b1 = Uuid::now_v7();
    insert_nack(&w.pool, &w.team_a, a1, "a-1", base).await;
    insert_nack(&w.pool, &w.team_a, a2, "a-2", base - Duration::seconds(1)).await;
    insert_nack(&w.pool, &w.team_b, b1, "b-1", base).await;

    let a_events = xds_nacks::list_window(
        &w.pool,
        &NackWindowQuery {
            team_id: w.team_a.id,
            since: None,
            until: None,
            before: None,
            limit: 100,
        },
    )
    .await
    .expect("list a");
    let a_ids: Vec<Uuid> = a_events.iter().map(|e| e.id).collect();
    assert_eq!(a_ids, vec![a1, a2], "only team A rows, newest first");
    assert!(
        !a_ids.contains(&b1),
        "team B row never leaks into team A's window"
    );
    assert_eq!(
        xds_nacks::count_window(&w.pool, w.team_a.id, None, None)
            .await
            .expect("count a"),
        2,
        "count is scoped to team A"
    );

    let b_events = xds_nacks::list_window(
        &w.pool,
        &NackWindowQuery {
            team_id: w.team_b.id,
            since: None,
            until: None,
            before: None,
            limit: 100,
        },
    )
    .await
    .expect("list b");
    let b_ids: Vec<Uuid> = b_events.iter().map(|e| e.id).collect();
    assert_eq!(b_ids, vec![b1], "team B sees only its own row");
}
