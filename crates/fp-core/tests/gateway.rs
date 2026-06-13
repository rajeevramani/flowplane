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
        least_request: None,
        ring_hash: None,
        maglev: None,
        dns_lookup_family: None,
        connect_timeout_secs: 5,
        use_tls: false,
        upstream_tls: None,
        protocol: None,
        health_checks: None,
        circuit_breakers: None,
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
        org_selector_required: false,
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
        org_selector_required: false,
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
    let err = svc::get_cluster(&w.pool, &w.outsider, w.team, &name, RequestId::generate())
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
    let stored = svc::get_cluster(&w.pool, &w.admin, w.team, &name, RequestId::generate())
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
        org_selector_required: false,
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

mod referential {
    use super::*;
    use fp_core::services::gateway as gw;
    use fp_domain::gateway::listener::ListenerSpec;
    use fp_domain::gateway::route_config::{
        PathMatch, RouteAction, RouteConfigSpec, RouteRule, VirtualHost,
    };

    fn rc_spec(cluster: &str) -> RouteConfigSpec {
        RouteConfigSpec {
            virtual_hosts: vec![VirtualHost {
                name: "default".into(),
                domains: vec!["*".into()],
                routes: vec![RouteRule {
                    name: "all".into(),
                    matcher: PathMatch::Prefix { prefix: "/".into() },
                    headers: Vec::new(),
                    query_parameters: Vec::new(),
                    action: RouteAction {
                        cluster: Some(cluster.into()),
                        weighted_clusters: None,
                        redirect: None,
                        prefix_rewrite: None,
                        template_rewrite: None,
                        timeout_secs: 15,
                        retry_policy: None,
                        rate_limits: Vec::new(),
                    },
                    filter_overrides: Vec::new(),
                }],
                rate_limits: Vec::new(),
                filter_overrides: Vec::new(),
            }],
        }
    }

    #[tokio::test]
    async fn references_resolve_and_deletion_is_guarded_end_to_end() {
        let Some(w) = world().await else { return };
        let rid = RequestId::generate;

        // Route config referencing a missing cluster: validation error naming it.
        let err = gw::create_route_config(
            &w.pool,
            &w.admin,
            w.team,
            &unique("rc"),
            rc_spec("ghost-cluster"),
            rid(),
        )
        .await
        .expect_err("missing cluster reference");
        assert_eq!(err.code, ErrorCode::ValidationFailed);
        assert!(err.message.contains("ghost-cluster"));

        // Create the chain: cluster -> route config -> listener.
        let cluster_name = unique("upstream");
        svc::create_cluster(
            &w.pool,
            &w.admin,
            w.team,
            &cluster_name,
            spec("10.0.0.9"),
            rid(),
        )
        .await
        .expect("cluster");
        let rc_name = unique("routes");
        gw::create_route_config(
            &w.pool,
            &w.admin,
            w.team,
            &rc_name,
            rc_spec(&cluster_name),
            rid(),
        )
        .await
        .expect("route config");
        let listener_name = unique("edge");
        gw::create_listener(
            &w.pool,
            &w.admin,
            w.team,
            &listener_name,
            ListenerSpec {
                address: "0.0.0.0".into(),
                port: 18443,
                route_config: Some(rc_name.clone()),
                http_filters: Vec::new(),
                tls_context: None,
            },
            rid(),
        )
        .await
        .expect("listener");

        // Deleting the referenced cluster: conflict naming the dependent route config.
        let err = svc::delete_cluster(&w.pool, &w.admin, w.team, &cluster_name, 1, rid())
            .await
            .expect_err("referenced cluster must not delete");
        assert_eq!(err.code, ErrorCode::Conflict);
        assert!(
            err.message.contains(&rc_name),
            "dependents are named: {}",
            err.message
        );

        // Deleting the referenced route config: conflict naming the dependent listener.
        let err = gw::delete_route_config(&w.pool, &w.admin, w.team, &rc_name, 1, rid())
            .await
            .expect_err("referenced route config must not delete");
        assert_eq!(err.code, ErrorCode::Conflict);
        assert!(err.message.contains(&listener_name));

        // Unwind in dependency order: listener -> route config -> cluster. No orphans.
        gw::delete_listener(&w.pool, &w.admin, w.team, &listener_name, 1, rid())
            .await
            .expect("delete listener");
        gw::delete_route_config(&w.pool, &w.admin, w.team, &rc_name, 1, rid())
            .await
            .expect("delete route config");
        svc::delete_cluster(&w.pool, &w.admin, w.team, &cluster_name, 1, rid())
            .await
            .expect("delete cluster");
        let (refs,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM route_config_cluster_refs r \
             JOIN route_configs rc ON rc.id = r.route_config_id WHERE rc.team_id = $1",
        )
        .bind(w.team.id.as_uuid())
        .fetch_one(&w.pool)
        .await
        .expect("refs");
        assert_eq!(refs, 0, "no orphaned reference rows");
    }

    #[tokio::test]
    async fn port_collisions_within_a_team_conflict() {
        let Some(w) = world().await else { return };
        let rid = RequestId::generate;
        let make = |name: String, port: u16| {
            gw::create_listener(
                &w.pool,
                &w.admin,
                w.team,
                // move name into the future
                Box::leak(name.into_boxed_str()),
                ListenerSpec {
                    address: "0.0.0.0".into(),
                    port,
                    route_config: None,
                    http_filters: Vec::new(),
                    tls_context: None,
                },
                rid(),
            )
        };
        make(unique("l1"), 19001).await.expect("first listener");
        let err = make(unique("l2"), 19001)
            .await
            .expect_err("same port, same team");
        assert_eq!(err.code, ErrorCode::Conflict);
        assert!(err.hint.is_some());
    }
}
