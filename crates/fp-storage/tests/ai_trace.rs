//! AI trace-event repository integration tests (feature ai-gateway-e2e-trace, slice s2).
//!
//! Parallel-safe: every test creates its own uniquely named org/team and keys rows by
//! fresh UUID request ids; nothing assumes global row counts.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::authz::TeamRef;
use fp_domain::{ListenerId, OrgId, RouteConfigId, TeamId};
use fp_storage::repos::{ai_trace, identity};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

struct World {
    pool: PgPool,
    org: OrgId,
    team_a: TeamId,
    team_b: TeamId,
}

impl World {
    fn team_ref(&self, team_id: TeamId) -> TeamRef {
        TeamRef {
            id: team_id,
            org_id: self.org,
        }
    }
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    let org = identity::create_org(&pool, &unique("org-trace"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&pool, org.id, &unique("team-trace-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org.id, &unique("team-trace-b"), "")
        .await
        .expect("team b");
    Some(World {
        pool,
        org: org.id,
        team_a: team_a.id,
        team_b: team_b.id,
    })
}

fn listener_event(
    team_id: TeamId,
    request_id: &str,
    route_config_id: RouteConfigId,
    listener_id: ListenerId,
) -> ai_trace::AiTraceEventUpsert {
    ai_trace::AiTraceEventUpsert {
        team_id,
        request_id: request_id.to_string(),
        trace_id: Some("0af7651916cd43dd8448eb211c80319c".into()),
        route_config_id,
        listener_id: Some(listener_id),
        provider_id: None,
        model: Some("gpt-5".into()),
        status_code: Some(200),
        hops: json!([
            {"hop": "route_match", "started_at": "2026-07-04T00:00:00.100000Z", "ended_at": "2026-07-04T00:00:00.150000Z", "outcome": "matched", "origin": "listener", "failed": false, "detail": {"model": "gpt-5"}},
            {"hop": "auth", "started_at": "2026-07-04T00:00:00.150000Z", "ended_at": "2026-07-04T00:00:00.150000Z", "outcome": "not_configured", "origin": "listener", "failed": false, "detail": {}},
            {"hop": "budget", "started_at": "2026-07-04T00:00:00.160000Z", "ended_at": "2026-07-04T00:00:00.170000Z", "outcome": "allowed", "origin": "listener", "failed": false, "detail": {"mode": "enforcing"}},
        ]),
    }
}

fn upstream_event(
    team_id: TeamId,
    request_id: &str,
    route_config_id: RouteConfigId,
) -> ai_trace::AiTraceEventUpsert {
    ai_trace::AiTraceEventUpsert {
        team_id,
        request_id: request_id.to_string(),
        trace_id: None,
        route_config_id,
        listener_id: None,
        provider_id: Some(fp_domain::AiProviderId::from(Uuid::now_v7())),
        model: None,
        status_code: None,
        hops: json!([
            {"hop": "budget", "started_at": "2026-07-04T00:00:00.300000Z", "ended_at": "2026-07-04T00:00:00.310000Z", "outcome": "allowed", "origin": "upstream", "failed": false, "detail": {"mode": "enforcing"}},
            {"hop": "credential_injection", "started_at": "2026-07-04T00:00:00.310000Z", "ended_at": "2026-07-04T00:00:00.320000Z", "outcome": "injected", "origin": "upstream", "failed": false, "detail": {"auth_header": "authorization"}},
            {"hop": "upstream", "started_at": "2026-07-04T00:00:00.320000Z", "ended_at": "2026-07-04T00:00:00.900000Z", "outcome": "ok", "origin": "upstream", "failed": false, "detail": {"status": 200}},
            {"hop": "usage", "started_at": "2026-07-04T00:00:00.900000Z", "ended_at": "2026-07-04T00:00:00.900000Z", "outcome": "settled", "origin": "upstream", "failed": false, "detail": {"total_tokens": 5}},
        ]),
    }
}

fn hop_names(hops: &Value) -> Vec<String> {
    hops.as_array()
        .unwrap()
        .iter()
        .map(|hop| hop["hop"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn upsert_merges_listener_and_upstream_contributions_order_independently() {
    let Some(w) = world().await else { return };
    let route_config_id = RouteConfigId::from(Uuid::now_v7());
    let listener_id = ListenerId::from(Uuid::now_v7());

    let provider_id = fp_domain::AiProviderId::from(Uuid::now_v7());

    // Request 1: listener stream writes first.
    let req_1 = Uuid::now_v7().to_string();
    let listener_1 = listener_event(w.team_a, &req_1, route_config_id, listener_id);
    let mut upstream_1 = upstream_event(w.team_a, &req_1, route_config_id);
    upstream_1.provider_id = Some(provider_id);
    ai_trace::upsert_trace_event(&w.pool, &listener_1)
        .await
        .expect("listener upsert");
    ai_trace::upsert_trace_event(&w.pool, &upstream_1)
        .await
        .expect("upstream upsert");

    // Request 2: identical contributions, upstream stream writes first.
    let req_2 = Uuid::now_v7().to_string();
    let listener_2 = listener_event(w.team_a, &req_2, route_config_id, listener_id);
    let mut upstream_2 = upstream_event(w.team_a, &req_2, route_config_id);
    upstream_2.provider_id = Some(provider_id);
    ai_trace::upsert_trace_event(&w.pool, &upstream_2)
        .await
        .expect("upstream-first upsert");
    ai_trace::upsert_trace_event(&w.pool, &listener_2)
        .await
        .expect("listener-second upsert");

    let rows = ai_trace::list_trace_events(
        &w.pool,
        w.team_a,
        ai_trace::AiTraceQuery {
            request_id: Some(&req_1),
            trace_id: None,
            limit: 10,
        },
    )
    .await
    .expect("list");
    assert_eq!(rows.len(), 1, "both streams merged into one row");
    let first = &rows[0];
    let rows = ai_trace::list_trace_events(
        &w.pool,
        w.team_a,
        ai_trace::AiTraceQuery {
            request_id: Some(&req_2),
            trace_id: None,
            limit: 10,
        },
    )
    .await
    .expect("list second");
    assert_eq!(rows.len(), 1);
    let second = &rows[0];

    // Semantic content converges regardless of which stream wrote first.
    assert_eq!(first.hops, second.hops);
    assert_eq!(first.trace_id, second.trace_id);
    assert_eq!(first.listener_id, second.listener_id);
    assert_eq!(first.provider_id, second.provider_id);
    assert_eq!(first.model, second.model);
    assert_eq!(first.status_code, second.status_code);
    assert_eq!(first.failure_hop, None);
    assert_eq!(
        hop_names(&first.hops),
        vec![
            "route_match",
            "auth",
            "budget",
            "credential_injection",
            "upstream",
            "usage"
        ],
        "duplicate budget hop resolved to one entry, all hops present"
    );
    // The conflicting budget hop resolved to the upstream-origin entry in both orders.
    let budget = first
        .hops
        .as_array()
        .unwrap()
        .iter()
        .find(|hop| hop["hop"] == "budget")
        .unwrap();
    assert_eq!(budget["origin"], "upstream");
    assert_eq!(first.status_code, Some(200));
    assert_eq!(first.model.as_deref(), Some("gpt-5"));
    assert!(first.trace_id.is_some());
}

#[tokio::test]
async fn list_is_scoped_by_construction_and_finds_nothing_for_other_team() {
    let Some(w) = world().await else { return };
    let route_config_id = RouteConfigId::from(Uuid::now_v7());
    let listener_id = ListenerId::from(Uuid::now_v7());
    let request_id = Uuid::now_v7().to_string();
    let event = listener_event(w.team_a, &request_id, route_config_id, listener_id);
    ai_trace::upsert_trace_event(&w.pool, &event)
        .await
        .expect("upsert");

    let mine = ai_trace::list_trace_events(
        &w.pool,
        w.team_a,
        ai_trace::AiTraceQuery {
            request_id: Some(&request_id),
            trace_id: None,
            limit: 10,
        },
    )
    .await
    .expect("own team list");
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].team_id, w.team_a);

    let theirs = ai_trace::list_trace_events(
        &w.pool,
        w.team_b,
        ai_trace::AiTraceQuery {
            request_id: Some(&request_id),
            trace_id: None,
            limit: 10,
        },
    )
    .await
    .expect("other team list");
    assert!(theirs.is_empty(), "trace reads are team-scoped");

    let by_trace_id = ai_trace::list_trace_events(
        &w.pool,
        w.team_a,
        ai_trace::AiTraceQuery {
            request_id: None,
            trace_id: mine[0].trace_id.as_deref(),
            limit: 10,
        },
    )
    .await
    .expect("trace id list");
    assert!(by_trace_id.iter().any(|row| row.request_id == request_id));
}

#[tokio::test]
async fn expires_at_defaults_to_thirty_days_after_created_at_without_policy_row() {
    let Some(w) = world().await else { return };
    let route_config_id = RouteConfigId::from(Uuid::now_v7());
    let listener_id = ListenerId::from(Uuid::now_v7());
    let request_id = Uuid::now_v7().to_string();
    let event = listener_event(w.team_a, &request_id, route_config_id, listener_id);
    ai_trace::upsert_trace_event(&w.pool, &event)
        .await
        .expect("upsert");

    let exact: bool = sqlx::query(
        "SELECT expires_at = created_at + make_interval(days => $3) AS exact \
         FROM ai_trace_events WHERE team_id = $1 AND request_id = $2",
    )
    .bind(w.team_a.as_uuid())
    .bind(&request_id)
    .bind(ai_trace::DEFAULT_AI_TRACE_TTL_DAYS)
    .fetch_one(&w.pool)
    .await
    .expect("expiry row")
    .get("exact");
    assert!(
        exact,
        "with no ai_retention_policies row expires_at is exactly created_at + 30 days"
    );
}

// Slice s5: one policy row per team, PUT semantics — absent → None, set creates at version 1,
// set again replaces in place (same row id) and bumps the version.
#[tokio::test]
async fn retention_policy_is_a_single_versioned_row_per_team() {
    let Some(w) = world().await else { return };

    assert!(
        ai_trace::get_retention_policy(&w.pool, w.team_a)
            .await
            .expect("get absent")
            .is_none(),
        "no policy row until one is set"
    );

    let mut tx = w.pool.begin().await.expect("begin");
    let created = ai_trace::set_retention_policy(&mut tx, w.team_ref(w.team_a), 7, None)
        .await
        .expect("set");
    tx.commit().await.expect("commit");
    assert_eq!(created.trace_ttl_days, 7);
    assert_eq!(created.version, 1);
    assert_eq!(created.team_id, w.team_a);

    let mut tx = w.pool.begin().await.expect("begin");
    let replaced = ai_trace::set_retention_policy(&mut tx, w.team_ref(w.team_a), 14, None)
        .await
        .expect("replace");
    tx.commit().await.expect("commit replace");
    assert_eq!(replaced.trace_ttl_days, 14);
    assert_eq!(replaced.version, 2, "replace bumps the revision");
    assert_eq!(
        replaced.id, created.id,
        "one row per team, replaced in place"
    );

    let fetched = ai_trace::get_retention_policy(&w.pool, w.team_a)
        .await
        .expect("get stored")
        .expect("policy row");
    assert_eq!(fetched.trace_ttl_days, 14);
    assert_eq!(fetched.version, 2);

    // Team B is untouched by team A's policy.
    assert!(ai_trace::get_retention_policy(&w.pool, w.team_b)
        .await
        .expect("get team b")
        .is_none());
}

#[tokio::test]
async fn retention_policy_conditional_replace_enforces_expected_revision() {
    let Some(w) = world().await else { return };

    let mut tx = w.pool.begin().await.expect("begin");
    let created = ai_trace::set_retention_policy(&mut tx, w.team_ref(w.team_a), 7, None)
        .await
        .expect("create");
    tx.commit().await.expect("commit create");

    let mut tx = w.pool.begin().await.expect("begin match");
    let replaced =
        ai_trace::set_retention_policy(&mut tx, w.team_ref(w.team_a), 14, Some(created.version))
            .await
            .expect("matching revision replaces");
    tx.commit().await.expect("commit match");
    assert_eq!(replaced.id, created.id);
    assert_eq!(replaced.trace_ttl_days, 14);
    assert_eq!(replaced.version, created.version + 1);

    let mut tx = w.pool.begin().await.expect("begin stale");
    let err =
        ai_trace::set_retention_policy(&mut tx, w.team_ref(w.team_a), 21, Some(created.version))
            .await
            .expect_err("stale revision fails");
    tx.commit().await.expect("commit stale tx");
    assert_eq!(err.code, fp_domain::ErrorCode::RevisionMismatch);

    let fetched = ai_trace::get_retention_policy(&w.pool, w.team_a)
        .await
        .expect("get after stale")
        .expect("policy remains");
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.trace_ttl_days, 14);
    assert_eq!(fetched.version, replaced.version);
}

#[tokio::test]
async fn retention_policy_conditional_replace_missing_row_does_not_create() {
    let Some(w) = world().await else { return };

    let mut tx = w.pool.begin().await.expect("begin");
    let err = ai_trace::set_retention_policy(&mut tx, w.team_ref(w.team_a), 7, Some(1))
        .await
        .expect_err("missing row with precondition fails");
    tx.commit().await.expect("commit");
    assert_eq!(err.code, fp_domain::ErrorCode::RevisionMismatch);

    assert!(
        ai_trace::get_retention_policy(&w.pool, w.team_a)
            .await
            .expect("get absent")
            .is_none(),
        "conditional replace must not create a policy row"
    );
}

// Slice s5 (design AC 13): with a team policy of N days, a NEW trace row's expires_at equals
// created_at + N days; the no-policy 30-day default is covered by
// `expires_at_defaults_to_thirty_days_after_created_at_without_policy_row` above.
#[tokio::test]
async fn insert_time_ttl_honors_a_preset_policy() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("begin");
    ai_trace::set_retention_policy(&mut tx, w.team_ref(w.team_a), 7, None)
        .await
        .expect("set policy");
    tx.commit().await.expect("commit");

    let request_id = Uuid::now_v7().to_string();
    let event = listener_event(
        w.team_a,
        &request_id,
        RouteConfigId::from(Uuid::now_v7()),
        ListenerId::from(Uuid::now_v7()),
    );
    ai_trace::upsert_trace_event(&w.pool, &event)
        .await
        .expect("upsert");

    let exact: bool = sqlx::query(
        "SELECT expires_at = created_at + make_interval(days => 7) AS exact \
         FROM ai_trace_events WHERE team_id = $1 AND request_id = $2",
    )
    .bind(w.team_a.as_uuid())
    .bind(&request_id)
    .fetch_one(&w.pool)
    .await
    .expect("expiry row")
    .get("exact");
    assert!(
        exact,
        "with a 7-day policy expires_at is exactly created_at + 7 days"
    );
}

// Slice s5 (design AC 13): the sweep removes only rows with expires_at <= as_of, and only
// the owning team's data — an unexpired sibling row and another team's rows survive.
#[tokio::test]
async fn delete_expired_trace_events_scopes_to_expired_rows_only() {
    let Some(w) = world().await else { return };
    let route_config_id = RouteConfigId::from(Uuid::now_v7());
    let listener_id = ListenerId::from(Uuid::now_v7());

    // Team A: one row that will be expired, one that stays fresh. Team B: one fresh row.
    let expired_id = Uuid::now_v7().to_string();
    let fresh_a_id = Uuid::now_v7().to_string();
    let fresh_b_id = Uuid::now_v7().to_string();
    for (team, request_id) in [
        (w.team_a, &expired_id),
        (w.team_a, &fresh_a_id),
        (w.team_b, &fresh_b_id),
    ] {
        let event = listener_event(team, request_id, route_config_id, listener_id);
        ai_trace::upsert_trace_event(&w.pool, &event)
            .await
            .expect("upsert");
    }
    // Age one team-A row past its expiry (rows are otherwise 30-day fresh).
    sqlx::query(
        "UPDATE ai_trace_events SET expires_at = now() - interval '1 minute' \
         WHERE team_id = $1 AND request_id = $2",
    )
    .bind(w.team_a.as_uuid())
    .bind(&expired_id)
    .execute(&w.pool)
    .await
    .expect("age row");

    // The sweep deletes at least the aged row (the shared DB may hold other tests' expired
    // rows, so assert on this world's rows, not the global count).
    let deleted = ai_trace::delete_expired_trace_events(&w.pool, sqlx::types::chrono::Utc::now())
        .await
        .expect("sweep");
    assert!(
        deleted >= 1,
        "the aged row must be swept, deleted={deleted}"
    );

    let survivors = |team: TeamId, request_id: &str| {
        let pool = w.pool.clone();
        let request_id = request_id.to_string();
        async move {
            ai_trace::list_trace_events(
                &pool,
                team,
                ai_trace::AiTraceQuery {
                    request_id: Some(&request_id),
                    trace_id: None,
                    limit: 10,
                },
            )
            .await
            .expect("list")
            .len()
        }
    };
    assert_eq!(
        survivors(w.team_a, &expired_id).await,
        0,
        "expired row is gone"
    );
    assert_eq!(
        survivors(w.team_a, &fresh_a_id).await,
        1,
        "same team's unexpired row survives"
    );
    assert_eq!(
        survivors(w.team_b, &fresh_b_id).await,
        1,
        "other team's row survives"
    );
}

#[tokio::test]
async fn migration_enforces_unique_request_per_team_and_required_indexes() {
    let Some(w) = world().await else { return };
    // The unique upsert key and the two query indexes exist as designed.
    let indexes: Vec<String> = sqlx::query(
        "SELECT indexname FROM pg_indexes WHERE tablename = 'ai_trace_events' ORDER BY indexname",
    )
    .fetch_all(&w.pool)
    .await
    .expect("indexes")
    .into_iter()
    .map(|row| row.get::<String, _>("indexname"))
    .collect();
    for expected in [
        "uq_ai_trace_events_team_request",
        "idx_ai_trace_events_team_created",
        "idx_ai_trace_events_expires",
    ] {
        assert!(
            indexes.iter().any(|name| name == expected),
            "missing index {expected}, found {indexes:?}"
        );
    }
    // ai_retention_policies exists with a unique team_id.
    let unique_team: bool = sqlx::query_scalar(
        "SELECT EXISTS (\
           SELECT 1 FROM pg_indexes WHERE tablename = 'ai_retention_policies' \
           AND indexdef LIKE '%UNIQUE%' AND indexdef LIKE '%team_id%')",
    )
    .fetch_one(&w.pool)
    .await
    .expect("retention index");
    assert!(unique_team, "ai_retention_policies.team_id must be unique");
}
