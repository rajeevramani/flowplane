//! fpv2-8am integration coverage: AI route mutations (create/update/delete) are
//! single-transaction and serialized per team. A failure anywhere — including a
//! listener port collision during re-materialization — rolls back EVERYTHING:
//! no partial teardown, no orphan rows, no outbox residue. Provider updates
//! racing dependent route updates converge deterministically.
//!
//! Black-box tests: they drive `fp_core::services::ai` and observe only the
//! `ai_routes` / `clusters` / `listeners` / `route_configs` tables, the
//! `events` outbox table, and the service read APIs. Unique org/team/resource
//! names (uuid suffix) and per-test baseline outbox sequence numbers keep every
//! test parallel-safe against siblings sharing the database.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use fp_core::services::ai as ai_svc;
use fp_core::services::secrets::{self as secret_svc, SecretWrite};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::{
    AiProvider, AiProviderKind, AiProviderSpec, AiRoute, AiRouteBackend, AiRouteSpec, ErrorCode,
    OrgRole, RequestId, SecretId, SecretSpec,
};
use fp_storage::repos::identity;
use sqlx::PgPool;
use tokio::sync::Barrier;

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
    let org = identity::create_org(&pool, &unique("org-ai-1tx"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team-ai-1tx"), "")
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
                secret: "c2luZ2xldHgtdGVzdA==".into(),
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

fn route_spec(port: u16, backends: Vec<AiRouteBackend>) -> AiRouteSpec {
    AiRouteSpec {
        listener_port: port,
        path: "/v1/chat/completions".into(),
        backends,
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
        route_spec(port, backends),
        RequestId::generate(),
    )
    .await
    .expect("create route")
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

async fn listener_row_exists(w: &World, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM listeners WHERE team_id = $1 AND name = $2")
        .bind(w.team.id.as_uuid())
        .bind(name)
        .fetch_one(&w.pool)
        .await
        .expect("listener row query")
        > 0
}

async fn route_config_row_exists(w: &World, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM route_configs WHERE team_id = $1 AND name = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(name)
    .fetch_one(&w.pool)
    .await
    .expect("route_config row query")
        > 0
}

async fn ai_route_row_exists(w: &World, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ai_routes WHERE team_id = $1 AND name = $2")
        .bind(w.team.id.as_uuid())
        .bind(name)
        .fetch_one(&w.pool)
        .await
        .expect("ai_routes row query")
        > 0
}

/// Count of this team's ai-owned cluster rows (fresh team per test, so this is
/// test-scoped, not global).
async fn team_ai_cluster_count(w: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM clusters WHERE team_id = $1 AND owner_kind = 'ai'")
        .bind(w.team.id.as_uuid())
        .fetch_one(&w.pool)
        .await
        .expect("team ai cluster count")
}

/// Current outbox head — the baseline for "events appended by *this* mutation".
async fn events_head(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM events")
        .fetch_one(pool)
        .await
        .expect("events head")
}

/// Outbox events of one type appended after `since` for this team, optionally
/// narrowed to one resource name (payloads all carry a `name` field).
async fn team_events(w: &World, since: i64, event_type: &str, name: Option<&str>) -> i64 {
    match name {
        Some(name) => sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE event_type = $1 AND team_id = $2 AND seq > $3 AND payload->>'name' = $4",
        )
        .bind(event_type)
        .bind(w.team.id.as_uuid())
        .bind(since)
        .bind(name)
        .fetch_one(&w.pool)
        .await
        .expect("named event count"),
        None => sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE event_type = $1 AND team_id = $2 AND seq > $3",
        )
        .bind(event_type)
        .bind(w.team.id.as_uuid())
        .bind(since)
        .fetch_one(&w.pool)
        .await
        .expect("event count"),
    }
}

const DELETION_EVENTS: [&str; 3] = [
    "cluster.deleted",
    "route_config.deleted",
    "listener.deleted",
];
const UPSERT_EVENTS: [&str; 3] = [
    "cluster.upserted",
    "route_config.upserted",
    "listener.upserted",
];

// AC1: all-or-nothing UPDATE. Updating R1 onto R2's listener port fails on the
// per-team (team_id, port) uniqueness, and the failure rolls back EVERYTHING:
// R1's route row keeps its old spec/port/version, all of R1's original
// materialized resources still exist (no transient teardown was committed),
// and no deletion events reached the outbox.
#[tokio::test]
async fn ac1_failed_route_update_rolls_back_everything() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host = format!("{}.example", unique("ai-u"));

    let provider = create_provider(&w, "prov-u", &format!("https://{host}"), secret).await;
    let p1 = unique_port();
    let p2 = p1 + 1;
    let r1 = create_route(&w, "route-u1", p1, vec![backend(&provider)]).await;
    let r2 = create_route(&w, "route-u2", p2, vec![backend(&provider)]).await;

    let baseline = events_head(&w.pool).await;
    let err = ai_svc::update_route(
        &w.pool,
        &w.admin,
        w.team,
        &r1.name,
        route_spec(p2, vec![backend(&provider)]),
        r1.version,
        RequestId::generate(),
    )
    .await
    .expect_err("moving R1 onto R2's listener port must fail (port collision)");
    eprintln!("port-collision update failed as expected: {err}");

    // R1's route row is untouched: old spec (old port), old version, old
    // materialized names.
    let r1_after = get_route(&w, &r1.name).await;
    assert_eq!(r1_after.spec, r1.spec, "R1 keeps its old spec (port {p1})");
    assert_eq!(r1_after.version, r1.version, "R1 keeps its old version");
    assert_eq!(
        r1_after.materialized, r1.materialized,
        "R1 keeps its original materialized resource names"
    );

    // All of R1's ORIGINAL materialized resources still exist — nothing was
    // transiently torn down and committed.
    for name in &r1.materialized.cluster_names {
        let (owner_kind, _) = cluster_row(&w, name)
            .await
            .unwrap_or_else(|| panic!("R1 backend cluster {name} must survive the failed update"));
        assert_eq!(owner_kind, "ai");
    }
    assert!(
        listener_row_exists(&w, &r1.materialized.listener_name).await,
        "R1's listener row must survive the failed update"
    );
    assert!(
        route_config_row_exists(&w, &r1.materialized.route_config_name).await,
        "R1's route config row must survive the failed update"
    );

    // No deletion events for R1's resource names were committed to the outbox.
    for (event_type, name) in [
        ("listener.deleted", r1.materialized.listener_name.as_str()),
        (
            "route_config.deleted",
            r1.materialized.route_config_name.as_str(),
        ),
    ] {
        assert_eq!(
            team_events(&w, baseline, event_type, Some(name)).await,
            0,
            "no {event_type} for {name} from the rolled-back update"
        );
    }
    for name in &r1.materialized.cluster_names {
        assert_eq!(
            team_events(&w, baseline, "cluster.deleted", Some(name)).await,
            0,
            "no cluster.deleted for {name} from the rolled-back update"
        );
    }
    // Belt-and-braces: nothing (deletion OR upsert) leaked team-wide either.
    for event_type in DELETION_EVENTS.iter().chain(UPSERT_EVENTS.iter()) {
        assert_eq!(
            team_events(&w, baseline, event_type, None).await,
            0,
            "no {event_type} residue at all from the rolled-back update"
        );
    }

    // R2 is a bystander: untouched.
    let r2_after = get_route(&w, &r2.name).await;
    assert_eq!(r2_after.spec, r2.spec);
    assert_eq!(r2_after.version, r2.version);
}

// AC2: all-or-nothing CREATE. Creating a route on a port already used by an
// existing route in the same team fails, leaving no route row, no orphan
// ai-owned cluster rows, and no upsert events in the outbox.
#[tokio::test]
async fn ac2_failed_route_create_leaves_no_orphans() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host = format!("{}.example", unique("ai-c"));

    let provider = create_provider(&w, "prov-c", &format!("https://{host}"), secret).await;
    let port = unique_port();
    let existing = create_route(&w, "route-c1", port, vec![backend(&provider)]).await;
    let clusters_before = team_ai_cluster_count(&w).await;

    let baseline = events_head(&w.pool).await;
    let doomed_name = unique("route-c2");
    let err = ai_svc::create_route(
        &w.pool,
        &w.admin,
        w.team,
        &doomed_name,
        route_spec(port, vec![backend(&provider)]),
        RequestId::generate(),
    )
    .await
    .expect_err("creating a second route on the same team port must fail");
    eprintln!("port-collision create failed as expected: {err}");

    assert!(
        !ai_route_row_exists(&w, &doomed_name).await,
        "no route row for the failed create"
    );
    assert_eq!(
        team_ai_cluster_count(&w).await,
        clusters_before,
        "no orphan ai-owned cluster rows from the failed create"
    );
    // The existing route's materialization is intact.
    for name in &existing.materialized.cluster_names {
        assert!(
            cluster_row(&w, name).await.is_some(),
            "existing route's cluster {name} untouched"
        );
    }
    assert!(listener_row_exists(&w, &existing.materialized.listener_name).await);
    // No upsert (or deletion) events for the doomed route's would-be resources
    // reached the outbox — team-scoped, so this covers every would-be name.
    for event_type in UPSERT_EVENTS.iter().chain(DELETION_EVENTS.iter()) {
        assert_eq!(
            team_events(&w, baseline, event_type, None).await,
            0,
            "no {event_type} residue from the rolled-back create"
        );
    }
}

// AC3: delete still works and is atomic — the route row and ALL its
// materialized rows disappear together, and the deletion events for its
// resource names are committed to the outbox.
#[tokio::test]
async fn ac3_delete_route_removes_rows_and_emits_deletion_events() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host = format!("{}.example", unique("ai-d"));

    let provider = create_provider(&w, "prov-d", &format!("https://{host}"), secret).await;
    let route = create_route(&w, "route-d", unique_port(), vec![backend(&provider)]).await;

    let baseline = events_head(&w.pool).await;
    ai_svc::delete_route(
        &w.pool,
        &w.admin,
        w.team,
        &route.name,
        route.version,
        RequestId::generate(),
    )
    .await
    .expect("delete route");

    assert!(
        !ai_route_row_exists(&w, &route.name).await,
        "route row gone after delete"
    );
    for name in &route.materialized.cluster_names {
        assert!(
            cluster_row(&w, name).await.is_none(),
            "backend cluster {name} gone after delete"
        );
        assert_eq!(
            team_events(&w, baseline, "cluster.deleted", Some(name)).await,
            1,
            "exactly one cluster.deleted for {name}"
        );
    }
    assert!(
        !listener_row_exists(&w, &route.materialized.listener_name).await,
        "listener row gone after delete"
    );
    assert!(
        !route_config_row_exists(&w, &route.materialized.route_config_name).await,
        "route config row gone after delete"
    );
    assert_eq!(
        team_events(
            &w,
            baseline,
            "listener.deleted",
            Some(&route.materialized.listener_name)
        )
        .await,
        1,
        "exactly one listener.deleted for the route's listener"
    );
    assert_eq!(
        team_events(
            &w,
            baseline,
            "route_config.deleted",
            Some(&route.materialized.route_config_name)
        )
        .await,
        1,
        "exactly one route_config.deleted for the route's route config"
    );
}

// AC4 (design AC4/AC10): a provider update racing a dependent route update
// converges deterministically under the per-team serialization. Every
// iteration: the provider update succeeds outright; the route update either
// succeeds or fails with exactly RevisionMismatch; and after both settle every
// backend cluster row points at exactly the FINAL committed provider host.
#[tokio::test]
async fn ac4_provider_update_racing_route_update_converges() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let base_port = unique_port();

    const ITERATIONS: u16 = 10;
    for i in 0..ITERATIONS {
        let host_x = format!("{}.example", unique("ai-rx"));
        let host_y = format!("{}.example", unique("ai-ry"));
        let provider = create_provider(&w, "prov-race", &format!("https://{host_x}"), secret).await;
        let port = base_port + i;
        let route = create_route(&w, "route-race", port, vec![backend(&provider)]).await;
        let v = route.version;

        let barrier = Arc::new(Barrier::new(2));

        // Task A: provider base_url X -> Y.
        let a = {
            let pool = w.pool.clone();
            let admin = w.admin.clone();
            let team = w.team;
            let provider_name = provider.name.clone();
            let provider_version = provider.version;
            let spec = provider_spec(&format!("https://{host_y}"), secret);
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                ai_svc::update_provider(
                    &pool,
                    &admin,
                    team,
                    &provider_name,
                    spec,
                    provider_version,
                    RequestId::generate(),
                    Default::default(),
                )
                .await
            })
        };

        // Task B: route update with the pre-race version (same spec).
        let b = {
            let pool = w.pool.clone();
            let admin = w.admin.clone();
            let team = w.team;
            let route_name = route.name.clone();
            let spec = route.spec.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                ai_svc::update_route(
                    &pool,
                    &admin,
                    team,
                    &route_name,
                    spec,
                    v,
                    RequestId::generate(),
                )
                .await
            })
        };

        let (a_result, b_result) = (a.await.expect("task A join"), b.await.expect("task B join"));

        // (a) the provider update must succeed outright — no retry expected.
        let updated = a_result
            .unwrap_or_else(|e| panic!("iteration {i}: provider update must succeed, got: {e}"));
        assert_eq!(updated.spec.base_url, format!("https://{host_y}"));

        // (b) the route update either succeeds or fails with exactly RevisionMismatch.
        if let Err(e) = &b_result {
            assert_eq!(
                e.code,
                ErrorCode::RevisionMismatch,
                "iteration {i}: route update may only fail with RevisionMismatch, got: {e}"
            );
        }

        // (c)+(d) after both settle: read the FINAL provider row and require
        // every backend cluster row of R to point at exactly that host.
        let provider_after = ai_svc::get_provider(
            &w.pool,
            &w.admin,
            w.team,
            &provider.name,
            RequestId::generate(),
        )
        .await
        .expect("get provider after race");
        let final_host = provider_after
            .spec
            .base_url
            .strip_prefix("https://")
            .expect("https base_url");
        assert_eq!(
            final_host, host_y,
            "iteration {i}: the final provider host is Y (A committed)"
        );

        let route_after = get_route(&w, &route.name).await;
        for name in &route_after.materialized.cluster_names {
            let (owner_kind, spec) = cluster_row(&w, name)
                .await
                .unwrap_or_else(|| panic!("iteration {i}: cluster {name} missing after race"));
            assert_eq!(owner_kind, "ai");
            assert_eq!(
                endpoint_host(&spec),
                final_host,
                "iteration {i}: cluster {name} points at exactly the final provider host \
                 (route update {})",
                if b_result.is_ok() {
                    "won"
                } else {
                    "lost with RevisionMismatch"
                }
            );
        }
    }
}

// AC5 (design AC9): cross-team smoke — the per-team serialization does not
// couple different teams; two teams' route creates run concurrently and both
// succeed. No timing assertions.
#[tokio::test]
async fn ac5_cross_team_route_creates_both_succeed() {
    let Some(w1) = world().await else { return };
    let Some(w2) = world().await else { return };

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for w in [&w1, &w2] {
        let secret = create_secret(w).await;
        let host = format!("{}.example", unique("ai-x"));
        let provider = create_provider(w, "prov-x", &format!("https://{host}"), secret).await;
        let pool = w.pool.clone();
        let admin = w.admin.clone();
        let team = w.team;
        let spec = route_spec(unique_port(), vec![backend(&provider)]);
        let name = unique("route-x");
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            ai_svc::create_route(&pool, &admin, team, &name, spec, RequestId::generate()).await
        }));
    }

    for (idx, handle) in handles.into_iter().enumerate() {
        let route = handle
            .await
            .expect("join")
            .unwrap_or_else(|e| panic!("team {idx} concurrent create must succeed: {e}"));
        assert!(!route.materialized.cluster_names.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Deterministic lock-contention tests (Codex S2 review): instead of racing on
// wall-clock, a dedicated transaction HOLDS the team's advisory lock while a
// mutation is spawned; pg_locks is polled until that mutation is provably
// QUEUED on the lock (granted = false), then the holder releases and the
// outcome of the forced ordering is asserted. PostgreSQL grants advisory-lock
// waiters FIFO, so start order = grant order.
// ---------------------------------------------------------------------------

/// Take the team's AI-materialization advisory lock on a dedicated tx.
async fn hold_team_lock(
    pool: &PgPool,
    team_id: fp_domain::TeamId,
) -> sqlx::Transaction<'_, sqlx::Postgres> {
    let key = fp_storage::repos::ai::ai_materialization_lock_key(team_id);
    let mut tx = pool.begin().await.expect("lock holder tx");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *tx)
        .await
        .expect("hold advisory lock");
    tx
}

/// Wait (bounded) until exactly `n` sessions are queued on the team's key.
async fn wait_for_lock_waiters(pool: &PgPool, team_id: fp_domain::TeamId, n: i64) {
    let key = fp_storage::repos::ai::ai_materialization_lock_key(team_id);
    let classid = (key >> 32) as i32;
    let objid = (key & 0xFFFF_FFFF) as i32;
    for _ in 0..200 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks \
             WHERE locktype = 'advisory' AND granted = false \
               AND classid = $1::oid AND objid = $2::oid \
               AND objsubid = 1 AND database = (SELECT oid FROM pg_database \
                                                WHERE datname = current_database())",
        )
        .bind(classid)
        .bind(objid)
        .fetch_one(pool)
        .await
        .expect("pg_locks query");
        if waiting >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("mutation never queued on the team advisory lock (expected {n} waiter[s])");
}

/// Forced ordering 1 — provider update commits BEFORE the route update gets
/// the lock: the route writer's pre-race revision is stale and it must fail
/// with exactly RevisionMismatch (design AC4), while clusters carry the new
/// provider host.
#[tokio::test]
async fn deterministic_provider_first_forces_route_revision_mismatch() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host_x = format!("{}.example", unique("ai-dx"));
    let host_y = format!("{}.example", unique("ai-dy"));
    let provider = create_provider(&w, "prov-det1", &format!("https://{host_x}"), secret).await;
    let route = create_route(&w, "route-det1", unique_port(), vec![backend(&provider)]).await;

    let holder = hold_team_lock(&w.pool, w.team.id).await;

    // Queue the provider update FIRST (FIFO -> it wins the lock on release).
    let a = {
        let (pool, admin, team) = (w.pool.clone(), w.admin.clone(), w.team);
        let name = provider.name.clone();
        let spec = provider_spec(&format!("https://{host_y}"), secret);
        let version = provider.version;
        tokio::spawn(async move {
            ai_svc::update_provider(
                &pool,
                &admin,
                team,
                &name,
                spec,
                version,
                RequestId::generate(),
                Default::default(),
            )
            .await
        })
    };
    wait_for_lock_waiters(&w.pool, w.team.id, 1).await;

    // Queue the route update SECOND with the pre-race revision.
    let b = {
        let (pool, admin, team) = (w.pool.clone(), w.admin.clone(), w.team);
        let name = route.name.clone();
        let spec = route.spec.clone();
        let version = route.version;
        tokio::spawn(async move {
            ai_svc::update_route(
                &pool,
                &admin,
                team,
                &name,
                spec,
                version,
                RequestId::generate(),
            )
            .await
        })
    };
    wait_for_lock_waiters(&w.pool, w.team.id, 2).await;

    drop(holder); // release: A gets the lock, then B.

    a.await
        .expect("join a")
        .expect("provider update must succeed");
    let route_err = b
        .await
        .expect("join b")
        .expect_err("route update queued behind the provider bump must see a stale revision");
    assert_eq!(
        route_err.code,
        ErrorCode::RevisionMismatch,
        "forced provider-first ordering must yield RevisionMismatch, got: {route_err}"
    );

    for name in &route.materialized.cluster_names {
        let (_, spec) = cluster_row(&w, name).await.expect("cluster row");
        assert_eq!(
            endpoint_host(&spec),
            host_y,
            "cluster {name} carries the new provider host"
        );
    }
}

/// Forced ordering 2 — route update commits BEFORE the provider update gets
/// the lock: both succeed, and the provider update re-materializes the route's
/// REBUILT clusters, so the final state still converges on the new host.
#[tokio::test]
async fn deterministic_route_first_lets_both_succeed_and_converge() {
    let Some(w) = world().await else { return };
    let secret = create_secret(&w).await;
    let host_x = format!("{}.example", unique("ai-ex"));
    let host_y = format!("{}.example", unique("ai-ey"));
    let provider = create_provider(&w, "prov-det2", &format!("https://{host_x}"), secret).await;
    let route = create_route(&w, "route-det2", unique_port(), vec![backend(&provider)]).await;

    let holder = hold_team_lock(&w.pool, w.team.id).await;

    // Queue the route update FIRST.
    let b = {
        let (pool, admin, team) = (w.pool.clone(), w.admin.clone(), w.team);
        let name = route.name.clone();
        let spec = route.spec.clone();
        let version = route.version;
        tokio::spawn(async move {
            ai_svc::update_route(
                &pool,
                &admin,
                team,
                &name,
                spec,
                version,
                RequestId::generate(),
            )
            .await
        })
    };
    wait_for_lock_waiters(&w.pool, w.team.id, 1).await;

    // Queue the provider update SECOND.
    let a = {
        let (pool, admin, team) = (w.pool.clone(), w.admin.clone(), w.team);
        let name = provider.name.clone();
        let spec = provider_spec(&format!("https://{host_y}"), secret);
        let version = provider.version;
        tokio::spawn(async move {
            ai_svc::update_provider(
                &pool,
                &admin,
                team,
                &name,
                spec,
                version,
                RequestId::generate(),
                Default::default(),
            )
            .await
        })
    };
    wait_for_lock_waiters(&w.pool, w.team.id, 2).await;

    drop(holder); // release: B commits first, then A re-materializes B's rebuild.

    let updated_route = b
        .await
        .expect("join b")
        .expect("route update queued first must succeed");
    a.await
        .expect("join a")
        .expect("provider update must succeed");

    for name in &updated_route.materialized.cluster_names {
        let (_, spec) = cluster_row(&w, name).await.expect("cluster row");
        assert_eq!(
            endpoint_host(&spec),
            host_y,
            "cluster {name} converges on the final host"
        );
    }
}

/// Cross-team NON-serialization, deterministically (design AC9): while team
/// A's materialization lock is HELD, a different team's route create must
/// complete — if the lock were global, this would deadlock/time out.
#[tokio::test]
async fn deterministic_other_team_mutations_ignore_held_lock() {
    let Some(w_a) = world().await else { return };
    let Some(w_b) = world().await else { return };

    let holder = hold_team_lock(&w_a.pool, w_a.team.id).await;

    let secret_b = create_secret(&w_b).await;
    let provider_b = create_provider(&w_b, "prov-xteam", "https://xteam.example", secret_b).await;
    let route_b = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        create_route(
            &w_b,
            "route-xteam",
            unique_port(),
            vec![backend(&provider_b)],
        ),
    )
    .await
    .expect("team B's create_route must not serialize behind team A's held lock");
    assert!(!route_b.materialized.cluster_names.is_empty());

    drop(holder);
}
