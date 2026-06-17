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

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("body must be JSON")
}

async fn app_with_tokens() -> Option<(axum::Router, String, String)> {
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
    let subject_a = unique("sub-a");
    let subject_b = unique("sub-b");
    let user_a = identity::upsert_user_by_subject(&pool, &subject_a, "a@test", "A")
        .await
        .expect("user a");
    let user_b = identity::upsert_user_by_subject(&pool, &subject_b, "b@test", "B")
        .await
        .expect("user b");
    identity::add_org_membership(&pool, user_a, org.id, OrgRole::Admin)
        .await
        .expect("member a");
    identity::add_org_membership(&pool, user_b, org.id, OrgRole::Admin)
        .await
        .expect("member b");

    let token_a = issuer.mint(&subject_a, "a@test", "A", 600).expect("mint a");
    let token_b = issuer.mint(&subject_b, "b@test", "B", 600).expect("mint b");

    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        discovery_forwarding_policy: Default::default(),
    });
    Some((app, token_a, token_b))
}

fn request(token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/mcp")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn initialize(id: i64) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": { "protocolVersion": "2025-11-25" }
    })
}

fn ping(id: i64) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "ping",
        "params": {}
    })
}

#[tokio::test]
async fn mcp_initialize_and_ping_allow_headless_clients() {
    let Some((app, token, _)) = app_with_tokens().await else {
        return;
    };

    let response = app
        .clone()
        .oneshot(request(&token, initialize(1)))
        .await
        .expect("initialize response");
    assert_eq!(response.status(), StatusCode::OK);
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();
    assert_eq!(
        response
            .headers()
            .get("mcp-protocol-version")
            .and_then(|v| v.to_str().ok()),
        Some("2025-11-25")
    );
    let json = body_json(response).await;
    assert_eq!(json["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        json["result"]["capabilities"]["tools"]["listChanged"],
        false
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("mcp-session-id", session)
                .body(Body::from(ping(2).to_string()))
                .expect("ping"),
        )
        .await
        .expect("ping response");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["result"], serde_json::json!({}));
}

#[tokio::test]
async fn mcp_session_is_bound_to_reauthenticated_principal() {
    let Some((app, token_a, token_b)) = app_with_tokens().await else {
        return;
    };
    let response = app
        .clone()
        .oneshot(request(&token_a, initialize(1)))
        .await
        .expect("initialize response");
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session id")
        .to_string();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header("authorization", format!("Bearer {token_b}"))
                .header("content-type", "application/json")
                .header("mcp-session-id", session)
                .body(Body::from(ping(2).to_string()))
                .expect("ping"),
        )
        .await
        .expect("ping response");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["error"]["data"]["kind"], "authz");
}

#[tokio::test]
async fn mcp_origin_policy_allows_absent_and_allowed_origin_but_rejects_denied_origin() {
    std::env::set_var("FLOWPLANE_MCP_ALLOWED_ORIGINS", "https://allowed.example");
    let Some((app, token, _)) = app_with_tokens().await else {
        return;
    };

    let no_origin = app
        .clone()
        .oneshot(request(&token, initialize(1)))
        .await
        .expect("no origin");
    assert_eq!(no_origin.status(), StatusCode::OK);

    let allowed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("origin", "https://allowed.example:8443")
                .body(Body::from(initialize(2).to_string()))
                .expect("allowed"),
        )
        .await
        .expect("allowed origin");
    assert_eq!(allowed.status(), StatusCode::OK);

    let denied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("origin", "https://denied.example")
                .body(Body::from(initialize(3).to_string()))
                .expect("denied"),
        )
        .await
        .expect("denied origin");
    assert_eq!(denied.status(), StatusCode::OK);
    let json = body_json(denied).await;
    assert_eq!(json["error"]["data"]["kind"], "origin");
}

#[tokio::test]
async fn mcp_rejects_unsupported_protocol_versions() {
    let Some((app, token, _)) = app_with_tokens().await else {
        return;
    };
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("mcp-protocol-version", "1999-01-01")
                .body(Body::from(initialize(1).to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["error"]["data"]["kind"], "protocol");
}
