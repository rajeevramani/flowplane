//! Black-box contract tests for the agent-grants READ endpoint (slice fpv2-5kn.3 /
//! S3): `GET /api/v1/agents/{agent_id}/grants`.
//!
//! Endpoint under test returns the uniform Page envelope
//! `{ items: [...], total, limit, offset }`, where each item is
//! `{ id, team_id, team_name, resource, action }`. Authorization is on
//! `Resource::Grants` (`grants:read`), NOT `agents:read`; rows are scoped to the
//! caller's `grants:read` teams. Items order by `team_name ASC, resource ASC,
//! action ASC, id ASC`. `limit` defaults to 50 and clamps to `1..=500`; a NEGATIVE
//! `offset` is rejected with 400 (unlike the agents-list, which floors it). `total`
//! is the authorized row count AFTER scope. A cross-org or unknown `agent_id` is
//! 404 (anti-enumeration); a same-org agent the caller has no `grants:read` overlap
//! with is 200 with an empty `items` (NOT 404).
//!
//! Contract asserted purely from acceptance criteria (fpv2-5kn.3):
//!   * AC1 — an agent granted `[(team-a, api-definitions, update),
//!     (team-b, mcp-tools, read)]`, read by an org admin, returns EXACTLY those two
//!     items, each carrying a grant `id` and the resolved `team_name`, ordered
//!     `team_name ASC, resource ASC, action ASC, id ASC`.
//!   * AC2 — Page contract: `limit` defaults to 50 and clamps to `1..=500`; a
//!     negative `offset` is 400; `total` is the authorized count and stays stable
//!     across pages.
//!   * AC4 — a non-admin holding BOTH `agents:read` AND `grants:read` on team-a,
//!     reading an agent granted on team-a AND team-b, sees ONLY the team-a row; a
//!     non-admin holding `agents:read` but NOT `grants:read` anywhere is 403.
//!   * AC5 — the two `org: None` shapes: a zero-membership caller is 403, audited
//!     `no_matching_grant`; a multi-org caller with no selector (even holding
//!     `grants:read`) is 400 `org_selector_required` with NO authz-denial audit row.
//!   * AC6 — no token/hash/credential field appears in any grants response body
//!     (scanned over the Page `items`).
//!   * cross-org 404 — an admin of org-1 requesting the grants of an agent that
//!     belongs to org-2 is 404 (never 403, never leaked rows).
//!
//! Parallel-safe (constitution invariant 18): every org/team/user/agent is
//! uuid-suffixed and unique per test; assertions are over rows each test created;
//! audit checks are keyed by this request's `x-request-id`. Skipped (with a notice)
//! when FLOWPLANE_TEST_DATABASE_URL is unset.

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

/// The grants URI for an agent id.
fn grants_uri(agent_id: Uuid) -> String {
    format!("/api/v1/agents/{agent_id}/grants")
}

/// The grants URI for an agent id with a raw query string appended.
fn grants_query(agent_id: Uuid, query: &str) -> String {
    format!("/api/v1/agents/{agent_id}/grants?{query}")
}

/// Create a cp-tool agent through the product surface (org-admin token) holding the
/// given grants, returning its id. `grants` is a list of `(team_id, resource, action)`.
async fn create_agent_with_grants(
    env: &Env,
    admin_token: &str,
    grants: &[(TeamId, &str, &str)],
) -> Uuid {
    let grants: Vec<serde_json::Value> = grants
        .iter()
        .map(|(team, resource, action)| {
            serde_json::json!({
                "team_id": team.as_uuid(),
                "resource": resource,
                "action": action,
            })
        })
        .collect();
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
                "kind": "cp-tool",
                "grants": grants,
            })),
        ))
        .await
        .expect("create agent");
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "create cp-tool agent must succeed"
    );
    let body = json_of(response).await;
    Uuid::parse_str(body["agent"]["id"].as_str().expect("agent id")).expect("uuid")
}

/// The `items` array of a Page envelope. Fails loudly if the body is a bare array or
/// otherwise lacks `items`.
fn items(body: &serde_json::Value) -> &Vec<serde_json::Value> {
    assert!(
        !body.is_array(),
        "grants must be the Page envelope object, not a bare array: {body}"
    );
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("grants Page must carry an `items` array: {body}"))
}

/// Assert the body is a well-formed Page envelope and return `(items, total)`.
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

/// The `(resource, action)` key sequence of the returned items, in returned order.
fn keys_of(body: &serde_json::Value) -> Vec<(String, String)> {
    items(body)
        .iter()
        .map(|g| {
            (
                g["resource"].as_str().expect("resource str").to_string(),
                g["action"].as_str().expect("action str").to_string(),
            )
        })
        .collect()
}

/// The `(team_name, resource, action)` triple sequence of the returned items, in
/// returned order.
fn triples_of(body: &serde_json::Value) -> Vec<(String, String, String)> {
    items(body)
        .iter()
        .map(|g| {
            (
                g["team_name"].as_str().expect("team_name str").to_string(),
                g["resource"].as_str().expect("resource str").to_string(),
                g["action"].as_str().expect("action str").to_string(),
            )
        })
        .collect()
}

/// Assert every item is a well-formed grant view: a parseable `id`, a `team_id` UUID,
/// a non-empty `team_name`, and string `resource`/`action`.
fn assert_item_shape(body: &serde_json::Value) {
    for item in items(body) {
        let id = item["id"]
            .as_str()
            .unwrap_or_else(|| panic!("grant item missing id: {item}"));
        Uuid::parse_str(id).unwrap_or_else(|_| panic!("grant id must be a uuid: {item}"));
        let team_id = item["team_id"]
            .as_str()
            .unwrap_or_else(|| panic!("grant item missing team_id: {item}"));
        Uuid::parse_str(team_id).unwrap_or_else(|_| panic!("team_id must be a uuid: {item}"));
        assert!(
            item["team_name"].as_str().is_some_and(|s| !s.is_empty()),
            "grant item must carry a resolved non-empty team_name: {item}"
        );
        assert!(
            item["resource"].as_str().is_some(),
            "grant item must carry a string resource: {item}"
        );
        assert!(
            item["action"].as_str().is_some(),
            "grant item must carry a string action: {item}"
        );
    }
}

/// Assert an error body is the standard envelope object (never a leaked grant array),
/// whose request_id matches the x-request-id header.
fn assert_error_envelope(body: &serde_json::Value, code: &str, rid: Option<Uuid>) {
    assert!(
        body.is_object(),
        "error responses must be the envelope object, not grant data: {body}"
    );
    assert!(
        !body.is_array(),
        "an error must never carry a grant list: {body}"
    );
    assert!(
        body.get("items").is_none(),
        "an error envelope must not carry a Page items array: {body}"
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
/// `unique("team-a")` always sorts before `unique("team-b")`, so team-A rows precede
/// team-B rows under the endpoint's `team_name ASC` ordering.
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

// --- AC1: exact grant rows, resolved team_name, deterministic ordering -----------------

#[tokio::test]
async fn admin_reads_exact_grant_rows_with_team_names_ordered() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[
            (fx.team_a_id, "api-definitions", "update"),
            (fx.team_b_id, "mcp-tools", "read"),
        ],
    )
    .await;

    let (status, _, body) = get(&env, &grants_uri(agent), &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "org admin reads grants: {body}");

    let (its, total) = page(&body);
    assert_eq!(
        total, 2,
        "total is the exact authorized grant count: {body}"
    );
    assert_eq!(its.len(), 2, "both grants fit on the default page: {body}");
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

    assert_item_shape(&body);

    // Ordering: team-a (name sorts first) then team-b; team_name resolved per row.
    assert_eq!(
        triples_of(&body),
        vec![
            (
                fx.team_a_name.clone(),
                "api-definitions".into(),
                "update".into()
            ),
            (fx.team_b_name.clone(), "mcp-tools".into(), "read".into()),
        ],
        "grants returned EXACTLY the two seeded rows with resolved team names, \
         ordered team_name ASC: {body}"
    );

    // Each row carries a distinct grant id.
    let ids: Vec<&str> = its.iter().map(|g| g["id"].as_str().expect("id")).collect();
    assert_ne!(ids[0], ids[1], "each grant row carries its own id: {body}");
}

#[tokio::test]
async fn grants_ordered_by_team_then_resource() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    // Seed out of sorted order across two teams and two resources on team-a.
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[
            (fx.team_b_id, "api-definitions", "update"),
            (fx.team_a_id, "mcp-tools", "read"),
            (fx.team_a_id, "api-definitions", "update"),
        ],
    )
    .await;

    let (status, _, body) = get(&env, &grants_uri(agent), &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "org admin reads grants: {body}");

    // team_name ASC (team-a before team-b), then resource ASC within a team
    // (api-definitions before mcp-tools).
    assert_eq!(
        triples_of(&body),
        vec![
            (
                fx.team_a_name.clone(),
                "api-definitions".into(),
                "update".into()
            ),
            (fx.team_a_name.clone(), "mcp-tools".into(), "read".into()),
            (
                fx.team_b_name.clone(),
                "api-definitions".into(),
                "update".into()
            ),
        ],
        "items sort team_name ASC then resource ASC (seeded b,a-m,a-a): {body}"
    );
}

// --- AC2: Page contract — defaults, clamps, negative offset, stable total --------------

#[tokio::test]
async fn grants_page_defaults_and_clamps_limit() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[
            (fx.team_a_id, "api-definitions", "update"),
            (fx.team_a_id, "clusters", "read"),
            (fx.team_a_id, "mcp-tools", "read"),
        ],
    )
    .await;

    // Default limit is 50.
    let (status, _, body) = get(&env, &grants_uri(agent), &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "default page ok: {body}");
    assert_eq!(
        body["limit"].as_i64(),
        Some(50),
        "limit defaults to 50: {body}"
    );

    // limit=0 clamps up to 1.
    let (status, _, body) = get(&env, &grants_query(agent, "limit=0"), &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "limit=0 ok: {body}");
    let (its, total) = page(&body);
    assert_eq!(
        body["limit"].as_i64(),
        Some(1),
        "limit=0 clamps up to 1: {body}"
    );
    assert_eq!(its.len(), 1, "limit clamped to 1 caps items to 1: {body}");
    assert_eq!(total, 3, "total is the full authorized count: {body}");

    // limit=1000 clamps down to 500.
    let (status, _, body) = get(
        &env,
        &grants_query(agent, "limit=1000"),
        &fx.admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "limit=1000 ok: {body}");
    assert_eq!(
        body["limit"].as_i64(),
        Some(500),
        "limit=1000 clamps down to 500: {body}"
    );
}

#[tokio::test]
async fn grants_negative_offset_is_400() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[(fx.team_a_id, "api-definitions", "update")],
    )
    .await;

    let (status, _, body) = get(
        &env,
        &grants_query(agent, "offset=-1"),
        &fx.admin_token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a negative offset must be rejected 400 (this endpoint does not floor), got {status}: {body}"
    );
    // Never a Page of grants riding along on the rejection.
    assert!(
        body.is_object() && body.get("items").is_none(),
        "the 400 body must be an error envelope, never a grant Page: {body}"
    );
}

#[tokio::test]
async fn grants_total_stable_across_pages() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    // Three grants on team-a with resources sorting api-definitions < clusters < mcp-tools.
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[
            (fx.team_a_id, "api-definitions", "update"),
            (fx.team_a_id, "clusters", "read"),
            (fx.team_a_id, "mcp-tools", "read"),
        ],
    )
    .await;

    // Page 1: limit=2 caps items to 2; total is the full authorized count (3).
    let (status, _, body) = get(&env, &grants_query(agent, "limit=2"), &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "page 1 ok: {body}");
    let (its, total) = page(&body);
    assert_eq!(body["limit"].as_i64(), Some(2), "limit echoed: {body}");
    assert_eq!(its.len(), 2, "limit=2 caps items to 2: {body}");
    assert_eq!(total, 3, "total is the full authorized count: {body}");
    assert_eq!(
        keys_of(&body),
        vec![
            ("api-definitions".into(), "update".into()),
            ("clusters".into(), "read".into()),
        ],
        "page 1 holds the first two by resource ASC: {body}"
    );

    // Page 2: offset=2 skips the first two; total unchanged.
    let (status, _, body) = get(
        &env,
        &grants_query(agent, "limit=2&offset=2"),
        &fx.admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "page 2 ok: {body}");
    let (its, total) = page(&body);
    assert_eq!(body["offset"].as_i64(), Some(2), "offset echoed: {body}");
    assert_eq!(its.len(), 1, "one grant remains after offset=2: {body}");
    assert_eq!(total, 3, "total is stable across pages: {body}");
    assert_eq!(
        keys_of(&body),
        vec![("mcp-tools".into(), "read".into())],
        "page 2 holds the last grant by resource ASC: {body}"
    );
}

// --- AC4: grants-shape row scoping to the caller's grants:read teams -------------------

#[tokio::test]
async fn grants_reader_sees_only_overlapping_team_rows() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    // Agent holds one grant on team-a and one on team-b.
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[
            (fx.team_a_id, "api-definitions", "update"),
            (fx.team_b_id, "mcp-tools", "read"),
        ],
    )
    .await;

    // Non-admin member holding BOTH agents:read AND grants:read on team-a ONLY.
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
    identity::add_grant(
        &env.pool,
        reader,
        fx.org_id,
        fx.team_a_id,
        Resource::Grants,
        Action::Read,
        None,
    )
    .await
    .expect("grants:read on team A");

    let (status, _, body) = get(&env, &grants_uri(agent), &token, None).await;
    assert_eq!(status, StatusCode::OK, "team-a grants reader ok: {body}");
    let (its, total) = page(&body);
    assert_eq!(
        total, 1,
        "total is the authorized row count after grants:read scope (team-a only): {body}"
    );
    assert_eq!(its.len(), 1, "exactly the team-a row is returned: {body}");
    assert_eq!(
        triples_of(&body),
        vec![(
            fx.team_a_name.clone(),
            "api-definitions".into(),
            "update".into()
        )],
        "reader sees ONLY the team-a grant, never the team-b grant: {body}"
    );
    // Belt-and-braces: team-b must appear nowhere in the body.
    assert!(
        !body.to_string().contains(&fx.team_b_name),
        "the team-b grant/team must not leak to a team-a-only grants reader: {body}"
    );
}

#[tokio::test]
async fn agents_read_without_grants_read_is_forbidden() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[(fx.team_a_id, "api-definitions", "update")],
    )
    .await;

    // Non-admin holding agents:read on team-a but NO grants:read anywhere.
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

    let (status, rid, body) = get(&env, &grants_uri(agent), &token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "agents:read without grants:read must be 403 on the grants endpoint, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
}

// --- AC5: the two `org: None` shapes ---------------------------------------------------

#[tokio::test]
async fn zero_membership_caller_denied_and_audited() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[(fx.team_a_id, "api-definitions", "update")],
    )
    .await;

    // A user with ZERO org memberships (org: None, NOT multi-org). Nothing to select,
    // so authorization runs and denies.
    let token = user_with_no_memberships(&env).await;

    let (status, rid, body) = get(&env, &grants_uri(agent), &token, None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "zero-membership caller must be 403 (not 400), got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    let rid = rid.expect("request id");
    assert!(
        no_matching_grant_denials(&env.pool, rid).await >= 1,
        "the zero-membership denial must be audited authz.denied/no_matching_grant for {rid}"
    );
}

#[tokio::test]
async fn multi_org_no_selector_gets_400_and_writes_no_denial_audit() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[(fx.team_a_id, "api-definitions", "update")],
    )
    .await;

    // A second org the caller also belongs to.
    let org_b = org_with_two_teams(&env).await;

    // Member of BOTH orgs who genuinely HOLDS grants:read (on org_a/team_a). With no
    // selector the active org is ambiguous, so org resolution must fail-fast with 400
    // org_selector_required — BEFORE authorization — never 403 and never a leak.
    let (user, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_org_membership(&env.pool, user, org_b.org_id, OrgRole::Member)
        .await
        .expect("second org membership");
    identity::add_grant(
        &env.pool,
        user,
        fx.org_id,
        fx.team_a_id,
        Resource::Grants,
        Action::Read,
        None,
    )
    .await
    .expect("grants:read on org_a team A");

    let (status, rid, body) = get(&env, &grants_uri(agent), &token, None).await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "a multi-org holder of grants:read must NOT get 403: {body}"
    );
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "multi-org, no selector must be 400 org_selector_required, got {status}: {body}"
    );
    assert_error_envelope(&body, "org_selector_required", rid);
    let rid = rid.expect("request id");
    assert_eq!(
        any_denials(&env.pool, rid).await,
        0,
        "org resolution precedes authz — a selector-required 400 must write NO \
         authz.denied audit row for request {rid}"
    );
}

// --- AC6: no credential material in grants response bodies -----------------------------

#[tokio::test]
async fn grants_bodies_carry_no_credential_material() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[
            (fx.team_a_id, "api-definitions", "update"),
            (fx.team_b_id, "mcp-tools", "read"),
        ],
    )
    .await;

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

    let (status, _, body) = get(&env, &grants_uri(agent), &fx.admin_token, None).await;
    assert_eq!(status, StatusCode::OK, "grants ok: {body}");
    let its = items(&body);
    assert_eq!(
        its.len(),
        2,
        "both grants present to make the scan meaningful: {body}"
    );
    // Scan the Page `items` so the anti-leak assertion is over the grant views themselves.
    let raw = serde_json::Value::Array(its.clone())
        .to_string()
        .to_ascii_lowercase();
    for needle in forbidden {
        assert!(
            !raw.contains(needle),
            "grants items leaked credential-shaped field '{needle}': {body}"
        );
    }
}

// --- Endpoint anti-enumeration: cross-org / unknown → 404, same-org no-overlap → empty --

#[tokio::test]
async fn cross_org_agent_is_404() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;

    // An agent that belongs to a DIFFERENT org.
    let other = org_with_two_teams(&env).await;
    let foreign_agent = create_agent_with_grants(
        &env,
        &other.admin_token,
        &[(other.team_a_id, "api-definitions", "update")],
    )
    .await;

    // org-1 admin requesting org-2's agent grants: must read as absent, never 403,
    // never leak org-2's rows.
    let (status, rid, body) = get(&env, &grants_uri(foreign_agent), &fx.admin_token, None).await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "403 would confirm the agent exists to an outsider: {body}"
    );
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a cross-org agent must be 404, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);
    assert!(
        !body.to_string().contains(&other.team_a_name),
        "no org-2 team/grant may leak on the 404: {body}"
    );
}

#[tokio::test]
async fn unknown_agent_is_404() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    // Ensure the caller is fully authorized so a 404 reflects the unknown agent, not authz.
    let _real = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[(fx.team_a_id, "api-definitions", "update")],
    )
    .await;

    let (status, rid, body) = get(&env, &grants_uri(Uuid::now_v7()), &fx.admin_token, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unknown agent id must be 404, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);
}

#[tokio::test]
async fn same_org_no_overlap_is_200_empty_not_404() {
    let Some(env) = env().await else { return };
    let fx = org_with_two_teams(&env).await;
    // Agent granted ONLY on team-b.
    let agent = create_agent_with_grants(
        &env,
        &fx.admin_token,
        &[(fx.team_b_id, "mcp-tools", "read")],
    )
    .await;

    // Same-org caller holding grants:read on team-a only — no overlap with the agent's
    // team-b grant.
    let (reader, token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        reader,
        fx.org_id,
        fx.team_a_id,
        Resource::Grants,
        Action::Read,
        None,
    )
    .await
    .expect("grants:read on team A");

    let (status, _, body) = get(&env, &grants_uri(agent), &token, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a same-org agent with no grants:read overlap must be 200 (NOT 404), got {status}: {body}"
    );
    let (its, total) = page(&body);
    assert!(
        its.is_empty(),
        "no overlapping team means an empty items page: {body}"
    );
    assert_eq!(
        total, 0,
        "total is 0 after scope for a no-overlap caller: {body}"
    );
    assert!(
        !body.to_string().contains(&fx.team_b_name),
        "the agent's team-b grant must not leak to a team-a-only reader: {body}"
    );
}
