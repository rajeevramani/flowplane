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
    insert_discovery_listener(
        &mut tx,
        w.team_a,
        session.id,
        &session.listener_name,
        listener_id,
    )
    .await;
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
    assert_eq!(rows[0].provenance.discovery_listener_id, listener_id);
    assert_eq!(
        rows[0].provenance.forwarded_upstream_host,
        session.upstream_host
    );
    assert_eq!(
        rows[0].provenance.forwarded_upstream_port,
        session.upstream_port
    );
    assert_eq!(
        rows[0].provenance.forwarded_upstream_ip,
        session.validated_upstream_ip
    );
    assert_eq!(
        rows[0].provenance.forwarded_upstream_tls,
        session.upstream_tls
    );
    assert!(rows[0].provenance.route_matched);

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
async fn discovery_duplicate_merge_rejects_after_target_completion() {
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
    let err = discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &body,
        &provenance(session.id, listener_id, "api-b.example.test"),
    )
    .await
    .expect_err("completed session rejects duplicate merge");
    assert_eq!(err.code, ErrorCode::Conflict);
    tx.rollback().await.expect("rollback rejected body");

    let refreshed = discovery::get(&w.pool, w.team_a.id, &session.id.to_string())
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(refreshed.path_count, 1);
    assert_eq!(refreshed.byte_count, 0);
    assert_eq!(refreshed.drop_count, 0);
}

#[tokio::test]
async fn discovery_duplicate_merge_ignores_forged_forwarded_upstream_metadata_while_capturing() {
    let Some(w) = world().await else { return };
    let session_id = DiscoverySessionId::generate();
    let caller_listener_id = ListenerId::generate();
    let server_listener_id = ListenerId::generate();
    let mut tx = w.pool.begin().await.expect("tx");
    let session =
        create_session_with_listener(&mut tx, w.team_a, session_id, spec(), server_listener_id)
            .await
            .expect("session");
    tx.commit().await.expect("commit session");

    let mut forged = provenance(session.id, caller_listener_id, "api-a.example.test");
    forged.forwarded_upstream_host = "attacker.internal.test".into();
    forged.forwarded_upstream_port = 8443;
    forged.forwarded_upstream_ip = "10.0.0.10".into();
    forged.forwarded_upstream_tls = false;
    let mut tx = w.pool.begin().await.expect("metadata tx");
    let captured = discovery::ingest_raw_observation(
        &mut tx,
        w.team_a,
        &observation("req-forged", "/forged"),
        &forged,
    )
    .await
    .expect("metadata ingest");
    tx.commit().await.expect("commit metadata");

    assert_eq!(
        captured.provenance.discovery_listener_id,
        server_listener_id
    );
    assert_eq!(captured.provenance.forwarded_upstream_host, "example.test");
    assert_eq!(captured.provenance.forwarded_upstream_port, 443);
    assert_eq!(captured.provenance.forwarded_upstream_ip, "93.184.216.34");
    assert!(captured.provenance.forwarded_upstream_tls);

    let mut forged_merge = forged.clone();
    forged_merge.observed_host = "api-b.example.test".into();
    forged_merge.forwarded_upstream_host = "metadata.service.local".into();
    forged_merge.forwarded_upstream_ip = "169.254.169.254".into();
    let mut body = observation("req-forged", "/forged");
    body.metadata_seen = false;
    body.body_seen = true;
    body.response_body = Some("body".into());
    let mut tx = w.pool.begin().await.expect("body tx");
    let merged = discovery::ingest_raw_observation(&mut tx, w.team_a, &body, &forged_merge)
        .await
        .expect("body merge");
    tx.commit().await.expect("commit body");

    assert_eq!(merged.provenance.discovery_listener_id, server_listener_id);
    assert_eq!(merged.provenance.observed_host, "api-b.example.test");
    assert_eq!(merged.provenance.forwarded_upstream_host, "example.test");
    assert_eq!(merged.provenance.forwarded_upstream_ip, "93.184.216.34");
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
    create_session_with_listener(tx, team, id, spec, ListenerId::generate()).await
}

async fn create_session_with_listener(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    id: DiscoverySessionId,
    spec: DiscoverySessionSpec,
    listener_id: ListenerId,
) -> fp_domain::DomainResult<fp_domain::DiscoverySession> {
    let session = discovery::create(
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
    .await?;
    insert_discovery_listener(tx, team, session.id, &session.listener_name, listener_id).await;
    Ok(session)
}

async fn insert_discovery_listener(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team: TeamRef,
    session_id: DiscoverySessionId,
    listener_name: &str,
    listener_id: ListenerId,
) {
    sqlx::query(
        "INSERT INTO listeners (id, team_id, org_id, name, spec, owner_kind, owner_id) \
         VALUES ($1, $2, $3, $4, $5, 'discovery', $6)",
    )
    .bind(listener_id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(listener_name)
    .bind(serde_json::json!({
        "address": "0.0.0.0",
        "port": 19080,
        "protocol": "http",
        "route_config": "discovery-route",
        "http_filters": [],
        "access_logs": [],
        "tls_context": null
    }))
    .bind(session_id.as_uuid())
    .execute(&mut **tx)
    .await
    .expect("insert discovery listener");
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
