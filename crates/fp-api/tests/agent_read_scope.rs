//! Black-box read-scoping + authz contract tests for the agent READ endpoints
//! (slice fpv2-5kn.1 row scoping, updated for fpv2-5kn.2 / S2 list contract).
//!
//! Endpoints under test:
//!   * `GET /api/v1/agents`            → the uniform Page envelope
//!     `{ items: [<agent views>], total, limit, offset }` (S2 breaking change from
//!     the S1 bare array). `limit` defaults to 50, clamped 1..=500; `offset` floored
//!     at 0 (a negative offset is treated as 0, not an error). `items` are ordered by
//!     agent name ASC then id ASC, and `total` is the caller-authorized count AFTER
//!     row-scoping and the optional `?team=` filter (not the page length). The
//!     optional `?team=<name|uuid>` filter narrows the list to agents holding at least
//!     one grant on that team; a `?team=` naming a team in ANOTHER org, or an
//!     unresolvable value, is 404; a multi-org caller with no selector using a `?team=`
//!     NAME is 400 `org_selector_required`.
//!   * `GET /api/v1/agents/{agent_id}` → single agent view, or an error (404).
//!     UNCHANGED by S2.
//!
//! Contract asserted purely from acceptance criteria (fpv2-5kn AC3/AC4/AC5/AC6):
//!   * AC3 — the list returns the Page envelope: `items`/`total`/`limit`/`offset`
//!     present, `total` is the authorized count, `items` ordered name-ASC, `limit`
//!     caps and `offset` skips while `total` stays the full authorized count; the
//!     `?team=` filter narrows by team name or UUID, cross-org / unresolvable team
//!     values 404, and a multi-org `?team=<name>` with no selector is 400.
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
//!   * AC6 — no token/hash/credential field appears in any agent response body
//!     (scanned over the Page `items` for the list).
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
        xds_degraded: None,
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

/// A cp-tool agent with an explicit `name`, holding one `clusters:read` grant on
/// `team_id`. Used to force a predictable name-ASC ordering.
async fn agent_named_granted_on(env: &Env, admin_token: &str, name: &str, team_id: TeamId) -> Uuid {
    let response = env
        .app
        .clone()
        .oneshot(request(
            "POST",
            AGENTS_URI,
            admin_token,
            None,
            Some(serde_json::json!({
                "name": name,
                "kind": "cp-tool",
                "grants": [{
                    "team_id": team_id.as_uuid(),
                    "resource": "clusters",
                    "action": "read"
                }],
            })),
        ))
        .await
        .expect("create named agent");
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "create named agent must succeed"
    );
    let body = json_of(response).await;
    Uuid::parse_str(body["agent"]["id"].as_str().expect("agent id")).expect("uuid")
}

/// The `items` array of a list Page envelope. Fails loudly if the body is a bare
/// array (the pre-S2 shape) or otherwise lacks `items`.
fn items(body: &serde_json::Value) -> &Vec<serde_json::Value> {
    assert!(
        !body.is_array(),
        "list must be the Page envelope object, not a bare array (S2 breaking change): {body}"
    );
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("list Page must carry an `items` array: {body}"))
}

/// Assert the list body is a well-formed Page envelope and return `(items, total)`.
fn page(body: &serde_json::Value) -> (&Vec<serde_json::Value>, i64) {
    let its = items(body);
    for field in ["total", "limit", "offset"] {
        assert!(
            body[field].is_i64() || body[field].is_u64(),
            "Page envelope must carry integer `{field}`: {body}"
        );
    }
    (its, body["total"].as_i64().expect("total i64"))
}

/// The subset of `items` whose ids are in `ours`, in returned order.
fn ordered_ids_among(body: &serde_json::Value, ours: &[Uuid]) -> Vec<Uuid> {
    items(body)
        .iter()
        .filter_map(|a| a["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()))
        .filter(|id| ours.contains(id))
        .collect()
}

fn list_contains(body: &serde_json::Value, agent_id: Uuid) -> bool {
    items(body).iter().any(|a| a["id"] == agent_id.to_string())
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
    team_a_name: String,
    team_b_id: TeamId,
    team_b_name: String,
    admin_token: String,
}

/// One org with two uuid-unique teams A and B plus an org-admin (the agent minter).
async fn org_with_two_teams(env: &Env) -> OrgFixture {
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let team_a_name = unique("team-a");
    let team_a = identity::create_team(&env.pool, org.id, &team_a_name, "")
        .await
        .expect("team a");
    let team_b_name = unique("team-b");
    let team_b = identity::create_team(&env.pool, org.id, &team_b_name, "")
        .await
        .expect("team b");
    let (_, admin_token) = user_with_org_role(env, org.id, OrgRole::Admin).await;
    OrgFixture {
        org_id: org.id,
        team_a_id: team_a.id,
        team_a_name,
        team_b_id: team_b.id,
        team_b_name,
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

    // List body (org admin — contains real agents). Scan the Page `items` so the
    // anti-leak assertion is over the agent views themselves.
    let (status, _, body) = get(&env, AGENTS_URI, &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "list ok: {body}");
    let its = items(&body);
    assert!(
        its.iter().any(|a| a["id"] == agent_a.to_string()),
        "admin list must contain the created agent to make the scan meaningful: {body}"
    );
    scan("agent list items", &serde_json::Value::Array(its.clone()));

    // Detail body.
    let (status, _, body) = get(&env, &agent_uri(agent_a), &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "detail ok: {body}");
    scan("agent detail", &body);
}

// --- AC3 (S2): the list endpoint returns the uniform Page envelope --------------------

/// The list URI with a raw query string appended.
fn agents_query(query: &str) -> String {
    format!("{AGENTS_URI}?{query}")
}

#[tokio::test]
async fn list_returns_page_envelope_with_authorized_total() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;
    let agent_b = agent_granted_on(&env, &fx.admin_token, fx.team_b_id).await;

    // A foreign org's agent must not be counted in this org's `total`.
    let other = org_with_two_teams(&env).await;
    let _foreign = agent_granted_on(&env, &other.admin_token, other.team_a_id).await;

    let (status, _, body) = get(&env, AGENTS_URI, &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "list ok: {body}");

    // Shape: an object with items/total/limit/offset (NOT a bare array).
    let (its, total) = page(&body);
    assert_eq!(
        body["limit"].as_i64(),
        Some(50),
        "limit defaults to 50: {body}"
    );
    assert_eq!(
        body["offset"].as_i64(),
        Some(0),
        "offset defaults to 0: {body}"
    );

    // The active org holds exactly the two agents this test created; `total` is the
    // authorized count (the foreign-org agent is excluded), and with a fresh org it
    // equals the page length here.
    assert_eq!(
        total, 2,
        "total is the authorized count for this org: {body}"
    );
    assert_eq!(
        its.len(),
        2,
        "both org agents fit on the default page: {body}"
    );
    assert!(
        list_contains(&body, agent_a),
        "team-a agent present: {body}"
    );
    assert!(
        list_contains(&body, agent_b),
        "team-b agent present: {body}"
    );
}

#[tokio::test]
async fn list_items_ordered_by_name_asc() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    // Three agents whose names sort a < b < c, created OUT of that order.
    let prefix = unique("ord");
    let name_a = format!("{prefix}-a");
    let name_b = format!("{prefix}-b");
    let name_c = format!("{prefix}-c");
    let id_c = agent_named_granted_on(&env, &fx.admin_token, &name_c, fx.team_a_id).await;
    let id_a = agent_named_granted_on(&env, &fx.admin_token, &name_a, fx.team_a_id).await;
    let id_b = agent_named_granted_on(&env, &fx.admin_token, &name_b, fx.team_a_id).await;

    let (status, _, body) = get(&env, AGENTS_URI, &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "list ok: {body}");

    let ordered = ordered_ids_among(&body, &[id_a, id_b, id_c]);
    assert_eq!(
        ordered,
        vec![id_a, id_b, id_c],
        "items must come back sorted by agent name ASC (created c,a,b): {body}"
    );
}

#[tokio::test]
async fn team_filter_by_name_narrows_to_that_team() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;
    let agent_b = agent_granted_on(&env, &fx.admin_token, fx.team_b_id).await;

    // ?team=<team-a name>: only the team-a agent.
    let uri = agents_query(&format!("team={}", fx.team_a_name));
    let (status, _, body) = get(&env, &uri, &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "team-a filter ok: {body}");
    let (_, total) = page(&body);
    assert_eq!(total, 1, "team-a filter narrows total to one agent: {body}");
    assert!(
        list_contains(&body, agent_a),
        "team-a agent present: {body}"
    );
    assert!(
        !list_contains(&body, agent_b),
        "team-b agent must be filtered out by ?team=<team-a>: {body}"
    );

    // ?team=<team-b name>: only the team-b agent (exercises the other team name).
    let uri = agents_query(&format!("team={}", fx.team_b_name));
    let (status, _, body) = get(&env, &uri, &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "team-b filter ok: {body}");
    let (_, total) = page(&body);
    assert_eq!(total, 1, "team-b filter narrows total to one agent: {body}");
    assert!(
        list_contains(&body, agent_b),
        "team-b agent present: {body}"
    );
    assert!(
        !list_contains(&body, agent_a),
        "team-a agent must be filtered out by ?team=<team-b>: {body}"
    );
}

#[tokio::test]
async fn team_filter_by_uuid_matches_by_name() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;
    let agent_b = agent_granted_on(&env, &fx.admin_token, fx.team_b_id).await;

    // ?team=<team-a UUID> behaves exactly like ?team=<team-a name>.
    let uri = agents_query(&format!("team={}", fx.team_a_id.as_uuid()));
    let (status, _, body) = get(&env, &uri, &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "team uuid filter ok: {body}");
    let (_, total) = page(&body);
    assert_eq!(
        total, 1,
        "team uuid filter narrows total to one agent: {body}"
    );
    assert!(
        list_contains(&body, agent_a),
        "team-a agent present: {body}"
    );
    assert!(
        !list_contains(&body, agent_b),
        "team-b agent filtered out by ?team=<team-a uuid>: {body}"
    );
}

#[tokio::test]
async fn team_filter_cross_org_uuid_is_404() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let _agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;

    // A real team that lives in a DIFFERENT org.
    let other = org_with_two_teams(&env).await;

    // org-1 admin filtering by org-2's team UUID: the resolver is global but the
    // endpoint must reject cross-org with 404 (never leak org-2 rows).
    let uri = agents_query(&format!("team={}", other.team_a_id.as_uuid()));
    let (status, rid, body) = get(&env, &uri, &fx.admin_token, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "filtering by a team UUID in another org must be 404, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);
}

#[tokio::test]
async fn team_filter_unresolvable_is_404() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let _agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;

    // Unknown UUID → 404.
    let uri = agents_query(&format!("team={}", Uuid::now_v7()));
    let (status, rid, body) = get(&env, &uri, &fx.admin_token, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unknown team UUID must be 404, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);

    // Bogus name → 404.
    let uri = agents_query(&format!("team={}", unique("no-such-team")));
    let (status, rid, body) = get(&env, &uri, &fx.admin_token, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unresolvable team name must be 404, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);
}

#[tokio::test]
async fn multi_org_team_name_no_selector_is_400() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let _agent_a = agent_granted_on(&env, &fx.admin_token, fx.team_a_id).await;
    let org_b = org_with_two_teams(&env).await;

    // Member of BOTH orgs holding agents:read on org_a/team_a. Resolving a ?team=
    // NAME needs an active org, so with no selector it must be 400 org_selector_required.
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

    let uri = agents_query(&format!("team={}", fx.team_a_name));
    let (status, rid, body) = get(&env, &uri, &token, None).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "multi-org ?team=<name> with no selector must be 400, got {status}: {body}"
    );
    assert_error_envelope(&body, "org_selector_required", rid);
}

#[tokio::test]
async fn list_paging_limit_and_offset() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    // Three agents with a known name order a < b < c.
    let prefix = unique("page");
    let id_a =
        agent_named_granted_on(&env, &fx.admin_token, &format!("{prefix}-a"), fx.team_a_id).await;
    let id_b =
        agent_named_granted_on(&env, &fx.admin_token, &format!("{prefix}-b"), fx.team_a_id).await;
    let id_c =
        agent_named_granted_on(&env, &fx.admin_token, &format!("{prefix}-c"), fx.team_a_id).await;

    // Page 1: limit=2 caps items to 2; total is the full authorized count (3).
    let (status, _, body) = get(&env, &agents_query("limit=2"), &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "page 1 ok: {body}");
    let (its, total) = page(&body);
    assert_eq!(body["limit"].as_i64(), Some(2), "limit echoed: {body}");
    assert_eq!(its.len(), 2, "limit=2 caps items to 2: {body}");
    assert_eq!(total, 3, "total stays the full authorized count: {body}");
    assert_eq!(
        ordered_ids_among(&body, &[id_a, id_b, id_c]),
        vec![id_a, id_b],
        "page 1 holds the first two by name ASC: {body}"
    );

    // Page 2: offset=2 skips the first two; total unchanged.
    let (status, _, body) = get(
        &env,
        &agents_query("limit=2&offset=2"),
        &fx.admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "page 2 ok: {body}");
    let (its, total) = page(&body);
    assert_eq!(body["offset"].as_i64(), Some(2), "offset echoed: {body}");
    assert_eq!(its.len(), 1, "one agent remains after offset=2: {body}");
    assert_eq!(total, 3, "total is stable across pages: {body}");
    assert_eq!(
        ordered_ids_among(&body, &[id_a, id_b, id_c]),
        vec![id_c],
        "page 2 holds the last agent by name ASC: {body}"
    );
}
