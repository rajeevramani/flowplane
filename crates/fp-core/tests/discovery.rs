//! S9 discovery lifecycle through the core service.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::{
    clusters as cluster_svc, discovery as discovery_svc, gateway as gateway_svc,
    learning as learning_svc,
};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::api_lifecycle::ObservationIngest;
use fp_domain::authz::TeamRef;
use fp_domain::discovery::{DiscoveryObservationProvenance, DiscoverySessionSpec};
use fp_domain::{DiscoverySessionId, ErrorCode, ListenerId, OrgRole, RequestId};
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
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
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

fn discovery_input(name: String, listener_port: i32) -> discovery_svc::StartDiscoveryInput {
    discovery_svc::StartDiscoveryInput {
        name,
        spec: DiscoverySessionSpec {
            listener_port,
            upstream_host: "93.184.216.34".into(),
            upstream_port: 80,
            upstream_tls: false,
            target_sample_count: 25,
            max_duration_seconds: Some(60),
            max_bytes: 1024 * 1024,
            max_distinct_paths: 50,
        },
    }
}

#[tokio::test]
async fn discovery_lifecycle_hides_resources_from_user_paths_and_tears_down() {
    let Some(w) = world().await else { return };
    let session = discovery_svc::start_session(
        &w.pool,
        &w.admin,
        w.team,
        discovery_input(unique("discover"), 19080),
        RequestId::generate(),
    )
    .await
    .expect("start discovery");

    for (table, name) in [
        ("clusters", session.cluster_name.as_str()),
        ("route_configs", session.route_config_name.as_str()),
        ("listeners", session.listener_name.as_str()),
    ] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE team_id = $1 AND name = $2 AND owner_kind = 'discovery'"
        ))
        .bind(w.team.id.as_uuid())
        .bind(name)
        .fetch_one(&w.pool)
        .await
        .expect("discovery resource row");
        assert_eq!(count, 1, "{table} row is present for xDS loading");
    }

    let err = cluster_svc::get_cluster(
        &w.pool,
        &w.admin,
        w.team,
        &session.cluster_name,
        RequestId::generate(),
    )
    .await
    .expect_err("discovery cluster hidden");
    assert_eq!(err.code, ErrorCode::NotFound);

    let err = gateway_svc::get_listener(
        &w.pool,
        &w.admin,
        w.team,
        &session.listener_name,
        RequestId::generate(),
    )
    .await
    .expect_err("discovery listener hidden");
    assert_eq!(err.code, ErrorCode::NotFound);

    let stopped = discovery_svc::stop_session(
        &w.pool,
        &w.admin,
        w.team,
        &session.name,
        RequestId::generate(),
    )
    .await
    .expect("stop discovery");
    assert_eq!(stopped.status.as_str(), "completed");

    for (table, name) in [
        ("clusters", session.cluster_name.as_str()),
        ("route_configs", session.route_config_name.as_str()),
        ("listeners", session.listener_name.as_str()),
    ] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE team_id = $1 AND name = $2"
        ))
        .bind(w.team.id.as_uuid())
        .bind(name)
        .fetch_one(&w.pool)
        .await
        .expect("resource teardown");
        assert_eq!(count, 0, "{table} row removed on stop");
    }
}

#[tokio::test]
async fn discovery_learning_creates_one_spec_per_host_cluster() {
    let Some(w) = world().await else { return };
    let session_id = DiscoverySessionId::generate();
    let listener_id = ListenerId::generate();
    let mut tx = w.pool.begin().await.expect("tx");
    let session = discovery::create(
        &mut tx,
        w.team,
        discovery::DiscoverySessionInsert {
            id: session_id,
            name: &unique("discover"),
            spec: &DiscoverySessionSpec {
                listener_port: 19081,
                upstream_host: "example.test".into(),
                upstream_port: 443,
                upstream_tls: true,
                target_sample_count: 25,
                max_duration_seconds: Some(60),
                max_bytes: 1024 * 1024,
                max_distinct_paths: 50,
            },
            validated_upstream_ip: "93.184.216.34",
            cluster_name: &unique("cluster"),
            route_config_name: &unique("route-config"),
            listener_name: &unique("listener"),
        },
    )
    .await
    .expect("session");
    for (request_id, host) in [
        ("req-a", "api-a.example.test"),
        ("req-b", "api-b.example.test"),
    ] {
        discovery::ingest_raw_observation(
            &mut tx,
            w.team,
            &observation(request_id),
            &provenance(session.id, listener_id, host),
        )
        .await
        .expect("ingest");
    }
    discovery::complete(&mut tx, w.team.id, &session.id.to_string())
        .await
        .expect("complete");
    tx.commit().await.expect("commit setup");

    let specs = learning_svc::create_spec_versions_from_discovery_session(
        &w.pool,
        &w.admin,
        w.team,
        &session.id.to_string(),
        RequestId::generate(),
    )
    .await
    .expect("learn specs");

    assert_eq!(specs.len(), 2);
    let hosts = specs
        .iter()
        .map(|spec| {
            spec.spec
                .pointer("/x-flowplane-learning-source/observed_host")
                .and_then(|value| value.as_str())
                .expect("observed host")
        })
        .collect::<Vec<_>>();
    assert_eq!(hosts, vec!["api-a.example.test", "api-b.example.test"]);
}

fn observation(request_id: &str) -> ObservationIngest {
    ObservationIngest {
        request_id: request_id.into(),
        method: "GET".into(),
        path: "/v1/items".into(),
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
