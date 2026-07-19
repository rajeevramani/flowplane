//! Black-box MCP parity tests for `cp_ai_usage` windowing (slice fpv2-0t4.1).
//!
//! Contract under test: `cp_ai_usage` accepts optional RFC 3339 `since`/`until` args with
//! the same half-open `[since, until)` semantics as `GET /teams/{team}/ai/usage`, returns
//! the `{ items, total }` envelope, and rejects malformed or over-cap windows through the
//! same validation path — parity with REST by construction (both call the same service).
//!
//! Parallel-safety (constitution invariant 18): fresh uuid-unique org/team per test; all
//! assertions are scoped to that team's rows only.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::{OrgId, OrgRole};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

struct Env {
    app: axum::Router,
    issuer: DevIssuer,
    pool: sqlx::PgPool,
}

async fn env() -> Option<Env> {
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

    let app = fp_api::build_router(fp_api::AppState {
        pool: pool.clone(),
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
    Some(Env { app, issuer, pool })
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

async fn org_admin(env: &Env, org_id: OrgId) -> String {
    let subject = unique("sub");
    let email = format!("{}@test", unique("user"));
    let user = identity::upsert_user_by_subject(&env.pool, &subject, &email, "Test User")
        .await
        .expect("user");
    identity::add_org_membership(&env.pool, user, org_id, OrgRole::Admin)
        .await
        .expect("org membership");
    env.issuer
        .mint(&subject, &email, "Test User", 600)
        .expect("mint")
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

async fn initialize(env: &Env, token: &str) -> String {
    let response = env
        .app
        .clone()
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
        .expect("session id")
        .to_string()
}

async fn call_ai_usage(
    env: &Env,
    token: &str,
    session: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let response = env
        .app
        .clone()
        .oneshot(mcp_request(
            token,
            Some(session),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "cp_ai_usage", "arguments": arguments }
            }),
        ))
        .await
        .expect("tools/call");
    assert_eq!(response.status(), StatusCode::OK);
    json_of(response).await
}

/// The tool result payload as JSON: parses `result.content[0].text`.
fn tool_payload(call: &serde_json::Value) -> serde_json::Value {
    let text = call["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("tool text content: {call}"));
    serde_json::from_str(text).unwrap_or_else(|_| panic!("tool payload json: {text}"))
}

async fn seed_usage_event(env: &Env, team_id: fp_domain::TeamId, total_tokens: i64) {
    sqlx::query(
        "INSERT INTO ai_usage_events \
         (id, team_id, route_config_id, provider_id, prompt_tokens, completion_tokens, \
          total_tokens, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())",
    )
    .bind(Uuid::now_v7())
    .bind(team_id.as_uuid())
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(total_tokens / 2)
    .bind(total_tokens - total_tokens / 2)
    .bind(total_tokens)
    .execute(&env.pool)
    .await
    .expect("seed usage event");
}

#[tokio::test]
async fn cp_ai_usage_windowed_parity_with_rest() {
    let Some(env) = env().await else { return };
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&env.pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let token = org_admin(&env, org.id).await;
    seed_usage_event(&env, team.id, 42).await;

    // REST read (all-time + windowed) — the parity baseline.
    let rest = |uri: String| {
        let app = env.app.clone();
        let token = token.clone();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(uri)
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK);
            json_of(response).await
        }
    };
    let rest_all = rest(format!("/api/v1/teams/{}/ai/usage", team.name)).await;
    assert_eq!(rest_all["total"], 1);
    assert_eq!(rest_all["items"][0]["total_tokens"], 42);

    let since = (chrono::Utc::now() - chrono::Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let rest_windowed = rest(format!(
        "/api/v1/teams/{}/ai/usage?since={since}",
        team.name
    ))
    .await;

    // MCP: no args = all-time; with since = same window; both mirror REST exactly.
    let session = initialize(&env, &token).await;
    let call = call_ai_usage(
        &env,
        &token,
        &session,
        serde_json::json!({ "team": team.name }),
    )
    .await;
    assert_eq!(call["result"]["isError"], false, "all-time call: {call}");
    let payload = tool_payload(&call);
    assert_eq!(payload["total"], rest_all["total"]);
    assert_eq!(payload["items"], rest_all["items"]);

    let call = call_ai_usage(
        &env,
        &token,
        &session,
        serde_json::json!({ "team": team.name, "since": since }),
    )
    .await;
    assert_eq!(call["result"]["isError"], false, "windowed call: {call}");
    let payload = tool_payload(&call);
    assert_eq!(payload["total"], rest_windowed["total"]);
    assert_eq!(payload["items"], rest_windowed["items"]);
}

#[tokio::test]
async fn cp_ai_usage_rejects_malformed_and_over_cap_windows() {
    let Some(env) = env().await else { return };
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&env.pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let token = org_admin(&env, org.id).await;
    let session = initialize(&env, &token).await;

    // Malformed since: present-but-unparseable is an error, never silently ignored.
    let call = call_ai_usage(
        &env,
        &token,
        &session,
        serde_json::json!({ "team": team.name, "since": "not-a-timestamp" }),
    )
    .await;
    assert!(
        call["result"]["isError"] == true || call["error"].is_object(),
        "malformed since must fail: {call}"
    );

    // Wrong JSON type: a present non-string value must be rejected, not treated as
    // omitted (which would silently widen the read to all-time).
    let call = call_ai_usage(
        &env,
        &token,
        &session,
        serde_json::json!({ "team": team.name, "since": 123 }),
    )
    .await;
    assert!(
        call["result"]["isError"] == true || call["error"].is_object(),
        "non-string since must fail: {call}"
    );
    let call = call_ai_usage(
        &env,
        &token,
        &session,
        serde_json::json!({ "team": team.name, "until": serde_json::Value::Null }),
    )
    .await;
    assert!(
        call["result"]["isError"] == true || call["error"].is_object(),
        "null until must fail: {call}"
    );

    // Over the 92-day span cap with since present: same validation as REST.
    let since = (chrono::Utc::now() - chrono::Duration::days(93))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let call = call_ai_usage(
        &env,
        &token,
        &session,
        serde_json::json!({ "team": team.name, "since": since }),
    )
    .await;
    assert!(
        call["result"]["isError"] == true || call["error"].is_object(),
        "over-cap window must fail: {call}"
    );
}
