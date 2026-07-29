//! fpv2-xni — `GET /api/v1/teams/{team}/xds/status` exposes the per-team withdrawn (degraded)
//! resource list, sourced from a narrow in-memory handle injected into `AppState` (the same seam
//! as `xds_readiness`). These tests drive the real router over a real DB with a fake handle, so
//! they exercise the handler wiring (team id passed through, empty when the handle is absent)
//! without standing up the whole xDS snapshot cache.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::{OrgRole, TeamId};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json")
}

/// A fake degraded-source that returns preset withdrawn resources only for `team`, empty for any
/// other team — proving the handler forwards the resolved team id and the list is team-scoped.
struct FakeDegraded {
    team: TeamId,
    resources: Vec<fp_api::state::WithdrawnResource>,
}

impl fp_api::state::XdsDegradedSource for FakeDegraded {
    fn withdrawn<'a>(
        &'a self,
        team_id: TeamId,
    ) -> Pin<Box<dyn Future<Output = Vec<fp_api::state::WithdrawnResource>> + Send + 'a>> {
        let out = if team_id == self.team {
            self.resources.clone()
        } else {
            Vec::new()
        };
        Box::pin(async move { out })
    }
}

struct World {
    pool: sqlx::PgPool,
    validator: Arc<fp_core::OidcValidator>,
    token: String,
}

async fn world() -> Option<(World, String, TeamId, String)> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
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
    let token = issuer.mint(&subject, "xni@test", "Xni", 600).expect("mint");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&pool, org.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org.id, &unique("team-b"), "")
        .await
        .expect("team b");
    let user = identity::upsert_user_by_subject(&pool, &subject, "xni@test", "Xni")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("member");

    Some((
        World {
            pool,
            validator: Arc::new(validator),
            token,
        },
        team_a.name,
        team_a.id,
        team_b.name,
    ))
}

fn build(
    world: &World,
    xds_degraded: Option<Arc<dyn fp_api::state::XdsDegradedSource>>,
) -> axum::Router {
    fp_api::build_router(fp_api::AppState {
        pool: world.pool.clone(),
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(world.validator.clone()),
        write_throttle: Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        xds_degraded,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    })
}

fn get_status(world: &World, team: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/v1/teams/{team}/xds/status"))
        .header("authorization", format!("Bearer {}", world.token))
        .body(Body::empty())
        .expect("request")
}

#[tokio::test]
async fn xds_status_reports_withdrawn_resources_team_scoped() {
    let Some((world, team_a, team_a_id, team_b)) = world().await else {
        return;
    };
    let source = Arc::new(FakeDegraded {
        team: team_a_id,
        resources: vec![
            fp_api::state::WithdrawnResource {
                type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".into(),
                name: "orders-svc".into(),
                error: "NACK: duplicate cluster".into(),
            },
            fp_api::state::WithdrawnResource {
                type_url: "type.googleapis.com/envoy.config.listener.v3.Listener".into(),
                name: "public-listener".into(),
                error: "translation failed".into(),
            },
        ],
    });
    let app = build(&world, Some(source));

    // Team A: its withdrawn resources surface, with reasons.
    let resp = app
        .clone()
        .oneshot(get_status(&world, &team_a))
        .await
        .expect("status A");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_of(resp).await;
    let withdrawn = body["withdrawn"].as_array().expect("withdrawn array");
    assert_eq!(
        withdrawn.len(),
        2,
        "both withdrawn resources surface: {body}"
    );
    let names: Vec<&str> = withdrawn
        .iter()
        .map(|w| w["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"orders-svc") && names.contains(&"public-listener"),
        "{body}"
    );
    assert!(!withdrawn[0]["type_url"].as_str().unwrap().is_empty());
    assert!(
        withdrawn.iter().any(|w| w["error"] == "translation failed"),
        "reason carried: {body}"
    );

    // Team B (same org, same handle): the list is team-scoped — the handler forwarded B's id, so
    // the fake returns nothing.
    let resp = app
        .clone()
        .oneshot(get_status(&world, &team_b))
        .await
        .expect("status B");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_of(resp).await;
    assert_eq!(
        body["withdrawn"].as_array().expect("withdrawn array").len(),
        0,
        "team B has no withdrawn resources (team-scoped): {body}"
    );
}

#[tokio::test]
async fn xds_status_withdrawn_is_empty_when_handle_absent() {
    let Some((world, team_a, _team_a_id, _team_b)) = world().await else {
        return;
    };
    // No snapshot-cache handle wired (API-only deployment) → the endpoint still succeeds and
    // reports an empty withdrawn list, never omitting the field.
    let app = build(&world, None);
    let resp = app
        .oneshot(get_status(&world, &team_a))
        .await
        .expect("status");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_of(resp).await;
    assert_eq!(
        body["withdrawn"]
            .as_array()
            .expect("withdrawn present as array")
            .len(),
        0,
        "withdrawn must be an empty array when the handle is absent: {body}"
    );
}
