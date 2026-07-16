//! Black-box read-scoping contract tests for `GET /api/v1/orgs/{org}`
//! (slice fpv2-1co.2 — org governance read scope, single-org GET).
//!
//! Written from acceptance criteria only:
//! 4. By-id in-scope: org A's member (A active) `GET /api/v1/orgs/{A-uuid}` → 200
//!    with org A (asserted by id AND name).
//! 5. By-id out-of-scope → 404: A's member `GET /api/v1/orgs/{B-uuid}` → 404
//!    (explicitly NOT 403 — anti-enumeration) and the error body must not leak
//!    org B's name. A platform admin `GET /api/v1/orgs/{B-uuid}` → 200 with org B.
//! 6. Write path unchanged (re-assert): a non-platform org member `POST /api/v1/orgs`
//!    gets the existing 403 denial; `DELETE /api/v1/orgs/{their-own-org}` is also 403
//!    (status + standard error envelope only, no exact-message pinning).
//! 8. Suspended org by id: a tenant member of a suspended org gets 404 fetching it
//!    by uuid; a platform admin gets 200 for the same org.
//!
//! Parallel-safe (constitution invariant 18): every org/user is uuid-suffixed and
//! unique per test; assertions are per-caller/per-row (never global counts); the
//! ONLY serialized critical section is the `instance_meta.platform_org_id`
//! advisory lock (id 420001) in the platform-admin tests. Skipped (with a notice)
//! when FLOWPLANE_TEST_DATABASE_URL is unset.

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

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

fn org_uri(org: &str) -> String {
    format!("/api/v1/orgs/{org}")
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

/// Send a request through the router, returning (status, x-request-id, JSON body).
async fn send(env: &Env, req: Request<Body>) -> (StatusCode, Option<Uuid>, serde_json::Value) {
    let response = env.app.clone().oneshot(req).await.expect("response");
    let status = response.status();
    let rid = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok());
    (status, rid, json_of(response).await)
}

/// GET /api/v1/orgs/{org} as `token`.
async fn get_org(
    env: &Env,
    token: &str,
    org: &str,
    org_selector: Option<&str>,
) -> (StatusCode, Option<Uuid>, serde_json::Value) {
    send(
        env,
        request("GET", &org_uri(org), token, org_selector, None),
    )
    .await
}

/// Assert a 200 body is exactly the given org (id AND name — a foreign or stale
/// org row satisfying only one of the two must fail).
fn assert_is_org(body: &serde_json::Value, org: &fp_domain::Organization, who: &str) {
    assert_eq!(
        body["id"],
        org.id.as_uuid().to_string(),
        "{who}: response org id must be {}: {body}",
        org.id.as_uuid(),
    );
    assert_eq!(
        body["name"],
        org.name.as_str(),
        "{who}: response org name must be {}: {body}",
        org.name,
    );
}

/// Assert a response body is the standard error envelope: an object whose
/// request_id matches the x-request-id header (and, when `code` is Some, whose
/// code matches — criterion 6 forbids pinning exact messages, so message text is
/// never asserted).
fn assert_error_envelope(body: &serde_json::Value, code: Option<&str>, rid: Option<Uuid>) {
    assert!(
        body.is_object(),
        "error responses must be the envelope object, not data: {body}"
    );
    if let Some(code) = code {
        assert_eq!(body["code"], code, "unexpected error code in {body}");
    }
    let rid = rid.expect("x-request-id header present");
    assert_eq!(
        body["request_id"],
        rid.to_string(),
        "envelope and header request id agree"
    );
}

// --- Criterion 4: by-id in-scope ------------------------------------------------------

#[tokio::test]
async fn org_member_gets_own_org_by_id() {
    let Some(env) = env().await else { return };
    let org_a = identity::create_org(&env.pool, &unique("org-a"), "")
        .await
        .expect("org a");
    // A decoy org this caller does NOT belong to: its data must never bleed into
    // the response for org A.
    let org_b = identity::create_org(&env.pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let (_, token) = user_with_org_role(&env, org_a.id, OrgRole::Member).await;

    let foreign_absent = |body: &serde_json::Value, form: &str| {
        let raw = body.to_string();
        assert!(
            !raw.contains(&org_b.id.as_uuid().to_string()) && !raw.contains(&org_b.name),
            "org B must never appear in A's response ({form}): {body}"
        );
    };

    let (status, _, body) = get_org(&env, &token, &org_a.id.as_uuid().to_string(), None).await;
    assert_eq!(status, StatusCode::OK, "A's member gets A by uuid: {body}");
    assert_is_org(&body, &org_a, "A's member (by uuid)");
    foreign_absent(&body, "by uuid");

    // The path accepts a name too (route shape: "Organization name or UUID") —
    // the in-scope contract must hold for both forms.
    let (status, _, body) = get_org(&env, &token, &org_a.name, None).await;
    assert_eq!(status, StatusCode::OK, "A's member gets A by name: {body}");
    assert_is_org(&body, &org_a, "A's member (by name)");
    foreign_absent(&body, "by name");
}

// --- Criterion 5: by-id out-of-scope → 404 (anti-enumeration) -------------------------

#[tokio::test]
async fn org_member_gets_404_not_403_for_foreign_org() {
    let Some(env) = env().await else { return };
    let org_a = identity::create_org(&env.pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&env.pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let (_, token) = user_with_org_role(&env, org_a.id, OrgRole::Member).await;

    // By uuid: 404, explicitly NOT 403 — a 403 would confirm the org exists.
    let (status, rid, body) = get_org(&env, &token, &org_b.id.as_uuid().to_string(), None).await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "an out-of-scope org must read as absent (404), never as forbidden \
         (403 confirms existence — enumeration oracle): {body}"
    );
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "A's member fetching B by uuid gets 404, got {status}: {body}"
    );
    assert_error_envelope(&body, Some("not_found"), rid);
    // The denial must not leak org B's (uuid-unique) name anywhere in the body.
    assert!(
        !body.to_string().contains(&org_b.name),
        "404 body must not leak org B's name: {body}"
    );

    // Adversarial extension of the same anti-enumeration contract: probing by
    // NAME (the other accepted path form) must also read as absence, and must
    // not reveal org B's id — that would confirm existence AND hand over the id.
    // (The 404 body is generic and echoes nothing — not even the caller's own
    // input; see existing_foreign_name_and_missing_name_are_indistinguishable.)
    let (status, rid, body) = get_org(&env, &token, &org_b.name, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "A's member probing B by name gets 404, got {status}: {body}"
    );
    assert_error_envelope(&body, Some("not_found"), rid);
    assert!(
        !body.to_string().contains(&org_b.id.as_uuid().to_string()),
        "404 body must not reveal org B's id to a name probe: {body}"
    );
}

#[tokio::test]
async fn existing_foreign_name_and_missing_name_are_indistinguishable() {
    // The name form must not be an existence oracle: for a tenant caller, probing an
    // EXISTING foreign org by name and probing a name that matches NO org must produce
    // the identical observable error shape (status + code + message + hint/details) —
    // only the request id may differ.
    let Some(env) = env().await else { return };
    let org_a = identity::create_org(&env.pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&env.pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let (_, token) = user_with_org_role(&env, org_a.id, OrgRole::Member).await;

    let (status_existing, rid_existing, body_existing) =
        get_org(&env, &token, &org_b.name, None).await;
    let missing_name = unique("no-such-org");
    let (status_missing, rid_missing, body_missing) =
        get_org(&env, &token, &missing_name, None).await;

    assert_eq!(status_existing, StatusCode::NOT_FOUND);
    assert_eq!(status_missing, StatusCode::NOT_FOUND);
    assert_error_envelope(&body_existing, Some("not_found"), rid_existing);
    assert_error_envelope(&body_missing, Some("not_found"), rid_missing);

    // Full-shape comparison: strip the (legitimately unique) request id and require the
    // remaining envelopes to be identical JSON values. Any divergence — message echoing the
    // name, an extra hint, different details — is an enumeration oracle.
    let strip_rid = |body: &serde_json::Value| {
        let mut b = body.clone();
        if let Some(obj) = b.as_object_mut() {
            obj.remove("request_id");
        }
        b
    };
    assert_eq!(
        strip_rid(&body_existing),
        strip_rid(&body_missing),
        "existing-foreign-name and missing-name 404 envelopes must be identical:\n  \
         existing: {body_existing}\n  missing: {body_missing}"
    );
    // And neither may echo the probed names or leak B's id.
    for (body, probe) in [
        (&body_existing, &org_b.name),
        (&body_missing, &missing_name),
    ] {
        let raw = body.to_string();
        assert!(
            !raw.contains(probe) && !raw.contains(&org_b.id.as_uuid().to_string()),
            "404 body must not echo the probed name or leak an org id: {body}"
        );
    }
}

#[tokio::test]
async fn platform_admin_gets_foreign_org_by_id() {
    let Some(env) = env().await else { return };
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
    let req = request(
        "GET",
        &org_uri(&org_b.id.as_uuid().to_string()),
        &token,
        None,
        None,
    );

    // This test owns instance-level shared state: `instance_meta.platform_org_id`
    // is an instance-wide singleton. Serialize against parallel siblings (the org
    // list / bootstrap / team-grants tests use the same lock id) on ONE dedicated
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
    // restoration no expect/unwrap/assert may run — every fallible step is
    // captured as a Result so restoration below is reached on EVERY exit path.
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
    let restore_result = restore_platform_org(&mut lock_conn, prior.as_deref()).await;
    let unlock_result = sqlx::query("SELECT pg_advisory_unlock(420001)")
        .execute(&mut *lock_conn)
        .await;
    drop(lock_conn);
    restore_result.expect("restore prior platform_org_id");
    unlock_result.expect("advisory unlock on instance_meta.platform_org_id");

    let (status, body) = outcome.expect("critical section");
    assert_eq!(
        status,
        StatusCode::OK,
        "platform admin gets any org by uuid: {body}"
    );
    assert_is_org(&body, &org_b, "platform admin");
}

/// Restore the prior `instance_meta.platform_org_id` (or its absence) on the
/// dedicated lock connection. Returns the sqlx outcome — callers must not panic
/// before also attempting the advisory unlock.
async fn restore_platform_org(
    lock_conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
    prior: Option<&str>,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    match prior {
        Some(value) => {
            sqlx::query(
                "INSERT INTO instance_meta (key, value) VALUES ('platform_org_id', $1) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
            )
            .bind(value)
            .execute(&mut **lock_conn)
            .await
        }
        None => {
            sqlx::query("DELETE FROM instance_meta WHERE key = 'platform_org_id'")
                .execute(&mut **lock_conn)
                .await
        }
    }
}

// --- Criterion 6: write path unchanged --------------------------------------------------

#[tokio::test]
async fn tenant_member_cannot_create_or_delete_orgs() {
    let Some(env) = env().await else { return };
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let (_, token) = user_with_org_role(&env, org.id, OrgRole::Member).await;

    // POST /api/v1/orgs → the existing 403 denial (status + envelope only; the
    // exact message is deliberately NOT pinned).
    let attempted_name = unique("rogue-org");
    let (status, rid, body) = send(
        &env,
        request(
            "POST",
            "/api/v1/orgs",
            &token,
            None,
            Some(serde_json::json!({ "name": attempted_name })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "tenant member creating an org must be denied, got {status}: {body}"
    );
    assert_error_envelope(&body, Some("forbidden"), rid);
    // The denial must have had no side effect: no such org row may exist.
    let created: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM organizations WHERE name = $1)")
            .bind(&attempted_name)
            .fetch_one(&env.pool)
            .await
            .expect("existence check");
    assert!(!created, "a 403'd create must not have created the org");

    // DELETE /api/v1/orgs/{their-own-org} → also 403 (membership grants no org
    // write power).
    let (status, rid, body) = send(
        &env,
        request(
            "DELETE",
            &org_uri(&org.id.as_uuid().to_string()),
            &token,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "tenant member deleting their own org must be denied, got {status}: {body}"
    );
    assert_error_envelope(&body, Some("forbidden"), rid);

    // The org must still be there afterwards — visible to its own member (also
    // re-exercises criterion 4's read path after a denied write).
    let (status, _, body) = get_org(&env, &token, &org.id.as_uuid().to_string(), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "org survives a denied delete: {body}"
    );
    assert_is_org(&body, &org, "member after denied delete");
}

// --- Criterion 8: suspended org by id ---------------------------------------------------

#[tokio::test]
async fn suspended_org_reads_as_absent_to_its_member_but_not_to_platform_admin() {
    let Some(env) = env().await else { return };
    // Shape chosen: TWO memberships. The auth middleware's principal loader joins
    // org_memberships against organizations with status = 'active' (harness fact,
    // crates/fp-storage/src/repos/identity.rs load_principal), so a user whose
    // ONLY membership is a suspended org resolves to ZERO selectable memberships
    // and can never establish an org context — the request would be rejected
    // before the handler's scoping logic runs. To exercise the handler's 404 we
    // therefore give the member a second, ACTIVE org C (selected explicitly via
    // the x-flowplane-org header) plus membership in the suspended org S, and
    // fetch S by uuid.
    let org_c = identity::create_org(&env.pool, &unique("org-c"), "")
        .await
        .expect("org c");
    let org_s = identity::create_org(&env.pool, &unique("org-s"), "")
        .await
        .expect("org s");
    let (user, token) = user_with_org_role(&env, org_c.id, OrgRole::Member).await;
    identity::add_org_membership(&env.pool, user, org_s.id, OrgRole::Member)
        .await
        .expect("suspended-org membership");

    // Suspend S by direct SQL — there is no REST suspend endpoint. S is unique to
    // this test, so no cross-test state is touched.
    sqlx::query("UPDATE organizations SET status = 'suspended' WHERE id = $1")
        .bind(org_s.id.as_uuid())
        .execute(&env.pool)
        .await
        .expect("suspend org s");

    // The suspended org's own member gets 404 — suspension makes the org read as
    // absent to tenants, even ones holding a membership row in it.
    let (status, rid, body) = get_org(
        &env,
        &token,
        &org_s.id.as_uuid().to_string(),
        Some(&org_c.id.as_uuid().to_string()),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "a suspended org must read as absent, not forbidden: {body}"
    );
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "member fetching their suspended org by uuid gets 404, got {status}: {body}"
    );
    assert_error_envelope(&body, Some("not_found"), rid);
    assert!(
        !body.to_string().contains(&org_s.name),
        "404 body must not leak the suspended org's name: {body}"
    );

    // Platform admin sees the suspended org (governance view). Same advisory-lock
    // save/set/restore discipline as the other platform-admin test: everything is
    // prepared before the lock; the critical section is panic-free; restoration
    // runs on every exit path.
    let platform_org = identity::create_org(&env.pool, &unique("platform-org"), "")
        .await
        .expect("platform org");
    let (_, admin_token) = user_with_org_role(&env, platform_org.id, OrgRole::Owner).await;
    let req = request(
        "GET",
        &org_uri(&org_s.id.as_uuid().to_string()),
        &admin_token,
        None,
        None,
    );

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

    let restore_result = restore_platform_org(&mut lock_conn, prior.as_deref()).await;
    let unlock_result = sqlx::query("SELECT pg_advisory_unlock(420001)")
        .execute(&mut *lock_conn)
        .await;
    drop(lock_conn);
    restore_result.expect("restore prior platform_org_id");
    unlock_result.expect("advisory unlock on instance_meta.platform_org_id");

    let (status, body) = outcome.expect("critical section");
    assert_eq!(
        status,
        StatusCode::OK,
        "platform admin sees the suspended org: {body}"
    );
    assert_is_org(&body, &org_s, "platform admin (suspended org)");
}
