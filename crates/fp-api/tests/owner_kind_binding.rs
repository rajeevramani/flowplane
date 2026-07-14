//! HTTP smoke pin for owner-kind-matched reference resolution: POSTing a user route
//! config that references an ai-owned cluster by name must come back as the standard
//! 400 validation envelope through the real router + middleware (ValidationFailed → 400
//! end to end). The service/repo matrix lives in fp-core's gateway_owner_kind_matrix.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::gateway::cluster::{ClusterSpec, Endpoint, LbPolicy};
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

#[tokio::test]
async fn posting_user_route_config_referencing_ai_cluster_is_a_400_validation_envelope() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");
    let subject = unique("sub");
    let token = issuer
        .mint(&subject, "ownerkind@test", "OwnerKind", 600)
        .expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &subject, "ownerkind@test", "OwnerKind")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    // Seed an AI-owned cluster below the user surface (as the AI pipeline does).
    let ai_cluster = unique("ai-upstream");
    let team_ref = fp_domain::authz::TeamRef {
        id: team.id,
        org_id: org.id,
    };
    let spec = ClusterSpec {
        aggregate_clusters: Vec::new(),
        endpoints: vec![Endpoint {
            host: "10.9.9.9".into(),
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
    };
    let mut tx = pool.begin().await.expect("begin");
    fp_storage::repos::clusters::create_ai_owned(
        &mut tx,
        team_ref,
        uuid::Uuid::now_v7(),
        &ai_cluster,
        &spec,
    )
    .await
    .expect("seed ai cluster");
    tx.commit().await.expect("commit");

    let query_pool = pool.clone();
    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });

    let rc_name = unique("rc");
    let body = serde_json::json!({
        "name": rc_name,
        "spec": {
            "virtual_hosts": [{
                "name": "default",
                "domains": ["*"],
                "routes": [{
                    "name": "all",
                    "match": {"prefix": {"prefix": "/"}},
                    "action": {"cluster": ai_cluster, "timeout_secs": 15}
                }]
            }]
        }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/teams/{}/route-configs", team.name))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("create route config");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(json["code"], "validation_failed", "envelope: {json}");
    assert!(
        json["message"]
            .as_str()
            .expect("message")
            .contains(&ai_cluster),
        "message must name the unresolvable cluster: {json}"
    );
    assert!(
        json["request_id"].as_str().is_some_and(|id| !id.is_empty()),
        "envelope must carry a request_id: {json}"
    );

    // The rejected create left nothing behind.
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM route_configs WHERE team_id = $1 AND name = $2")
            .bind(team.id.as_uuid())
            .bind(&rc_name)
            .fetch_one(&query_pool)
            .await
            .expect("rc count");
    assert_eq!(
        count, 0,
        "rejected create must not persist the route config"
    );
}
