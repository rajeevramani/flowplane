#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::{PgPool, Row};
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
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

struct Fixture {
    app: axum::Router,
    pool: PgPool,
    admin_token: String,
    member_token: String,
    team_name: String,
    team_id: uuid::Uuid,
    other_team_name: String,
}

async fn fixture() -> Option<Fixture> {
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

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let other_team = identity::create_team(&pool, org.id, &unique("other"), "")
        .await
        .expect("other team");
    let admin_subject = unique("admin-sub");
    let member_subject = unique("member-sub");
    let admin = identity::upsert_user_by_subject(&pool, &admin_subject, "admin@test", "Admin")
        .await
        .expect("admin");
    let member = identity::upsert_user_by_subject(&pool, &member_subject, "member@test", "Member")
        .await
        .expect("member");
    identity::add_org_membership(&pool, admin, org.id, OrgRole::Admin)
        .await
        .expect("admin membership");
    identity::add_org_membership(&pool, member, org.id, OrgRole::Member)
        .await
        .expect("member membership");

    let admin_token = issuer
        .mint(&admin_subject, "admin@test", "Admin", 600)
        .expect("admin token");
    let member_token = issuer
        .mint(&member_subject, "member@test", "Member", 600)
        .expect("member token");

    let app = fp_api::build_router(fp_api::AppState {
        pool: pool.clone(),
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
    });

    Some(Fixture {
        app,
        pool,
        admin_token,
        member_token,
        team_name: team.name,
        team_id: team.id.as_uuid(),
        other_team_name: other_team.name,
    })
}

fn mcp_request(token: &str, session: Option<&str>, body: serde_json::Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/mcp")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    builder.body(Body::from(body.to_string())).expect("request")
}

async fn initialize(app: axum::Router, token: &str) -> String {
    let response = app
        .oneshot(mcp_request(
            token,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            }),
        ))
        .await
        .expect("initialize");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session")
        .to_string()
}

async fn tools_list(app: axum::Router, token: &str, session: &str, team: &str) -> Vec<String> {
    let response = app
        .oneshot(mcp_request(
            token,
            Some(session),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": { "team": team }
            }),
        ))
        .await
        .expect("tools/list");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_string())
        .collect()
}

async fn create_agent(
    app: axum::Router,
    admin_token: &str,
    kind: &str,
    team_id: uuid::Uuid,
    grants: Vec<(&str, &str)>,
) -> String {
    let grants = grants
        .into_iter()
        .map(|(resource, action)| {
            serde_json::json!({
                "team_id": team_id,
                "resource": resource,
                "action": action,
            })
        })
        .collect::<Vec<_>>();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents")
                .header("authorization", format!("Bearer {admin_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": unique("agent"),
                        "kind": kind,
                        "grants": grants,
                    })
                    .to_string(),
                ))
                .expect("create agent"),
        )
        .await
        .expect("agent response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    body["token"].as_str().expect("token").to_string()
}

#[tokio::test]
async fn tools_list_filters_by_principal_kind_grant_and_team() {
    let Some(fx) = fixture().await else {
        return;
    };

    let admin_session = initialize(fx.app.clone(), &fx.admin_token).await;
    let admin_tools = tools_list(
        fx.app.clone(),
        &fx.admin_token,
        &admin_session,
        &fx.team_name,
    )
    .await;
    assert!(admin_tools.contains(&"cp_clusters_create".to_string()));
    assert!(admin_tools.contains(&"ops_stats_overview".to_string()));

    let member_session = initialize(fx.app.clone(), &fx.member_token).await;
    let member_tools = tools_list(
        fx.app.clone(),
        &fx.member_token,
        &member_session,
        &fx.team_name,
    )
    .await;
    assert!(
        member_tools.is_empty(),
        "grantless member should see no CP tools"
    );

    let cp_token = create_agent(
        fx.app.clone(),
        &fx.admin_token,
        "cp-tool",
        fx.team_id,
        vec![("clusters", "read")],
    )
    .await;
    let cp_session = initialize(fx.app.clone(), &cp_token).await;
    let cp_tools = tools_list(fx.app.clone(), &cp_token, &cp_session, &fx.team_name).await;
    assert!(cp_tools.contains(&"cp_clusters_list".to_string()));
    assert!(cp_tools.contains(&"cp_clusters_get".to_string()));
    assert!(!cp_tools.contains(&"cp_clusters_create".to_string()));
    let cross_team_tools =
        tools_list(fx.app.clone(), &cp_token, &cp_session, &fx.other_team_name).await;
    assert!(
        cross_team_tools.is_empty(),
        "agent grant must not cross teams"
    );

    let gateway_token = create_agent(
        fx.app.clone(),
        &fx.admin_token,
        "gateway-tool",
        fx.team_id,
        vec![],
    )
    .await;
    let gateway_session = initialize(fx.app.clone(), &gateway_token).await;
    let gateway_tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.team_name,
    )
    .await;
    assert!(
        gateway_tools.is_empty(),
        "gateway agents do not list CP tools"
    );
}

#[tokio::test]
async fn tools_call_uses_service_path_and_emits_mutation_audit() {
    let Some(fx) = fixture().await else {
        return;
    };
    let session = initialize(fx.app.clone(), &fx.admin_token).await;
    let cluster = unique("cluster");
    let response = fx
        .app
        .clone()
        .oneshot(mcp_request(
            &fx.admin_token,
            Some(&session),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "cp_clusters_create",
                    "arguments": {
                        "team": fx.team_name,
                        "name": cluster,
                        "spec": {
                            "endpoints": [{ "host": "10.0.0.10", "port": 8080 }]
                        }
                    }
                }
            }),
        ))
        .await
        .expect("tools/call");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["result"]["isError"], false);
    assert_eq!(body["result"]["structuredContent"]["name"], cluster);

    let audit = sqlx::query(
        "SELECT action, resource FROM audit_log \
         WHERE team_id = $1 AND action = 'cluster.create' AND resource = $2 \
         ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(fx.team_id)
    .bind(format!("clusters/{cluster}"))
    .fetch_one(&fx.pool)
    .await
    .expect("audit row");
    assert_eq!(audit.get::<String, _>("action"), "cluster.create");
    assert_eq!(
        audit.get::<String, _>("resource"),
        format!("clusters/{cluster}")
    );
}
