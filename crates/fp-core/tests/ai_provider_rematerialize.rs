//! fpv2-6mj integration coverage: updating an AI provider re-materializes the
//! ai-owned backend clusters of every route referencing it, in the same
//! transaction, appending one `cluster.upserted` outbox event per affected
//! backend cluster — without touching routes' `status` and while bumping each
//! dependent route's `version` as a conflict signal.
//!
//! Black-box tests: they drive `fp_core::services::ai` and observe only the
//! `clusters` table, the `events` outbox table, and the service read APIs.
//! Unique org/team/provider/route/secret names (uuid suffix) and per-test
//! baseline outbox sequence numbers keep every test parallel-safe against
//! siblings sharing the database.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::ai as ai_svc;
use fp_core::services::secrets::{self as secret_svc, SecretWrite};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::{
    AiProvider, AiProviderKind, AiProviderSpec, AiRoute, AiRouteBackend, AiRouteSpec,
    AiRouteStatus, OrgRole, RequestId, SecretId, SecretSpec,
};
use fp_storage::repos::identity;
use sqlx::PgPool;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

/// Random listener port. Listener port uniqueness is per team
/// (`idx_listeners_team_port`) and every test gets a fresh team, so this only
/// needs to avoid collisions *within* one test — callers needing several
/// routes take consecutive ports from one draw.
fn unique_port() -> u16 {
    let b = uuid::Uuid::now_v7().into_bytes();
    20000 + (u16::from_be_bytes([b[14], b[15]]) % 40000)
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
    let org = identity::create_org(&pool, &unique("org-ai-remat"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team-ai-remat"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &unique("sub"), "admin@example.test", "A")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("membership");
    Some(World {
        pool,
        team: TeamRef {
            id: team.id,
            org_id: org.id,
        },
        admin: PrincipalCtx::User {
            user_id: user,
            platform_admin: false,
            org_selector_required: false,
            org: Some((org.id, OrgRole::Admin)),
            grants: GrantSet::default(),
        },
    })
}

async fn create_secret(w: &World) -> SecretId {
    let name = unique("ai-key");
    secret_svc::create_secret(
        &w.pool,
        &w.admin,
        w.team,
        SecretWrite {
            name: &name,
            description: "",
            spec: SecretSpec::GenericSecret {
                secret: "cmVtYXQtdGVzdA==".into(),
            },
            expires_at: None,
        },
        RequestId::generate(),
    )
    .await
    .expect("create secret")
    .id
}

fn provider_spec(base_url: &str, secret: SecretId) -> AiProviderSpec {
    AiProviderSpec {
        kind: AiProviderKind::OpenaiCompatible,
        base_url: base_url.into(),
        path_prefix: Some("/v1".into()),
        credential_secret_id: secret,
        models: vec!["gpt-5".into()],
        auth_header: "authorization".into(),
    }
}

async fn create_provider(w: &World, prefix: &str, base_url: &str, secret: SecretId) -> AiProvider {
    ai_svc::create_provider(
        &w.pool,
        &w.admin,
        w.team,
        &unique(prefix),
        provider_spec(base_url, secret),
        RequestId::generate(),
        Default::default(),
    )
    .await
    .expect("create provider")
}

fn backend(provider: &AiProvider) -> AiRouteBackend {
    AiRouteBackend {
        provider_id: provider.id,
        models: vec!["gpt-5".into()],
        model_override: None,
        weight: 1,
        priority: 0,
    }
}

async fn create_route(
    w: &World,
    prefix: &str,
    port: u16,
    backends: Vec<AiRouteBackend>,
) -> AiRoute {
    ai_svc::create_route(
        &w.pool,
        &w.admin,
        w.team,
        &unique(prefix),
        AiRouteSpec {
            listener_port: port,
            path: "/v1/chat/completions".into(),
            backends,
        },
        RequestId::generate(),
    )
    .await
    .expect("create route")
}

async fn update_provider(w: &World, provider: &AiProvider, base_url: &str) -> AiProvider {
    ai_svc::update_provider(
        &w.pool,
        &w.admin,
        w.team,
        &provider.name,
        provider_spec(base_url, provider.spec.credential_secret_id),
        provider.version,
        RequestId::generate(),
        Default::default(),
    )
    .await
    .expect("update provider")
}

async fn get_route(w: &World, name: &str) -> AiRoute {
    ai_svc::get_route(&w.pool, &w.admin, w.team, name, RequestId::generate())
        .await
        .expect("get route")
}

/// Fetch `(owner_kind, spec)` for one cluster row of this team, straight from
/// the `clusters` table (the single representation the ACs are written against).
async fn cluster_row(w: &World, name: &str) -> Option<(String, serde_json::Value)> {
    sqlx::query_as::<_, (String, serde_json::Value)>(
        "SELECT owner_kind, spec FROM clusters WHERE team_id = $1 AND name = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(name)
    .fetch_optional(&w.pool)
    .await
    .expect("cluster row query")
}

fn endpoint_host(spec: &serde_json::Value) -> &str {
    spec["endpoints"][0]["host"]
        .as_str()
        .expect("cluster spec endpoint host")
}

fn endpoint_port(spec: &serde_json::Value) -> u64 {
    spec["endpoints"][0]["port"]
        .as_u64()
        .expect("cluster spec endpoint port")
}

/// Among a route's materialized backend clusters, find the one currently
/// pointing at `host` (each backend cluster embeds its provider's base_url host).
async fn backend_cluster_for_host(w: &World, route: &AiRoute, host: &str) -> String {
    for name in &route.materialized.cluster_names {
        let (owner_kind, spec) = cluster_row(w, name)
            .await
            .unwrap_or_else(|| panic!("materialized cluster {name} missing"));
        assert_eq!(owner_kind, "ai", "route-materialized clusters are ai-owned");
        if endpoint_host(&spec) == host {
            return name.clone();
        }
    }
    panic!(
        "no backend cluster of route {} points at host {host} (clusters: {:?})",
        route.name, route.materialized.cluster_names
    );
}

/// Current outbox head — the baseline for "events appended by *this* mutation".
async fn events_head(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM events")
        .fetch_one(pool)
        .await
        .expect("events head")
}

/// `cluster.upserted` outbox events appended after `since` for one named
/// cluster of this team. Scoped by team + name, never a global count.
async fn cluster_upserted_events(w: &World, since: i64, cluster_name: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events \
         WHERE event_type = 'cluster.upserted' AND team_id = $1 AND seq > $2 \
           AND payload->>'name' = $3",
    )
    .bind(w.team.id.as_uuid())
    .bind(since)
    .bind(cluster_name)
    .fetch_one(&w.pool)
    .await
    .expect("cluster.upserted count")
}

/// All `cluster.upserted` events appended after `since` for this team (fresh
/// team per test, so this is still test-scoped, not global).
async fn team_cluster_upserted_events(w: &World, since: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events \
         WHERE event_type = 'cluster.upserted' AND team_id = $1 AND seq > $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(since)
    .fetch_one(&w.pool)
    .await
    .expect("team cluster.upserted count")
}

// AC1: updating a referenced provider's base_url succeeds and, in the same
// request (no route mutation by the caller), the route's backend cluster row
// in `clusters` now carries the new base_url's host/port, use_tls, and SNI.
#[tokio::test]
async fn ac1_provider_base_url_update_rematerializes_dependent_cluster_spec() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let old_host = format!("{}.example", unique("ai-old"));
    let new_host = format!("{}.example", unique("ai-new"));

    let provider = create_provider(&w, "prov", &format!("https://{old_host}:8443"), secret).await;
    let route = create_route(&w, "route", unique_port(), vec![backend(&provider)]).await;
    assert_eq!(
        route.materialized.cluster_names.len(),
        1,
        "one backend materializes one cluster"
    );
    let cluster_name = backend_cluster_for_host(&w, &route, &old_host).await;
    let (_, before) = cluster_row(&w, &cluster_name).await.expect("cluster row");
    assert_eq!(endpoint_host(&before), old_host);
    assert_eq!(endpoint_port(&before), 8443);

    update_provider(&w, &provider, &format!("https://{new_host}:9443")).await;

    let (owner_kind, after) = cluster_row(&w, &cluster_name)
        .await
        .expect("cluster row survives the update");
    assert_eq!(owner_kind, "ai");
    assert_eq!(endpoint_host(&after), new_host, "cluster carries Y's host");
    assert_eq!(endpoint_port(&after), 9443, "cluster carries Y's port");
    assert_eq!(after["use_tls"], serde_json::json!(true), "https keeps TLS");
    assert!(
        !serde_json::to_string(&after)
            .expect("serialize")
            .contains(&old_host),
        "no residue of X's host (e.g. a stale SNI) anywhere in the re-materialized spec"
    );

    // SNI/TLS check without guessing the encoding: re-materialization at Y must
    // produce exactly the spec a *fresh* materialization at Y produces.
    let control_provider =
        create_provider(&w, "prov-ctl", &format!("https://{new_host}:9443"), secret).await;
    let control_route = create_route(
        &w,
        "route-ctl",
        unique_port(),
        vec![backend(&control_provider)],
    )
    .await;
    let control_cluster = backend_cluster_for_host(&w, &control_route, &new_host).await;
    let (_, control_spec) = cluster_row(&w, &control_cluster)
        .await
        .expect("control row");
    assert_eq!(
        after, control_spec,
        "re-materialized cluster spec (incl. SNI/upstream TLS) equals a fresh \
         materialization from a provider at Y"
    );

    // Same request only: the route spec itself is untouched.
    let route_after = get_route(&w, &route.name).await;
    assert_eq!(route_after.spec, route.spec, "route spec is not rewritten");
    assert_eq!(
        route_after.materialized, route.materialized,
        "materialized resource names are stable across provider updates"
    );
}

// AC2: one ClusterUpserted outbox event per affected backend cluster, and none
// for clusters that belong to other providers' backends.
#[tokio::test]
async fn ac2_outbox_gets_cluster_upserted_per_affected_cluster_and_none_for_others() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host_p = format!("{}.example", unique("ai-p"));
    let host_q = format!("{}.example", unique("ai-q"));

    let provider_p = create_provider(&w, "prov-p", &format!("https://{host_p}"), secret).await;
    let provider_q = create_provider(&w, "prov-q", &format!("https://{host_q}"), secret).await;
    let port = unique_port();
    let route_p = create_route(&w, "route-p", port, vec![backend(&provider_p)]).await;
    let route_q = create_route(&w, "route-q", port + 1, vec![backend(&provider_q)]).await;
    let cluster_p = backend_cluster_for_host(&w, &route_p, &host_p).await;
    let cluster_q = backend_cluster_for_host(&w, &route_q, &host_q).await;

    let baseline = events_head(&w.pool).await;
    let new_host = format!("{}.example", unique("ai-p2"));
    update_provider(&w, &provider_p, &format!("https://{new_host}")).await;

    assert_eq!(
        cluster_upserted_events(&w, baseline, &cluster_p).await,
        1,
        "exactly one cluster.upserted for the affected backend cluster"
    );
    assert_eq!(
        cluster_upserted_events(&w, baseline, &cluster_q).await,
        0,
        "no cluster.upserted for the other provider's backend cluster"
    );
}

// AC3: a provider update never touches any route's status — routes stay
// 'active'; nothing writes 'stale'.
#[tokio::test]
async fn ac3_provider_update_leaves_dependent_route_status_active() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host = format!("{}.example", unique("ai-st"));

    let provider = create_provider(&w, "prov-st", &format!("https://{host}"), secret).await;
    let route = create_route(&w, "route-st", unique_port(), vec![backend(&provider)]).await;
    assert_eq!(route.status, AiRouteStatus::Active);

    let new_host = format!("{}.example", unique("ai-st2"));
    update_provider(&w, &provider, &format!("https://{new_host}")).await;

    let route_after = get_route(&w, &route.name).await;
    assert_eq!(
        route_after.status,
        AiRouteStatus::Active,
        "route status stays active across a provider update"
    );
    let raw_status: String =
        sqlx::query_scalar("SELECT status FROM ai_routes WHERE team_id = $1 AND name = $2")
            .bind(w.team.id.as_uuid())
            .bind(&route.name)
            .fetch_one(&w.pool)
            .await
            .expect("route status column");
    assert_eq!(raw_status, "active", "nothing writes 'stale' to the row");
}

// AC5: if a dependent cluster row is missing (simulated corruption), the whole
// provider update fails atomically — provider row unchanged, no partial
// re-materialization, no outbox residue, no route version bump.
#[tokio::test]
async fn ac5_missing_dependent_cluster_row_fails_the_whole_update_atomically() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let old_host = format!("{}.example", unique("ai-gone"));

    let provider = create_provider(&w, "prov-gone", &format!("https://{old_host}"), secret).await;
    let route = create_route(&w, "route-gone", unique_port(), vec![backend(&provider)]).await;
    let cluster_name = backend_cluster_for_host(&w, &route, &old_host).await;

    // Simulate corruption: the dependent ai-owned cluster row is unreachable under its
    // materialized name. (A raw DELETE is blocked by the route_config_cluster_refs FK, so
    // rename the row instead — same effect for the lookup-by-name path under test.)
    let corrupted = sqlx::query(
        "UPDATE clusters SET name = 'corrupt-' || name WHERE team_id = $1 AND name = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(&cluster_name)
    .execute(&w.pool)
    .await
    .expect("rename cluster row")
    .rows_affected();
    assert_eq!(corrupted, 1);

    let baseline = events_head(&w.pool).await;
    let new_host = format!("{}.example", unique("ai-gone2"));
    ai_svc::update_provider(
        &w.pool,
        &w.admin,
        w.team,
        &provider.name,
        provider_spec(&format!("https://{new_host}"), secret),
        provider.version,
        RequestId::generate(),
        Default::default(),
    )
    .await
    .expect_err("update must fail when a dependent cluster row is missing");

    let provider_after = ai_svc::get_provider(
        &w.pool,
        &w.admin,
        w.team,
        &provider.name,
        RequestId::generate(),
    )
    .await
    .expect("provider still readable");
    assert_eq!(
        provider_after.spec.base_url,
        format!("https://{old_host}"),
        "provider base_url unchanged after failed update"
    );
    assert_eq!(
        provider_after.version, provider.version,
        "provider version unchanged after failed update"
    );
    assert_eq!(
        get_route(&w, &route.name).await.version,
        route.version,
        "route version not bumped by a failed update"
    );
    assert_eq!(
        team_cluster_upserted_events(&w, baseline).await,
        0,
        "no cluster.upserted residue from the rolled-back transaction"
    );
}

// AC6: a provider with zero dependent routes updates exactly as before —
// success, new spec visible, and no cluster events appended.
#[tokio::test]
async fn ac6_provider_update_with_zero_dependent_routes_succeeds() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let old_host = format!("{}.example", unique("ai-solo"));
    let new_host = format!("{}.example", unique("ai-solo2"));

    let provider = create_provider(&w, "prov-solo", &format!("https://{old_host}"), secret).await;
    let baseline = events_head(&w.pool).await;

    let updated = update_provider(&w, &provider, &format!("https://{new_host}")).await;

    assert_eq!(updated.spec.base_url, format!("https://{new_host}"));
    assert!(
        updated.version > provider.version,
        "successful update advances the provider version"
    );
    let fetched = ai_svc::get_provider(
        &w.pool,
        &w.admin,
        w.team,
        &provider.name,
        RequestId::generate(),
    )
    .await
    .expect("get provider");
    assert_eq!(fetched.spec.base_url, format!("https://{new_host}"));
    assert_eq!(
        team_cluster_upserted_events(&w, baseline).await,
        0,
        "no dependent routes, so no cluster.upserted events"
    );
}

// AC7: multi-fan-out — one provider behind two routes, one route with mixed
// backends. Exactly the matching backend clusters are re-materialized (and
// evented); the other provider's cluster is untouched.
#[tokio::test]
async fn ac7_fan_out_updates_exactly_matching_backend_clusters_across_routes() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host_p = format!("{}.example", unique("ai-fp"));
    let host_q = format!("{}.example", unique("ai-fq"));
    let new_host = format!("{}.example", unique("ai-fp2"));

    let provider_p = create_provider(&w, "prov-fp", &format!("https://{host_p}"), secret).await;
    let provider_q = create_provider(&w, "prov-fq", &format!("https://{host_q}"), secret).await;
    let port = unique_port();
    let route_one = create_route(&w, "route-f1", port, vec![backend(&provider_p)]).await;
    let route_two = create_route(
        &w,
        "route-f2",
        port + 1,
        vec![backend(&provider_p), backend(&provider_q)],
    )
    .await;
    assert_eq!(route_two.materialized.cluster_names.len(), 2);

    let c_one_p = backend_cluster_for_host(&w, &route_one, &host_p).await;
    let c_two_p = backend_cluster_for_host(&w, &route_two, &host_p).await;
    let c_two_q = backend_cluster_for_host(&w, &route_two, &host_q).await;
    let (_, q_spec_before) = cluster_row(&w, &c_two_q).await.expect("q cluster row");

    let baseline = events_head(&w.pool).await;
    update_provider(&w, &provider_p, &format!("https://{new_host}")).await;

    // Both of P's backend clusters (across both routes) now point at the new host.
    for name in [&c_one_p, &c_two_p] {
        let (_, spec) = cluster_row(&w, name).await.expect("cluster row");
        assert_eq!(
            endpoint_host(&spec),
            new_host,
            "backend cluster {name} re-materialized to the new host"
        );
        assert_eq!(
            cluster_upserted_events(&w, baseline, name).await,
            1,
            "exactly one cluster.upserted for {name}"
        );
    }

    // Q's backend cluster in the mixed route is untouched, byte for byte.
    let (_, q_spec_after) = cluster_row(&w, &c_two_q).await.expect("q cluster row");
    assert_eq!(
        q_spec_after, q_spec_before,
        "other provider's backend cluster spec is untouched"
    );
    assert_eq!(
        cluster_upserted_events(&w, baseline, &c_two_q).await,
        0,
        "no cluster.upserted for the other provider's backend cluster"
    );

    // Both referencing routes get the +1 conflict-signal version bump.
    assert_eq!(
        get_route(&w, &route_one.name).await.version,
        route_one.version + 1
    );
    assert_eq!(
        get_route(&w, &route_two.name).await.version,
        route_two.version + 1
    );
}

// Version-bump AC: dependent routes' version is bumped by exactly +1 per
// provider update; routes not referencing the provider keep their version.
#[tokio::test]
async fn provider_update_bumps_only_referencing_route_versions_by_one() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host_p = format!("{}.example", unique("ai-vp"));
    let host_q = format!("{}.example", unique("ai-vq"));

    let provider_p = create_provider(&w, "prov-vp", &format!("https://{host_p}"), secret).await;
    let provider_q = create_provider(&w, "prov-vq", &format!("https://{host_q}"), secret).await;
    let port = unique_port();
    let route_p = create_route(&w, "route-vp", port, vec![backend(&provider_p)]).await;
    let route_q = create_route(&w, "route-vq", port + 1, vec![backend(&provider_q)]).await;

    let first_host = format!("{}.example", unique("ai-vp1"));
    let updated = update_provider(&w, &provider_p, &format!("https://{first_host}")).await;

    assert_eq!(
        get_route(&w, &route_p.name).await.version,
        route_p.version + 1,
        "referencing route version bumped by exactly +1"
    );
    assert_eq!(
        get_route(&w, &route_q.name).await.version,
        route_q.version,
        "non-referencing route version untouched"
    );

    // A second update bumps by exactly one more (per-update, not cumulative drift).
    let new_host = format!("{}.example", unique("ai-vp2"));
    update_provider(&w, &updated, &format!("https://{new_host}")).await;
    assert_eq!(
        get_route(&w, &route_p.name).await.version,
        route_p.version + 2,
        "each provider update bumps the referencing route by +1"
    );
    assert_eq!(
        get_route(&w, &route_q.name).await.version,
        route_q.version,
        "non-referencing route still untouched after a second update"
    );
}

/// Coverage hardening (Codex S1 review minor): the same provider occupying
/// MULTIPLE backend positions of one route must yield one rewrite + one event
/// per position, and the dedup in dependency discovery must not double-process
/// the route (which would double-emit events).
#[tokio::test]
async fn same_provider_in_multiple_backend_positions_rewrites_each_position_once() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let old_host = format!("{}.example", unique("ai-multi"));

    let provider = create_provider(&w, "prov-multi", &format!("https://{old_host}"), secret).await;
    let route = create_route(
        &w,
        "route-multi",
        unique_port(),
        vec![backend(&provider), backend(&provider)],
    )
    .await;
    assert!(
        route.materialized.cluster_names.len() >= 2,
        "two backends materialize two backend clusters"
    );

    let baseline = events_head(&w.pool).await;
    let new_host = format!("{}.example", unique("ai-multi2"));
    update_provider(&w, &provider, &format!("https://{new_host}")).await;

    for name in &route.materialized.cluster_names[..2] {
        let (_, spec) = cluster_row(&w, name).await.expect("backend cluster row");
        assert_eq!(endpoint_host(&spec), new_host, "position {name} rewritten");
        assert_eq!(
            cluster_upserted_events(&w, baseline, name).await,
            1,
            "exactly one cluster.upserted for {name} — no double-processing"
        );
    }
    assert_eq!(
        team_cluster_upserted_events(&w, baseline).await,
        2,
        "exactly two events total for the two backend positions"
    );
    assert_eq!(
        get_route(&w, &route.name).await.version,
        route.version + 1,
        "route version bumped exactly once despite two matching positions"
    );
}
