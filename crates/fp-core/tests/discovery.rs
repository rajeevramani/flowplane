//! S9 discovery lifecycle through the core service.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::{
    clusters as cluster_svc, discovery as discovery_svc, gateway as gateway_svc,
};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::discovery::DiscoverySessionSpec;
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::identity;
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
