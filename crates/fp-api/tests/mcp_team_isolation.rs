//! Black-box acceptance tests for team-scoped MCP session visibility
//! (feature fpv2-lhr — MCP session team attribution).
//!
//! Contract under test: `GET /api/v1/teams/{team}/mcp/status` (`active_sessions`) and
//! `GET /api/v1/teams/{team}/mcp/connections` list only sessions with successfully
//! authorized MCP activity (`tools/list` / `tools/call`) for THAT team. Denied or
//! merely-initialized sessions attribute nothing, and the `connection_id` shown to a
//! team is a per-team identifier that cannot be correlated across teams.
//!
//! Parallel-safety (constitution invariant 18): the session registry is process-global
//! and shared across concurrently running tests, so these tests NEVER assert on absolute
//! connection counts or full-array shapes. Every assertion is a set-difference against a
//! captured before-set of `connection_id`s (or a before-count for `active_sessions`) on
//! org/team fixtures whose names are uuid-unique per test, so foreign sessions can never
//! attribute to our teams. Skips cleanly when FLOWPLANE_TEST_DATABASE_URL is unset.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::{OrgId, OrgRole, TeamId};
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

/// Create a user with one org membership and mint a bearer token for them.
async fn user_with_org_role(env: &Env, org_id: OrgId, role: OrgRole) -> String {
    let subject = unique("sub");
    let email = format!("{}@test", unique("user"));
    let user = identity::upsert_user_by_subject(&env.pool, &subject, &email, "Test User")
        .await
        .expect("user");
    identity::add_org_membership(&env.pool, user, org_id, role)
        .await
        .expect("org membership");
    env.issuer
        .mint(&subject, &email, "Test User", 600)
        .expect("mint")
}

struct OrgFixture {
    org_id: OrgId,
    team_a_id: TeamId,
    team_a_name: String,
    team_b_id: TeamId,
    team_b_name: String,
    /// Same-org org admin: reader for both teams' status/connections views, and the
    /// principal used to mint agents.
    admin_token: String,
}

/// One org with two uuid-unique teams A and B plus an org-admin reader.
async fn org_with_two_teams(env: &Env) -> OrgFixture {
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&env.pool, org.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&env.pool, org.id, &unique("team-b"), "")
        .await
        .expect("team b");
    let admin_token = user_with_org_role(env, org.id, OrgRole::Admin).await;
    OrgFixture {
        org_id: org.id,
        team_a_id: team_a.id,
        team_a_name: team_a.name,
        team_b_id: team_b.id,
        team_b_name: team_b.name,
        admin_token,
    }
}

/// Create an agent of `kind` with the given (team, resource, action) grants; returns its
/// bearer token. Goes through the product agent-creation surface, like a real operator.
async fn create_agent(
    env: &Env,
    admin_token: &str,
    kind: &str,
    grants: Vec<(TeamId, &str, &str)>,
) -> String {
    let grants = grants
        .into_iter()
        .map(|(team_id, resource, action)| {
            serde_json::json!({
                "team_id": team_id.as_uuid(),
                "resource": resource,
                "action": action,
            })
        })
        .collect::<Vec<_>>();
    let response = env
        .app
        .clone()
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
                .expect("create agent request"),
        )
        .await
        .expect("create agent response");
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "create {kind} agent"
    );
    let body = json_of(response).await;
    body["token"].as_str().expect("agent token").to_string()
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

/// Initialize an MCP session, returning the mcp-session-id.
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

/// tools/list for a team; returns the raw JSON-RPC body (result or error).
async fn tools_list(env: &Env, token: &str, session: &str, team: &str) -> serde_json::Value {
    let response = env
        .app
        .clone()
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
    json_of(response).await
}

/// tools/call; returns the raw JSON-RPC body (result or error).
async fn tools_call(
    env: &Env,
    token: &str,
    session: &str,
    name: &str,
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
                "params": { "name": name, "arguments": arguments }
            }),
        ))
        .await
        .expect("tools/call");
    assert_eq!(response.status(), StatusCode::OK);
    json_of(response).await
}

/// GET /teams/{team}/mcp/connections as `token`; returns the raw array.
async fn connections(env: &Env, token: &str, team: &str) -> Vec<serde_json::Value> {
    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/teams/{team}/mcp/connections"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("connections request"),
        )
        .await
        .expect("connections response");
    assert_eq!(response.status(), StatusCode::OK, "connections for {team}");
    json_of(response)
        .await
        .as_array()
        .expect("connections array")
        .clone()
}

fn ids_of(entries: &[serde_json::Value]) -> BTreeSet<String> {
    entries
        .iter()
        .map(|c| {
            c["connection_id"]
                .as_str()
                .expect("connection_id string")
                .to_string()
        })
        .collect()
}

async fn connection_ids(env: &Env, token: &str, team: &str) -> BTreeSet<String> {
    ids_of(&connections(env, token, team).await)
}

/// GET /teams/{team}/mcp/status as `token`; returns `active_sessions`.
async fn active_sessions(env: &Env, token: &str, team: &str) -> u64 {
    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/teams/{team}/mcp/status"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("status request"),
        )
        .await
        .expect("status response");
    assert_eq!(response.status(), StatusCode::OK, "status for {team}");
    json_of(response).await["active_sessions"]
        .as_u64()
        .expect("active_sessions u64")
}

fn new_ids(before: &BTreeSet<String>, after: &BTreeSet<String>) -> BTreeSet<String> {
    after.difference(before).cloned().collect()
}

// --- Criterion 1 (+6): a B-only session is visible in B, invisible in A and in a
// --- foreign org's team -------------------------------------------------------------

#[tokio::test]
async fn b_only_session_appears_in_b_and_never_in_a_or_foreign_org() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    // Foreign org with its own team + admin reader (criterion 6, cheap here).
    let other_org = identity::create_org(&env.pool, &unique("org-2"), "")
        .await
        .expect("other org");
    let other_team = identity::create_team(&env.pool, other_org.id, &unique("team-y"), "")
        .await
        .expect("other team");
    let other_admin = user_with_org_role(&env, other_org.id, OrgRole::Admin).await;

    // Principal P: cp-tool agent with clusters:read on B ONLY.
    let agent_token = create_agent(
        &env,
        &fx.admin_token,
        "cp-tool",
        vec![(fx.team_b_id, "clusters", "read")],
    )
    .await;

    // Capture before-sets/counts for every view we assert on.
    let a_before = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_before = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;
    let y_before = connection_ids(&env, &other_admin, &other_team.name).await;
    let a_sessions_before = active_sessions(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_sessions_before = active_sessions(&env, &fx.admin_token, &fx.team_b_name).await;

    // P initializes and performs a successful authorized tools/call for team B.
    let session = initialize(&env, &agent_token).await;
    let call = tools_call(
        &env,
        &agent_token,
        &session,
        "cp_clusters_list",
        serde_json::json!({ "team": fx.team_b_name }),
    )
    .await;
    assert_eq!(
        call["result"]["isError"], false,
        "authorized cp_clusters_list on B must succeed: {call}"
    );

    // Team A's view must not have grown by P's B-only session.
    let a_after = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    assert!(
        new_ids(&a_before, &a_after).is_empty(),
        "B-only session must not appear in team A's connections: new {:?}",
        new_ids(&a_before, &a_after)
    );
    assert_eq!(
        active_sessions(&env, &fx.admin_token, &fx.team_a_name).await,
        a_sessions_before,
        "team A's active_sessions must not increase for a B-only session"
    );

    // Team B's view gains exactly one new entry, with the documented shape.
    let b_connections_after = connections(&env, &fx.admin_token, &fx.team_b_name).await;
    let b_new = new_ids(&b_before, &ids_of(&b_connections_after));
    assert_eq!(
        b_new.len(),
        1,
        "team B must gain exactly one new connection, got new {b_new:?}"
    );
    let new_id = b_new.iter().next().expect("new id").clone();
    Uuid::parse_str(&new_id).expect("connection_id must be a uuid");
    let entry = b_connections_after
        .iter()
        .find(|c| c["connection_id"] == new_id.as_str())
        .expect("new entry present in B's listing");
    assert!(
        !entry["principal_kind"]
            .as_str()
            .expect("principal_kind string")
            .is_empty(),
        "principal_kind populated: {entry}"
    );
    assert_eq!(entry["transport"], "streamable_http_post", "{entry}");
    assert_eq!(entry["sse"], false, "{entry}");
    assert!(entry["age_seconds"].as_u64().is_some(), "{entry}");
    assert!(entry["idle_seconds"].as_u64().is_some(), "{entry}");

    assert_eq!(
        active_sessions(&env, &fx.admin_token, &fx.team_b_name).await,
        b_sessions_before + 1,
        "team B's active_sessions must increase by exactly 1"
    );

    // Criterion 6: the org1 session never surfaces in org2's team listing.
    let y_after = connection_ids(&env, &other_admin, &other_team.name).await;
    assert!(
        new_ids(&y_before, &y_after).is_empty(),
        "org1 session must never appear in a foreign org's team listing"
    );
    assert!(
        !y_after.contains(&new_id),
        "team B's connection_id must not be visible in a foreign org's listing"
    );
    // The org fixture is fully local to this test; silence the unused-field lint.
    let _ = fx.org_id;
}

// --- Criterion 2: one session authorized for A and B gets distinct, stable per-team
// --- connection ids ------------------------------------------------------------------

#[tokio::test]
async fn multi_team_session_presents_distinct_stable_ids_per_team() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    let agent_token = create_agent(
        &env,
        &fx.admin_token,
        "cp-tool",
        vec![
            (fx.team_a_id, "clusters", "read"),
            (fx.team_b_id, "clusters", "read"),
        ],
    )
    .await;

    let a_before = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_before = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;

    // ONE session, authorized tools/call for A and then for B.
    let session = initialize(&env, &agent_token).await;
    for team in [&fx.team_a_name, &fx.team_b_name] {
        let call = tools_call(
            &env,
            &agent_token,
            &session,
            "cp_clusters_list",
            serde_json::json!({ "team": team }),
        )
        .await;
        assert_eq!(
            call["result"]["isError"], false,
            "authorized cp_clusters_list on {team} must succeed: {call}"
        );
    }

    let a_after = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_after = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;
    let a_new = new_ids(&a_before, &a_after);
    let b_new = new_ids(&b_before, &b_after);
    assert_eq!(a_new.len(), 1, "team A gains exactly one entry: {a_new:?}");
    assert_eq!(b_new.len(), 1, "team B gains exactly one entry: {b_new:?}");
    let a_id = a_new.iter().next().expect("a id").clone();
    let b_id = b_new.iter().next().expect("b id").clone();
    assert_ne!(
        a_id, b_id,
        "the same session must present DIFFERENT connection_ids to A and B \
         (per-team ids must not be correlatable across teams)"
    );
    // Neither team's view may leak the other team's identifier for this session.
    assert!(
        !a_after.contains(&b_id),
        "B's id must not appear in A's view"
    );
    assert!(
        !b_after.contains(&a_id),
        "A's id must not appear in B's view"
    );

    // Stability: repeated GETs return the same per-team ids for the session.
    let a_again = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_again = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;
    assert_eq!(
        new_ids(&a_before, &a_again),
        a_new,
        "team A's id for the session must be stable across reads"
    );
    assert_eq!(
        new_ids(&b_before, &b_again),
        b_new,
        "team B's id for the session must be stable across reads"
    );
}

// --- Criterion 3: denied tools/call attributes nothing --------------------------------

#[tokio::test]
async fn denied_tools_call_attributes_no_connection() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    // Principal with a grant on A but NO grant on B.
    let agent_token = create_agent(
        &env,
        &fx.admin_token,
        "cp-tool",
        vec![(fx.team_a_id, "clusters", "read")],
    )
    .await;
    // Grantless same-org member: a second denied principal.
    let member_token = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;

    let a_before = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_before = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;
    let b_sessions_before = active_sessions(&env, &fx.admin_token, &fx.team_b_name).await;

    // Agent (A-grant only) calls into B: authz error, no attribution anywhere.
    let agent_session = initialize(&env, &agent_token).await;
    let denied = tools_call(
        &env,
        &agent_token,
        &agent_session,
        "cp_clusters_list",
        serde_json::json!({ "team": fx.team_b_name }),
    )
    .await;
    assert!(
        denied.get("error").is_some() || denied["result"]["isError"] == true,
        "cross-team call must be denied: {denied}"
    );
    assert!(
        denied["result"]["isError"] != false,
        "cross-team call must not succeed: {denied}"
    );

    // Grantless member calls into B: authz error via JSON-RPC.
    let member_session = initialize(&env, &member_token).await;
    let denied = tools_call(
        &env,
        &member_token,
        &member_session,
        "cp_clusters_list",
        serde_json::json!({ "team": fx.team_b_name }),
    )
    .await;
    assert_eq!(
        denied["error"]["data"]["kind"], "authz",
        "grantless member must get a JSON-RPC authz error: {denied}"
    );

    // Neither denied session may surface in B's listing or count.
    let b_after = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;
    assert!(
        new_ids(&b_before, &b_after).is_empty(),
        "denied calls must not attribute a connection to B: new {:?}",
        new_ids(&b_before, &b_after)
    );
    assert_eq!(
        active_sessions(&env, &fx.admin_token, &fx.team_b_name).await,
        b_sessions_before,
        "denied calls must not increase B's active_sessions"
    );

    // And the denied B-call did not attribute to A either (no A activity happened).
    let a_after = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    assert!(
        new_ids(&a_before, &a_after).is_empty(),
        "a denied call for B must not attribute the session to A: new {:?}",
        new_ids(&a_before, &a_after)
    );
}

// --- Criterion 4: tools/list stamps attribution only when the principal has an
// --- MCP-relevant grant on the team ----------------------------------------------------

#[tokio::test]
async fn tools_list_attributes_only_with_relevant_grant() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    // (a) Agent with a tool grant on A: tools/list {team: A} alone attributes to A.
    let agent_token = create_agent(
        &env,
        &fx.admin_token,
        "cp-tool",
        vec![(fx.team_a_id, "clusters", "read")],
    )
    .await;
    let a_before = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;

    let agent_session = initialize(&env, &agent_token).await;
    let listed = tools_list(&env, &agent_token, &agent_session, &fx.team_a_name).await;
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    assert!(
        tools.iter().any(|t| t["name"] == "cp_clusters_list"),
        "granted agent must see cp_clusters_list for A: {listed}"
    );

    let a_after = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    assert_eq!(
        new_ids(&a_before, &a_after).len(),
        1,
        "tools/list by a granted principal must attribute one connection to A: new {:?}",
        new_ids(&a_before, &a_after)
    );

    // (b) Principal with zero MCP-relevant grants on B: tools/list {team: B} attributes
    // nothing (an empty tools array in the response is acceptable; the contract here is
    // the connections listing).
    let member_token = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    let b_before = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;

    let member_session = initialize(&env, &member_token).await;
    let _ = tools_list(&env, &member_token, &member_session, &fx.team_b_name).await;

    let b_after = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;
    assert!(
        new_ids(&b_before, &b_after).is_empty(),
        "tools/list by a grantless principal must not attribute to B: new {:?}",
        new_ids(&b_before, &b_after)
    );
}

// --- Criterion 5: initialize alone attributes to no team -------------------------------

#[tokio::test]
async fn initialize_alone_attributes_to_no_team() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    let a_before = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_before = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;
    let a_sessions_before = active_sessions(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_sessions_before = active_sessions(&env, &fx.admin_token, &fx.team_b_name).await;

    // Org admin (broadest same-org standing) initializes and does nothing else.
    let _session = initialize(&env, &fx.admin_token).await;

    let a_after = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_after = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;
    assert!(
        new_ids(&a_before, &a_after).is_empty(),
        "initialize-only session must not appear in A: new {:?}",
        new_ids(&a_before, &a_after)
    );
    assert!(
        new_ids(&b_before, &b_after).is_empty(),
        "initialize-only session must not appear in B: new {:?}",
        new_ids(&b_before, &b_after)
    );
    assert_eq!(
        active_sessions(&env, &fx.admin_token, &fx.team_a_name).await,
        a_sessions_before,
        "initialize-only session must not count toward A's active_sessions"
    );
    assert_eq!(
        active_sessions(&env, &fx.admin_token, &fx.team_b_name).await,
        b_sessions_before,
        "initialize-only session must not count toward B's active_sessions"
    );
}

// --- Criterion 4 corollary: attribution is authorization-based, not result-count-based --

/// A principal whose ONLY grant on the team is the dynamic-tool execute grant
/// (`mcp-tools:execute`), on a team with zero published `api_*` tools. Its
/// `tools/list {team}` may legitimately return an empty tools array — but the call
/// passed the team's authorization, so it must still stamp exactly one connection.
#[tokio::test]
async fn tools_list_dynamic_only_grant_stamps_even_with_zero_published_tools() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    // gateway-tool is the agent kind that carries mcp-tools:execute (dynamic tools only);
    // team A has no published api definitions, so there is nothing to list.
    let agent_token = create_agent(
        &env,
        &fx.admin_token,
        "gateway-tool",
        vec![(fx.team_a_id, "mcp-tools", "execute")],
    )
    .await;

    let a_before = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;

    let session = initialize(&env, &agent_token).await;
    let listed = tools_list(&env, &agent_token, &session, &fx.team_a_name).await;
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    assert!(
        tools.iter().all(|t| t["name"]
            .as_str()
            .map(|n| n.starts_with("api_"))
            .unwrap_or(false)),
        "dynamic-only principal on a team with no published APIs must see no static \
         tools (an empty array is expected): {listed}"
    );

    let a_after = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    assert_eq!(
        new_ids(&a_before, &a_after).len(),
        1,
        "tools/list must attribute the session even when zero tools are listable: \
         attribution is authorization-based, not result-count-based (new {:?})",
        new_ids(&a_before, &a_after)
    );
}

// --- idle_seconds is relative to the TEAM's authorized activity, not the session -------

/// One session authorized for A, then (after a real ~3s gap) for B: A's team-relative
/// idle clock must keep running (>= 2s) and stay ahead of B's — later activity on
/// another team must not reset A's idle_seconds. (Latency-tolerant sanity check; the
/// deterministic pin of team-relative rendering is the `visible_sessions` unit test
/// with crafted clocks in `mcp_api.rs`.)
#[tokio::test]
async fn idle_seconds_is_team_relative_not_session_global() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    let agent_token = create_agent(
        &env,
        &fx.admin_token,
        "cp-tool",
        vec![
            (fx.team_a_id, "clusters", "read"),
            (fx.team_b_id, "clusters", "read"),
        ],
    )
    .await;

    let a_before = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    let b_before = connection_ids(&env, &fx.admin_token, &fx.team_b_name).await;

    let session = initialize(&env, &agent_token).await;
    let call_a = tools_call(
        &env,
        &agent_token,
        &session,
        "cp_clusters_list",
        serde_json::json!({ "team": fx.team_a_name }),
    )
    .await;
    assert_eq!(call_a["result"]["isError"], false, "call for A: {call_a}");

    // Real sleep: this measures the team-relative idle clock, not a TTL.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let call_b = tools_call(
        &env,
        &agent_token,
        &session,
        "cp_clusters_list",
        serde_json::json!({ "team": fx.team_b_name }),
    )
    .await;
    assert_eq!(call_b["result"]["isError"], false, "call for B: {call_b}");

    // Read B BEFORE A: b_idle is then measured at the earlier read against the later
    // stamp, and a_idle at the later read against the earlier stamp, so the relative
    // assertion below holds under arbitrary request latency (no wall-clock assumption).
    let b_connections = connections(&env, &fx.admin_token, &fx.team_b_name).await;
    let a_connections = connections(&env, &fx.admin_token, &fx.team_a_name).await;
    let a_new = new_ids(&a_before, &ids_of(&a_connections));
    let b_new = new_ids(&b_before, &ids_of(&b_connections));
    assert_eq!(a_new.len(), 1, "one new entry in A: {a_new:?}");
    assert_eq!(b_new.len(), 1, "one new entry in B: {b_new:?}");
    let a_id = a_new.iter().next().expect("a id").clone();
    let b_id = b_new.iter().next().expect("b id").clone();

    let a_entry = a_connections
        .iter()
        .find(|c| c["connection_id"] == a_id.as_str())
        .expect("A's entry");
    let b_entry = b_connections
        .iter()
        .find(|c| c["connection_id"] == b_id.as_str())
        .expect("B's entry");
    let a_idle = a_entry["idle_seconds"].as_u64().expect("A idle_seconds");
    let b_idle = b_entry["idle_seconds"].as_u64().expect("B idle_seconds");
    assert!(
        a_idle >= 2,
        "A's idle clock must keep running after the 3s gap despite later B activity \
         in the same session (team-relative, not session-global): got {a_idle}s"
    );
    // Relative bound, not wall-clock: with B read before A (above), b_idle is bounded
    // by an earlier read against a later stamp and a_idle by a later read against an
    // earlier stamp, so b_idle < a_idle cannot flake from request latency. (Sanity
    // check only — the deterministic session-global-regression pin is the crafted-clock
    // `visible_sessions` unit test in mcp_api.rs.)
    assert!(
        b_idle < a_idle,
        "B's idle clock must be fresher than A's (team-relative clocks): a={a_idle}s b={b_idle}s"
    );
}

// --- Slice 2: the session is not an authorization cache (per-request re-auth) ----------

/// Revoking a grant, then issuing tools/call on the SAME session, must deny: earlier
/// successful authorization on the session confers nothing to later requests. The
/// pre-revocation connections entry may legitimately remain listed (attribution is a
/// recent-activity display, not a live-grants view), but the denied call must not add
/// a second entry.
#[tokio::test]
async fn grant_revocation_denies_next_call_on_same_session() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    let agent_token = create_agent(
        &env,
        &fx.admin_token,
        "cp-tool",
        vec![(fx.team_a_id, "clusters", "read")],
    )
    .await;
    // Locate the grant row to revoke. Team A is uuid-unique to this test and this agent
    // holds its only clusters:read grant, so the row is unambiguous. (Direct row lookup
    // mirrors tests/agent_auth.rs; revocation itself goes through the product surface.)
    let grant_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM agent_grants \
         WHERE team_id = $1 \
           AND resource = 'clusters' AND action = 'read'",
    )
    .bind(fx.team_a_id.as_uuid())
    .fetch_one(&env.pool)
    .await
    .expect("agent grant row");

    let a_before = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;

    // Same-session authorized call succeeds and attributes one connection to A.
    let session = initialize(&env, &agent_token).await;
    let granted = tools_call(
        &env,
        &agent_token,
        &session,
        "cp_clusters_list",
        serde_json::json!({ "team": fx.team_a_name }),
    )
    .await;
    assert_eq!(
        granted["result"]["isError"], false,
        "pre-revocation cp_clusters_list on A must succeed: {granted}"
    );
    let a_after_grant = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    assert_eq!(
        new_ids(&a_before, &a_after_grant).len(),
        1,
        "the authorized call attributes exactly one connection to A"
    );

    // Revoke the grant through the product grant-management surface.
    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/api/v1/teams/{}/grants/{grant_id}",
                    fx.team_a_name
                ))
                .header("authorization", format!("Bearer {}", fx.admin_token))
                .body(Body::empty())
                .expect("revoke request"),
        )
        .await
        .expect("revoke response");
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "grant revocation must succeed"
    );

    // The SAME session's next call must be denied: per-request re-auth, no session cache.
    let denied = tools_call(
        &env,
        &agent_token,
        &session,
        "cp_clusters_list",
        serde_json::json!({ "team": fx.team_a_name }),
    )
    .await;
    assert_eq!(
        denied["error"]["data"]["kind"], "authz",
        "post-revocation call on the same session must be an authz error \
         (the session is not an authorization cache): {denied}"
    );

    // The denied call must not attribute a SECOND entry to A. (The pre-revocation entry
    // may still be listed — attribution is recent-activity display, not live grants.)
    let a_after_denied = connection_ids(&env, &fx.admin_token, &fx.team_a_name).await;
    assert!(
        new_ids(&a_after_grant, &a_after_denied).is_empty(),
        "a denied post-revocation call must not add a new connection entry: new {:?}",
        new_ids(&a_after_grant, &a_after_denied)
    );
}
