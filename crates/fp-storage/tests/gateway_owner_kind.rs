//! Owner-kind-matched reference resolution (security-scan finding 9): a parent resource
//! resolves only children of its own owner kind — a user route config cannot bind an
//! ai-/discovery-owned cluster, a user listener cannot bind an ai-/discovery-owned route
//! config, and the ai/discovery materialization paths keep resolving their own kind.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::authz::TeamRef;
use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec};
use fp_domain::gateway::route_config::{
    PathMatch, RouteAction, RouteConfigSpec, RouteRule, VirtualHost,
};
use fp_domain::ErrorCode;
use fp_storage::repos::{clusters, gateway, identity};
use sqlx::PgPool;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

fn cluster_spec(host: &str) -> fp_domain::gateway::cluster::ClusterSpec {
    use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
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

fn listener_spec(port: u16, route_config: Option<String>) -> ListenerSpec {
    ListenerSpec {
        address: "0.0.0.0".into(),
        port,
        public_base_url: None,
        protocol: ListenerProtocol::Http,
        route_config,
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: None,
    }
}

struct World {
    pool: PgPool,
    team: TeamRef,
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
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    Some(World {
        pool,
        team: TeamRef {
            id: team.id,
            org_id: org.id,
        },
    })
}

async fn ref_rows_for_rc_name(pool: &PgPool, team: TeamRef, rc_name: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM route_config_cluster_refs r \
         JOIN route_configs rc ON rc.id = r.route_config_id \
         WHERE rc.team_id = $1 AND rc.name = $2",
    )
    .bind(team.id.as_uuid())
    .bind(rc_name)
    .fetch_one(pool)
    .await
    .expect("ref count")
}

async fn listener_rows_named(pool: &PgPool, team: TeamRef, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM listeners WHERE team_id = $1 AND name = $2")
        .bind(team.id.as_uuid())
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("listener count")
}

/// listener_route_config_refs rows pointing at the named route config (any listener).
async fn listener_refs_to_rc(pool: &PgPool, team: TeamRef, rc_name: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM listener_route_config_refs r \
         JOIN route_configs rc ON rc.id = r.route_config_id \
         WHERE rc.team_id = $1 AND rc.name = $2",
    )
    .bind(team.id.as_uuid())
    .bind(rc_name)
    .fetch_one(pool)
    .await
    .expect("listener ref count")
}

/// Route-config names the named listener is bound to.
async fn listener_bound_rc_names(pool: &PgPool, team: TeamRef, listener_name: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT rc.name FROM listener_route_config_refs r \
         JOIN listeners l ON l.id = r.listener_id \
         JOIN route_configs rc ON rc.id = r.route_config_id \
         WHERE l.team_id = $1 AND l.name = $2 ORDER BY rc.name",
    )
    .bind(team.id.as_uuid())
    .bind(listener_name)
    .fetch_all(pool)
    .await
    .expect("listener bound rc names")
}

#[tokio::test]
async fn user_route_config_cannot_resolve_ai_or_discovery_cluster() {
    let Some(w) = world().await else { return };
    let ai_cluster = unique("ai-upstream");
    let disco_cluster = unique("disco-upstream");
    let mut tx = w.pool.begin().await.expect("tx");
    clusters::create_ai_owned(
        &mut tx,
        w.team,
        Uuid::now_v7(),
        &ai_cluster,
        &cluster_spec("10.0.0.1"),
    )
    .await
    .expect("ai cluster");
    clusters::create_discovery_owned(
        &mut tx,
        w.team,
        Uuid::now_v7(),
        &disco_cluster,
        &cluster_spec("10.0.0.2"),
    )
    .await
    .expect("discovery cluster");
    tx.commit().await.expect("commit");

    for target in [&ai_cluster, &disco_cluster] {
        let rc_name = unique("rc");
        let mut tx = w.pool.begin().await.expect("tx");
        let err = gateway::create_route_config(&mut tx, w.team, &rc_name, &rc_spec(target))
            .await
            .expect_err("user rc must not bind a non-user cluster");
        assert_eq!(err.code, ErrorCode::ValidationFailed);
        assert!(
            err.message.contains(target.as_str()),
            "missing name listed: {}",
            err.message
        );
        drop(tx); // rolled back
        assert_eq!(ref_rows_for_rc_name(&w.pool, w.team, &rc_name).await, 0);
    }
}

#[tokio::test]
async fn user_route_config_update_cannot_switch_to_ai_cluster() {
    let Some(w) = world().await else { return };
    let user_cluster = unique("upstream");
    let ai_cluster = unique("ai-upstream");
    let rc_name = unique("rc");
    let mut tx = w.pool.begin().await.expect("tx");
    clusters::create(&mut tx, w.team, &user_cluster, &cluster_spec("10.0.0.3"))
        .await
        .expect("user cluster");
    clusters::create_ai_owned(
        &mut tx,
        w.team,
        Uuid::now_v7(),
        &ai_cluster,
        &cluster_spec("10.0.0.4"),
    )
    .await
    .expect("ai cluster");
    gateway::create_route_config(&mut tx, w.team, &rc_name, &rc_spec(&user_cluster))
        .await
        .expect("user rc with user cluster");
    tx.commit().await.expect("commit");

    let mut tx = w.pool.begin().await.expect("tx");
    let err = gateway::update_route_config(&mut tx, w.team, &rc_name, &rc_spec(&ai_cluster), 1)
        .await
        .expect_err("update must not bind an ai cluster");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(err.message.contains(&ai_cluster));
}

#[tokio::test]
async fn user_listener_cannot_resolve_ai_route_config_create_or_update() {
    let Some(w) = world().await else { return };
    let ai_cluster = unique("ai-upstream");
    let ai_rc = unique("ai-rc");
    let user_cluster = unique("upstream");
    let user_rc = unique("rc");
    let owner = Uuid::now_v7();
    let mut tx = w.pool.begin().await.expect("tx");
    clusters::create_ai_owned(
        &mut tx,
        w.team,
        owner,
        &ai_cluster,
        &cluster_spec("10.0.0.5"),
    )
    .await
    .expect("ai cluster");
    gateway::create_ai_route_config(&mut tx, w.team, owner, &ai_rc, &rc_spec(&ai_cluster))
        .await
        .expect("ai rc resolves ai cluster");
    clusters::create(&mut tx, w.team, &user_cluster, &cluster_spec("10.0.0.6"))
        .await
        .expect("user cluster");
    gateway::create_route_config(&mut tx, w.team, &user_rc, &rc_spec(&user_cluster))
        .await
        .expect("user rc");
    tx.commit().await.expect("commit");

    // Create: user listener naming the ai route config.
    let rejected_listener = unique("edge");
    let mut tx = w.pool.begin().await.expect("tx");
    let err = gateway::create_listener(
        &mut tx,
        w.team,
        &rejected_listener,
        &listener_spec(28401, Some(ai_rc.clone())),
    )
    .await
    .expect_err("user listener must not bind an ai route config");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(err.message.contains(&ai_rc));
    drop(tx); // rolled back
    assert_eq!(
        listener_rows_named(&w.pool, w.team, &rejected_listener).await,
        0,
        "rejected create must not persist the listener"
    );
    assert_eq!(
        listener_refs_to_rc(&w.pool, w.team, &ai_rc).await,
        0,
        "no listener_route_config_refs row may bind the ai route config"
    );

    // Update: user listener switched from a user rc to the ai rc.
    let listener_name = unique("edge");
    let mut tx = w.pool.begin().await.expect("tx");
    gateway::create_listener(
        &mut tx,
        w.team,
        &listener_name,
        &listener_spec(28402, Some(user_rc.clone())),
    )
    .await
    .expect("user listener with user rc");
    tx.commit().await.expect("commit");
    let mut tx = w.pool.begin().await.expect("tx");
    let err = gateway::update_listener(
        &mut tx,
        w.team,
        &listener_name,
        &listener_spec(28402, Some(ai_rc.clone())),
        1,
    )
    .await
    .expect_err("listener update must not bind an ai route config");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(err.message.contains(&ai_rc));
    drop(tx); // rolled back
              // The normalized binding row is untouched: still exactly the user route config.
    assert_eq!(
        listener_bound_rc_names(&w.pool, w.team, &listener_name).await,
        vec![user_rc.clone()],
        "original listener_route_config_refs row must remain intact"
    );
    assert_eq!(
        listener_refs_to_rc(&w.pool, w.team, &ai_rc).await,
        0,
        "no listener_route_config_refs row may bind the ai route config"
    );
}

#[tokio::test]
async fn same_kind_resolution_still_works_for_all_owner_kinds() {
    let Some(w) = world().await else { return };
    let owner = Uuid::now_v7();

    // user chain: cluster -> rc -> listener, create + update.
    let user_cluster = unique("upstream");
    let user_rc = unique("rc");
    let listener_name = unique("edge");
    let mut tx = w.pool.begin().await.expect("tx");
    clusters::create(&mut tx, w.team, &user_cluster, &cluster_spec("10.0.1.1"))
        .await
        .expect("user cluster");
    gateway::create_route_config(&mut tx, w.team, &user_rc, &rc_spec(&user_cluster))
        .await
        .expect("user rc create");
    gateway::update_route_config(&mut tx, w.team, &user_rc, &rc_spec(&user_cluster), 1)
        .await
        .expect("user rc update");
    gateway::create_listener(
        &mut tx,
        w.team,
        &listener_name,
        &listener_spec(28403, Some(user_rc.clone())),
    )
    .await
    .expect("user listener create");
    gateway::update_listener(
        &mut tx,
        w.team,
        &listener_name,
        &listener_spec(28403, Some(user_rc.clone())),
        1,
    )
    .await
    .expect("user listener update");
    tx.commit().await.expect("commit");
    assert_eq!(ref_rows_for_rc_name(&w.pool, w.team, &user_rc).await, 1);

    // ai chain resolves its own kind.
    let ai_cluster = unique("ai-upstream");
    let ai_rc = unique("ai-rc");
    let mut tx = w.pool.begin().await.expect("tx");
    clusters::create_ai_owned(
        &mut tx,
        w.team,
        owner,
        &ai_cluster,
        &cluster_spec("10.0.1.2"),
    )
    .await
    .expect("ai cluster");
    gateway::create_ai_route_config(&mut tx, w.team, owner, &ai_rc, &rc_spec(&ai_cluster))
        .await
        .expect("ai rc resolves ai cluster");
    gateway::create_ai_listener(
        &mut tx,
        w.team,
        owner,
        &unique("ai-edge"),
        &listener_spec(28404, Some(ai_rc.clone())),
    )
    .await
    .expect("ai listener resolves ai rc");
    tx.commit().await.expect("commit");

    // discovery chain resolves its own kind.
    let disco_cluster = unique("disco-upstream");
    let disco_rc = unique("disco-rc");
    let mut tx = w.pool.begin().await.expect("tx");
    clusters::create_discovery_owned(
        &mut tx,
        w.team,
        owner,
        &disco_cluster,
        &cluster_spec("10.0.1.3"),
    )
    .await
    .expect("discovery cluster");
    gateway::create_discovery_route_config(
        &mut tx,
        w.team,
        owner,
        &disco_rc,
        &rc_spec(&disco_cluster),
    )
    .await
    .expect("discovery rc resolves discovery cluster");
    gateway::create_discovery_listener(
        &mut tx,
        w.team,
        owner,
        &unique("disco-edge"),
        &listener_spec(28405, Some(disco_rc.clone())),
    )
    .await
    .expect("discovery listener resolves discovery rc");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn ai_route_config_cannot_resolve_user_cluster() {
    let Some(w) = world().await else { return };
    let user_cluster = unique("upstream");
    let mut tx = w.pool.begin().await.expect("tx");
    clusters::create(&mut tx, w.team, &user_cluster, &cluster_spec("10.0.2.1"))
        .await
        .expect("user cluster");
    let err = gateway::create_ai_route_config(
        &mut tx,
        w.team,
        Uuid::now_v7(),
        &unique("ai-rc"),
        &rc_spec(&user_cluster),
    )
    .await
    .expect_err("ai rc must not bind a user cluster");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(err.message.contains(&user_cluster));
}
