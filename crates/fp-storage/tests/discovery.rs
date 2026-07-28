#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::api_lifecycle::ObservationIngest;
use fp_domain::authz::TeamRef;
use fp_domain::discovery::{DiscoveryObservationProvenance, DiscoverySessionSpec};
use fp_domain::{DiscoverySessionId, ErrorCode, ListenerId};
use fp_storage::repos::{discovery, identity};
use sqlx::types::chrono::Utc;
use sqlx::PgPool;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

struct World {
    pool: PgPool,
    team_a: TeamRef,
    team_b: TeamRef,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let team_a = identity::create_team(&pool, org_a.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org_b.id, &unique("team-b"), "")
        .await
        .expect("team b");

    Some(World {
        pool,
        team_a: TeamRef {
            id: team_a.id,
            org_id: org_a.id,
        },
        team_b: TeamRef {
            id: team_b.id,
            org_id: org_b.id,
        },
    })
}

#[tokio::test]
async fn discovery_observations_persist_payload_and_provenance() {
    let Some(w) = world().await else { return };
    let session_id = DiscoverySessionId::generate();
    let listener_id = ListenerId::generate();
    let mut tx = w.pool.begin().await.expect("tx");
    let session = discovery::create(
        &mut tx,
        w.team_a,
        discovery::DiscoverySessionInsert {
            id: session_id,
            name: &unique("discover"),
            spec: &spec(),
            validated_upstream_ip: "93.184.216.34",
            cluster_name: &unique("cluster"),
            route_config_name: &unique("route-config"),
            listener_name: &unique("listener"),
        },
    )
    .await
    .expect("session");
    discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &observation("req-1", "/v1/items"),
        &provenance(session.id, listener_id, "api-a.example.test"),
    )
    .await
    .expect("ingest");
    discovery::complete(&mut tx, w.team_a.id, &session.id.to_string())
        .await
        .expect("complete");
    let (_, rows) =
        discovery::completed_observations_for_update(&mut tx, w.team_a.id, &session.id.to_string())
            .await
            .expect("observations");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].raw.capture_session_id, None);
    assert_eq!(rows[0].raw.path, "/v1/items");
    assert_eq!(rows[0].provenance.observed_host, "api-a.example.test");

    let err =
        discovery::completed_observations_for_update(&mut tx, w.team_b.id, &session.id.to_string())
            .await
            .expect_err("cross-team session hidden");
    assert_eq!(err.code, ErrorCode::NotFound);
}

fn spec() -> DiscoverySessionSpec {
    DiscoverySessionSpec {
        listener_port: 19080,
        upstream_host: "example.test".into(),
        upstream_port: 443,
        upstream_tls: true,
        target_sample_count: 25,
        max_duration_seconds: Some(60),
        max_bytes: 1024 * 1024,
        max_distinct_paths: 50,
    }
}

fn spec_with(
    target_sample_count: i32,
    max_bytes: i64,
    max_distinct_paths: i32,
) -> DiscoverySessionSpec {
    DiscoverySessionSpec {
        target_sample_count,
        max_bytes,
        max_distinct_paths,
        ..spec()
    }
}

fn body_observation(request_id: &str, path: &str, response_body: &str) -> ObservationIngest {
    let mut o = observation(request_id, path);
    o.response_body = Some(response_body.into());
    o
}

/// Create a discovery session in its own committed transaction and return it with a listener id.
async fn seed_session(
    w: &World,
    team: TeamRef,
    name: &str,
    spec: &DiscoverySessionSpec,
) -> (fp_domain::DiscoverySession, ListenerId) {
    let listener_id = ListenerId::generate();
    let mut tx = w.pool.begin().await.expect("session tx");
    let session = discovery::create(
        &mut tx,
        team,
        discovery::DiscoverySessionInsert {
            id: DiscoverySessionId::generate(),
            name,
            spec,
            validated_upstream_ip: "93.184.216.34",
            cluster_name: &unique("cluster"),
            route_config_name: &unique("route-config"),
            listener_name: &unique("listener"),
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");
    (session, listener_id)
}

/// Ingest one observation in its own committed transaction (mirrors the per-observation tx in
/// crates/fp-xds/src/capture.rs); returns the domain error when ingest is rejected, committing so
/// drop_count persists.
async fn ingest_committed(
    w: &World,
    team: TeamRef,
    input: &ObservationIngest,
    prov: &DiscoveryObservationProvenance,
) -> Result<fp_domain::DiscoveryObservation, fp_domain::DomainError> {
    let mut tx = w.pool.begin().await.expect("ingest tx");
    let result = discovery::ingest_raw_observation(&mut tx, team, input, prov).await;
    tx.commit().await.expect("commit ingest");
    result
}

async fn get_session(
    w: &World,
    team: TeamRef,
    session: &DiscoverySessionId,
) -> fp_domain::DiscoverySession {
    discovery::get(&w.pool, team.id, &session.to_string())
        .await
        .expect("get session")
        .expect("session present")
}

#[tokio::test]
async fn discovery_ingest_bumps_sample_path_and_byte_counters() {
    let Some(w) = world().await else { return };
    let (session, listener) = seed_session(
        &w,
        w.team_a,
        &unique("counters"),
        &spec_with(25, 1024 * 1024, 50),
    )
    .await;
    let prov = provenance(session.id, listener, "api-a.example.test");

    // Two observations on the same path, one on a distinct path → 3 samples, 2 distinct paths.
    ingest_committed(
        &w,
        w.team_a,
        &body_observation("req-1", "/items", "aaaa"),
        &prov,
    )
    .await
    .expect("ingest 1");
    ingest_committed(
        &w,
        w.team_a,
        &body_observation("req-2", "/items", "aaaa"),
        &prov,
    )
    .await
    .expect("ingest 2");
    ingest_committed(
        &w,
        w.team_a,
        &body_observation("req-3", "/orders", "aaaa"),
        &prov,
    )
    .await
    .expect("ingest 3");

    let refreshed = get_session(&w, w.team_a, &session.id).await;
    assert_eq!(
        refreshed.sample_count, 3,
        "one sample per distinct request id"
    );
    assert_eq!(refreshed.path_count, 2, "distinct paths counted once");
    assert_eq!(refreshed.byte_count, 12, "4 response-body bytes per sample");
    assert_eq!(refreshed.drop_count, 0);
    assert_eq!(
        refreshed.status,
        fp_domain::DiscoverySessionStatus::Capturing
    );
}

#[tokio::test]
async fn discovery_ingest_auto_completes_on_target() {
    let Some(w) = world().await else { return };
    let (session, listener) = seed_session(
        &w,
        w.team_a,
        &unique("auto-complete"),
        &spec_with(1, 1024 * 1024, 50),
    )
    .await;
    let prov = provenance(session.id, listener, "api-a.example.test");

    ingest_committed(&w, w.team_a, &observation("req-1", "/done"), &prov)
        .await
        .expect("ingest");

    let refreshed = get_session(&w, w.team_a, &session.id).await;
    assert_eq!(
        refreshed.status,
        fp_domain::DiscoverySessionStatus::Completed
    );
    assert!(refreshed.completed_at.is_some());
    assert_eq!(refreshed.sample_count, 1);
}

#[tokio::test]
async fn discovery_late_body_merges_after_target_completion() {
    let Some(w) = world().await else { return };
    let (session, listener) = seed_session(
        &w,
        w.team_a,
        &unique("late-body"),
        &spec_with(1, 1024 * 1024, 50),
    )
    .await;
    let prov = provenance(session.id, listener, "api-a.example.test");

    // Metadata-only half reaches the target and auto-completes the session.
    ingest_committed(&w, w.team_a, &observation("req-late", "/late"), &prov)
        .await
        .expect("metadata ingest");
    assert_eq!(
        get_session(&w, w.team_a, &session.id).await.status,
        fp_domain::DiscoverySessionStatus::Completed
    );

    // Trailing body half for the SAME request must still merge, not be rejected with NotFound.
    let mut body = observation("req-late", "/late");
    body.metadata_seen = false;
    body.body_seen = true;
    body.response_status = None;
    body.response_body = Some("late-body".into());
    let merged = ingest_committed(&w, w.team_a, &body, &prov)
        .await
        .expect("late body must merge, not NotFound");
    assert!(merged.raw.body_seen);
    assert_eq!(merged.raw.response_body.as_deref(), Some("late-body"));
    assert_eq!(
        merged.raw.response_status,
        Some(200),
        "metadata status preserved"
    );

    let refreshed = get_session(&w, w.team_a, &session.id).await;
    assert_eq!(
        refreshed.status,
        fp_domain::DiscoverySessionStatus::Completed
    );
    assert_eq!(refreshed.sample_count, 1, "merge is not a new sample");
    assert_eq!(refreshed.byte_count, 9, "byte_count reflects merged body");
    assert_eq!(refreshed.drop_count, 0);
}

#[tokio::test]
async fn discovery_new_observation_after_completion_is_rejected_without_drop() {
    let Some(w) = world().await else { return };
    let (session, listener) = seed_session(
        &w,
        w.team_a,
        &unique("post-complete"),
        &spec_with(1, 1024 * 1024, 50),
    )
    .await;
    let prov = provenance(session.id, listener, "api-a.example.test");

    ingest_committed(&w, w.team_a, &observation("req-1", "/one"), &prov)
        .await
        .expect("ingest");

    // A brand-new request on the now-completed session is a conflict (not a quota drop).
    let err = ingest_committed(&w, w.team_a, &observation("req-2", "/two"), &prov)
        .await
        .expect_err("new observation rejected after completion");
    assert_eq!(err.code, ErrorCode::Conflict);

    let refreshed = get_session(&w, w.team_a, &session.id).await;
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(
        refreshed.drop_count, 0,
        "status conflict is not a quota drop"
    );
}

#[tokio::test]
async fn discovery_distinct_path_quota_drops_and_counts() {
    let Some(w) = world().await else { return };
    let (session, listener) = seed_session(
        &w,
        w.team_a,
        &unique("path-quota"),
        &spec_with(25, 1024 * 1024, 1),
    )
    .await;
    let prov = provenance(session.id, listener, "api-a.example.test");

    ingest_committed(&w, w.team_a, &observation("req-1", "/a"), &prov)
        .await
        .expect("first path");
    let err = ingest_committed(&w, w.team_a, &observation("req-2", "/b"), &prov)
        .await
        .expect_err("second distinct path exceeds max_distinct_paths");
    assert_eq!(err.code, ErrorCode::QuotaExceeded);

    let refreshed = get_session(&w, w.team_a, &session.id).await;
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(refreshed.path_count, 1);
    assert_eq!(refreshed.drop_count, 1);
}

#[tokio::test]
async fn discovery_byte_quota_drops_and_counts() {
    let Some(w) = world().await else { return };
    let (session, listener) =
        seed_session(&w, w.team_a, &unique("byte-quota"), &spec_with(25, 5, 50)).await;
    let prov = provenance(session.id, listener, "api-a.example.test");

    ingest_committed(
        &w,
        w.team_a,
        &body_observation("req-1", "/a", "aaaa"),
        &prov,
    )
    .await
    .expect("4 bytes fits under max_bytes 5");
    let err = ingest_committed(
        &w,
        w.team_a,
        &body_observation("req-2", "/b", "bbbbbb"),
        &prov,
    )
    .await
    .expect_err("cumulative bytes exceed max_bytes");
    assert_eq!(err.code, ErrorCode::QuotaExceeded);

    let refreshed = get_session(&w, w.team_a, &session.id).await;
    assert_eq!(refreshed.byte_count, 4, "rejected observation not counted");
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(refreshed.drop_count, 1);
}

#[tokio::test]
async fn discovery_complete_is_idempotent_after_target_auto_completion() {
    // Regression: ingest can auto-complete a session at its target; stop_session then calls
    // complete() to finalize + tear down forwarding resources. complete() must accept an
    // already-completed session (not NotFound) or teardown would be stranded.
    let Some(w) = world().await else { return };
    let (session, listener) = seed_session(
        &w,
        w.team_a,
        &unique("stop-after-auto"),
        &spec_with(1, 1024 * 1024, 50),
    )
    .await;
    let prov = provenance(session.id, listener, "api-a.example.test");

    ingest_committed(&w, w.team_a, &observation("req-1", "/done"), &prov)
        .await
        .expect("ingest auto-completes");
    let auto = get_session(&w, w.team_a, &session.id).await;
    assert_eq!(auto.status, fp_domain::DiscoverySessionStatus::Completed);
    let auto_completed_at = auto
        .completed_at
        .expect("completed_at set on auto-complete");

    // Explicit stop (via complete) on the already-completed session must succeed.
    let mut tx = w.pool.begin().await.expect("complete tx");
    let stopped = discovery::complete(&mut tx, w.team_a.id, &session.id.to_string())
        .await
        .expect("complete must be idempotent on an auto-completed session");
    tx.commit().await.expect("commit complete");
    assert_eq!(stopped.status, fp_domain::DiscoverySessionStatus::Completed);
    assert_eq!(
        stopped.completed_at,
        Some(auto_completed_at),
        "original completed_at preserved, not reset"
    );
}

fn observation(request_id: &str, path: &str) -> ObservationIngest {
    ObservationIngest {
        request_id: request_id.into(),
        method: "GET".into(),
        path: path.into(),
        response_status: Some(200),
        request_headers: serde_json::Map::new(),
        response_headers: serde_json::Map::new(),
        request_body: None,
        response_body: None,
        request_body_truncated: false,
        response_body_truncated: false,
        request_body_bytes: None,
        response_body_bytes: None,
        metadata_seen: true,
        body_seen: false,
        observed_at: Utc::now(),
    }
}

fn provenance(
    session_id: DiscoverySessionId,
    listener_id: ListenerId,
    host: &str,
) -> DiscoveryObservationProvenance {
    DiscoveryObservationProvenance {
        discovery_session_id: session_id,
        discovery_listener_id: listener_id,
        observed_host: host.into(),
        observed_sni: None,
        route_matched: false,
        forwarded_upstream_host: "example.test".into(),
        forwarded_upstream_port: 443,
        forwarded_upstream_ip: "93.184.216.34".into(),
        forwarded_upstream_tls: true,
    }
}
