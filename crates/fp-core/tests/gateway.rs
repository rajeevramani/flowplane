//! S3 exit tests: the clusters vertical against real PostgreSQL — service-layer authz,
//! transactional events, optimistic concurrency under real contention, tenancy, quota.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::clusters as svc;
use fp_core::services::egress_policy::EgressPolicy;
use fp_core::services::filesystem_path_policy::FilesystemPathPolicy;
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::event::DomainEvent;
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy, UpstreamTlsConfig};
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::identity;
use sqlx::PgPool;
use std::net::{IpAddr, Ipv4Addr};

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

fn spec(host: &str) -> ClusterSpec {
    ClusterSpec {
        aggregate_clusters: Vec::new(),
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

fn public_egress_policy() -> EgressPolicy {
    EgressPolicy::with_static_hosts(
        Vec::new(),
        Vec::new(),
        vec![(
            "api.example.com".into(),
            443,
            vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 20))],
        )],
    )
}

async fn event_count(pool: &PgPool, team: TeamRef, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1 AND team_id = $2")
        .bind(event_type)
        .bind(team.id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("event count")
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
    let outsider_get_rid = RequestId::generate();
    let err = svc::get_cluster(&w.pool, &w.outsider, w.team, &name, outsider_get_rid)
        .await
        .expect_err("outsider must not see it");
    assert_eq!(err.code, ErrorCode::NotFound);

    // Outsider cannot mutate either — and the failure discloses nothing.
    let outsider_delete_rid = RequestId::generate();
    let err = svc::delete_cluster(&w.pool, &w.outsider, w.team, &name, 1, outsider_delete_rid)
        .await
        .expect_err("outsider must not delete");
    assert_eq!(err.code, ErrorCode::NotFound);

    let denial_rows: Vec<(String, String, String, uuid::Uuid, uuid::Uuid, String, String)> =
        sqlx::query_as(
            "SELECT action, resource, outcome, org_id, team_id, detail->>'resource', detail->>'reason' \
             FROM audit_log WHERE request_id = ANY($1) ORDER BY occurred_at",
        )
        .bind(vec![outsider_get_rid.as_uuid(), outsider_delete_rid.as_uuid()])
        .fetch_all(&w.pool)
        .await
        .expect("denial audit rows");
    assert_eq!(
        denial_rows,
        vec![
            (
                "authz.denied".into(),
                "clusters".into(),
                "denied".into(),
                w.team.org_id.as_uuid(),
                w.team.id.as_uuid(),
                "clusters".into(),
                "cross_org".into(),
            ),
            (
                "authz.denied".into(),
                "clusters".into(),
                "denied".into(),
                w.team.org_id.as_uuid(),
                w.team.id.as_uuid(),
                "clusters".into(),
                "cross_org".into(),
            ),
        ]
    );
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

#[tokio::test]
async fn cluster_ca_file_rejection_does_not_persist_row_or_outbox_event() {
    let Some(w) = world().await else { return };
    let name = unique("file-ca");
    let before = event_count(&w.pool, w.team, "cluster.upserted").await;
    let mut cluster = spec("api.example.com");
    cluster.endpoints[0].port = 443;
    cluster.use_tls = true;
    cluster.upstream_tls = Some(UpstreamTlsConfig {
        sni: Some("api.example.com".into()),
        validation_context_sds_secret_name: None,
        ca_cert_file: Some("/etc/tenant/ca.pem".into()),
        auto_sni_san_validation: false,
        insecure_skip_verify: false,
    });

    let err = svc::create_cluster_with_policies(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        cluster,
        RequestId::generate(),
        &public_egress_policy(),
        &FilesystemPathPolicy::disabled(),
    )
    .await
    .expect_err("tenant CA file rejected");
    assert_eq!(err.code, ErrorCode::ValidationFailed);

    let stored: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM clusters WHERE team_id = $1 AND name = $2")
            .bind(w.team.id.as_uuid())
            .bind(&name)
            .fetch_optional(&w.pool)
            .await
            .expect("cluster lookup");
    assert!(stored.is_none(), "rejected cluster must not persist");
    assert_eq!(
        event_count(&w.pool, w.team, "cluster.upserted").await,
        before,
        "rejected cluster must not append an outbox event"
    );
}

#[tokio::test]
async fn cluster_sds_validation_context_still_persists() {
    let Some(w) = world().await else { return };
    let name = unique("sds-ca");
    let mut cluster = spec("api.example.com");
    cluster.endpoints[0].port = 443;
    cluster.use_tls = true;
    cluster.upstream_tls = Some(UpstreamTlsConfig {
        sni: Some("api.example.com".into()),
        validation_context_sds_secret_name: Some("tenant-ca".into()),
        ca_cert_file: None,
        auto_sni_san_validation: true,
        insecure_skip_verify: false,
    });
    let created = svc::create_cluster_with_policies(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        cluster,
        RequestId::generate(),
        &public_egress_policy(),
        &FilesystemPathPolicy::disabled(),
    )
    .await
    .expect("SDS-backed cluster persists");
    assert_eq!(created.name, name);
}

#[tokio::test]
async fn listener_file_path_rejection_does_not_persist_row_or_outbox_event() {
    let Some(w) = world().await else { return };
    use fp_core::services::gateway as gw;
    use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec, ListenerTlsConfig};

    let name = unique("file-listener");
    let before = event_count(&w.pool, w.team, "listener.upserted").await;
    let spec = ListenerSpec {
        address: "0.0.0.0".into(),
        port: 18443,
        public_base_url: None,
        protocol: ListenerProtocol::Https,
        route_config: None,
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: Some(ListenerTlsConfig {
            cert_chain_file: Some("/etc/tenant/cert.pem".into()),
            private_key_file: Some("/etc/tenant/key.pem".into()),
            ca_cert_file: None,
            require_client_certificate: false,
            tls_certificate_sds_secret_name: None,
            validation_context_sds_secret_name: None,
        }),
    };

    let err = gw::create_listener_with_file_policy(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        spec,
        RequestId::generate(),
        false,
        &FilesystemPathPolicy::disabled(),
    )
    .await
    .expect_err("tenant listener TLS files rejected");
    assert_eq!(err.code, ErrorCode::ValidationFailed);

    let stored: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM listeners WHERE team_id = $1 AND name = $2")
            .bind(w.team.id.as_uuid())
            .bind(&name)
            .fetch_optional(&w.pool)
            .await
            .expect("listener lookup");
    assert!(stored.is_none(), "rejected listener must not persist");
    assert_eq!(
        event_count(&w.pool, w.team, "listener.upserted").await,
        before,
        "rejected listener must not append an outbox event"
    );
}

#[tokio::test]
async fn listener_sds_tls_still_persists() {
    let Some(w) = world().await else { return };
    use fp_core::services::gateway as gw;
    use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec, ListenerTlsConfig};

    let name = unique("sds-listener");
    let spec = ListenerSpec {
        address: "0.0.0.0".into(),
        port: 18444,
        public_base_url: None,
        protocol: ListenerProtocol::Https,
        route_config: None,
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: Some(ListenerTlsConfig {
            cert_chain_file: None,
            private_key_file: None,
            ca_cert_file: None,
            require_client_certificate: false,
            tls_certificate_sds_secret_name: Some("edge-cert".into()),
            validation_context_sds_secret_name: Some("edge-ca".into()),
        }),
    };
    let created = gw::create_listener_with_file_policy(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        spec,
        RequestId::generate(),
        false,
        &FilesystemPathPolicy::disabled(),
    )
    .await
    .expect("SDS-backed listener persists");
    assert_eq!(created.name, name);
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
                        direct_response: None,
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
                public_base_url: None,
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(rc_name.clone()),
                http_filters: Vec::new(),
                access_logs: Vec::new(),
                tls_context: None,
            },
            rid(),
            false,
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
    async fn manual_listener_names_cannot_use_ai_prefix() {
        let Some(w) = world().await else { return };
        let rid = RequestId::generate;

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

        let err = gw::create_listener(
            &w.pool,
            &w.admin,
            w.team,
            "ai-manual-listener",
            ListenerSpec {
                address: "0.0.0.0".into(),
                port: 18444,
                public_base_url: None,
                protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                route_config: Some(rc_name),
                http_filters: Vec::new(),
                access_logs: Vec::new(),
                tls_context: None,
            },
            rid(),
            false,
        )
        .await
        .expect_err("ai prefix is reserved");

        assert_eq!(err.code, ErrorCode::ValidationFailed);
        assert!(err.message.contains("reserved for AI routes"));
    }

    #[tokio::test]
    async fn user_route_configs_cannot_reference_ai_owned_clusters_but_ai_routes_can() {
        let Some(w) = world().await else { return };
        let ai_cluster_name = unique("ai-provider");
        let owner_id = uuid::Uuid::new_v4();
        let rid = RequestId::generate;

        let mut tx = w.pool.begin().await.expect("begin");
        fp_storage::repos::clusters::create_ai_owned(
            &mut tx,
            w.team,
            owner_id,
            &ai_cluster_name,
            &spec("203.0.113.42"),
        )
        .await
        .expect("ai cluster");
        tx.commit().await.expect("commit ai cluster");

        let err = gw::create_route_config(
            &w.pool,
            &w.admin,
            w.team,
            &unique("user-routes"),
            rc_spec(&ai_cluster_name),
            rid(),
        )
        .await
        .expect_err("user route config must not resolve ai-owned cluster");
        assert_eq!(err.code, ErrorCode::ValidationFailed);
        assert!(err.message.contains(&ai_cluster_name));

        let mut tx = w.pool.begin().await.expect("begin");
        let ai_route = fp_storage::repos::gateway::create_ai_route_config(
            &mut tx,
            w.team,
            owner_id,
            &unique("ai-routes"),
            &rc_spec(&ai_cluster_name),
        )
        .await
        .expect("ai-owned route config can resolve ai-owned cluster");
        tx.commit().await.expect("commit ai route");

        let ref_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM route_config_cluster_refs \
             WHERE team_id = $1 AND route_config_id = $2",
        )
        .bind(w.team.id.as_uuid())
        .bind(ai_route.id.as_uuid())
        .fetch_one(&w.pool)
        .await
        .expect("ref count");
        assert_eq!(ref_count, 1);
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
                    public_base_url: None,
                    protocol: fp_domain::gateway::listener::ListenerProtocol::Http,
                    route_config: None,
                    http_filters: Vec::new(),
                    access_logs: Vec::new(),
                    tls_context: None,
                },
                rid(),
                false,
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

mod expose_shortcut {
    use super::*;
    use fp_core::services::expose as exp;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tokio::sync::Barrier;

    #[tokio::test]
    async fn auto_port_expose_retries_concurrent_collisions() {
        let Some(w) = world().await else { return };
        let count = 8;
        let barrier = Arc::new(Barrier::new(count));
        let mut tasks = Vec::new();

        for _ in 0..count {
            let pool = w.pool.clone();
            let ctx = w.admin.clone();
            let team = w.team;
            let barrier = Arc::clone(&barrier);
            let name = unique("expose");
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                exp::expose(
                    &pool,
                    &ctx,
                    team,
                    exp::ExposeRequest {
                        name,
                        upstream: "http://127.0.0.1:3001".into(),
                        path: "/".into(),
                        port: None,
                        public_base_url: None,
                    },
                    RequestId::generate(),
                )
                .await
            }));
        }

        let mut ports = BTreeSet::new();
        for task in tasks {
            let exposed = task.await.expect("join").expect("expose");
            assert!(
                ports.insert(exposed.port),
                "auto-allocated duplicate port {}",
                exposed.port
            );
            assert_eq!(exposed.curl_url, None);
            assert_eq!(
                exposed.endpoint_source,
                exp::ExposeEndpointSource::Unconfigured
            );
        }

        let (listener_count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM listeners WHERE team_id = $1")
                .bind(w.team.id.as_uuid())
                .fetch_one(&w.pool)
                .await
                .expect("listener count");
        let (cluster_count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM clusters WHERE team_id = $1")
                .bind(w.team.id.as_uuid())
                .fetch_one(&w.pool)
                .await
                .expect("cluster count");
        let (route_config_count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM route_configs WHERE team_id = $1")
                .bind(w.team.id.as_uuid())
                .fetch_one(&w.pool)
                .await
                .expect("route config count");

        assert_eq!(listener_count, count as i64);
        assert_eq!(cluster_count, count as i64);
        assert_eq!(route_config_count, count as i64);
    }
}
