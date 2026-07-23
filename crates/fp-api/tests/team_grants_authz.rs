//! Black-box authorization contract tests for `GET /api/v1/teams/{team}/grants`
//! (slice fpv2-d6c.1 — team-grants authz leak).
//!
//! Written from acceptance criteria only: reading a team's grant list requires the
//! `grants:read` capability on THAT team (or same-org org-admin). Same-org membership,
//! grants on other teams, platform-admin standing, and non-cp-tool agent kinds confer
//! nothing. Cross-org callers get 404 (anti-enumeration), and denials are audited.
//!
//! Parallel-safe: every org/team/user/agent is uuid-suffixed and unique per test;
//! assertions only touch rows each test created; in-process router via `oneshot`.
//! Skipped (with a notice) when FLOWPLANE_TEST_DATABASE_URL is unset.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::authz::{Action, Resource};
use fp_domain::{OrgId, OrgRole, TeamId, UserId};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::{PgPool, Row};
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

struct Env {
    app: axum::Router,
    issuer: DevIssuer,
    pool: PgPool,
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

/// Create a user with one org membership and mint a bearer token for them.
async fn user_with_org_role(env: &Env, org_id: OrgId, role: OrgRole) -> (UserId, String) {
    let subject = unique("sub");
    let email = format!("{}@test", unique("user"));
    let user = identity::upsert_user_by_subject(&env.pool, &subject, &email, "Test User")
        .await
        .expect("user");
    identity::add_org_membership(&env.pool, user, org_id, role)
        .await
        .expect("org membership");
    let token = env
        .issuer
        .mint(&subject, &email, "Test User", 600)
        .expect("mint");
    (user, token)
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

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

/// GET as `token`, returning (status, request-id header, JSON body).
async fn get_json(
    env: &Env,
    uri: &str,
    token: &str,
) -> (StatusCode, Option<Uuid>, serde_json::Value) {
    let response = env
        .app
        .clone()
        .oneshot(request("GET", uri, token, None))
        .await
        .expect("response");
    let status = response.status();
    let rid = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok());
    (status, rid, json_of(response).await)
}

/// Assert a response body is the standard error envelope for `code` and leaks no
/// grant rows (a grants listing is a JSON array; the envelope is an object).
fn assert_error_envelope(body: &serde_json::Value, code: &str, rid: Option<Uuid>) {
    assert!(
        body.is_object(),
        "error responses must be the envelope object, not data: {body}"
    );
    assert_eq!(body["code"], code, "unexpected error code in {body}");
    assert!(
        body.get("resource").is_none() && body.get("action").is_none(),
        "error envelope must not carry grant fields: {body}"
    );
    let rid = rid.expect("x-request-id header present");
    assert_eq!(
        body["request_id"],
        rid.to_string(),
        "envelope and header request id agree"
    );
}

struct OrgFixture {
    org_id: OrgId,
    team_a_id: TeamId,
    team_a_name: String,
    team_b_id: TeamId,
    team_b_name: String,
}

/// One org with two teams A and B, all uuid-unique.
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
    OrgFixture {
        org_id: org.id,
        team_a_id: team_a.id,
        team_a_name: team_a.name,
        team_b_id: team_b.id,
        team_b_name: team_b.name,
    }
}

/// Seed a user-grant row on `team` so the grants listing has real data to leak
/// (a vacuously empty list could mask a broken read path). Returns the grantee.
async fn seed_grant_on(env: &Env, org_id: OrgId, team_id: TeamId) -> UserId {
    let (grantee, _) = user_with_org_role(env, org_id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        grantee,
        org_id,
        team_id,
        Resource::Clusters,
        Action::Read,
        None,
    )
    .await
    .expect("seed grant");
    grantee
}

fn grants_uri(team: &str) -> String {
    format!("/api/v1/teams/{team}/grants")
}

// --- Criterion 1: cross-team leak closed + audit ---------------------------------

#[tokio::test]
async fn member_of_team_a_cannot_read_team_b_grants_and_denial_is_audited() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    seed_grant_on(&env, fx.org_id, fx.team_b_id).await;

    let (attacker, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_team_membership(&env.pool, attacker, fx.team_a_id)
        .await
        .expect("team a membership");

    let (status, rid, body) = get_json(&env, &grants_uri(&fx.team_b_name), &token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "team A member must not read team B grants, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);

    // The denial for THIS request is audited as authz.denied against team B.
    let rid = rid.expect("request id");
    let rows = sqlx::query(
        "SELECT team_id, outcome FROM audit_log \
         WHERE action = 'authz.denied' AND request_id = $1",
    )
    .bind(rid)
    .fetch_all(&env.pool)
    .await
    .expect("audit rows");
    assert!(
        !rows.is_empty(),
        "expected an authz.denied audit row for request {rid}"
    );
    assert!(
        rows.iter()
            .any(|r| r.get::<Option<Uuid>, _>("team_id") == Some(fx.team_b_id.as_uuid())),
        "authz.denied audit row must reference team B"
    );
}

// --- Criterion 2: membership alone confers nothing --------------------------------

#[tokio::test]
async fn member_of_team_b_without_grant_cannot_read_own_team_grants() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    seed_grant_on(&env, fx.org_id, fx.team_b_id).await;

    let (member, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_team_membership(&env.pool, member, fx.team_b_id)
        .await
        .expect("team b membership");

    let (status, rid, body) = get_json(&env, &grants_uri(&fx.team_b_name), &token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "team B membership alone must not grant grants:read, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
}

// --- Criterion 3: grant on another team is insufficient ---------------------------

#[tokio::test]
async fn grants_read_on_team_a_does_not_open_team_b() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    seed_grant_on(&env, fx.org_id, fx.team_b_id).await;

    let (holder, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        holder,
        fx.org_id,
        fx.team_a_id,
        Resource::Grants,
        Action::Read,
        None,
    )
    .await
    .expect("grants:read on team A");

    // Sanity: the grant works where it was given — otherwise the denial below
    // could pass vacuously against a fixture that never authorizes anyone.
    let (status, _, body) = get_json(&env, &grants_uri(&fx.team_a_name), &token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "grants:read on team A must open team A, got {status}: {body}"
    );

    let (status, rid, body) = get_json(&env, &grants_uri(&fx.team_b_name), &token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "grants:read on team A must NOT open team B, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
}

// --- Criterion 4: pure platform admin is denied -----------------------------------

#[tokio::test]
async fn pure_platform_admin_cannot_read_team_grants() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let grantee = seed_grant_on(&env, fx.org_id, fx.team_b_id).await;

    // Platform admin = Owner of the platform org, with no tenant-org membership. The
    // principal, membership, and token are all prepared BEFORE the singleton is touched,
    // so any panic here leaves no shared state behind.
    let platform_org = identity::create_org(&env.pool, &unique("platform-org"), "")
        .await
        .expect("platform org");
    let (_, token) = user_with_org_role(&env, platform_org.id, OrgRole::Owner).await;

    // This test owns instance-level shared state: `instance_meta.platform_org_id` is an
    // instance-wide singleton. Serialize against parallel siblings (the bootstrap tests
    // use the same lock id for this resource) on ONE dedicated connection — advisory
    // locks are connection-scoped — then restore the prior value before asserting.
    let mut lock_conn = env.pool.acquire().await.expect("acquire lock connection");
    sqlx::query("SELECT pg_advisory_lock(420001)")
        .execute(&mut *lock_conn)
        .await
        .expect("advisory lock on instance_meta.platform_org_id");
    let prior: Option<String> =
        sqlx::query_scalar("SELECT value FROM instance_meta WHERE key = 'platform_org_id'")
            .fetch_optional(&mut *lock_conn)
            .await
            .expect("read prior platform_org_id");

    // Address team B by UUID: name resolution needs an org context a pure platform
    // admin does not have; the UUID path is the enumeration attempt. The request is
    // built BEFORE the singleton is touched — its builder panics on malformed input,
    // which must not happen inside the critical section.
    let uri = grants_uri(&fx.team_b_id.as_uuid().to_string());
    let req = request("GET", &uri, &token, None);

    // PANIC-FREE critical section: between the singleton mutation and its restoration no
    // expect/unwrap/assert may run — every fallible step is captured as a Result so that
    // restoration below is reached on EVERY exit path.
    let outcome: Result<(StatusCode, Option<Uuid>, serde_json::Value), String> = async {
        identity::set_platform_org(&env.pool, platform_org.id)
            .await
            .map_err(|e| format!("set platform org: {e}"))?;

        let response = env
            .app
            .clone()
            .oneshot(req)
            .await
            .map_err(|e| format!("send request: {e}"))?;
        let status = response.status();
        let rid = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| Uuid::parse_str(v).ok());
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|e| format!("read body: {e}"))?
            .to_bytes();
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse body: {e}"))?;
        Ok((status, rid, body))
    }
    .await;

    // ALWAYS restore instance_meta.platform_org_id and release the lock — before any
    // unwrap or assertion — so no exit path leaves the singleton mutated for siblings.
    // Restore and unlock outcomes are captured (not expect-ed) so a restore failure
    // still reaches the unlock attempt; panics surface only after both ran. (If unlock
    // itself fails, the connection drop below releases the session-scoped lock.)
    let restore_result = match &prior {
        Some(value) => {
            sqlx::query(
                "INSERT INTO instance_meta (key, value) VALUES ('platform_org_id', $1) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
            )
            .bind(value)
            .execute(&mut *lock_conn)
            .await
        }
        None => {
            sqlx::query("DELETE FROM instance_meta WHERE key = 'platform_org_id'")
                .execute(&mut *lock_conn)
                .await
        }
    };
    let unlock_result = sqlx::query("SELECT pg_advisory_unlock(420001)")
        .execute(&mut *lock_conn)
        .await;
    drop(lock_conn);
    restore_result.expect("restore prior platform_org_id");
    unlock_result.expect("advisory unlock on instance_meta.platform_org_id");

    // Only after restoration: surface any captured failure, then assert the contract.
    let (status, rid, body) = outcome.expect("critical section");
    assert!(
        !status.is_success(),
        "platform admin must not read tenant team grants, got {status}: {body}"
    );
    assert!(
        !body.is_array(),
        "no grant data may leak to a platform admin: {body}"
    );
    assert!(
        !body.to_string().contains(&grantee.as_uuid().to_string()),
        "grantee identity leaked to platform admin: {body}"
    );
    // Pinned observed contract: platform admin addressing a tenant team by UUID.
    assert_eq!(status, StatusCode::FORBIDDEN, "pinned status, body: {body}");
    assert_error_envelope(&body, "forbidden", rid);
}

// --- Criterion 5: legitimate access preserved --------------------------------------

#[tokio::test]
async fn same_org_admin_can_list_team_grants() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let grantee = seed_grant_on(&env, fx.org_id, fx.team_b_id).await;

    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let (status, _, body) = get_json(&env, &grants_uri(&fx.team_b_name), &token).await;
    assert_eq!(status, StatusCode::OK, "org admin reads grants: {body}");
    let grants = body.as_array().expect("grant list");
    assert!(
        grants.iter().any(|g| {
            g["user_id"] == grantee.as_uuid().to_string()
                && g["resource"] == "clusters"
                && g["action"] == "read"
        }),
        "org admin must see team B's grant rows: {body}"
    );
}

#[tokio::test]
async fn exact_grants_read_holder_can_list_team_grants() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let grantee = seed_grant_on(&env, fx.org_id, fx.team_b_id).await;

    let (holder, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        holder,
        fx.org_id,
        fx.team_b_id,
        Resource::Grants,
        Action::Read,
        None,
    )
    .await
    .expect("grants:read on team B");

    let (status, _, body) = get_json(&env, &grants_uri(&fx.team_b_name), &token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "grants:read on B must allow listing B, got {status}: {body}"
    );
    let grants = body.as_array().expect("grant list");
    assert!(
        grants.iter().any(|g| {
            g["user_id"] == grantee.as_uuid().to_string()
                && g["resource"] == "clusters"
                && g["action"] == "read"
        }),
        "holder must see team B's grant rows: {body}"
    );
    assert!(
        grants.iter().any(|g| {
            g["user_id"] == holder.as_uuid().to_string()
                && g["resource"] == "grants"
                && g["action"] == "read"
        }),
        "holder's own grants:read row is listed: {body}"
    );
}

// --- Criterion 6: members roster stays same-org-permissive -------------------------

#[tokio::test]
async fn same_org_member_can_still_list_other_team_members() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    let (roster_user, _) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_team_membership(&env.pool, roster_user, fx.team_b_id)
        .await
        .expect("team b membership");

    let (caller, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_team_membership(&env.pool, caller, fx.team_a_id)
        .await
        .expect("team a membership");

    let (status, _, body) = get_json(
        &env,
        &format!("/api/v1/teams/{}/members", fx.team_b_name),
        &token,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "same-org member roster read must keep working, got {status}: {body}"
    );
    let members = body.as_array().expect("member list");
    assert!(
        members
            .iter()
            .any(|m| m["user_id"] == roster_user.as_uuid().to_string()),
        "team B roster lists its member: {body}"
    );
}

// --- Criterion 7: cross-org anti-enumeration ----------------------------------------

#[tokio::test]
async fn cross_org_caller_gets_404_not_403() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    seed_grant_on(&env, fx.org_id, fx.team_b_id).await;

    let other_org = identity::create_org(&env.pool, &unique("org-p"), "")
        .await
        .expect("other org");
    let (_, token) = user_with_org_role(&env, other_org.id, OrgRole::Admin).await;

    for team_ref in [fx.team_b_name.clone(), fx.team_b_id.as_uuid().to_string()] {
        let (status, rid, body) = get_json(&env, &grants_uri(&team_ref), &token).await;
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "403 for {team_ref} confirms the team exists to an outsider: {body}"
        );
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "cross-org access to {team_ref} must read as absent, got {status}: {body}"
        );
        assert_error_envelope(&body, "not_found", rid);
    }
}

// --- Criterion 8: agent kind matrix -------------------------------------------------

async fn create_agent(
    env: &Env,
    admin_token: &str,
    kind: &str,
    grants: Vec<serde_json::Value>,
) -> (Uuid, String) {
    let response = env
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/agents",
            admin_token,
            Some(serde_json::json!({
                "name": unique("agent"),
                "kind": kind,
                "grants": grants,
            })),
        ))
        .await
        .expect("create agent");
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "create {kind} agent"
    );
    let body = json_of(response).await;
    let id = Uuid::parse_str(body["agent"]["id"].as_str().expect("agent id")).expect("uuid");
    let token = body["token"].as_str().expect("agent token").to_string();
    (id, token)
}

#[tokio::test]
async fn gateway_agent_is_denied_even_with_rogue_grants_read_row() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    seed_grant_on(&env, fx.org_id, fx.team_b_id).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let (agent_id, token) = create_agent(
        &env,
        &admin_token,
        "gateway-tool",
        vec![serde_json::json!({
            "team_id": fx.team_b_id.as_uuid(),
            "resource": "mcp-tools",
            "action": "execute"
        })],
    )
    .await;
    // The API refuses non-mcp-tools grants for gateway agents, so plant the rogue
    // grants:read row directly: kind must dominate even over a present grant row.
    // Stays direct SQL after 0033 rather than moving to the service layer — the row is
    // deliberately one the service will not create, so routing it through the service
    // would destroy exactly what this test proves.
    sqlx::query(
        "INSERT INTO agent_grants (id, agent_id, org_id, team_id, resource, action) \
         VALUES (gen_random_uuid(), $1, $2, $3, 'grants', 'read')",
    )
    .bind(agent_id)
    .bind(fx.org_id.as_uuid())
    .bind(fx.team_b_id.as_uuid())
    .execute(&env.pool)
    .await
    .expect("rogue agent grant row");

    let (status, rid, body) = get_json(&env, &grants_uri(&fx.team_b_name), &token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "gateway-tool agent must never read grants, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
}

#[tokio::test]
async fn api_consumer_agent_is_denied_even_with_rogue_grants_read_row() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    seed_grant_on(&env, fx.org_id, fx.team_b_id).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let (agent_id, token) = create_agent(&env, &admin_token, "api-consumer", vec![]).await;
    // Plant an exact grants:read row for the api-consumer directly: structural
    // (kind-based) denial must dominate even over a persisted matching grant.
    // Direct SQL is intentional here for the same reason as above.
    sqlx::query(
        "INSERT INTO agent_grants (id, agent_id, org_id, team_id, resource, action) \
         VALUES (gen_random_uuid(), $1, $2, $3, 'grants', 'read')",
    )
    .bind(agent_id)
    .bind(fx.org_id.as_uuid())
    .bind(fx.team_b_id.as_uuid())
    .execute(&env.pool)
    .await
    .expect("rogue agent grant row");

    let (status, rid, body) = get_json(&env, &grants_uri(&fx.team_b_name), &token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "api-consumer agent must not read grants even with a grants:read row, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
}

#[tokio::test]
async fn cp_tool_agent_without_grant_is_denied() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    seed_grant_on(&env, fx.org_id, fx.team_b_id).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let (_, token) = create_agent(&env, &admin_token, "cp-tool", vec![]).await;
    let (status, rid, body) = get_json(&env, &grants_uri(&fx.team_b_name), &token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "cp-tool agent without grants:read must be denied, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
}

#[tokio::test]
async fn cp_tool_agent_with_exact_grants_read_can_list() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let grantee = seed_grant_on(&env, fx.org_id, fx.team_b_id).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let (_, token) = create_agent(
        &env,
        &admin_token,
        "cp-tool",
        vec![serde_json::json!({
            "team_id": fx.team_b_id.as_uuid(),
            "resource": "grants",
            "action": "read"
        })],
    )
    .await;
    let (status, _, body) = get_json(&env, &grants_uri(&fx.team_b_name), &token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "cp-tool agent with grants:read on B must list B, got {status}: {body}"
    );
    let grants = body.as_array().expect("grant list");
    assert!(
        grants.iter().any(|g| {
            g["user_id"] == grantee.as_uuid().to_string()
                && g["resource"] == "clusters"
                && g["action"] == "read"
        }),
        "agent sees team B's user grant rows: {body}"
    );
}
