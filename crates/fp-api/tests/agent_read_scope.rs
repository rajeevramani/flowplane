//! Black-box read-scoping + authz contract tests for the agent READ endpoints
//! (slice fpv2-5kn.1 — agent reads on the shared authz engine with row scoping).
//!
//! Endpoints under test:
//!   * `GET /api/v1/agents`            → bare JSON array of agent views (NOT paged).
//!   * `GET /api/v1/agents/{agent_id}` → single agent view, or an error (404).
//!
//! Contract asserted purely from acceptance criteria (fpv2-5kn.1 AC4/AC5/AC6):
//!   * AC4 — org admin reads EVERY agent in the active org (and any one by id); an
//!     org admin who ALSO holds an explicit matching grant is not narrowed. A
//!     non-admin holding `agents:read` on team-a lists ONLY agents holding at least
//!     one grant on team-a (an agent granted only on team-b must NOT appear), and
//!     detail 404s for an out-of-scope agent. A same-org principal with no relevant
//!     authority → 403 on both, audited.
//!   * AC5 — the two `org: None` shapes diverge: a zero-membership caller → 403,
//!     audited `no_matching_grant`; a multi-org caller with no org selector (even
//!     holding `agents:read` grants) → 400 `org_selector_required` with NO
//!     authz-denial audit row (org resolution precedes authorization).
//!   * AC6 — no token/hash/credential field appears in any agent response body.
//!
//! Parallel-safe (constitution invariant 18): every org/team/user/agent is
//! uuid-suffixed and unique per test; assertions are set-membership over rows each
//! test created (id-based contains / absence, never global counts); audit checks are
//! keyed by this request's `x-request-id`. Skipped (with a notice) when
//! FLOWPLANE_TEST_DATABASE_URL is unset.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::authz::{Action, Resource};
use fp_domain::{OrgId, OrgRole, TeamId, UserId};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

/// Header carrying the active-org selector (an org name or UUID).
const ORG_SELECTOR_HEADER: &str = "x-flowplane-org";

const AGENTS_URI: &str = "/api/v1/agents";

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

/// Mint a bearer token for a user with ZERO org memberships (`org: None`, not multi-org).
async fn user_with_no_memberships(env: &Env) -> String {
    let subject = unique("sub");
    let email = format!("{}@test", unique("user"));
    identity::upsert_user_by_subject(&env.pool, &subject, &email, "Test User")
        .await
        .expect("user");
    env.issuer
        .mint(&subject, &email, "Test User", 600)
        .expect("mint")
}

fn request(
    method: &str,
    uri: &str,
    token: &str,
    org_selector: Option<&str>,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    if let Some(selector) = org_selector {
        builder = builder.header(ORG_SELECTOR_HEADER, selector);
    }
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

/// GET `uri` as `token` (optionally with an org selector header), returning
/// (status, x-request-id header, JSON body).
async fn get(
    env: &Env,
    uri: &str,
    token: &str,
    org_selector: Option<&str>,
) -> (StatusCode, Option<Uuid>, serde_json::Value) {
    let response = env
        .app
        .clone()
        .oneshot(request("GET", uri, token, org_selector, None))
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

fn agent_uri(agent_id: Uuid) -> String {
    format!("/api/v1/agents/{agent_id}")
}

/// Create an agent through the product surface (org-admin token), returning its id.
async fn create_agent(
    env: &Env,
    admin_token: &str,
    kind: &str,
    grants: Vec<serde_json::Value>,
) -> Uuid {
    let response = env
        .app
        .clone()
        .oneshot(request(
            "POST",
            AGENTS_URI,
            admin_token,
            None,
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
        "create {kind} agent must succeed"
    );
    let body = json_of(response).await;
    Uuid::parse_str(body["agent"]["id"].as_str().expect("agent id")).expect("uuid")
}

/// A cp-tool agent holding a single `clusters:read` grant on `team_id`.
async fn agent_granted_on(env: &Env, admin_token: &str, team_id: TeamId) -> Uuid {
    create_agent(
        env,
        admin_token,
        "cp-tool",
        vec![serde_json::json!({
            "team_id": team_id.as_uuid(),
            "resource": "clusters",
            "action": "read"
        })],
    )
    .await
}

fn list_contains(body: &serde_json::Value, agent_id: Uuid) -> bool {
    body.as_array()
        .expect("agent list is a JSON array")
        .iter()
        .any(|a| a["id"] == agent_id.to_string())
}

/// Assert an error body is the standard envelope object (never a leaked agent array),
/// whose request_id matches the x-request-id header.
fn assert_error_envelope(body: &serde_json::Value, code: &str, rid: Option<Uuid>) {
    assert!(
        body.is_object(),
        "error responses must be the envelope object, not agent data: {body}"
    );
    assert!(
        !body.is_array(),
        "an error must never carry an agent list: {body}"
    );
    assert_eq!(body["code"], code, "unexpected error code in {body}");
    let rid = rid.expect("x-request-id header present");
    assert_eq!(
        body["request_id"],
        rid.to_string(),
        "envelope and header request id agree"
    );
}

/// Count `authz.denied` audit rows carrying `reason = no_matching_grant` for a request.
async fn no_matching_grant_denials(pool: &PgPool, rid: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log \
         WHERE action = 'authz.denied' AND outcome = 'denied' \
           AND request_id = $1 AND detail->>'reason' = 'no_matching_grant'",
    )
    .bind(rid)
    .fetch_one(pool)
    .await
    .expect("audit denial count")
}

/// Count ANY `authz.denied` audit rows for a request (used to prove no denial was written).
async fn any_denials(pool: &PgPool, rid: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'authz.denied' AND request_id = $1",
    )
    .bind(rid)
    .fetch_one(pool)
    .await
    .expect("audit denial count")
}

struct OrgFixture {
    org_id: OrgId,
    team_a_id: TeamId,
    team_b_id: TeamId,
    admin_token: String,
}

/// One org with two uuid-unique teams A and B plus an org-admin (the agent minter).
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
    let (_, admin_token) = user_with_org_role(env, org.id, OrgRole::Admin).await;
    OrgFixture {
        org_id: org.id,
        team_a_id: team_a.id,
        team_b_id: team_b.id,
        admin_token,
    }
}

// --- AC4: org admin reads every agent in the active org -------------------------------

#[tokio::test]
async fn org_admin_lists_every_agent_in_org() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;
    let agent_b = agent_granted_on(&env, &fx.admin_token, fx.team_b_id).await;

    // A foreign org's agent must never surface in this org's list.
    let other = org_with_two_teams(&env).await;
    let foreign_agent = agent_granted_on(&env, &other.admin_token, other.team_a_id).await;

    let (status, _, body) = get(&env, AGENTS_URI, &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "org admin lists agents: {body}");
    assert!(
        list_contains(&body, agent_a),
        "org admin must see the team-a agent: {body}"
    );
    assert!(
        list_contains(&body, agent_b),
        "org admin must see the team-b agent (org-wide, not team-scoped): {body}"
    );
    assert!(
        !list_contains(&body, foreign_agent),
        "org admin must NOT see a foreign org's agent: {body}"
    );

    // Detail: the org admin can read any agent in the org by id.
    for id in [agent_a, agent_b] {
        let (status, _, body) = get(&env, &agent_uri(id), &fx.admin_token, None).await;
        assert_eq!(status, StatusCode::OK, "org admin reads agent {id}: {body}");
        assert_eq!(
            body["id"],
            id.to_string(),
            "detail returns the agent: {body}"
        );
    }
    // Foreign-org agent by id must not be visible (out of the active org).
    let (status, _, body) = get(&env, &agent_uri(foreign_agent), &fx.admin_token, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "foreign-org agent must be absent to this org's admin, got {status}: {body}"
    );
    let _ = fx.org_id;
}

#[tokio::test]
async fn org_admin_with_explicit_matching_grant_still_sees_all() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;
    let agent_b = agent_granted_on(&env, &fx.admin_token, fx.team_b_id).await;

    // A NEW org admin who ALSO holds an explicit `agents:read` grant scoped to team-a:
    // holding a specific grant must not narrow admin standing down to team-a's agents.
    let (admin, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    identity::add_grant(
        &env.pool,
        admin,
        fx.org_id,
        fx.team_a_id,
        Resource::Agents,
        Action::Read,
        None,
    )
    .await
    .expect("agents:read on team A for the admin");

    let (status, _, body) = get(&env, AGENTS_URI, &admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "admin+grant lists agents: {body}");
    assert!(
        list_contains(&body, agent_a),
        "admin+grant must still see the team-a agent: {body}"
    );
    assert!(
        list_contains(&body, agent_b),
        "admin+grant must STILL see the team-b agent — an explicit team-a grant must \
         not narrow an org admin to team-a's agents: {body}"
    );
}

// --- AC4: non-admin holder of agents:read on team-a is row-scoped ----------------------

#[tokio::test]
async fn team_a_reader_lists_only_team_a_granted_agents() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;
    let agent_b = agent_granted_on(&env, &fx.admin_token, fx.team_b_id).await;

    // Non-admin org member holding `agents:read` on team-a ONLY.
    let (reader, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        reader,
        fx.org_id,
        fx.team_a_id,
        Resource::Agents,
        Action::Read,
        None,
    )
    .await
    .expect("agents:read on team A");

    let (status, _, body) = get(&env, AGENTS_URI, &token, None).await;
    assert_eq!(status, StatusCode::OK, "team-a reader lists agents: {body}");
    assert!(
        list_contains(&body, agent_a),
        "team-a reader must see the team-a-granted agent: {body}"
    );
    assert!(
        !list_contains(&body, agent_b),
        "team-a reader must NOT see an agent granted only on team-b (never the \
         org-wide set): {body}"
    );
}

#[tokio::test]
async fn team_a_reader_detail_is_scoped_to_team_a() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;
    let agent_b = agent_granted_on(&env, &fx.admin_token, fx.team_b_id).await;

    let (reader, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        reader,
        fx.org_id,
        fx.team_a_id,
        Resource::Agents,
        Action::Read,
        None,
    )
    .await
    .expect("agents:read on team A");

    // In-scope agent: readable.
    let (status, _, body) = get(&env, &agent_uri(agent_a), &token, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "team-a reader reads the team-a agent, got {status}: {body}"
    );
    assert_eq!(body["id"], agent_a.to_string(), "detail is agent A: {body}");

    // Out-of-scope agent (granted only on team-b): 404 error envelope, never the agent
    // view. (The 404 message may echo the id the caller supplied in the URL — that is
    // not a leak; the anti-leak check is that NO agent DATA fields come back.)
    let (status, rid, body) = get(&env, &agent_uri(agent_b), &token, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "team-a reader must NOT read a team-b-only agent (detail 404), got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);
    for leaked in ["kind", "status", "org_id"] {
        assert!(
            body.get(leaked).is_none(),
            "the 404 body must not carry the agent's {leaked} field: {body}"
        );
    }
}

// --- AC4: no relevant authority → 403 both, audited ------------------------------------

#[tokio::test]
async fn same_org_member_without_grant_denied_and_audited() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;

    // Same-org member: active org resolves (single membership) but no `agents:read`
    // grant and not an admin — no relevant authority.
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;

    // List → 403, audited no_matching_grant.
    let (status, rid, body) = get(&env, AGENTS_URI, &token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "member without agents:read must be denied the list, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    let rid = rid.expect("request id");
    assert!(
        no_matching_grant_denials(&env.pool, rid).await >= 1,
        "the list denial must be audited authz.denied/no_matching_grant for request {rid}"
    );

    // Detail → 403, audited no_matching_grant.
    let (status, rid, body) = get(&env, &agent_uri(agent_a), &token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "member without agents:read must be denied detail, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    let rid = rid.expect("request id");
    assert!(
        no_matching_grant_denials(&env.pool, rid).await >= 1,
        "the detail denial must be audited authz.denied/no_matching_grant for request {rid}"
    );
}

// --- AC5: the two `org: None` shapes ---------------------------------------------------

#[tokio::test]
async fn zero_membership_caller_denied_and_audited() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;

    // A user with ZERO org memberships (org: None, NOT multi-org). No selector is
    // required — there is nothing to select — so authorization runs and denies.
    let token = user_with_no_memberships(&env).await;

    let (status, rid, body) = get(&env, AGENTS_URI, &token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "zero-membership caller must be denied the list (not 400), got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    let rid = rid.expect("request id");
    assert!(
        no_matching_grant_denials(&env.pool, rid).await >= 1,
        "the zero-membership list denial must be audited no_matching_grant for {rid}"
    );

    let (status, rid, body) = get(&env, &agent_uri(agent_a), &token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "zero-membership caller must be denied detail (not 400), got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    let rid = rid.expect("request id");
    assert!(
        no_matching_grant_denials(&env.pool, rid).await >= 1,
        "the zero-membership detail denial must be audited no_matching_grant for {rid}"
    );
}

#[tokio::test]
async fn multi_org_no_selector_gets_400_and_writes_no_denial_audit() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;

    // A second org the caller also belongs to.
    let org_b = org_with_two_teams(&env).await;

    // The adversarial principal: a member of BOTH orgs who genuinely HOLDS an
    // `agents:read` grant (on org_a/team_a). With no selector the active org is
    // ambiguous, so org resolution must fail-fast with 400 org_selector_required —
    // BEFORE authorization — never a 403 and never a leak of the granted agent.
    let (user, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_org_membership(&env.pool, user, org_b.org_id, OrgRole::Member)
        .await
        .expect("second org membership");
    identity::add_grant(
        &env.pool,
        user,
        fx.org_id,
        fx.team_a_id,
        Resource::Agents,
        Action::Read,
        None,
    )
    .await
    .expect("agents:read on org_a team A");

    // List → 400 org_selector_required, no authz-denial row, no agent data.
    let (status, rid, body) = get(&env, AGENTS_URI, &token, None).await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "a multi-org holder of agents:read must NOT get 403: {body}"
    );
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "multi-org, no selector must be 400 org_selector_required, got {status}: {body}"
    );
    assert_error_envelope(&body, "org_selector_required", rid);
    // The body is the error envelope (an object, not a list) — no granted agent may
    // ride along on the selector-required response.
    assert!(
        !body.to_string().contains(&agent_a.to_string()),
        "no granted agent may ride along on the selector-required response: {body}"
    );
    let rid = rid.expect("request id");
    assert_eq!(
        any_denials(&env.pool, rid).await,
        0,
        "org resolution precedes authz — a selector-required 400 must write NO \
         authz.denied audit row for request {rid}"
    );

    // Detail → same contract.
    let (status, rid, body) = get(&env, &agent_uri(agent_a), &token, None).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "multi-org detail with no selector must be 400, got {status}: {body}"
    );
    assert_error_envelope(&body, "org_selector_required", rid);
    let rid = rid.expect("request id");
    assert_eq!(
        any_denials(&env.pool, rid).await,
        0,
        "detail selector-required 400 must write NO authz.denied audit row for {rid}"
    );
}

// --- AC6: no credential material in agent response bodies ------------------------------

#[tokio::test]
async fn agent_bodies_carry_no_credential_material() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;
    let _agent_b = agent_granted_on(&env, &fx.admin_token, fx.team_b_id).await;

    // Forbidden substrings scanned case-insensitively across the serialized JSON.
    // "fpat_" is the agent-token prefix — the strongest signal an actual credential
    // leaked; the rest catch stored secrets/hashes appearing under any field name.
    let forbidden = [
        "fpat_",
        "token",
        "hash",
        "secret",
        "password",
        "credential",
        "private_key",
        "api_key",
    ];
    let scan = |label: &str, body: &serde_json::Value| {
        let raw = body.to_string().to_ascii_lowercase();
        for needle in forbidden {
            assert!(
                !raw.contains(needle),
                "{label} response leaked credential-shaped field '{needle}': {body}"
            );
        }
    };

    // List body (org admin — contains real agents).
    let (status, _, body) = get(&env, AGENTS_URI, &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "list ok: {body}");
    assert!(body.is_array(), "list is a bare array: {body}");
    scan("agent list", &body);

    // Detail body.
    let (status, _, body) = get(&env, &agent_uri(agent_a), &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "detail ok: {body}");
    scan("agent detail", &body);
}
