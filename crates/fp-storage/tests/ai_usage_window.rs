#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

//! Spec-first integration tests for `fp_storage::repos::ai::usage_summary`
//! (slice fpv2-0t4.1): half-open time windows, grouped totals, pagination,
//! cross-team isolation, and index suitability of the windowed grouped query.

use chrono::{DateTime, Duration, TimeZone, Utc};
use fp_domain::authz::TeamRef;
use fp_domain::{AiProviderId, AiUsageSummary, OpenAiTokenUsage, RouteConfigId};
use fp_storage::repos::ai::{self, AiUsageQuery};
use fp_storage::repos::identity;
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

async fn insert_provider_and_route(pool: &PgPool, team: TeamRef) -> (AiProviderId, RouteConfigId) {
    let secret_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets \
         (id, team_id, org_id, name, description, secret_type, configuration_encrypted, nonce, encryption_key_id) \
         VALUES ($1, $2, $3, $4, '', 'generic_secret', $5, $6, 'default')",
    )
    .bind(secret_id)
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(unique("ai-key"))
    .bind(Vec::<u8>::from([1_u8]))
    .bind(Vec::<u8>::from([2_u8; 12]))
    .execute(pool)
    .await
    .expect("secret");

    let provider_id = AiProviderId::generate();
    sqlx::query(
        "INSERT INTO ai_providers \
         (id, team_id, org_id, name, kind, base_url, credential_secret_id, auth_header) \
         VALUES ($1, $2, $3, $4, 'openai', 'https://api.openai.example', $5, 'authorization')",
    )
    .bind(provider_id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(unique("provider"))
    .bind(secret_id)
    .execute(pool)
    .await
    .expect("provider");

    let route_config_id = RouteConfigId::generate();
    sqlx::query(
        "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, '{\"virtual_hosts\":[]}'::jsonb)",
    )
    .bind(route_config_id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(unique("route-config"))
    .execute(pool)
    .await
    .expect("route config");

    (provider_id, route_config_id)
}

/// Insert a usage event with an explicit `created_at` (the public
/// `record_usage_event` stamps `now()`, which windowed tests cannot use).
async fn insert_event_at(
    pool: &PgPool,
    team: TeamRef,
    route_config_id: RouteConfigId,
    provider_id: AiProviderId,
    (prompt, completion, total): (i64, i64, i64),
    created_at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO ai_usage_events \
         (id, team_id, route_config_id, provider_id, prompt_tokens, completion_tokens, total_tokens, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(team.id.as_uuid())
    .bind(route_config_id.as_uuid())
    .bind(provider_id.as_uuid())
    .bind(prompt)
    .bind(completion)
    .bind(total)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("insert usage event");
}

async fn record_now(
    pool: &PgPool,
    team: TeamRef,
    route_config_id: RouteConfigId,
    provider_id: AiProviderId,
    (prompt_tokens, completion_tokens, total_tokens): (u64, u64, u64),
) {
    ai::record_usage_event(
        pool,
        ai::AiUsageEventInsert {
            team_id: team.id,
            route_config_id,
            provider_id,
            backend_position: Some(0),
            usage: OpenAiTokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
            },
        },
    )
    .await
    .expect("record usage event");
}

/// A query with no filters, no window, and a page large enough to hold
/// everything a single test seeds. Override fields with struct-update syntax.
fn all_time() -> AiUsageQuery {
    AiUsageQuery {
        route_config_id: None,
        provider_id: None,
        since: None,
        until: None,
        limit: 100,
        offset: 0,
    }
}

fn find_pair(
    items: &[AiUsageSummary],
    route_config_id: RouteConfigId,
    provider_id: AiProviderId,
) -> Option<&AiUsageSummary> {
    items
        .iter()
        .find(|s| s.route_config_id == Some(route_config_id) && s.provider_id == Some(provider_id))
}

/// Fixed whole-second boundaries so `>=` / `<` comparisons are exact after the
/// microsecond-precision TIMESTAMPTZ round-trip.
fn t1() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap()
}

fn t2() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 5, 1, 13, 0, 0).unwrap()
}

/// Seeds the canonical five-event fixture around [T1, T2) for one
/// (route, provider) pair: before T1, exactly at T1, inside, exactly at T2,
/// and after T2. Returns the pair.
async fn seed_boundary_events(pool: &PgPool, team: TeamRef) -> (AiProviderId, RouteConfigId) {
    let (provider, route) = insert_provider_and_route(pool, team).await;
    let before = t1() - Duration::hours(1);
    let inside = t1() + Duration::minutes(30);
    let after = t2() + Duration::hours(1);
    insert_event_at(pool, team, route, provider, (1, 2, 3), before).await;
    insert_event_at(pool, team, route, provider, (10, 20, 30), t1()).await;
    insert_event_at(pool, team, route, provider, (100, 200, 300), inside).await;
    insert_event_at(pool, team, route, provider, (1000, 2000, 3000), t2()).await;
    insert_event_at(pool, team, route, provider, (7, 7, 14), after).await;
    (provider, route)
}

#[tokio::test]
async fn windowed_read_is_half_open_inclusive_since_exclusive_until() {
    let Some(w) = world().await else { return };
    let (provider, route) = seed_boundary_events(&w.pool, w.team_a).await;

    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            since: Some(t1()),
            until: Some(t2()),
            ..all_time()
        },
    )
    .await
    .expect("windowed usage summary");

    assert_eq!(total, 1, "one grouped (route, provider) pair in window");
    assert_eq!(items.len(), 1);
    let row = find_pair(&items, route, provider).expect("summary row for the seeded pair");
    // Included: the event AT since (>=) and the event inside. Excluded: the
    // event before since, the event AT until (<), and the event after until.
    assert_eq!(
        row.event_count, 2,
        "created_at >= since AND created_at < until"
    );
    assert_eq!(row.prompt_tokens, 110);
    assert_eq!(row.completion_tokens, 220);
    assert_eq!(row.total_tokens, 330);
}

#[tokio::test]
async fn omitted_bound_is_unbounded_on_that_side() {
    let Some(w) = world().await else { return };
    let (provider, route) = seed_boundary_events(&w.pool, w.team_a).await;

    // since only: everything from T1 onward (at-T1, inside, at-T2, after).
    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            since: Some(t1()),
            ..all_time()
        },
    )
    .await
    .expect("since-only summary");
    assert_eq!(total, 1);
    let row = find_pair(&items, route, provider).expect("row");
    assert_eq!(row.event_count, 4, "since-only window is unbounded above");
    assert_eq!(row.prompt_tokens, 10 + 100 + 1000 + 7);

    // until only: everything strictly before T2 (before, at-T1, inside).
    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            until: Some(t2()),
            ..all_time()
        },
    )
    .await
    .expect("until-only summary");
    assert_eq!(total, 1);
    let row = find_pair(&items, route, provider).expect("row");
    assert_eq!(row.event_count, 3, "until-only window is unbounded below");
    assert_eq!(row.prompt_tokens, 1 + 10 + 100);
}

#[tokio::test]
async fn omitted_window_reads_all_time() {
    let Some(w) = world().await else { return };
    let (provider, route) = seed_boundary_events(&w.pool, w.team_a).await;
    // One extra event stamped now() via the public recording API.
    record_now(&w.pool, w.team_a, route, provider, (5, 5, 10)).await;

    let (items, total) = ai::usage_summary(&w.pool, w.team_a.id, all_time())
        .await
        .expect("all-time summary");

    assert_eq!(total, 1);
    let row = find_pair(&items, route, provider).expect("row");
    assert_eq!(row.event_count, 6, "no window means every event counts");
    assert_eq!(row.prompt_tokens, 1 + 10 + 100 + 1000 + 7 + 5);
    assert_eq!(row.completion_tokens, 2 + 20 + 200 + 2000 + 7 + 5);
    assert_eq!(row.total_tokens, 3 + 30 + 300 + 3000 + 14 + 10);
}

#[tokio::test]
async fn total_is_grouped_pair_count_unaffected_by_limit_and_offset() {
    let Some(w) = world().await else { return };
    let mut pairs = Vec::new();
    for _ in 0..3 {
        let (provider, route) = insert_provider_and_route(&w.pool, w.team_a).await;
        record_now(&w.pool, w.team_a, route, provider, (1, 1, 2)).await;
        // Two events per pair so a raw-event count (6) can't masquerade as the
        // grouped-pair total (3).
        record_now(&w.pool, w.team_a, route, provider, (1, 1, 2)).await;
        pairs.push((provider, route));
    }

    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            limit: 1,
            offset: 0,
            ..all_time()
        },
    )
    .await
    .expect("page 1");
    assert_eq!(items.len(), 1, "limit caps the page");
    assert_eq!(
        total, 3,
        "total is the grouped pair count, not the page size"
    );
    assert_eq!(items[0].event_count, 2);

    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            limit: 100,
            offset: 2,
            ..all_time()
        },
    )
    .await
    .expect("offset page");
    assert_eq!(items.len(), 1, "offset skips grouped rows");
    assert_eq!(total, 3, "offset must not change total");

    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            limit: 1,
            offset: 5,
            ..all_time()
        },
    )
    .await
    .expect("past-the-end page");
    assert!(items.is_empty(), "offset past the end yields an empty page");
    assert_eq!(total, 3, "total is independent of pagination entirely");
}

#[tokio::test]
async fn window_composes_with_provider_and_route_filters() {
    let Some(w) = world().await else { return };
    let (provider_1, route_1) = insert_provider_and_route(&w.pool, w.team_a).await;
    let (provider_2, route_2) = insert_provider_and_route(&w.pool, w.team_a).await;
    let inside = t1() + Duration::minutes(5);
    let outside = t2() + Duration::minutes(5);
    insert_event_at(&w.pool, w.team_a, route_1, provider_1, (11, 13, 24), inside).await;
    insert_event_at(
        &w.pool,
        w.team_a,
        route_1,
        provider_1,
        (500, 500, 1000),
        outside,
    )
    .await;
    insert_event_at(&w.pool, w.team_a, route_2, provider_2, (17, 19, 36), inside).await;

    // Window + provider filter: only provider_1's in-window event.
    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            provider_id: Some(provider_1),
            since: Some(t1()),
            until: Some(t2()),
            ..all_time()
        },
    )
    .await
    .expect("provider-filtered windowed summary");
    assert_eq!(total, 1, "only the filtered provider's pair is counted");
    assert_eq!(items.len(), 1);
    let row = find_pair(&items, route_1, provider_1).expect("provider_1 row");
    assert_eq!(
        row.event_count, 1,
        "out-of-window event for provider_1 excluded"
    );
    assert_eq!(row.prompt_tokens, 11);
    assert_eq!(row.completion_tokens, 13);
    assert_eq!(row.total_tokens, 24);

    // Window + route filter: only route_2's in-window event.
    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            route_config_id: Some(route_2),
            since: Some(t1()),
            until: Some(t2()),
            ..all_time()
        },
    )
    .await
    .expect("route-filtered windowed summary");
    assert_eq!(total, 1);
    assert_eq!(items.len(), 1);
    let row = find_pair(&items, route_2, provider_2).expect("route_2 row");
    assert_eq!(row.event_count, 1);
    assert_eq!(row.total_tokens, 36);
}

#[tokio::test]
async fn cross_team_rows_never_appear_in_items_or_total() {
    let Some(w) = world().await else { return };
    let (provider_a, route_a) = insert_provider_and_route(&w.pool, w.team_a).await;
    let (provider_b, route_b) = insert_provider_and_route(&w.pool, w.team_b).await;
    let inside = t1() + Duration::minutes(10);

    insert_event_at(&w.pool, w.team_a, route_a, provider_a, (3, 4, 7), inside).await;
    // Team B events in the SAME window — one on B's own pair, and one
    // adversarially reusing team A's route/provider ids: a group-by that
    // ignored team_id would silently fold it into A's sums.
    insert_event_at(
        &w.pool,
        w.team_b,
        route_b,
        provider_b,
        (1000, 1000, 2000),
        inside,
    )
    .await;
    insert_event_at(
        &w.pool,
        w.team_b,
        route_a,
        provider_a,
        (9000, 9000, 18000),
        inside,
    )
    .await;

    // Windowed read for team A.
    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            since: Some(t1()),
            until: Some(t2()),
            ..all_time()
        },
    )
    .await
    .expect("team A windowed summary");
    assert_eq!(total, 1, "team B pairs must not be counted for team A");
    assert_eq!(items.len(), 1);
    let row = find_pair(&items, route_a, provider_a).expect("team A row");
    assert_eq!(
        row.event_count, 1,
        "team B's event on A's pair must not count"
    );
    assert_eq!(row.prompt_tokens, 3);
    assert_eq!(row.completion_tokens, 4);
    assert_eq!(row.total_tokens, 7);
    assert!(
        find_pair(&items, route_b, provider_b).is_none(),
        "team B's pair must never appear in team A's items"
    );

    // Unwindowed read for team A: isolation must hold without a window too.
    let (items, total) = ai::usage_summary(&w.pool, w.team_a.id, all_time())
        .await
        .expect("team A all-time summary");
    assert_eq!(total, 1);
    let row = find_pair(&items, route_a, provider_a).expect("team A row (all-time)");
    assert_eq!(row.event_count, 1);
    assert_eq!(row.total_tokens, 7);
    assert!(find_pair(&items, route_b, provider_b).is_none());

    // Team B sees only its own rows (including the one it wrote against A's
    // route/provider ids — it belongs to team B).
    let (items, total) = ai::usage_summary(&w.pool, w.team_b.id, all_time())
        .await
        .expect("team B all-time summary");
    assert_eq!(
        total, 2,
        "team B owns two pairs: its own and the reused ids"
    );
    let own = find_pair(&items, route_b, provider_b).expect("team B's own pair");
    assert_eq!(own.event_count, 1);
    assert_eq!(own.total_tokens, 2000);
    let reused = find_pair(&items, route_a, provider_a).expect("team B's reused-id pair");
    assert_eq!(reused.event_count, 1);
    assert_eq!(reused.total_tokens, 18000);
}

/// Bulk-seeds `count` events for `team` spread one second apart going back
/// from `now()`, so a narrow recent window is selective.
async fn bulk_seed_events(
    pool: &PgPool,
    team: TeamRef,
    route_config_id: RouteConfigId,
    provider_id: AiProviderId,
    count: i64,
) {
    sqlx::query(
        "INSERT INTO ai_usage_events \
         (id, team_id, route_config_id, provider_id, prompt_tokens, completion_tokens, total_tokens, created_at) \
         SELECT gen_random_uuid(), $1, $2, $3, 1, 1, 2, now() - (g * interval '1 second') \
         FROM generate_series(1, $4) AS g",
    )
    .bind(team.id.as_uuid())
    .bind(route_config_id.as_uuid())
    .bind(provider_id.as_uuid())
    .bind(count)
    .execute(pool)
    .await
    .expect("bulk seed usage events");
}

async fn explain_windowed_grouped_query(pool: &PgPool, team: TeamRef) -> String {
    // The spec's windowed grouped query shape over ai_usage_events, with
    // literal values so the planner sees real selectivity. The window covers
    // the most recent 60 of the seeded events.
    let sql = format!(
        "EXPLAIN (FORMAT TEXT) \
         SELECT route_config_id, provider_id, \
                sum(prompt_tokens), sum(completion_tokens), sum(total_tokens), count(*) \
         FROM ai_usage_events \
         WHERE team_id = '{team_id}' \
           AND created_at >= now() - interval '60 seconds' \
           AND created_at < now() \
         GROUP BY route_config_id, provider_id",
        team_id = team.id.as_uuid()
    );
    let lines: Vec<String> = sqlx::query_scalar(&sql)
        .fetch_all(pool)
        .await
        .expect("explain windowed grouped query");
    lines.join("\n")
}

fn plan_is_index_backed(plan: &str) -> bool {
    plan.contains("idx_ai_usage_events_team_created")
        || plan.contains("idx_ai_usage_events_route_config")
        || !plan.contains("Seq Scan on ai_usage_events")
}

#[tokio::test]
async fn windowed_grouped_query_uses_team_scoped_index() {
    let Some(w) = world().await else { return };
    let (provider_a, route_a) = insert_provider_and_route(&w.pool, w.team_a).await;
    let (provider_b, route_b) = insert_provider_and_route(&w.pool, w.team_b).await;

    bulk_seed_events(&w.pool, w.team_a, route_a, provider_a, 2_000).await;
    bulk_seed_events(&w.pool, w.team_b, route_b, provider_b, 200).await;
    sqlx::query("ANALYZE ai_usage_events")
        .execute(&w.pool)
        .await
        .expect("analyze");

    let mut plan = explain_windowed_grouped_query(&w.pool, w.team_a).await;
    if !plan_is_index_backed(&plan) {
        // Per spec: if the planner still seq-scans at 2000 rows, grow the
        // fixture to 10000 rows for this team and re-check.
        bulk_seed_events(&w.pool, w.team_a, route_a, provider_a, 8_000).await;
        sqlx::query("ANALYZE ai_usage_events")
            .execute(&w.pool)
            .await
            .expect("re-analyze");
        plan = explain_windowed_grouped_query(&w.pool, w.team_a).await;
    }

    assert!(
        plan_is_index_backed(&plan),
        "windowed grouped usage query must be served by a team_id index on \
         ai_usage_events (idx_ai_usage_events_team_created or \
         idx_ai_usage_events_route_config), not a sequential scan.\nplan:\n{plan}"
    );

    // Sanity: the query itself still answers correctly at this scale — the
    // window covers the 59 most recent seeded events (offsets 1..=59 seconds;
    // the `now()` upper bound is re-evaluated slightly after seeding, and the
    // half-open bound excludes nothing seeded at exactly now()).
    let (items, total) = ai::usage_summary(
        &w.pool,
        w.team_a.id,
        AiUsageQuery {
            since: Some(Utc::now() - Duration::seconds(60)),
            ..all_time()
        },
    )
    .await
    .expect("windowed summary over bulk fixture");
    assert_eq!(total, 1);
    let row = find_pair(&items, route_a, provider_a).expect("bulk pair row");
    assert!(
        row.event_count >= 50 && row.event_count <= 61,
        "expected roughly 59 events in the last 60s, got {}",
        row.event_count
    );
}
