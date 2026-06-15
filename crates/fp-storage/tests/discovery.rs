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
