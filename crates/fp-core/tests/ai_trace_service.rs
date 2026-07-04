//! Slice s4 integration: team-scoped AI trace retrieval through the fp-core service.
//! Authorization (`check_resource_access` on `(Resource::AiUsage, Action::Read)`) runs
//! before any repo read; the repo query is scoped by team id so a foreign request_id can
//! never match. Unique org/team/user names per run keep this parallel-safe against
//! sibling tests sharing the database (invariant 18).

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::{Action, Resource};
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::{ai_trace, identity};
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

/// Seed one trace row for `team_id` and return its request_id.
async fn seed_trace_row(pool: &PgPool, team_id: fp_domain::TeamId) -> String {
    let request_id = uuid::Uuid::now_v7().to_string();
    ai_trace::upsert_trace_event(
        pool,
        &ai_trace::AiTraceEventUpsert {
            team_id,
            request_id: request_id.clone(),
            trace_id: None,
            route_config_id: fp_domain::RouteConfigId::from(uuid::Uuid::now_v7()),
            listener_id: None,
            provider_id: None,
            model: Some("gpt-5".into()),
            status_code: Some(200),
            hops: serde_json::json!([
                {"hop": "route_match", "started_at": "2026-07-04T00:00:00Z",
                 "ended_at": "2026-07-04T00:00:00Z", "outcome": "matched",
                 "origin": "listener", "failed": false, "detail": {}}
            ]),
        },
    )
    .await
    .expect("seed trace row");
    request_id
}

#[tokio::test]
async fn trace_read_enforces_ai_usage_read_before_any_repo_read() {
    let Some(pool) = test_pool().await else {
        return;
    };

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let team_ref = identity::resolve_team_ref(&pool, team.id)
        .await
        .expect("q")
        .expect("team ref");
    let request_id = seed_trace_row(&pool, team.id).await;

    // A member holding (AiUsage, Read) on the team reads the trace.
    let reader_sub = unique("sub-reader");
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
    .expect("grant");
    let reader_ctx = principal_ctx(&pool, &reader_sub).await;
    let traces = fp_core::services::ai::trace_events(
        &pool,
        &reader_ctx,
        team_ref,
        ai_trace::AiTraceQuery {
            request_id: Some(&request_id),
            trace_id: None,
            limit: 50,
        },
        RequestId::generate(),
    )
    .await
    .expect("authorized trace read");
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].request_id, request_id);
    assert_eq!(traces[0].hops[0]["hop"], "route_match");

    // A member with OTHER grants on the same team — but not (AiUsage, Read) — gets 403.
    let other_sub = unique("sub-other");
    let other = identity::upsert_user_by_subject(&pool, &other_sub, "o@a.test", "Other")
        .await
        .expect("other");
    identity::add_org_membership(&pool, other, org.id, OrgRole::Member)
        .await
        .expect("member");
    identity::add_grant(
        &pool,
        other,
        org.id,
        team.id,
        Resource::AiProviders,
        Action::Read,
        None,
    )
    .await
    .expect("unrelated grant");
    let other_ctx = principal_ctx(&pool, &other_sub).await;
    let err = fp_core::services::ai::trace_events(
        &pool,
        &other_ctx,
        team_ref,
        ai_trace::AiTraceQuery {
            request_id: Some(&request_id),
            trace_id: None,
            limit: 50,
        },
        RequestId::generate(),
    )
    .await
    .expect_err("missing (ai-usage, read) grant must deny");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        err.message.contains("ai-usage:read"),
        "denial names the missing grant, got: {}",
        err.message
    );
}

#[tokio::test]
async fn cross_team_trace_reads_are_isolated_by_authz_and_by_scoping() {
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
    let team_a2 = identity::create_team(&pool, org_a.id, &unique("team-a2"), "")
        .await
        .expect("team a2");
    let ref_a = identity::resolve_team_ref(&pool, team_a.id)
        .await
        .expect("q")
        .expect("ref a");
    let ref_a2 = identity::resolve_team_ref(&pool, team_a2.id)
        .await
        .expect("q")
        .expect("ref a2");
    let request_id_a = seed_trace_row(&pool, team_a.id).await;

    // Org-boundary mapping: an org-B admin querying team A's trace path gets not_found
    // (anti-enumeration), never a row.
    let mallory_sub = unique("sub-mallory");
    let mallory = identity::upsert_user_by_subject(&pool, &mallory_sub, "m@b.test", "Mallory")
        .await
        .expect("mallory");
    identity::add_org_membership(&pool, mallory, org_b.id, OrgRole::Admin)
        .await
        .expect("member");
    let mallory_ctx = principal_ctx(&pool, &mallory_sub).await;
    let err = fp_core::services::ai::trace_events(
        &pool,
        &mallory_ctx,
        ref_a,
        ai_trace::AiTraceQuery {
            request_id: Some(&request_id_a),
            trace_id: None,
            limit: 50,
        },
        RequestId::generate(),
    )
    .await
    .expect_err("cross-org trace read must deny");
    assert_eq!(err.code, ErrorCode::NotFound);

    // Scoped-by-construction: a reader authorized on team A2 querying team A's request_id
    // through their OWN team scope gets zero rows — the repo query cannot match foreign rows.
    let a2_sub = unique("sub-a2");
    let a2_user = identity::upsert_user_by_subject(&pool, &a2_sub, "a2@a.test", "A2")
        .await
        .expect("a2 user");
    identity::add_org_membership(&pool, a2_user, org_a.id, OrgRole::Member)
        .await
        .expect("member");
    identity::add_grant(
        &pool,
        a2_user,
        org_a.id,
        team_a2.id,
        Resource::AiUsage,
        Action::Read,
        None,
    )
    .await
    .expect("grant");
    let a2_ctx = principal_ctx(&pool, &a2_sub).await;
    let traces = fp_core::services::ai::trace_events(
        &pool,
        &a2_ctx,
        ref_a2,
        ai_trace::AiTraceQuery {
            request_id: Some(&request_id_a),
            trace_id: None,
            limit: 50,
        },
        RequestId::generate(),
    )
    .await
    .expect("own-team read is authorized");
    assert!(
        traces.is_empty(),
        "team A's request_id must not be visible through team A2's scope"
    );

    // And the same a2 principal aimed directly at team A is denied (no grant there).
    let err = fp_core::services::ai::trace_events(
        &pool,
        &a2_ctx,
        ref_a,
        ai_trace::AiTraceQuery {
            request_id: Some(&request_id_a),
            trace_id: None,
            limit: 50,
        },
        RequestId::generate(),
    )
    .await
    .expect_err("no grant on team A");
    assert_eq!(err.code, ErrorCode::Forbidden);
}
