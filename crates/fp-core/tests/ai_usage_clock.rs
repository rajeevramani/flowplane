//! Slice fpv2-ejw.1 acceptance: a freshly PostgreSQL-stamped AI usage event must be
//! visible through an immediate `fp_core::services::ai::usage_summary` read with an
//! omitted `until`.
//!
//! Honesty note (fixed-behavior property guard, not a deterministic regression): the
//! insert path stamps `created_at` with PostgreSQL `now()` while the pre-fix service
//! resolves the omitted `until` from the host clock (`chrono::Utc::now()`), and the
//! window is half-open (`created_at < until`). This test is therefore only RED before
//! the fix when the database clock runs ahead of the host clock (the live diagnostic
//! measured `db_minus_host_ms=253.949`); after the fix (PostgreSQL as the single clock
//! authority) it is deterministically green. The clock skew observed at run time is
//! printed as a diagnostic so the environment seam is visible in test output.
//!
//! Unique org/team/provider/route identities per run keep this parallel-safe against
//! sibling tests sharing the database. Authorization is real: the read goes through a
//! principal holding an (AiUsage, Read) grant on the team. All fixture bytes are inert;
//! no credentials are created or printed.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::{Action, Resource};
use fp_domain::{AiProviderId, OpenAiTokenUsage, OrgRole, RequestId, RouteConfigId};
use fp_storage::repos::{ai, identity};
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

/// Measured DB-vs-host clock offset in milliseconds, printed as the environment RED seam
/// diagnostic. Positive means the database clock is ahead of the host clock.
async fn db_minus_host_ms(pool: &PgPool) -> f64 {
    let db_now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(pool)
        .await
        .expect("db now");
    let host_now = chrono::Utc::now();
    (db_now - host_now).num_microseconds().unwrap_or(0) as f64 / 1000.0
}

#[tokio::test]
async fn fresh_db_stamped_usage_event_is_visible_through_omitted_until_summary_read() {
    let Some(pool) = test_pool().await else {
        return;
    };

    let org = identity::create_org(&pool, &unique("org-usage-clock"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team-usage-clock"), "")
        .await
        .expect("team");
    let team_ref = identity::resolve_team_ref(&pool, team.id)
        .await
        .expect("q")
        .expect("team ref");

    // Real principal holding (AiUsage, Read) on the team — the read must pass authz.
    let reader_sub = unique("sub-usage-clock-reader");
    let reader = identity::upsert_user_by_subject(&pool, &reader_sub, "r@a.test", "Reader")
        .await
        .expect("reader");
    identity::add_org_membership(&pool, reader, org.id, OrgRole::Member)
        .await
        .expect("member");
    identity::add_grant(
        &pool,
        reader,
        org.id,
        team.id,
        Resource::AiUsage,
        Action::Read,
        None,
    )
    .await
    .expect("grant ai usage read");
    let reader_ctx = principal_ctx(&pool, &reader_sub).await;

    // ai_usage_events carries no FK on route_config_id/provider_id, so fresh UUIDs give
    // unique per-run identities without materializing gateway resources.
    let route_config_id = RouteConfigId::from(uuid::Uuid::now_v7());
    let provider_id = AiProviderId::from(uuid::Uuid::now_v7());
    let usage = OpenAiTokenUsage {
        prompt_tokens: 1_234,
        completion_tokens: 5_678,
        total_tokens: 6_912,
    };

    eprintln!(
        "diagnostic: db_minus_host_ms={:.3} (positive = DB ahead of host = pre-fix RED seam)",
        db_minus_host_ms(&pool).await
    );

    // The insert path stamps created_at with PostgreSQL now().
    ai::record_usage_event(
        &pool,
        ai::AiUsageEventInsert {
            team_id: team.id,
            route_config_id,
            provider_id,
            backend_position: Some(0),
            usage,
        },
    )
    .await
    .expect("record usage event");

    // Immediately read through the service with an omitted `until`: the fresh event must
    // fall inside the resolved half-open window.
    let (items, total) = fp_core::services::ai::usage_summary(
        &pool,
        &reader_ctx,
        team_ref,
        ai::AiUsageQuery {
            route_config_id: Some(route_config_id),
            provider_id: Some(provider_id),
            since: None,
            until: None,
            limit: 50,
            offset: 0,
        },
        RequestId::generate(),
    )
    .await
    .expect("authorized usage summary read");

    assert_eq!(
        total, 1,
        "the just-stamped event must produce exactly one grouped row in the total"
    );
    assert_eq!(items.len(), 1, "expected exactly one grouped summary row");
    let row = &items[0];
    assert_eq!(row.route_config_id, Some(route_config_id));
    assert_eq!(row.provider_id, Some(provider_id));
    assert_eq!(row.prompt_tokens, 1_234);
    assert_eq!(row.completion_tokens, 5_678);
    assert_eq!(row.total_tokens, 6_912);
    assert_eq!(row.event_count, 1);
}
