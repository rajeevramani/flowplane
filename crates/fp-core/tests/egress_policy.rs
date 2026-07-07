//! Shared egress policy service coverage.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::ai as ai_svc;
use fp_core::services::clusters as cluster_svc;
use fp_core::services::discovery as discovery_svc;
use fp_core::services::egress_policy::EgressPolicy;
use fp_core::services::expose as expose_svc;
use fp_core::services::secrets::{self as secret_svc, SecretWrite};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::discovery::DiscoverySessionSpec;
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
use fp_domain::{AiProviderKind, AiProviderSpec, ErrorCode, OrgRole, RequestId, SecretSpec};
use fp_storage::repos::identity;
use sqlx::PgPool;
use std::net::{IpAddr, SocketAddr};

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

fn cluster_spec(host: &str, port: u16) -> ClusterSpec {
    ClusterSpec {
        aggregate_clusters: Vec::new(),
        endpoints: vec![Endpoint {
            host: host.into(),
            port,
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

fn discovery_input(name: String, host: &str, port: i32) -> discovery_svc::StartDiscoveryInput {
    discovery_svc::StartDiscoveryInput {
        name,
        spec: DiscoverySessionSpec {
            listener_port: 19081,
            upstream_host: host.into(),
            upstream_port: port,
            upstream_tls: false,
            target_sample_count: 10,
            max_duration_seconds: Some(60),
            max_bytes: 1024 * 1024,
            max_distinct_paths: 50,
        },
    }
}

fn generic_secret(name: &str) -> SecretWrite<'_> {
    SecretWrite {
        name,
        description: "",
        spec: SecretSpec::GenericSecret {
            secret: "ZWdyZXNzLXBvbGljeS10ZXN0".into(),
        },
        expires_at: None,
    }
}

async fn audit_count(pool: &PgPool, rid: RequestId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE request_id = $1")
        .bind(rid.as_uuid())
        .fetch_one(pool)
        .await
        .expect("audit count")
}

async fn event_count(pool: &PgPool, team: TeamRef) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE team_id = $1")
        .bind(team.id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("event count")
}

#[tokio::test]
async fn denied_cluster_destination_does_not_persist_or_append_outbox() {
    let Some(w) = world().await else { return };
    let rid = RequestId::generate();
    let before_events = event_count(&w.pool, w.team).await;
    let name = unique("cluster");

    let err = cluster_svc::create_cluster_with_egress_policy(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        cluster_spec("127.0.0.1", 8080),
        rid,
        &EgressPolicy::default(),
    )
    .await
    .expect_err("loopback cluster endpoint denied");

    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert_eq!(audit_count(&w.pool, rid).await, 0);
    assert_eq!(event_count(&w.pool, w.team).await, before_events);
    let row_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM clusters WHERE team_id = $1 AND name = $2")
            .bind(w.team.id.as_uuid())
            .bind(&name)
            .fetch_one(&w.pool)
            .await
            .expect("cluster row count");
    assert_eq!(row_count, 0);
}

#[tokio::test]
async fn allowlisted_private_cluster_persists_and_audit_records_match() {
    let Some(w) = world().await else { return };
    let rid = RequestId::generate();
    let ip = "127.0.0.1".parse::<IpAddr>().unwrap();
    let policy = EgressPolicy::with_allowed(Vec::new(), vec![SocketAddr::new(ip, 8080)]);
    let name = unique("cluster");

    cluster_svc::create_cluster_with_egress_policy(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        cluster_spec("127.0.0.1", 8080),
        rid,
        &policy,
    )
    .await
    .expect("allowlisted private cluster persists");

    let detail: serde_json::Value =
        sqlx::query_scalar("SELECT detail FROM audit_log WHERE request_id = $1")
            .bind(rid.as_uuid())
            .fetch_one(&w.pool)
            .await
            .expect("audit detail");
    assert_eq!(
        detail["egress_policy"]["allowlist_match"],
        serde_json::json!("127.0.0.1:8080")
    );
}

#[tokio::test]
async fn denied_ai_provider_destination_does_not_persist_success_audit() {
    let Some(w) = world().await else { return };
    let secret = secret_svc::create_secret(
        &w.pool,
        &w.admin,
        w.team,
        generic_secret(&unique("ai-key")),
        RequestId::generate(),
    )
    .await
    .expect("secret");
    let rid = RequestId::generate();
    let name = unique("provider");

    let err = ai_svc::create_provider_with_egress_policy(
        &w.pool,
        &w.admin,
        w.team,
        &name,
        AiProviderSpec {
            kind: AiProviderKind::OpenaiCompatible,
            base_url: "http://169.254.169.254".into(),
            path_prefix: Some("/v1".into()),
            credential_secret_id: secret.id,
            models: vec!["gpt-5".into()],
            auth_header: "authorization".into(),
        },
        rid,
        &EgressPolicy::default(),
    )
    .await
    .expect_err("metadata provider denied");

    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert_eq!(audit_count(&w.pool, rid).await, 0);
    let row_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_providers WHERE team_id = $1 AND name = $2")
            .bind(w.team.id.as_uuid())
            .bind(&name)
            .fetch_one(&w.pool)
            .await
            .expect("AI provider row count");
    assert_eq!(row_count, 0);
}

#[tokio::test]
async fn denied_expose_destination_does_not_create_gateway_rows_or_events() {
    let Some(w) = world().await else { return };
    let rid = RequestId::generate();
    let before_events = event_count(&w.pool, w.team).await;
    let name = unique("expose");

    let err = expose_svc::expose_with_egress_policy(
        &w.pool,
        &w.admin,
        w.team,
        expose_svc::ExposeRequest {
            name: name.clone(),
            upstream: "http://127.0.0.1:3001".into(),
            path: "/".into(),
            port: Some(19082),
            public_base_url: None,
        },
        rid,
        &EgressPolicy::default(),
    )
    .await
    .expect_err("loopback expose denied");

    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert_eq!(audit_count(&w.pool, rid).await, 0);
    assert_eq!(event_count(&w.pool, w.team).await, before_events);
    for table in ["clusters", "route_configs", "listeners"] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE team_id = $1 AND name LIKE $2"
        ))
        .bind(w.team.id.as_uuid())
        .bind(format!("{name}%"))
        .fetch_one(&w.pool)
        .await
        .expect("gateway row count");
        assert_eq!(count, 0, "{table} row count");
    }
}

#[tokio::test]
async fn denied_discovery_destination_does_not_create_session_resources_or_events() {
    let Some(w) = world().await else { return };
    let rid = RequestId::generate();
    let before_events = event_count(&w.pool, w.team).await;
    let name = unique("discover");

    let err = discovery_svc::start_session_with_policy(
        &w.pool,
        &w.admin,
        w.team,
        discovery_input(name.clone(), "10.0.0.10", 8080),
        rid,
        &EgressPolicy::default(),
    )
    .await
    .expect_err("private discovery upstream denied");

    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert_eq!(audit_count(&w.pool, rid).await, 0);
    assert_eq!(event_count(&w.pool, w.team).await, before_events);
    let row_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM discovery_sessions WHERE team_id = $1 AND name = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(&name)
    .fetch_one(&w.pool)
    .await
    .expect("discovery session row count");
    assert_eq!(row_count, 0);
}
