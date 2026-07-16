//! Black-box read-scoping contract tests for `GET /api/v1/orgs`
//! (slice fpv2-1co.1 — org governance read scope).
//!
//! Written from acceptance criteria only:
//! 1. Cross-org list invisibility — an org member's list contains exactly the orgs
//!    they belong to, never a foreign org (asserted by id AND name).
//! 2. Platform admin lists all orgs (contains-both, never exact global counts).
//! 3. A multi-org member sees ALL their orgs whichever one is selected as active
//!    (via the `x-flowplane-org` selector header); with no selector they get the
//!    existing generic 403 authorization denial (not `org_selector_required`,
//!    not 404).
//! 7. Agent principals are denied with 403 — never an empty 200 list.
//!
//! Parallel-safe (constitution invariant 18): every org/team/user/agent is
//! uuid-suffixed and unique per test; assertions are set-membership over rows each
//! test created (per-caller "exactly" claims only — never global row counts); the
//! ONLY serialized critical section is the `instance_meta.platform_org_id`
//! advisory lock in the platform-admin test. Skipped (with a notice) when
//! FLOWPLANE_TEST_DATABASE_URL is unset.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::{OrgId, OrgRole, UserId};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

/// Header carrying the active-org selector (an org name or UUID).
const ORG_SELECTOR_HEADER: &str = "x-flowplane-org";

const ORGS_URI: &str = "/api/v1/orgs";

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

/// GET /api/v1/orgs as `token` (optionally with an org selector header),
/// returning (status, request-id header, JSON body).
async fn list_orgs(
    env: &Env,
    token: &str,
    org_selector: Option<&str>,
) -> (StatusCode, Option<Uuid>, serde_json::Value) {
    let response = env
        .app
        .clone()
        .oneshot(request("GET", ORGS_URI, token, org_selector, None))
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

/// Assert the org list contains an entry matching BOTH the org's id and name.
fn assert_org_listed(body: &serde_json::Value, org: &fp_domain::Organization, who: &str) {
    let orgs = body.as_array().expect("org list is a JSON array");
    assert!(
        orgs.iter()
            .any(|o| { o["id"] == org.id.as_uuid().to_string() && o["name"] == org.name.as_str() }),
        "{who} must see org {} ({}) in the list: {body}",
        org.name,
        org.id.as_uuid(),
    );
}

/// Adversarial absence check: neither the org's id nor its (uuid-unique) name may
/// appear ANYWHERE in the response body — not as an entry, not inside any field.
fn assert_org_invisible(body: &serde_json::Value, org: &fp_domain::Organization, who: &str) {
    let raw = body.to_string();
    assert!(
        !raw.contains(&org.id.as_uuid().to_string()),
        "{who} must never see org id {} in the response: {body}",
        org.id.as_uuid(),
    );
    assert!(
        !raw.contains(&org.name),
        "{who} must never see org name {} in the response: {body}",
        org.name,
    );
}

/// Assert a response body is the standard error envelope for `code`: an object (a
/// successful org listing is an array — an envelope object leaks no org rows) whose
/// request_id matches the x-request-id header.
fn assert_error_envelope(body: &serde_json::Value, code: &str, rid: Option<Uuid>) {
    assert!(
        body.is_object(),
        "error responses must be the envelope object, not data: {body}"
    );
    assert!(
        !body.is_array(),
        "an error must never carry an org list: {body}"
    );
    assert_eq!(body["code"], code, "unexpected error code in {body}");
    let rid = rid.expect("x-request-id header present");
    assert_eq!(
        body["request_id"],
        rid.to_string(),
        "envelope and header request id agree"
    );
}

// --- Criterion 1: cross-org list invisibility ---------------------------------------

#[tokio::test]
async fn org_member_lists_own_org_and_never_the_other_org() {
    let Some(env) = env().await else { return };
    let org_a = identity::create_org(&env.pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&env.pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let (_, token_a) = user_with_org_role(&env, org_a.id, OrgRole::Member).await;
    let (_, token_b) = user_with_org_role(&env, org_b.id, OrgRole::Member).await;

    // A's member (sole membership → A inferred as active org) lists orgs.
    let (status, _, body) = list_orgs(&env, &token_a, None).await;
    assert_eq!(status, StatusCode::OK, "A's member lists orgs: {body}");
    assert_org_listed(&body, &org_a, "A's member");
    assert_org_invisible(&body, &org_b, "A's member");
    // "Exactly the orgs they are a member of": this caller belongs to exactly one
    // org, so THEIR list has exactly one entry. Per-caller scoping claim only —
    // orgs other tests create concurrently must never show up here either.
    let orgs = body.as_array().expect("org list");
    assert_eq!(
        orgs.len(),
        1,
        "a single-org member's list is exactly their org: {body}"
    );

    // Symmetric direction: B's member sees B, never A.
    let (status, _, body) = list_orgs(&env, &token_b, None).await;
    assert_eq!(status, StatusCode::OK, "B's member lists orgs: {body}");
    assert_org_listed(&body, &org_b, "B's member");
    assert_org_invisible(&body, &org_a, "B's member");
}

// --- Criterion 2: platform admin lists all -------------------------------------------

#[tokio::test]
async fn platform_admin_list_contains_every_org() {
    let Some(env) = env().await else { return };
    let org_a = identity::create_org(&env.pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&env.pool, &unique("org-b"), "")
        .await
        .expect("org b");

    // Platform admin = Owner of the org recorded as instance_meta.platform_org_id.
    // The principal, membership, token, and request are all prepared BEFORE the
    // singleton is touched, so any panic here leaves no shared state behind.
    let platform_org = identity::create_org(&env.pool, &unique("platform-org"), "")
        .await
        .expect("platform org");
    let (_, token) = user_with_org_role(&env, platform_org.id, OrgRole::Owner).await;
    let req = request("GET", ORGS_URI, &token, None, None);

    // This test owns instance-level shared state: `instance_meta.platform_org_id` is
    // an instance-wide singleton. Serialize against parallel siblings (the bootstrap
    // and team-grants tests use the same lock id for this resource) on ONE dedicated
    // connection — advisory locks are connection-scoped — then restore the prior
    // value before asserting.
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

    // PANIC-FREE critical section: between the singleton mutation and its
    // restoration no expect/unwrap/assert may run — every fallible step is captured
    // as a Result so that restoration below is reached on EVERY exit path.
    let outcome: Result<(StatusCode, serde_json::Value), String> = async {
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
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|e| format!("read body: {e}"))?
            .to_bytes();
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse body: {e}"))?;
        Ok((status, body))
    }
    .await;

    // ALWAYS restore instance_meta.platform_org_id and release the lock — before
    // any unwrap or assertion — so no exit path leaves the singleton mutated for
    // siblings. Restore and unlock outcomes are captured (not expect-ed) so a
    // restore failure still reaches the unlock attempt; panics surface only after
    // both ran. (If unlock itself fails, the connection drop below releases the
    // session-scoped lock.)
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

    // Only after restoration: surface any captured failure, then assert the
    // contract. Contains-both, never an exact global count — other tests create
    // orgs concurrently.
    let (status, body) = outcome.expect("critical section");
    assert_eq!(status, StatusCode::OK, "platform admin lists orgs: {body}");
    assert_org_listed(&body, &org_a, "platform admin");
    assert_org_listed(&body, &org_b, "platform admin");
}

// --- Criterion 3: multi-org member ---------------------------------------------------

#[tokio::test]
async fn multi_org_member_sees_both_orgs_whichever_org_is_selected() {
    let Some(env) = env().await else { return };
    let org_a = identity::create_org(&env.pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&env.pool, &unique("org-b"), "")
        .await
        .expect("org b");
    // An org this user does NOT belong to: must stay invisible under every selector.
    let org_c = identity::create_org(&env.pool, &unique("org-c"), "")
        .await
        .expect("org c");

    let (user, token) = user_with_org_role(&env, org_a.id, OrgRole::Member).await;
    identity::add_org_membership(&env.pool, user, org_b.id, OrgRole::Member)
        .await
        .expect("org b membership");

    // Selecting A (by name) as the active org: the list still carries BOTH orgs the
    // caller belongs to — membership scope, not active-org scope.
    let (status, _, body) = list_orgs(&env, &token, Some(&org_a.name)).await;
    assert_eq!(status, StatusCode::OK, "selector=A lists orgs: {body}");
    assert_org_listed(&body, &org_a, "multi-org member (A selected)");
    assert_org_listed(&body, &org_b, "multi-org member (A selected)");
    assert_org_invisible(&body, &org_c, "multi-org member (A selected)");

    // Selecting B (by UUID — the selector accepts name or id) gives the same view.
    let (status, _, body) = list_orgs(&env, &token, Some(&org_b.id.as_uuid().to_string())).await;
    assert_eq!(status, StatusCode::OK, "selector=B lists orgs: {body}");
    assert_org_listed(&body, &org_a, "multi-org member (B selected)");
    assert_org_listed(&body, &org_b, "multi-org member (B selected)");
    assert_org_invisible(&body, &org_c, "multi-org member (B selected)");
}

#[tokio::test]
async fn multi_org_member_without_selector_gets_generic_403() {
    let Some(env) = env().await else { return };
    let org_a = identity::create_org(&env.pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&env.pool, &unique("org-b"), "")
        .await
        .expect("org b");

    let (user, token) = user_with_org_role(&env, org_a.id, OrgRole::Member).await;
    identity::add_org_membership(&env.pool, user, org_b.id, OrgRole::Member)
        .await
        .expect("org b membership");

    let (status, rid, body) = list_orgs(&env, &token, None).await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "no-selector denial must not read as absence: {body}"
    );
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "multi-org member with no selector gets the existing generic authz denial, \
         got {status}: {body}"
    );
    assert_ne!(
        body["code"], "org_selector_required",
        "the denial is the GENERIC authz denial, not a selector hint: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    // No org data may ride along on the denial.
    assert_org_invisible(&body, &org_a, "multi-org member (no selector)");
    assert_org_invisible(&body, &org_b, "multi-org member (no selector)");
}

// --- Criterion 7: agent principals denied --------------------------------------------

/// Create an agent through the API (org-admin token), returning its bearer token.
async fn create_agent(
    env: &Env,
    admin_token: &str,
    kind: &str,
    grants: Vec<serde_json::Value>,
) -> String {
    let response = env
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/agents",
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
        "create {kind} agent"
    );
    let body = json_of(response).await;
    body["token"].as_str().expect("agent token").to_string()
}

#[tokio::test]
async fn cp_tool_agent_cannot_list_orgs_even_with_real_grants() {
    let Some(env) = env().await else { return };
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&env.pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let (_, admin_token) = user_with_org_role(&env, org.id, OrgRole::Admin).await;

    // A cp-tool agent WITH a legitimate tenant grant: real capabilities elsewhere
    // must not open the org roster (a grantless denial could pass vacuously).
    let token = create_agent(
        &env,
        &admin_token,
        "cp-tool",
        vec![serde_json::json!({
            "team_id": team.id.as_uuid(),
            "resource": "clusters",
            "action": "read"
        })],
    )
    .await;

    let (status, rid, body) = list_orgs(&env, &token, None).await;
    assert!(
        !body.is_array(),
        "an agent must never receive an org list — not even an empty one: {body}"
    );
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "cp-tool agent must be denied the org list, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    assert_org_invisible(&body, &org, "cp-tool agent");
}

#[tokio::test]
async fn api_consumer_agent_cannot_list_orgs() {
    let Some(env) = env().await else { return };
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let (_, admin_token) = user_with_org_role(&env, org.id, OrgRole::Admin).await;

    let token = create_agent(&env, &admin_token, "api-consumer", vec![]).await;

    let (status, rid, body) = list_orgs(&env, &token, None).await;
    assert!(
        !body.is_array(),
        "an agent must never receive an org list — not even an empty one: {body}"
    );
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "api-consumer agent must be denied the org list, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    assert_org_invisible(&body, &org, "api-consumer agent");
}
