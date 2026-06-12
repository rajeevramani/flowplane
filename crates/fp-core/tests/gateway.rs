//! S3 exit tests: the clusters vertical against real PostgreSQL — service-layer authz,
//! transactional events, optimistic concurrency under real contention, tenancy, quota.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::clusters as svc;
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::event::DomainEvent;
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::identity;
use sqlx::PgPool;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

fn spec(host: &str) -> ClusterSpec {
    ClusterSpec {
        endpoints: vec![Endpoint {
            host: host.into(),
            port: 8080,
            weight: None,
        }],
        lb_policy: LbPolicy::RoundRobin,
        connect_timeout_secs: 5,
        use_tls: false,
        health_check: None,
        circuit_breaker: None,
        outlier_detection: None,
    }
}

struct World {
    pool: PgPool,
    team: TeamRef,
    admin: PrincipalCtx,
    outsider: PrincipalCtx,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team_row = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let team = TeamRef {
        id: team_row.id,
        org_id: org.id,
    };

    let admin_sub = unique("sub");
    let admin_id = identity::upsert_user_by_subject(&pool, &admin_sub, "a@t.test", "A")
        .await
        .expect("u");
    identity::add_org_membership(&pool, admin_id, org.id, OrgRole::Admin)
        .await
        .expect("m");
    let admin = PrincipalCtx::User {
        user_id: admin_id,
        platform_admin: false,
        org: Some((org.id, OrgRole::Admin)),
        grants: GrantSet::default(),
    };

    // A user in a DIFFERENT org entirely.
    let other_org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org2");
    let outsider_id = identity::upsert_user_by_subject(&pool, &unique("sub"), "o@o.test", "O")
        .await
        .expect("u");
    identity::add_org_membership(&pool, outsider_id, other_org.id, OrgRole::Owner)
        .await
        .expect("m");
    let outsider = PrincipalCtx::User {
        user_id: outsider_id,
        platform_admin: false,
        org: Some((other_org.id, OrgRole::Owner)),
        grants: GrantSet::default(),
    };

    Some(World {
        pool,
        team,
        admin,
        outsider,
    })
}

#[tokio::test]
async fn create_emits_event_and_cross_org_caller_sees_not_found() {
    let Some(w) = world().await else { return };
    let name = unique("payments");
    let rid = RequestId::generate();

    svc::create_cluster(&w.pool, &w.admin, w.team, &name, spec("10.0.0.1"), rid)
        .await
        .expect("create");

    // The event committed with the row (transactional outbox).
    let (event_count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM events WHERE event_type = 'cluster.upserted' AND team_id = $1",
    )
    .bind(w.team.id.as_uuid())
    .fetch_one(&w.pool)
    .await
    .expect("events");
    assert!(event_count >= 1, "ClusterUpserted event in the outbox");

    // The audit row committed too.
    let (audit_count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM audit_log WHERE request_id = $1")
            .bind(rid.as_uuid())
            .fetch_one(&w.pool)
            .await
            .expect("audit");
    assert_eq!(audit_count, 1);

    // Cross-org caller: not_found, never forbidden (anti-enumeration).
    let err = svc::get_cluster(&w.pool, &w.outsider, w.team, &name)
        .await
        .expect_err("outsider must not see it");
    assert_eq!(err.code, ErrorCode::NotFound);

    // Outsider cannot mutate either — and the failure discloses nothing.
    let err = svc::delete_cluster(
        &w.pool,
        &w.outsider,
        w.team,
        &name,
        1,
        RequestId::generate(),
    )
    .await
    .expect_err("outsider must not delete");
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn concurrent_updates_one_wins_one_gets_revision_mismatch() {
    let Some(w) = world().await else { return };
    let name = unique("contended");
    svc::create_cluster(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        spec("a"),
        RequestId::generate(),
    )
    .await
    .expect("create");

    // Both writers read revision 1, then race their updates.
    let (r1, r2) = tokio::join!(
        svc::update_cluster(
            &w.pool,
            &w.admin,
            w.team,
            &name,
            spec("writer-one"),
            1,
            RequestId::generate()
        ),
        svc::update_cluster(
            &w.pool,
            &w.admin,
            w.team,
            &name,
            spec("writer-two"),
            1,
            RequestId::generate()
        ),
    );
    let outcomes = [r1, r2];
    let wins = outcomes.iter().filter(|r| r.is_ok()).count();
    let mismatches = outcomes
        .iter()
        .filter(|r| matches!(r, Err(e) if e.code == ErrorCode::RevisionMismatch))
        .count();
    assert_eq!(
        (wins, mismatches),
        (1, 1),
        "exactly one writer wins, the other gets 409"
    );

    // No lost update: the survivor's spec is what's stored, at revision 2.
    let stored = svc::get_cluster(&w.pool, &w.admin, w.team, &name)
        .await
        .expect("get");
    assert_eq!(stored.version, 2);
    let winner_host = &stored.spec.endpoints[0].host;
    assert!(winner_host == "writer-one" || winner_host == "writer-two");
}

#[tokio::test]
async fn delete_requires_current_revision_and_emits_deletion_event() {
    let Some(w) = world().await else { return };
    let name = unique("doomed");
    svc::create_cluster(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        spec("a"),
        RequestId::generate(),
    )
    .await
    .expect("create");

    let err = svc::delete_cluster(&w.pool, &w.admin, w.team, &name, 99, RequestId::generate())
        .await
        .expect_err("stale revision");
    assert_eq!(err.code, ErrorCode::RevisionMismatch);

    svc::delete_cluster(&w.pool, &w.admin, w.team, &name, 1, RequestId::generate())
        .await
        .expect("delete with the right revision");

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type = 'cluster.deleted' AND team_id = $1 \
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(w.team.id.as_uuid())
    .fetch_one(&w.pool)
    .await
    .expect("deletion event");
    let event: DomainEvent = serde_json::from_value(payload).expect("parse");
    assert!(matches!(event, DomainEvent::ClusterDeleted { name: n, .. } if n == name));
}

#[tokio::test]
async fn quota_caps_cluster_count_per_team() {
    let Some(w) = world().await else { return };
    // Fill to the default limit (50). Sequential to keep the test deterministic.
    for i in 0..50 {
        svc::create_cluster(
            &w.pool,
            &w.admin,
            w.team,
            &format!("q{i}-{}", unique("c")),
            spec("h"),
            RequestId::generate(),
        )
        .await
        .expect("create within quota");
    }
    let err = svc::create_cluster(
        &w.pool,
        &w.admin,
        w.team,
        &unique("over"),
        spec("h"),
        RequestId::generate(),
    )
    .await
    .expect_err("51st cluster must trip the quota");
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    assert!(err.hint.is_some());
}

#[tokio::test]
async fn grantless_member_denied_with_actionable_forbidden() {
    let Some(w) = world().await else { return };
    // A member of the SAME org with no grants: forbidden (not 404 — team is visible to
    // their org), and the error names the exact missing grant.
    let member_id = identity::upsert_user_by_subject(&w.pool, &unique("sub"), "m@t.test", "M")
        .await
        .expect("u");
    identity::add_org_membership(&w.pool, member_id, w.team.org_id, OrgRole::Member)
        .await
        .expect("m");
    let member = PrincipalCtx::User {
        user_id: member_id,
        platform_admin: false,
        org: Some((w.team.org_id, OrgRole::Member)),
        grants: GrantSet::default(),
    };
    let err = svc::create_cluster(
        &w.pool,
        &member,
        w.team,
        &unique("nope"),
        spec("h"),
        RequestId::generate(),
    )
    .await
    .expect_err("no grant, no create");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(err.message.contains("clusters:create"));
}
