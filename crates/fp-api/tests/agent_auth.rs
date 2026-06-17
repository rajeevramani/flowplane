#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
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

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

async fn app_with_admin() -> Option<(axum::Router, String, String, uuid::Uuid)> {
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
    let subject = unique("sub");
    let user = identity::upsert_user_by_subject(&pool, &subject, "admin@test", "Admin")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("membership");

    let token = issuer
        .mint(&subject, "admin@test", "Admin", 600)
        .expect("mint");
    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
    });
    Some((app, token, team.name, team.id.as_uuid()))
}

fn request(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).expect("request")
}

async fn create_agent(
    app: axum::Router,
    admin_token: &str,
    name: &str,
    kind: &str,
    grants: Vec<serde_json::Value>,
) -> (uuid::Uuid, String) {
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/agents",
            admin_token,
            Some(serde_json::json!({
                "name": name,
                "kind": kind,
                "grants": grants,
            })),
        ))
        .await
        .expect("create agent");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    let id = uuid::Uuid::parse_str(body["agent"]["id"].as_str().expect("agent id")).expect("uuid");
    let token = body["token"].as_str().expect("agent token").to_string();
    assert!(token.starts_with("fpat_"));
    (id, token)
}

#[tokio::test]
async fn active_agent_tokens_authenticate_and_rotate_disable_fail_closed() {
    let Some((app, admin_token, _, _)) = app_with_admin().await else {
        return;
    };

    let (agent_id, token) = create_agent(
        app.clone(),
        &admin_token,
        &unique("agent"),
        "cp-tool",
        vec![],
    )
    .await;

    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1/auth/whoami", &token, None))
        .await
        .expect("whoami");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["user_id"], agent_id.to_string());

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/agents/{agent_id}/rotate-token"),
            &admin_token,
            None,
        ))
        .await
        .expect("rotate");
    assert_eq!(response.status(), StatusCode::OK);
    let rotated = json_of(response).await;
    let new_token = rotated["token"].as_str().expect("rotated token");
    assert_ne!(new_token, token);

    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1/auth/whoami", &token, None))
        .await
        .expect("old token");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/agents/{agent_id}/disable"),
            &admin_token,
            None,
        ))
        .await
        .expect("disable");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(request("GET", "/api/v1/auth/whoami", new_token, None))
        .await
        .expect("disabled token");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_session_reauth_rejects_disabled_agent_on_next_request() {
    let Some((app, admin_token, _, _)) = app_with_admin().await else {
        return;
    };
    let (agent_id, token) = create_agent(
        app.clone(),
        &admin_token,
        &unique("agent"),
        "cp-tool",
        vec![],
    )
    .await;

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/mcp",
            &token,
            Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            })),
        ))
        .await
        .expect("initialize");
    assert_eq!(response.status(), StatusCode::OK);
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session")
        .to_string();

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/agents/{agent_id}/disable"),
            &admin_token,
            None,
        ))
        .await
        .expect("disable");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("mcp-session-id", session)
                .body(Body::from(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 2,
                        "method": "ping",
                        "params": {}
                    })
                    .to_string(),
                ))
                .expect("ping"),
        )
        .await
        .expect("ping");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gateway_and_api_consumer_agents_do_not_get_control_plane_access() {
    let Some((app, admin_token, team_name, _)) = app_with_admin().await else {
        return;
    };

    let (_, gateway_token) = create_agent(
        app.clone(),
        &admin_token,
        &unique("gateway"),
        "gateway-tool",
        vec![],
    )
    .await;
    let (_, api_token) = create_agent(
        app.clone(),
        &admin_token,
        &unique("consumer"),
        "api-consumer",
        vec![],
    )
    .await;

    for token in [&gateway_token, &api_token] {
        let response = app
            .clone()
            .oneshot(request(
                "GET",
                &format!("/api/v1/teams/{team_name}/clusters"),
                token,
                None,
            ))
            .await
            .expect("clusters");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/mcp",
            &api_token,
            Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            })),
        ))
        .await
        .expect("api consumer mcp");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["error"]["data"]["kind"], "authz");
}

#[tokio::test]
async fn gateway_agent_grants_are_limited_to_mcp_tools_scope() {
    let Some((app, admin_token, _, team_id)) = app_with_admin().await else {
        return;
    };

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/agents",
            &admin_token,
            Some(serde_json::json!({
                "name": unique("bad-gateway"),
                "kind": "gateway-tool",
                "grants": [{
                    "team_id": team_id,
                    "resource": "clusters",
                    "action": "read"
                }]
            })),
        ))
        .await
        .expect("bad grant");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let (_agent_id, token) = create_agent(
        app.clone(),
        &admin_token,
        &unique("good-gateway"),
        "gateway-tool",
        vec![serde_json::json!({
            "team_id": team_id,
            "resource": "mcp-tools",
            "action": "execute"
        })],
    )
    .await;

    let response = app
        .oneshot(request("GET", "/api/v1/auth/whoami", &token, None))
        .await
        .expect("whoami");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["grant_count"], 1);
}
