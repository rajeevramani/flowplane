#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

//! Spec-first tests for `fp_storage::repos::ai::budget_window_states` (slice fpv2-0t4.2).
//!
//! Contract under test: for each requested budget id belonging to the team, return the
//! budget's CURRENT server-aligned window state
//! (`window_start = to_timestamp(floor(extract(epoch FROM now()) / window_seconds) * window_seconds)`),
//! with `used_units` taken from an `ai_budget_counters` row at exactly that aligned
//! `window_start` (0 when absent or stale), and `limit_units` / `window_seconds` echoing
//! the budget spec. Budgets of other teams must never be returned nor contribute counters.

use chrono::{DateTime, Duration, Utc};
use fp_domain::authz::TeamRef;
use fp_domain::{AiBudgetMode, AiBudgetSpec, AiBudgetState, TeamId};
use fp_storage::repos::{ai, identity};
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

fn spec(limit_units: u64, window_seconds: u32) -> AiBudgetSpec {
    AiBudgetSpec {
        mode: AiBudgetMode::Enforcing,
        limit_units,
        window_seconds,
        provider_id: None,
        route_config_id: None,
        prompt_token_weight: 1,
        completion_token_weight: 2,
    }
}

/// Create a budget through the repo API (no provider/route FKs needed since the
/// spec allows both to be NULL).
async fn create_budget(pool: &PgPool, team: TeamRef, spec: &AiBudgetSpec) -> fp_domain::AiBudget {
    let mut tx = pool.begin().await.expect("begin");
    let budget = ai::create_budget(&mut tx, team, &unique("budget"), spec)
        .await
        .expect("create budget");
    tx.commit().await.expect("commit");
    budget
}

/// Compute the current server-aligned window start exactly the way the server does,
/// so expectations use the same clock and the same alignment arithmetic.
async fn aligned_window_start(pool: &PgPool, window_seconds: u32) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT to_timestamp(floor(extract(epoch FROM now()) / $1) * $1)")
        .bind(f64::from(window_seconds))
        .fetch_one(pool)
        .await
        .expect("aligned window start")
}

async fn seed_counter(
    pool: &PgPool,
    budget_id: Uuid,
    team_id: TeamId,
    window_start: DateTime<Utc>,
    used_units: i64,
) {
    sqlx::query(
        "INSERT INTO ai_budget_counters (budget_id, team_id, window_start, used_units) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(budget_id)
    .bind(team_id.as_uuid())
    .bind(window_start)
    .bind(used_units)
    .execute(pool)
    .await
    .expect("seed counter");
}

fn state_for(
    states: &[(fp_domain::AiBudgetId, AiBudgetState)],
    budget_id: fp_domain::AiBudgetId,
) -> Option<&AiBudgetState> {
    states
        .iter()
        .find(|(id, _)| *id == budget_id)
        .map(|(_, state)| state)
}

/// `window_start` assertion robust to a window boundary crossing between the
/// expected-value fetch and the call under test: on mismatch, recompute the aligned
/// start once and accept if the call's value matches the recomputed one.
async fn assert_window_start(
    pool: &PgPool,
    expected: DateTime<Utc>,
    actual: DateTime<Utc>,
    window_seconds: u32,
    context: &str,
) {
    if actual == expected {
        return;
    }
    let recomputed = aligned_window_start(pool, window_seconds).await;
    assert_eq!(
        actual, recomputed,
        "{context}: window_start {actual} matches neither the pre-call aligned value \
         {expected} nor the recomputed one {recomputed}"
    );
}

#[tokio::test]
async fn no_counter_row_yields_zero_used_units_at_aligned_window() {
    let Some(w) = world().await else { return };
    let window_seconds = 86_400_u32;
    let budget = create_budget(&w.pool, w.team_a, &spec(5_000, window_seconds)).await;

    let expected_start = aligned_window_start(&w.pool, window_seconds).await;
    let states = ai::budget_window_states(&w.pool, w.team_a.id, &[budget.id.as_uuid()])
        .await
        .expect("budget_window_states");

    assert_eq!(states.len(), 1, "exactly the one requested budget");
    let state = state_for(&states, budget.id).expect("state for budget");
    assert_eq!(state.used_units, 0, "no counter row means zero usage");
    assert_eq!(state.limit_units, 5_000, "limit echoes the spec");
    assert_eq!(
        state.window_seconds, window_seconds,
        "window echoes the spec"
    );
    assert_window_start(
        &w.pool,
        expected_start,
        state.window_start,
        window_seconds,
        "no-counter budget",
    )
    .await;
}

#[tokio::test]
async fn counter_at_current_aligned_window_surfaces_used_units() {
    let Some(w) = world().await else { return };
    let window_seconds = 86_400_u32;
    let budget = create_budget(&w.pool, w.team_a, &spec(10_000, window_seconds)).await;

    let start = aligned_window_start(&w.pool, window_seconds).await;
    seed_counter(&w.pool, budget.id.as_uuid(), w.team_a.id, start, 731).await;

    let states = ai::budget_window_states(&w.pool, w.team_a.id, &[budget.id.as_uuid()])
        .await
        .expect("budget_window_states");

    assert_eq!(states.len(), 1);
    let state = state_for(&states, budget.id).expect("state for budget");
    assert_eq!(
        state.used_units, 731,
        "counter at the current aligned window must contribute"
    );
    assert_eq!(state.limit_units, 10_000);
    assert_eq!(state.window_seconds, window_seconds);
    assert_window_start(
        &w.pool,
        start,
        state.window_start,
        window_seconds,
        "current-counter budget",
    )
    .await;
}

#[tokio::test]
async fn stale_counter_in_previous_window_does_not_contribute() {
    let Some(w) = world().await else { return };
    let window_seconds = 86_400_u32;
    let budget = create_budget(&w.pool, w.team_a, &spec(10_000, window_seconds)).await;

    let current_start = aligned_window_start(&w.pool, window_seconds).await;
    let stale_start = current_start - Duration::seconds(i64::from(window_seconds));
    seed_counter(&w.pool, budget.id.as_uuid(), w.team_a.id, stale_start, 999).await;

    let states = ai::budget_window_states(&w.pool, w.team_a.id, &[budget.id.as_uuid()])
        .await
        .expect("budget_window_states");

    assert_eq!(states.len(), 1);
    let state = state_for(&states, budget.id).expect("state for budget");
    assert_eq!(
        state.used_units, 0,
        "counter at an old window_start must NOT contribute"
    );
    assert_window_start(
        &w.pool,
        current_start,
        state.window_start,
        window_seconds,
        "stale-counter budget",
    )
    .await;
}

#[tokio::test]
async fn batch_returns_correct_state_per_budget_across_window_sizes() {
    let Some(w) = world().await else { return };
    // Different window sizes; large ones so a boundary crossing mid-test is
    // vanishingly unlikely.
    let ws_hour = 3_600_u32;
    let ws_day = 86_400_u32;
    let ws_week = 604_800_u32;

    let budget_hour = create_budget(&w.pool, w.team_a, &spec(100, ws_hour)).await;
    let budget_day = create_budget(&w.pool, w.team_a, &spec(200, ws_day)).await;
    let budget_week = create_budget(&w.pool, w.team_a, &spec(300, ws_week)).await;

    let start_hour = aligned_window_start(&w.pool, ws_hour).await;
    let start_day = aligned_window_start(&w.pool, ws_day).await;
    let start_week = aligned_window_start(&w.pool, ws_week).await;

    // Mixed counter presence: current counter, no counter, stale counter.
    seed_counter(
        &w.pool,
        budget_hour.id.as_uuid(),
        w.team_a.id,
        start_hour,
        42,
    )
    .await;
    // budget_day: no counter at all.
    let stale_week_start = start_week - Duration::seconds(i64::from(ws_week));
    seed_counter(
        &w.pool,
        budget_week.id.as_uuid(),
        w.team_a.id,
        stale_week_start,
        555,
    )
    .await;

    let states = ai::budget_window_states(
        &w.pool,
        w.team_a.id,
        &[
            budget_hour.id.as_uuid(),
            budget_day.id.as_uuid(),
            budget_week.id.as_uuid(),
        ],
    )
    .await
    .expect("budget_window_states");

    assert_eq!(states.len(), 3, "one call returns all three budgets");

    let hour = state_for(&states, budget_hour.id).expect("hour budget state");
    assert_eq!(hour.used_units, 42);
    assert_eq!(hour.limit_units, 100);
    assert_eq!(hour.window_seconds, ws_hour);
    assert_window_start(
        &w.pool,
        start_hour,
        hour.window_start,
        ws_hour,
        "hour budget",
    )
    .await;

    let day = state_for(&states, budget_day.id).expect("day budget state");
    assert_eq!(day.used_units, 0, "no counter row means zero usage");
    assert_eq!(day.limit_units, 200);
    assert_eq!(day.window_seconds, ws_day);
    assert_window_start(&w.pool, start_day, day.window_start, ws_day, "day budget").await;

    let week = state_for(&states, budget_week.id).expect("week budget state");
    assert_eq!(week.used_units, 0, "stale counter must not contribute");
    assert_eq!(week.limit_units, 300);
    assert_eq!(week.window_seconds, ws_week);
    assert_window_start(
        &w.pool,
        start_week,
        week.window_start,
        ws_week,
        "week budget",
    )
    .await;
}

#[tokio::test]
async fn cross_team_budgets_and_counters_never_leak() {
    let Some(w) = world().await else { return };
    let window_seconds = 86_400_u32;

    // Same window size on both teams so both budgets share the exact same aligned
    // window_start — the adversarial join case.
    let budget_a = create_budget(&w.pool, w.team_a, &spec(1_000, window_seconds)).await;
    let budget_b = create_budget(&w.pool, w.team_b, &spec(2_000, window_seconds)).await;

    let start = aligned_window_start(&w.pool, window_seconds).await;
    // Team B has real usage at the identical current aligned window_start, on ITS
    // budget. Team A has none.
    seed_counter(&w.pool, budget_b.id.as_uuid(), w.team_b.id, start, 888).await;

    // Team A explicitly requests team B's budget id alongside its own.
    let states = ai::budget_window_states(
        &w.pool,
        w.team_a.id,
        &[budget_a.id.as_uuid(), budget_b.id.as_uuid()],
    )
    .await
    .expect("budget_window_states");

    assert!(
        state_for(&states, budget_b.id).is_none(),
        "another team's budget must be absent even when explicitly requested"
    );
    assert_eq!(states.len(), 1, "only the caller's own budget is returned");

    let state_a = state_for(&states, budget_a.id).expect("team A budget state");
    assert_eq!(
        state_a.used_units, 0,
        "team B's counter at the same aligned window_start must never leak into team A"
    );
    assert_eq!(state_a.limit_units, 1_000);
    assert_eq!(state_a.window_seconds, window_seconds);
    assert_window_start(
        &w.pool,
        start,
        state_a.window_start,
        window_seconds,
        "team A budget",
    )
    .await;

    // Sanity from team B's own perspective: its usage is visible to itself.
    let states_b = ai::budget_window_states(&w.pool, w.team_b.id, &[budget_b.id.as_uuid()])
        .await
        .expect("budget_window_states for team B");
    let state_b = state_for(&states_b, budget_b.id).expect("team B budget state");
    assert_eq!(state_b.used_units, 888);
}
