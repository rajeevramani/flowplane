#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::api_lifecycle::ObservationIngest;
use fp_domain::authz::TeamRef;
use fp_domain::discovery::{
    DiscoveryObservationProvenance, DiscoverySessionSpec, DiscoverySessionStatus,
};
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

#[tokio::test]
async fn discovery_ingest_redacts_headers_and_updates_counters() {
    let Some(w) = world().await else { return };
    let session_id = DiscoverySessionId::generate();
    let listener_id = ListenerId::generate();
    let mut tx = w.pool.begin().await.expect("tx");
    let session = create_session(&mut tx, w.team_a, session_id, spec())
        .await
        .expect("session");
    tx.commit().await.expect("commit session");

    let mut input = observation("req-redact", "/v1/items");
    input
        .request_headers
        .insert("authorization".into(), serde_json::json!("Bearer secret"));
    input
        .request_headers
        .insert("x-api-key".into(), serde_json::json!("key-secret"));
    input
        .request_headers
        .insert("x-envoy-internal".into(), serde_json::json!("true"));
    input
        .request_headers
        .insert("accept".into(), serde_json::json!("application/json"));
    input
        .response_headers
        .insert("set-cookie".into(), serde_json::json!("session=secret"));
    input
        .response_headers
        .insert("server".into(), serde_json::json!("envoy"));
    input
        .response_headers
        .insert("content-type".into(), serde_json::json!("application/json"));
    input.response_body = Some("{\"ok\":true}".into());
    input.body_seen = true;

    let mut tx = w.pool.begin().await.expect("ingest tx");
    let row = discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &input,
        &provenance(session.id, listener_id, "api-a.example.test"),
    )
    .await
    .expect("ingest");
    tx.commit().await.expect("commit ingest");

    assert_eq!(row.raw.request_headers["authorization"], "[REDACTED]");
    assert_eq!(row.raw.request_headers["x-api-key"], "[REDACTED]");
    assert!(row.raw.request_headers.get("x-envoy-internal").is_none());
    assert_eq!(row.raw.request_headers["accept"], "application/json");
    assert_eq!(row.raw.response_headers["set-cookie"], "[REDACTED]");
    assert!(row.raw.response_headers.get("server").is_none());
    assert_eq!(row.raw.response_headers["content-type"], "application/json");

    let refreshed = discovery::get(&w.pool, w.team_a.id, &session.id.to_string())
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(refreshed.path_count, 1);
    assert_eq!(refreshed.byte_count, row.raw.response_body_bytes);
    assert_eq!(refreshed.drop_count, 0);
}

#[tokio::test]
async fn discovery_duplicate_merge_does_not_consume_another_sample() {
    let Some(w) = world().await else { return };
    let session_id = DiscoverySessionId::generate();
    let listener_id = ListenerId::generate();
    let mut tx = w.pool.begin().await.expect("tx");
    let session = create_session(
        &mut tx,
        w.team_a,
        session_id,
        DiscoverySessionSpec {
            target_sample_count: 1,
            ..spec()
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let metadata = observation("req-merge", "/merge");
    let mut tx = w.pool.begin().await.expect("metadata tx");
    discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &metadata,
        &provenance(session.id, listener_id, "api-a.example.test"),
    )
    .await
    .expect("metadata ingest");
    tx.commit().await.expect("commit metadata");

    let completed = discovery::get(&w.pool, w.team_a.id, &session.id.to_string())
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(completed.status, DiscoverySessionStatus::Completed);
    assert_eq!(completed.sample_count, 1);

    let mut body = observation("req-merge", "/merge");
    body.response_status = None;
    body.metadata_seen = false;
    body.body_seen = true;
    body.request_body = Some("hello".into());
    body.response_body = Some("world".into());
    let mut tx = w.pool.begin().await.expect("body tx");
    let merged = discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &body,
        &provenance(session.id, listener_id, "api-b.example.test"),
    )
    .await
    .expect("body merge");
    tx.commit().await.expect("commit body");

    assert_eq!(merged.raw.request_body.as_deref(), Some("hello"));
    assert_eq!(merged.raw.response_body.as_deref(), Some("world"));
    assert_eq!(merged.provenance.observed_host, "api-b.example.test");
    let refreshed = discovery::get(&w.pool, w.team_a.id, &session.id.to_string())
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(refreshed.path_count, 1);
    assert_eq!(refreshed.byte_count, 10);
    assert_eq!(refreshed.drop_count, 0);
}

#[tokio::test]
async fn discovery_quota_drop_increments_without_inserting_new_raw_rows() {
    let Some(w) = world().await else { return };
    let session_id = DiscoverySessionId::generate();
    let listener_id = ListenerId::generate();
    let mut tx = w.pool.begin().await.expect("tx");
    let session = create_session(
        &mut tx,
        w.team_a,
        session_id,
        DiscoverySessionSpec {
            max_bytes: 5,
            ..spec()
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let mut too_large = observation("req-large", "/large");
    too_large.body_seen = true;
    too_large.response_body = Some("too-large".into());
    let mut tx = w.pool.begin().await.expect("ingest tx");
    let err = discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &too_large,
        &provenance(session.id, listener_id, "api-a.example.test"),
    )
    .await
    .expect_err("quota drop");
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    tx.commit().await.expect("commit drop count");

    let refreshed = discovery::get(&w.pool, w.team_a.id, &session.id.to_string())
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 0);
    assert_eq!(refreshed.byte_count, 0);
    assert_eq!(refreshed.path_count, 0);
    assert_eq!(refreshed.drop_count, 1);
    assert_eq!(raw_count_for_session(&w.pool, session.id).await, 0);
}

#[tokio::test]
async fn discovery_rejects_new_samples_after_target_count() {
    let Some(w) = world().await else { return };
    let session_id = DiscoverySessionId::generate();
    let listener_id = ListenerId::generate();
    let mut tx = w.pool.begin().await.expect("tx");
    let session = create_session(
        &mut tx,
        w.team_a,
        session_id,
        DiscoverySessionSpec {
            target_sample_count: 1,
            ..spec()
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let mut tx = w.pool.begin().await.expect("first tx");
    discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &observation("req-one", "/one"),
        &provenance(session.id, listener_id, "api-a.example.test"),
    )
    .await
    .expect("first ingest");
    tx.commit().await.expect("commit first");

    let mut tx = w.pool.begin().await.expect("second tx");
    let err = discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &observation("req-two", "/two"),
        &provenance(session.id, listener_id, "api-a.example.test"),
    )
    .await
    .expect_err("target quota");
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    tx.commit().await.expect("commit drop count");

    let refreshed = discovery::get(&w.pool, w.team_a.id, &session.id.to_string())
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(refreshed.drop_count, 1);
    assert_eq!(raw_count_for_session(&w.pool, session.id).await, 1);
}

#[tokio::test]
async fn discovery_enforces_distinct_path_limit_transactionally() {
    let Some(w) = world().await else { return };
    let session_id = DiscoverySessionId::generate();
    let listener_id = ListenerId::generate();
    let mut tx = w.pool.begin().await.expect("tx");
    let session = create_session(
        &mut tx,
        w.team_a,
        session_id,
        DiscoverySessionSpec {
            max_distinct_paths: 1,
            ..spec()
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let mut tx = w.pool.begin().await.expect("first tx");
    discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &observation("req-one", "/same"),
        &provenance(session.id, listener_id, "api-a.example.test"),
    )
    .await
    .expect("first ingest");
    tx.commit().await.expect("commit first");

    let mut tx = w.pool.begin().await.expect("second tx");
    discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &observation("req-two", "/same"),
        &provenance(session.id, listener_id, "api-a.example.test"),
    )
    .await
    .expect("same path ingest");
    tx.commit().await.expect("commit same path");

    let mut tx = w.pool.begin().await.expect("third tx");
    let err = discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &observation("req-three", "/different"),
        &provenance(session.id, listener_id, "api-a.example.test"),
    )
    .await
    .expect_err("path quota");
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    tx.commit().await.expect("commit drop count");

    let refreshed = discovery::get(&w.pool, w.team_a.id, &session.id.to_string())
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 2);
    assert_eq!(refreshed.path_count, 1);
    assert_eq!(refreshed.drop_count, 1);
    assert_eq!(raw_count_for_session(&w.pool, session.id).await, 2);
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

async fn create_session(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    id: DiscoverySessionId,
    spec: DiscoverySessionSpec,
) -> fp_domain::DomainResult<fp_domain::DiscoverySession> {
    discovery::create(
        tx,
        team,
        discovery::DiscoverySessionInsert {
            id,
            name: &unique("discover"),
            spec: &spec,
            validated_upstream_ip: "93.184.216.34",
            cluster_name: &unique("cluster"),
            route_config_name: &unique("route-config"),
            listener_name: &unique("listener"),
        },
    )
    .await
}

async fn raw_count_for_session(pool: &PgPool, session_id: DiscoverySessionId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM discovery_raw_observations WHERE discovery_session_id = $1",
    )
    .bind(session_id.as_uuid())
    .fetch_one(pool)
    .await
    .expect("raw count")
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
