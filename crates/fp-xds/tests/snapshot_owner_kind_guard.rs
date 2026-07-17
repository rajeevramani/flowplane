//! Serve-time owner-kind guard: when a team snapshot is rebuilt from the DB, USER-owned
//! parents that reference non-user (ai/discovery) children must be WITHDRAWN from the
//! served sets and surfaced as degraded entries — without touching the offending DB rows.
//!
//! The write path now rejects these shapes, so every offending row here is seeded via
//! direct SQL (the only way such rows can exist, e.g. legacy data or manual edits).

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::authz::TeamRef;
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
use fp_domain::gateway::listener::ListenerSpec;
use fp_domain::gateway::route_config::{
    PathMatch, RouteAction, RouteConfigSpec, RouteRule, VirtualHost,
};
use fp_storage::repos::identity;
use fp_xds::snapshot::SnapshotCache;
use prost::Message;
use std::collections::HashSet;

const ROUTE_TYPE_URL: &str = "type.googleapis.com/envoy.config.route.v3.RouteConfiguration";
const LISTENER_TYPE_URL: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
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

fn route_config_spec(cluster: &str) -> RouteConfigSpec {
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

fn listener_spec(port: u16, route_config: &str) -> ListenerSpec {
    ListenerSpec {
        address: "0.0.0.0".into(),
        port,
        public_base_url: None,
        protocol: Default::default(),
        route_config: Some(route_config.into()),
        http_filters: Vec::new(),
        access_logs: Vec::new(),
        tls_context: None,
    }
}

/// Env-gated world: shared PG, fresh org + team per test.
async fn world() -> Option<(sqlx::PgPool, TeamRef)> {
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
    Some((
        pool,
        TeamRef {
            id: team_row.id,
            org_id: org.id,
        },
    ))
}

fn owner_id_for(owner_kind: &str) -> Option<uuid::Uuid> {
    // Schema: owner_id must be NULL for 'user' and NOT NULL for 'ai'/'discovery'.
    if owner_kind == "user" {
        None
    } else {
        Some(uuid::Uuid::now_v7())
    }
}

/// Direct-SQL seed — bypasses the write path on purpose (it now rejects cross-owner refs).
async fn insert_row(
    pool: &sqlx::PgPool,
    table: &str,
    team: &TeamRef,
    name: &str,
    spec_json: &str,
    owner_kind: &str,
) {
    let sql = format!(
        "INSERT INTO {table} (id, team_id, org_id, name, spec, owner_kind, owner_id) \
         VALUES ($1, $2, $3, $4, $5::jsonb, $6, $7)"
    );
    sqlx::query(&sql)
        .bind(uuid::Uuid::now_v7())
        .bind(team.id.as_uuid())
        .bind(team.org_id.as_uuid())
        .bind(name)
        .bind(spec_json)
        .bind(owner_kind)
        .bind(owner_id_for(owner_kind))
        .execute(pool)
        .await
        .unwrap_or_else(|e| panic!("insert into {table} ({name}, {owner_kind}): {e}"));
}

async fn insert_cluster(
    pool: &sqlx::PgPool,
    team: &TeamRef,
    name: &str,
    spec: &ClusterSpec,
    owner_kind: &str,
) {
    let json = serde_json::to_string(spec).expect("serialize cluster spec");
    insert_row(pool, "clusters", team, name, &json, owner_kind).await;
}

async fn insert_route_config(
    pool: &sqlx::PgPool,
    team: &TeamRef,
    name: &str,
    spec: &RouteConfigSpec,
    owner_kind: &str,
) {
    let json = serde_json::to_string(spec).expect("serialize route config spec");
    insert_row(pool, "route_configs", team, name, &json, owner_kind).await;
}

async fn insert_listener(
    pool: &sqlx::PgPool,
    team: &TeamRef,
    name: &str,
    spec: &ListenerSpec,
    owner_kind: &str,
) {
    let json = serde_json::to_string(spec).expect("serialize listener spec");
    insert_row(pool, "listeners", team, name, &json, owner_kind).await;
}

/// The row must still exist, byte-identical owner metadata — the guard withdraws from the
/// snapshot, it never deletes or mutates DB state.
async fn assert_row_untouched(pool: &sqlx::PgPool, table: &str, team: &TeamRef, name: &str) {
    let sql = format!(
        "SELECT COUNT(*)::bigint FROM {table} WHERE team_id = $1 AND name = $2 \
         AND owner_kind = 'user' AND owner_id IS NULL AND version = 1"
    );
    let count: i64 = sqlx::query_scalar(&sql)
        .bind(team.id.as_uuid())
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("re-query {table}/{name}: {e}"));
    assert_eq!(
        count, 1,
        "{table} row '{name}' must survive the rebuild untouched (owner_kind/user, version 1)"
    );
}

async fn served_route_names(cache: &SnapshotCache, team: &TeamRef) -> HashSet<String> {
    let snap = cache.team(team.id).await;
    snap.routes
        .resources
        .iter()
        .map(|any| {
            envoy_types::pb::envoy::config::route::v3::RouteConfiguration::decode(&any.value[..])
                .expect("decode RouteConfiguration")
                .name
        })
        .collect()
}

async fn served_listener_names(cache: &SnapshotCache, team: &TeamRef) -> HashSet<String> {
    let snap = cache.team(team.id).await;
    snap.listeners
        .resources
        .iter()
        .map(|any| {
            envoy_types::pb::envoy::config::listener::v3::Listener::decode(&any.value[..])
                .expect("decode Listener")
                .name
        })
        .collect()
}

async fn served_cluster_names(cache: &SnapshotCache, team: &TeamRef) -> HashSet<String> {
    let snap = cache.team(team.id).await;
    snap.clusters
        .resources
        .iter()
        .map(|any| {
            envoy_types::pb::envoy::config::cluster::v3::Cluster::decode(&any.value[..])
                .expect("decode Cluster")
                .name
        })
        .collect()
}

/// 7a: user route config → ai cluster. The route config is withdrawn from the served
/// routes set, degraded (route type) with an error naming the condition, row untouched.
#[tokio::test]
async fn user_route_config_referencing_ai_cluster_is_withdrawn() {
    let Some((pool, team)) = world().await else {
        return;
    };
    let ai_cluster = unique("ai-cl");
    let user_rc = unique("user-rc");
    insert_cluster(&pool, &team, &ai_cluster, &cluster_spec("10.9.0.1"), "ai").await;
    insert_route_config(
        &pool,
        &team,
        &user_rc,
        &route_config_spec(&ai_cluster),
        "user",
    )
    .await;

    let cache = SnapshotCache::new();
    cache.rebuild_team(&pool, team.id).await.expect("rebuild");

    let routes = served_route_names(&cache, &team).await;
    assert!(
        !routes.contains(&user_rc),
        "user route config '{user_rc}' referencing ai cluster must NOT be served; served: {routes:?}"
    );

    let degraded = cache.degraded(team.id).await;
    let entry = degraded
        .iter()
        .find(|d| d.name == user_rc && d.type_url == ROUTE_TYPE_URL)
        .unwrap_or_else(|| {
            panic!("expected degraded route entry for '{user_rc}', got: {degraded:?}")
        });
    assert!(
        entry.error.contains(&ai_cluster) || entry.error.to_lowercase().contains("owner"),
        "degraded error should name the cross-owner condition (cluster '{ai_cluster}' or owner kind), got: {}",
        entry.error
    );

    assert_row_untouched(&pool, "route_configs", &team, &user_rc).await;
}

/// 7b: user listener → ai route config. The listener is withdrawn from the served
/// listeners set, degraded (listener type), row untouched.
#[tokio::test]
async fn user_listener_referencing_ai_route_config_is_withdrawn() {
    let Some((pool, team)) = world().await else {
        return;
    };
    let ai_cluster = unique("ai-cl");
    let ai_rc = unique("ai-rc");
    let user_listener = unique("user-lst");
    insert_cluster(&pool, &team, &ai_cluster, &cluster_spec("10.9.0.2"), "ai").await;
    insert_route_config(&pool, &team, &ai_rc, &route_config_spec(&ai_cluster), "ai").await;
    insert_listener(
        &pool,
        &team,
        &user_listener,
        &listener_spec(18081, &ai_rc),
        "user",
    )
    .await;

    let cache = SnapshotCache::new();
    cache.rebuild_team(&pool, team.id).await.expect("rebuild");

    let listeners = served_listener_names(&cache, &team).await;
    assert!(
        !listeners.contains(&user_listener),
        "user listener '{user_listener}' referencing ai route config must NOT be served; served: {listeners:?}"
    );

    let degraded = cache.degraded(team.id).await;
    let entry = degraded
        .iter()
        .find(|d| d.name == user_listener && d.type_url == LISTENER_TYPE_URL)
        .unwrap_or_else(|| {
            panic!("expected degraded listener entry for '{user_listener}', got: {degraded:?}")
        });
    assert!(
        entry.error.contains(&ai_rc) || entry.error.to_lowercase().contains("owner"),
        "degraded error should name the cross-owner condition (route config '{ai_rc}' or owner kind), got: {}",
        entry.error
    );

    assert_row_untouched(&pool, "listeners", &team, &user_listener).await;
}

/// 7c TRANSITIVE: user listener → user route config → ai cluster. The route config is
/// withdrawn AND the listener that RDS-references it is also withdrawn (no dangling RDS
/// reference); both appear in the degraded list; both rows untouched.
#[tokio::test]
async fn withdrawal_cascades_from_route_config_to_user_listener() {
    let Some((pool, team)) = world().await else {
        return;
    };
    let ai_cluster = unique("ai-cl");
    let user_rc = unique("user-rc");
    let user_listener = unique("user-lst");
    insert_cluster(&pool, &team, &ai_cluster, &cluster_spec("10.9.0.3"), "ai").await;
    insert_route_config(
        &pool,
        &team,
        &user_rc,
        &route_config_spec(&ai_cluster),
        "user",
    )
    .await;
    insert_listener(
        &pool,
        &team,
        &user_listener,
        &listener_spec(18082, &user_rc),
        "user",
    )
    .await;

    let cache = SnapshotCache::new();
    cache.rebuild_team(&pool, team.id).await.expect("rebuild");

    let routes = served_route_names(&cache, &team).await;
    assert!(
        !routes.contains(&user_rc),
        "route config '{user_rc}' must be withdrawn; served: {routes:?}"
    );
    let listeners = served_listener_names(&cache, &team).await;
    assert!(
        !listeners.contains(&user_listener),
        "listener '{user_listener}' must be withdrawn transitively (its route config was \
         withdrawn — serving it would dangle the RDS reference); served: {listeners:?}"
    );

    let degraded = cache.degraded(team.id).await;
    assert!(
        degraded
            .iter()
            .any(|d| d.name == user_rc && d.type_url == ROUTE_TYPE_URL),
        "expected degraded route entry for '{user_rc}', got: {degraded:?}"
    );
    // Don't over-pin the listener's error text — it may surface as "route config
    // unavailable" rather than a direct cross-owner message.
    assert!(
        degraded
            .iter()
            .any(|d| d.name == user_listener && d.type_url == LISTENER_TYPE_URL),
        "expected degraded listener entry for '{user_listener}', got: {degraded:?}"
    );

    assert_row_untouched(&pool, "route_configs", &team, &user_rc).await;
    assert_row_untouched(&pool, "listeners", &team, &user_listener).await;
}

/// 7d CONTROLS: in one team, a legitimate all-user chain AND an all-ai chain must BOTH
/// keep serving — the guard withdraws cross-owner bindings only, it must not over-skip.
#[tokio::test]
async fn same_kind_chains_keep_serving() {
    let Some((pool, team)) = world().await else {
        return;
    };
    let user_cluster = unique("user-cl");
    let user_rc = unique("user-rc");
    let user_listener = unique("user-lst");
    let ai_cluster = unique("ai-cl");
    let ai_rc = unique("ai-rc");
    let ai_listener = unique("ai-lst");

    insert_cluster(
        &pool,
        &team,
        &user_cluster,
        &cluster_spec("10.9.0.4"),
        "user",
    )
    .await;
    insert_route_config(
        &pool,
        &team,
        &user_rc,
        &route_config_spec(&user_cluster),
        "user",
    )
    .await;
    insert_listener(
        &pool,
        &team,
        &user_listener,
        &listener_spec(18083, &user_rc),
        "user",
    )
    .await;

    insert_cluster(&pool, &team, &ai_cluster, &cluster_spec("10.9.0.5"), "ai").await;
    insert_route_config(&pool, &team, &ai_rc, &route_config_spec(&ai_cluster), "ai").await;
    insert_listener(
        &pool,
        &team,
        &ai_listener,
        &listener_spec(18084, &ai_rc),
        "ai",
    )
    .await;

    let cache = SnapshotCache::new();
    cache.rebuild_team(&pool, team.id).await.expect("rebuild");

    let clusters = served_cluster_names(&cache, &team).await;
    let routes = served_route_names(&cache, &team).await;
    let listeners = served_listener_names(&cache, &team).await;

    for (set, name, what) in [
        (&clusters, &user_cluster, "user cluster"),
        (&clusters, &ai_cluster, "ai cluster"),
        (&routes, &user_rc, "user route config"),
        (&routes, &ai_rc, "ai route config"),
        (&listeners, &user_listener, "user listener"),
        (&listeners, &ai_listener, "ai listener"),
    ] {
        assert!(
            set.contains(name),
            "{what} '{name}' is a legitimate same-kind binding and must be served; set: {set:?}"
        );
    }
    // Exact membership: the team is fresh, so nothing else may appear either.
    assert_eq!(routes.len(), 2, "exactly the two route configs: {routes:?}");
    assert_eq!(
        listeners.len(),
        2,
        "exactly the two listeners: {listeners:?}"
    );

    let degraded = cache.degraded(team.id).await;
    assert!(
        degraded.is_empty(),
        "no degraded entries for legitimate same-kind chains, got: {degraded:?}"
    );
}
