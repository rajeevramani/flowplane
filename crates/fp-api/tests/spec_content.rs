//! Black-box contract tests for
//! `GET /api/v1/teams/{team}/api-definitions/{name}/specs/{version}/content`
//! (raw spec-document read model).
//!
//! Written from acceptance criteria only — the endpoint is exercised strictly over HTTP
//! through the production router/middleware stack. Fixtures use the pre-existing public
//! POST api-definitions endpoint (which imports spec version 1); the endpoint under
//! test is never touched by fixtures.
//!
//! Contract under test:
//! - AUTHORIZATION (dual grant, the load-bearing rule): the caller needs BOTH
//!   api-definitions:read AND learning-sessions:read on the team, uniformly for every
//!   source_kind (imported specs held to the same bar). api-definitions:read alone ->
//!   403 on /content while the metadata list GET .../specs still answers 200 for the
//!   SAME caller. learning-sessions:read alone -> also denied.
//! - 200 body is the canonical serde_json encoding of the stored spec document:
//!   Sha256(serde_json::to_vec(parsed body)) == spec_hash from the metadata list.
//!   Headers: ETag: "<spec_hash>" (quoted), Cache-Control: private, no-store.
//! - If-None-Match with the current ETag / the bare hash / "*" -> 304 Not Modified
//!   with the ETag header and an EMPTY body. A stale ETag -> 200 full body.
//! - Unknown version (999) -> 404; unknown API -> 404; cross-org caller -> 404
//!   anti-enumeration (never 403). Error responses carry ZERO spec-content bytes.
//! - LOG SAFETY: no log line emitted while serving the request contains spec-content
//!   bytes, on the 200 path or on denial paths. Verified with a per-future
//!   tracing subscriber writing into a shared buffer (parallel-safe: scoped with
//!   `WithSubscriber` around the oneshot future only).
//!
//! Parallel-safe: every org/team/user/api name is uuid-suffixed and unique per test;
//! markers are uuid-unique; in-process router via `oneshot` (no TCP ports). Skipped
//! (with a notice) when FLOWPLANE_TEST_DATABASE_URL is unset.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::authz::{Action, Resource};
use fp_domain::{OrgId, OrgRole, TeamId, UserId};
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use tracing::instrument::WithSubscriber;
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

/// Add one (resource, action) grant on a team for a user.
async fn grant(env: &Env, user: UserId, org: OrgId, team: TeamId, resource: Resource) {
    identity::add_grant(&env.pool, user, org, team, resource, Action::Read, None)
        .await
        .expect("grant");
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

/// GET returning the full observable surface: status, headers, raw body bytes.
async fn get_raw(
    env: &Env,
    uri: &str,
    token: &str,
    if_none_match: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    if let Some(inm) = if_none_match {
        builder = builder.header("if-none-match", inm);
    }
    let response = env
        .app
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

/// GET as `token`, returning (status, request-id header, JSON body).
async fn get_json(
    env: &Env,
    uri: &str,
    token: &str,
) -> (StatusCode, Option<Uuid>, serde_json::Value) {
    let (status, headers, bytes) = get_raw(env, uri, token, None).await;
    let rid = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok());
    let body = serde_json::from_slice(&bytes).expect("json body");
    (status, rid, body)
}

/// Assert a response body is the standard error envelope for `code`.
fn assert_error_envelope(body: &serde_json::Value, code: &str, rid: Option<Uuid>) {
    assert!(
        body.is_object(),
        "error responses must be the envelope object, not data: {body}"
    );
    assert_eq!(body["code"], code, "unexpected error code in {body}");
    let rid = rid.expect("x-request-id header present");
    assert_eq!(
        body["request_id"],
        rid.to_string(),
        "envelope and header request id agree"
    );
}

struct TeamFixture {
    org_id: OrgId,
    team_id: TeamId,
    team_name: String,
}

/// One uuid-unique org with one uuid-unique team.
async fn org_with_team(env: &Env) -> TeamFixture {
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&env.pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    TeamFixture {
        org_id: org.id,
        team_id: team.id,
        team_name: team.name,
    }
}

/// Create an API definition over HTTP with an inline OpenAPI document, producing the
/// imported spec version 1. `marker` is a uuid-unique string planted in
/// `info.description` inside the spec document, so content-leak assertions can grep
/// for it in denial bodies and captured logs.
async fn create_api_with_v1(env: &Env, token: &str, team_name: &str, marker: &str) -> String {
    let api_name = unique("api");
    let response = env
        .app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/teams/{team_name}/api-definitions"),
            token,
            Some(serde_json::json!({
                "name": api_name,
                "display_name": "Spec Content Fixture",
                "openapi": {
                    "openapi": "3.0.3",
                    "info": {
                        "title": "Spec Content Fixture",
                        "description": marker,
                        "version": "1.0.0"
                    },
                    "paths": {
                        "/items": {"get": {"operationId": "listItems"}}
                    }
                }
            })),
        ))
        .await
        .expect("create api");
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "create api definition fixture"
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(body["latest_spec"]["version"], 1, "import produced v1");
    api_name
}

fn specs_uri(team: &str, api: &str) -> String {
    format!("/api/v1/teams/{team}/api-definitions/{api}/specs")
}

fn content_uri(team: &str, api: &str, version: i64) -> String {
    format!("/api/v1/teams/{team}/api-definitions/{api}/specs/{version}/content")
}

/// Read version 1's spec_hash off the metadata list (NOT the endpoint under test).
async fn spec_hash_of_v1(env: &Env, token: &str, team: &str, api: &str) -> String {
    let (status, _, body) = get_json(env, &specs_uri(team, api), token).await;
    assert_eq!(status, StatusCode::OK, "metadata list fixture read: {body}");
    let item = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["version"] == 1)
        .expect("version 1 in the metadata list");
    item["spec_hash"]
        .as_str()
        .expect("spec_hash string")
        .to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Standard fixture: one org/team, an admin who creates the API (v1 imported), and a
/// plain Member holding BOTH read grants (the exact dual-grant bar — deliberately not
/// an org admin, so the test proves the two grants are sufficient by themselves).
/// Returns (fixture, api_name, marker, dual-grant token, v1 spec_hash).
async fn api_with_dual_grant_reader(env: &Env) -> (TeamFixture, String, String, String, String) {
    let fx = org_with_team(env).await;
    let (_, admin_token) = user_with_org_role(env, fx.org_id, OrgRole::Admin).await;
    let marker = unique("leakmark");
    let api_name = create_api_with_v1(env, &admin_token, &fx.team_name, &marker).await;

    let (reader, reader_token) = user_with_org_role(env, fx.org_id, OrgRole::Member).await;
    grant(env, reader, fx.org_id, fx.team_id, Resource::ApiDefinitions).await;
    grant(
        env,
        reader,
        fx.org_id,
        fx.team_id,
        Resource::LearningSessions,
    )
    .await;

    let hash = spec_hash_of_v1(env, &reader_token, &fx.team_name, &api_name).await;
    (fx, api_name, marker, reader_token, hash)
}

// --- Shared log-capture writer (parallel-safe: scoped per future, never global) ------

#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
    type Writer = SharedWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Run one GET with a dedicated TRACE-level subscriber capturing every log line
/// emitted while the request future runs. Returns (status, captured log text).
async fn get_capturing_logs(env: &Env, uri: &str, token: &str) -> (StatusCode, String) {
    let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(SharedWriter(buffer.clone()))
        .finish();
    let response = env
        .app
        .clone()
        .oneshot(request("GET", uri, token, None))
        .with_subscriber(subscriber)
        .await
        .expect("response");
    let status = response.status();
    // Drain the body under the default subscriber; the serving-side logs of interest
    // are emitted while the handler future runs, which the scope above covers.
    let _ = response.into_body().collect().await;
    let logs = String::from_utf8_lossy(&buffer.lock().expect("log buffer")).into_owned();
    (status, logs)
}

// --- Scenario 1: dual-grant caller gets the canonical document -----------------------

#[tokio::test]
async fn dual_grant_caller_gets_canonical_body_matching_hash_with_etag_and_no_store() {
    let Some(env) = env().await else { return };
    let (fx, api_name, marker, token, spec_hash) = api_with_dual_grant_reader(&env).await;

    let (status, headers, bytes) = get_raw(
        &env,
        &content_uri(&fx.team_name, &api_name, 1),
        &token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "both grants must open /content, got {status}: {}",
        String::from_utf8_lossy(&bytes)
    );

    // Body parses as JSON and IS the stored spec document (the planted marker rides
    // in info.description — a positive identity check, not just a hash match).
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("body is JSON");
    assert_eq!(
        doc["info"]["description"], marker,
        "the returned document is the one imported at create time: {doc}"
    );

    // The load-bearing integrity contract: Sha256 over the canonical serde_json
    // encoding of the response equals spec_hash from the metadata list.
    let canonical = serde_json::to_vec(&doc).expect("re-encode");
    assert_eq!(
        sha256_hex(&canonical),
        spec_hash,
        "Sha256(canonical body) must equal the metadata spec_hash"
    );

    // ETag: the spec_hash, quoted.
    let etag = headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("ETag header present on 200");
    assert_eq!(
        etag,
        format!("\"{spec_hash}\""),
        "ETag is the quoted spec_hash"
    );

    // Cache-Control: private, no-store — spec content must never land in shared caches.
    let cache_control = headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .expect("Cache-Control header present on 200")
        .to_ascii_lowercase();
    assert!(
        cache_control.contains("no-store"),
        "Cache-Control must carry no-store, got: {cache_control}"
    );
    assert!(
        cache_control.contains("private"),
        "Cache-Control must carry private, got: {cache_control}"
    );
}

// --- Scenario 2: If-None-Match revalidation ------------------------------------------

#[tokio::test]
async fn if_none_match_current_etag_bare_hash_and_star_hit_304_stale_gets_200() {
    let Some(env) = env().await else { return };
    let (fx, api_name, marker, token, spec_hash) = api_with_dual_grant_reader(&env).await;
    let uri = content_uri(&fx.team_name, &api_name, 1);
    let quoted = format!("\"{spec_hash}\"");

    // Current ETag (quoted), the bare hash, and "*" must all revalidate to 304.
    for inm in [quoted.as_str(), spec_hash.as_str(), "*"] {
        let (status, headers, bytes) = get_raw(&env, &uri, &token, Some(inm)).await;
        assert_eq!(
            status,
            StatusCode::NOT_MODIFIED,
            "If-None-Match: {inm} must revalidate, got {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
        // 304 must still carry the ETag so caches can re-associate the entry.
        let etag = headers
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_else(|| panic!("ETag header present on 304 for If-None-Match: {inm}"));
        assert_eq!(etag, quoted, "304 ETag matches the current hash");
        // 304 carries NO body — not even a partial or re-encoded document.
        assert!(
            bytes.is_empty(),
            "304 body must be empty for If-None-Match: {inm}, got {} bytes",
            bytes.len()
        );
    }

    // A stale/other ETag misses and gets the full 200 body back.
    let (status, headers, bytes) = get_raw(&env, &uri, &token, Some("\"deadbeef\"")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "stale ETag must return the full representation, got {status}"
    );
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("full JSON body on miss");
    assert_eq!(doc["info"]["description"], marker, "full document on miss");
    assert_eq!(
        headers.get("etag").and_then(|v| v.to_str().ok()),
        Some(quoted.as_str()),
        "miss response re-states the current ETag"
    );
}

// --- Scenario 3: api-definitions:read ONLY — metadata yes, content no ----------------

#[tokio::test]
async fn api_definitions_read_only_reads_metadata_but_is_403_on_content() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let marker = unique("leakmark");
    let api_name = create_api_with_v1(&env, &admin_token, &fx.team_name, &marker).await;
    let spec_hash = spec_hash_of_v1(&env, &admin_token, &fx.team_name, &api_name).await;

    // The half-grant caller: api-definitions:read only, no learning-sessions:read.
    let (half, half_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    grant(&env, half, fx.org_id, fx.team_id, Resource::ApiDefinitions).await;

    // SAME token: the metadata list stays open...
    let (status, _, body) = get_json(&env, &specs_uri(&fx.team_name, &api_name), &half_token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "api-definitions:read alone must keep the metadata list readable: {body}"
    );
    assert_eq!(body["total"], 1, "the imported v1 is listed: {body}");

    // ...while /content is denied — the dual-grant bar applies even to the imported
    // source_kind (uniform across every source_kind by contract).
    let (status, rid, body) =
        get_json(&env, &content_uri(&fx.team_name, &api_name, 1), &half_token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "api-definitions:read without learning-sessions:read must be 403 on /content: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);

    // Zero spec-content bytes on the denial: neither the marker nor the hash.
    let raw = body.to_string();
    assert!(
        !raw.contains(&marker) && !raw.contains(&spec_hash),
        "denial body leaked spec content or hash: {body}"
    );
}

// --- Scenario 4: learning-sessions:read ONLY is also denied --------------------------

#[tokio::test]
async fn learning_sessions_read_only_is_denied_on_content() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let marker = unique("leakmark");
    let api_name = create_api_with_v1(&env, &admin_token, &fx.team_name, &marker).await;
    let spec_hash = spec_hash_of_v1(&env, &admin_token, &fx.team_name, &api_name).await;

    // The other half-grant: learning-sessions:read only, no api-definitions:read.
    let (half, half_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    grant(
        &env,
        half,
        fx.org_id,
        fx.team_id,
        Resource::LearningSessions,
    )
    .await;

    let (status, rid, body) =
        get_json(&env, &content_uri(&fx.team_name, &api_name, 1), &half_token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "learning-sessions:read without api-definitions:read must be denied on /content: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    let raw = body.to_string();
    assert!(
        !raw.contains(&marker) && !raw.contains(&spec_hash),
        "denial body leaked spec content or hash: {body}"
    );
}

// --- Scenario 5: 404s — unknown version, unknown API, cross-org anti-enumeration -----

#[tokio::test]
async fn unknown_version_unknown_api_and_cross_org_all_read_as_404_without_content() {
    let Some(env) = env().await else { return };
    let (fx, api_name, marker, token, spec_hash) = api_with_dual_grant_reader(&env).await;

    // Unknown version on a real API: 404 for a fully-granted caller.
    let (status, rid, body) =
        get_json(&env, &content_uri(&fx.team_name, &api_name, 999), &token).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "version 999 does not exist, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);
    let raw = body.to_string();
    assert!(
        !raw.contains(&marker) && !raw.contains(&spec_hash),
        "unknown-version 404 leaked spec content: {body}"
    );

    // Unknown API name in the accessible team: 404.
    let (status, rid, body) = get_json(
        &env,
        &content_uri(&fx.team_name, &unique("ghost"), 1),
        &token,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown api name must read as absent, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);

    // Cross-org caller (an admin of ANOTHER org, so fully privileged over there):
    // 404, never 403 — a 403 would confirm the team/API exists to an outsider.
    let other_org = identity::create_org(&env.pool, &unique("org-p"), "")
        .await
        .expect("other org");
    let (_, foreign_token) = user_with_org_role(&env, other_org.id, OrgRole::Admin).await;

    for team_ref in [fx.team_name.clone(), fx.team_id.as_uuid().to_string()] {
        let (status, rid, body) =
            get_json(&env, &content_uri(&team_ref, &api_name, 1), &foreign_token).await;
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "403 for {team_ref} confirms the resource exists to an outsider: {body}"
        );
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "cross-org access to {team_ref} must read as absent, got {status}: {body}"
        );
        assert_error_envelope(&body, "not_found", rid);
        let raw = body.to_string();
        assert!(
            !raw.contains(&marker) && !raw.contains(&spec_hash),
            "cross-org 404 leaked spec content: {body}"
        );
    }
}

// --- Scenario 6: log safety — spec bytes never reach the logs ------------------------

#[tokio::test]
async fn logs_never_contain_spec_content_on_success_or_denial_paths() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let marker = unique("leakmark");
    let api_name = create_api_with_v1(&env, &admin_token, &fx.team_name, &marker).await;

    let (reader, reader_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    grant(
        &env,
        reader,
        fx.org_id,
        fx.team_id,
        Resource::ApiDefinitions,
    )
    .await;
    grant(
        &env,
        reader,
        fx.org_id,
        fx.team_id,
        Resource::LearningSessions,
    )
    .await;

    let (half, half_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    grant(&env, half, fx.org_id, fx.team_id, Resource::ApiDefinitions).await;

    let uri = content_uri(&fx.team_name, &api_name, 1);

    // 200 path: the document is served to the caller, yet its bytes stay out of logs.
    let (status, logs) = get_capturing_logs(&env, &uri, &reader_token).await;
    assert_eq!(status, StatusCode::OK, "dual-grant 200 path under capture");
    assert!(
        !logs.contains(&marker),
        "spec content leaked into logs on the 200 path; captured logs:\n{logs}"
    );

    // 403 path: denials must not log what was denied.
    let (status, logs) = get_capturing_logs(&env, &uri, &half_token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "half-grant 403 path under capture"
    );
    assert!(
        !logs.contains(&marker),
        "spec content leaked into logs on the 403 path; captured logs:\n{logs}"
    );
}
