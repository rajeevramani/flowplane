//! Owner-kind-matched reference resolution: a USER-owned parent (route config,
//! listener) must only resolve USER-owned children by name. References to ai- or
//! discovery-owned children fail with the EXISTING unknown-reference validation
//! error (ErrorCode::ValidationFailed naming the child) — indistinguishable from a
//! genuinely missing name — and leak no binding rows.
//!
//! Black-box, spec-first: exercised through fp-core service functions only.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::clusters as csvc;
use fp_core::services::gateway as gw;
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec};
use fp_domain::gateway::route_config::{
    PathMatch, RouteAction, RouteConfigSpec, RouteRule, VirtualHost,
};
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::identity;
use sqlx::PgPool;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

fn cluster_spec(host: &str) -> ClusterSpec {
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

fn listener_spec(route_config: Option<String>, port: u16) -> ListenerSpec {
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
    admin: PrincipalCtx,
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

    let admin_id = identity::upsert_user_by_subject(&pool, &unique("sub"), "ok@t.test", "OK")
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

    Some(World { pool, team, admin })
}

/// Seed an ai- or discovery-owned cluster directly at the repo layer (below the user
/// surface, the same way the AI/discovery pipelines do).
async fn seed_owned_cluster(w: &World, owner_kind: &str, name: &str) -> Uuid {
    let mut tx = w.pool.begin().await.expect("begin");
    let cluster = match owner_kind {
        "ai" => fp_storage::repos::clusters::create_ai_owned(
            &mut tx,
            w.team,
            Uuid::now_v7(),
            name,
            &cluster_spec("10.9.9.9"),
        )
        .await
        .expect("seed ai cluster"),
        "discovery" => fp_storage::repos::clusters::create_discovery_owned(
            &mut tx,
            w.team,
            Uuid::now_v7(),
            name,
            &cluster_spec("10.9.9.9"),
        )
        .await
        .expect("seed discovery cluster"),
        other => panic!("unsupported owner kind {other}"),
    };
    tx.commit().await.expect("commit");
    cluster.id.as_uuid()
}

/// Seed an ai- or discovery-owned route config via direct SQL (the fp-storage gateway
/// repo is the implementation under test and off-limits here).
async fn seed_owned_route_config(w: &World, owner_kind: &str, name: &str) {
    let spec = serde_json::to_value(rc_spec("seed-upstream")).expect("serialize seeded rc spec");
    sqlx::query(
        "INSERT INTO route_configs (id, team_id, org_id, name, spec, owner_kind, owner_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(w.team.id.as_uuid())
    .bind(w.team.org_id.as_uuid())
    .bind(name)
    .bind(spec)
    .bind(owner_kind)
    .bind(Uuid::now_v7())
    .execute(&w.pool)
    .await
    .expect("seed owned route config");
}

/// Cluster ids bound (via route_config_cluster_refs) to the named route config we own.
async fn bound_cluster_ids(w: &World, rc_name: &str) -> Vec<Uuid> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT r.cluster_id FROM route_config_cluster_refs r \
         JOIN route_configs rc ON rc.id = r.route_config_id \
         WHERE rc.team_id = $1 AND rc.name = $2 ORDER BY r.cluster_id",
    )
    .bind(w.team.id.as_uuid())
    .bind(rc_name)
    .fetch_all(&w.pool)
    .await
    .expect("bound cluster ids");
    rows.into_iter().map(|(id,)| id).collect()
}

/// Binding rows pointing at a specific cluster id (any parent).
async fn refs_to_cluster(w: &World, cluster_id: Uuid) -> i64 {
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM route_config_cluster_refs WHERE cluster_id = $1")
            .bind(cluster_id)
            .fetch_one(&w.pool)
            .await
            .expect("refs to cluster");
    count
}

async fn route_config_exists(w: &World, name: &str) -> bool {
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM route_configs WHERE team_id = $1 AND name = $2")
            .bind(w.team.id.as_uuid())
            .bind(name)
            .fetch_one(&w.pool)
            .await
            .expect("rc exists");
    count > 0
}

async fn listener_exists(w: &World, name: &str) -> bool {
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM listeners WHERE team_id = $1 AND name = $2")
            .bind(w.team.id.as_uuid())
            .bind(name)
            .fetch_one(&w.pool)
            .await
            .expect("listener exists");
    count > 0
}

/// listener_route_config_refs rows pointing at the named route config (any listener).
async fn listener_refs_to_route_config(w: &World, rc_name: &str) -> i64 {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM listener_route_config_refs r \
         JOIN route_configs rc ON rc.id = r.route_config_id \
         WHERE rc.team_id = $1 AND rc.name = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(rc_name)
    .fetch_one(&w.pool)
    .await
    .expect("listener refs to rc");
    count
}

/// Route-config names the named listener is bound to via listener_route_config_refs.
async fn listener_bound_rc_names(w: &World, listener_name: &str) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT rc.name FROM listener_route_config_refs r \
         JOIN listeners l ON l.id = r.listener_id \
         JOIN route_configs rc ON rc.id = r.route_config_id \
         WHERE l.team_id = $1 AND l.name = $2 ORDER BY rc.name",
    )
    .bind(w.team.id.as_uuid())
    .bind(listener_name)
    .fetch_all(&w.pool)
    .await
    .expect("listener bound rc names");
    rows.into_iter().map(|(name,)| name).collect()
}

// AC1: user route config -> ai-owned cluster name: rejected, no binding row, no rc row.
#[tokio::test]
async fn user_route_config_cannot_reference_ai_owned_cluster() {
    let Some(w) = world().await else { return };
    let ai_cluster = unique("ai-upstream");
    let ai_cluster_id = seed_owned_cluster(&w, "ai", &ai_cluster).await;

    let rc_name = unique("rc");
    let err = gw::create_route_config(
        &w.pool,
        &w.admin,
        w.team,
        &rc_name,
        rc_spec(&ai_cluster),
        RequestId::generate(),
    )
    .await
    .expect_err("user rc must not resolve an ai-owned cluster");
    assert_eq!(err.code, ErrorCode::ValidationFailed, "err: {err:?}");
    assert!(
        err.message.contains(&ai_cluster),
        "error must name the unresolvable cluster: {}",
        err.message
    );

    assert!(
        !route_config_exists(&w, &rc_name).await,
        "rejected create must not persist the route config"
    );
    assert_eq!(
        refs_to_cluster(&w, ai_cluster_id).await,
        0,
        "no route_config_cluster_refs row may bind the ai-owned cluster"
    );
}

// AC2: same for a discovery-owned cluster.
#[tokio::test]
async fn user_route_config_cannot_reference_discovery_owned_cluster() {
    let Some(w) = world().await else { return };
    let disco_cluster = unique("disco-upstream");
    let disco_cluster_id = seed_owned_cluster(&w, "discovery", &disco_cluster).await;

    let rc_name = unique("rc");
    let err = gw::create_route_config(
        &w.pool,
        &w.admin,
        w.team,
        &rc_name,
        rc_spec(&disco_cluster),
        RequestId::generate(),
    )
    .await
    .expect_err("user rc must not resolve a discovery-owned cluster");
    assert_eq!(err.code, ErrorCode::ValidationFailed, "err: {err:?}");
    assert!(
        err.message.contains(&disco_cluster),
        "error must name the unresolvable cluster: {}",
        err.message
    );

    assert!(
        !route_config_exists(&w, &rc_name).await,
        "rejected create must not persist the route config"
    );
    assert_eq!(
        refs_to_cluster(&w, disco_cluster_id).await,
        0,
        "no route_config_cluster_refs row may bind the discovery-owned cluster"
    );
}

// AC3: user listener -> ai-owned AND discovery-owned route config names: both rejected.
#[tokio::test]
async fn user_listener_cannot_reference_ai_or_discovery_owned_route_config() {
    let Some(w) = world().await else { return };
    let ai_rc = unique("airc");
    seed_owned_route_config(&w, "ai", &ai_rc).await;
    let disco_rc = unique("discorc");
    seed_owned_route_config(&w, "discovery", &disco_rc).await;

    let ai_listener = unique("edge");
    let err = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &ai_listener,
        listener_spec(Some(ai_rc.clone()), 28510),
        RequestId::generate(),
        false,
    )
    .await
    .expect_err("user listener must not resolve an ai-owned route config");
    assert_eq!(err.code, ErrorCode::ValidationFailed, "err: {err:?}");
    assert!(
        err.message.contains(&ai_rc),
        "error must name the unresolvable route config: {}",
        err.message
    );
    assert!(
        !listener_exists(&w, &ai_listener).await,
        "rejected create must not persist the listener"
    );
    assert_eq!(
        listener_refs_to_route_config(&w, &ai_rc).await,
        0,
        "no listener_route_config_refs row may bind the ai-owned route config"
    );

    let disco_listener = unique("edge");
    let err = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &disco_listener,
        listener_spec(Some(disco_rc.clone()), 28511),
        RequestId::generate(),
        false,
    )
    .await
    .expect_err("user listener must not resolve a discovery-owned route config");
    assert_eq!(err.code, ErrorCode::ValidationFailed, "err: {err:?}");
    assert!(
        err.message.contains(&disco_rc),
        "error must name the unresolvable route config: {}",
        err.message
    );
    assert!(
        !listener_exists(&w, &disco_listener).await,
        "rejected create must not persist the listener"
    );
    assert_eq!(
        listener_refs_to_route_config(&w, &disco_rc).await,
        0,
        "no listener_route_config_refs row may bind the discovery-owned route config"
    );
}

// AC4: UPDATE of an existing user route config to an ai/discovery cluster: rejected,
// original binding intact, version unchanged.
#[tokio::test]
async fn updating_user_route_config_to_wrong_kind_cluster_is_rejected() {
    let Some(w) = world().await else { return };
    let user_cluster = unique("user-upstream");
    let created = csvc::create_cluster(
        &w.pool,
        &w.admin,
        w.team,
        &user_cluster,
        cluster_spec("10.0.0.1"),
        RequestId::generate(),
        Default::default(),
    )
    .await
    .expect("user cluster");
    let rc_name = unique("rc");
    gw::create_route_config(
        &w.pool,
        &w.admin,
        w.team,
        &rc_name,
        rc_spec(&user_cluster),
        RequestId::generate(),
    )
    .await
    .expect("user rc referencing user cluster");

    let ai_cluster = unique("ai-upstream");
    let ai_cluster_id = seed_owned_cluster(&w, "ai", &ai_cluster).await;
    let disco_cluster = unique("disco-upstream");
    let disco_cluster_id = seed_owned_cluster(&w, "discovery", &disco_cluster).await;

    for (wrong_name, wrong_id) in [
        (&ai_cluster, ai_cluster_id),
        (&disco_cluster, disco_cluster_id),
    ] {
        let err = gw::update_route_config(
            &w.pool,
            &w.admin,
            w.team,
            &rc_name,
            rc_spec(wrong_name),
            1,
            RequestId::generate(),
        )
        .await
        .expect_err("update must not resolve a wrong-kind cluster");
        assert_eq!(err.code, ErrorCode::ValidationFailed, "err: {err:?}");
        assert!(
            err.message.contains(wrong_name.as_str()),
            "error must name the unresolvable cluster: {}",
            err.message
        );
        assert_eq!(
            refs_to_cluster(&w, wrong_id).await,
            0,
            "rejected update must not bind the {wrong_name} cluster"
        );
    }

    // The route config is untouched: still revision 1, still bound to the user cluster.
    let stored = gw::get_route_config(&w.pool, &w.admin, w.team, &rc_name, RequestId::generate())
        .await
        .expect("rc survives");
    assert_eq!(
        stored.version, 1,
        "failed updates must not bump the revision"
    );
    assert_eq!(
        bound_cluster_ids(&w, &rc_name).await,
        vec![created.id.as_uuid()],
        "original user-cluster binding must remain intact"
    );
}

// AC5: UPDATE of an existing user listener to an ai/discovery route config: rejected,
// listener unchanged.
#[tokio::test]
async fn updating_user_listener_to_wrong_kind_route_config_is_rejected() {
    let Some(w) = world().await else { return };
    let user_cluster = unique("user-upstream");
    csvc::create_cluster(
        &w.pool,
        &w.admin,
        w.team,
        &user_cluster,
        cluster_spec("10.0.0.1"),
        RequestId::generate(),
        Default::default(),
    )
    .await
    .expect("user cluster");
    let user_rc = unique("rc");
    gw::create_route_config(
        &w.pool,
        &w.admin,
        w.team,
        &user_rc,
        rc_spec(&user_cluster),
        RequestId::generate(),
    )
    .await
    .expect("user rc");
    let listener_name = unique("edge");
    gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &listener_name,
        listener_spec(Some(user_rc.clone()), 28520),
        RequestId::generate(),
        false,
    )
    .await
    .expect("user listener referencing user rc");

    let ai_rc = unique("airc");
    seed_owned_route_config(&w, "ai", &ai_rc).await;
    let disco_rc = unique("discorc");
    seed_owned_route_config(&w, "discovery", &disco_rc).await;

    for wrong_rc in [&ai_rc, &disco_rc] {
        let err = gw::update_listener(
            &w.pool,
            &w.admin,
            w.team,
            &listener_name,
            listener_spec(Some(wrong_rc.clone()), 28520),
            1,
            RequestId::generate(),
            false,
        )
        .await
        .expect_err("update must not resolve a wrong-kind route config");
        assert_eq!(err.code, ErrorCode::ValidationFailed, "err: {err:?}");
        assert!(
            err.message.contains(wrong_rc.as_str()),
            "error must name the unresolvable route config: {}",
            err.message
        );
    }

    // Listener untouched: revision 1, still pointing at the user route config.
    let stored = gw::get_listener(
        &w.pool,
        &w.admin,
        w.team,
        &listener_name,
        RequestId::generate(),
    )
    .await
    .expect("listener survives");
    assert_eq!(
        stored.version, 1,
        "failed updates must not bump the revision"
    );
    assert_eq!(
        stored.spec.route_config.as_deref(),
        Some(user_rc.as_str()),
        "original user route-config reference must remain intact"
    );
    // The normalized binding row is untouched too: still exactly the user rc,
    // and nothing points at the wrong-kind route configs.
    assert_eq!(
        listener_bound_rc_names(&w, &listener_name).await,
        vec![user_rc.clone()],
        "original listener_route_config_refs row must remain intact"
    );
    assert_eq!(listener_refs_to_route_config(&w, &ai_rc).await, 0);
    assert_eq!(listener_refs_to_route_config(&w, &disco_rc).await, 0);
}

// AC6: positive same-kind matrix — user rc referencing a user cluster creates AND
// updates with the binding intact; user listener referencing a user rc creates AND
// updates.
#[tokio::test]
async fn same_kind_references_work_across_create_and_update() {
    let Some(w) = world().await else { return };
    let cluster_one = unique("upstream-one");
    let created_one = csvc::create_cluster(
        &w.pool,
        &w.admin,
        w.team,
        &cluster_one,
        cluster_spec("10.0.0.1"),
        RequestId::generate(),
        Default::default(),
    )
    .await
    .expect("cluster one");
    let cluster_two = unique("upstream-two");
    let created_two = csvc::create_cluster(
        &w.pool,
        &w.admin,
        w.team,
        &cluster_two,
        cluster_spec("10.0.0.2"),
        RequestId::generate(),
        Default::default(),
    )
    .await
    .expect("cluster two");

    // Route config CREATE against a user cluster.
    let rc_name = unique("rc");
    let rc = gw::create_route_config(
        &w.pool,
        &w.admin,
        w.team,
        &rc_name,
        rc_spec(&cluster_one),
        RequestId::generate(),
    )
    .await
    .expect("user rc referencing user cluster");
    assert_eq!(rc.version, 1);
    assert_eq!(
        bound_cluster_ids(&w, &rc_name).await,
        vec![created_one.id.as_uuid()],
        "create must record the cluster binding"
    );

    // Route config UPDATE re-targeting another user cluster.
    let updated = gw::update_route_config(
        &w.pool,
        &w.admin,
        w.team,
        &rc_name,
        rc_spec(&cluster_two),
        1,
        RequestId::generate(),
    )
    .await
    .expect("update to another user cluster");
    assert_eq!(updated.version, 2);
    assert_eq!(
        bound_cluster_ids(&w, &rc_name).await,
        vec![created_two.id.as_uuid()],
        "update must move the binding to the new user cluster"
    );

    // Listener CREATE against the user route config.
    let listener_name = unique("edge");
    let listener = gw::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &listener_name,
        listener_spec(Some(rc_name.clone()), 28530),
        RequestId::generate(),
        false,
    )
    .await
    .expect("user listener referencing user rc");
    assert_eq!(listener.version, 1);

    // Listener UPDATE, still referencing the user route config.
    let updated = gw::update_listener(
        &w.pool,
        &w.admin,
        w.team,
        &listener_name,
        listener_spec(Some(rc_name.clone()), 28531),
        1,
        RequestId::generate(),
        false,
    )
    .await
    .expect("listener update keeping the user rc reference");
    assert_eq!(updated.version, 2);
    assert_eq!(updated.spec.route_config.as_deref(), Some(rc_name.as_str()));
    assert_eq!(updated.spec.port, 28531);
}
