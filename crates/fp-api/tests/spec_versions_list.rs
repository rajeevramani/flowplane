//! Black-box contract tests for `GET /api/v1/teams/{team}/api-definitions/{name}/specs`
//! (spec-version listing read model).
//!
//! Written from acceptance criteria only — the endpoint is exercised strictly over HTTP
//! through the production router/middleware stack. Fixtures use pre-existing public
//! endpoints (POST api-definitions) plus direct sqlx INSERTs into `spec_versions` /
//! `spec_version_review_events` (fixtures may touch the DB; the endpoint under test
//! never is).
//!
//! Contract under test:
//! - 200 with the uniform Page envelope {"items", "total", "limit", "offset"}.
//! - Items ordered version DESC (newest first).
//! - Item shape: id (uuid), version (i64), source_kind, format, spec_hash,
//!   optional latest_decision (ABSENT when a version has no review events), created_at.
//! - The item NEVER carries the spec document itself (no "spec" field, no content leak).
//! - limit/offset pagination like the existing api-definitions list.
//! - Unknown API in an accessible team -> 404 standard error envelope.
//! - Cross-org caller -> 404 (anti-enumeration), by team name AND team UUID.
//! - Same-org caller with no grant on the team -> denied at the team level with NO
//!   name-existence oracle (existing == absent status; 403 per the convention pinned in
//!   rate_limit_api.rs — the 403-vs-404 cosmetic question is tracked in bead fpv2-6y3).
//!
//! Parallel-safe: every org/team/user/api name is uuid-suffixed and unique per test;
//! no global row-count assertions; in-process router via `oneshot` (no TCP ports).
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
use sqlx::PgPool;
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

/// Assert a response body is the standard error envelope for `code`.
fn assert_error_envelope(body: &serde_json::Value, code: &str, rid: Option<Uuid>) {
    assert!(
        body.is_object(),
        "error responses must be the envelope object, not data: {body}"
    );
    assert_eq!(body["code"], code, "unexpected error code in {body}");
    assert!(
        body.get("items").is_none() && body.get("total").is_none(),
        "error envelope must not carry page fields: {body}"
    );
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
/// imported spec version 1. `marker` is a uuid-unique string planted inside the spec
/// document so leak assertions can grep for it in list responses.
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
                "display_name": "Spec List Fixture",
                "openapi": {
                    "openapi": "3.0.3",
                    "info": {"title": marker, "version": "1.0.0"},
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
    let body = json_of(response).await;
    assert_eq!(body["latest_spec"]["version"], 1, "import produced v1");
    api_name
}

/// Resolve the API definition's row id (fixture lookup, not the endpoint under test).
async fn api_definition_id(pool: &PgPool, team_id: TeamId, api_name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM api_definitions WHERE team_id = $1 AND name = $2")
        .bind(team_id.as_uuid())
        .bind(api_name)
        .fetch_one(pool)
        .await
        .expect("api definition id")
}

/// Resolve a spec version's row id by number (fixture lookup).
async fn spec_version_id(pool: &PgPool, api_id: Uuid, version: i64) -> Uuid {
    sqlx::query_scalar("SELECT id FROM spec_versions WHERE api_definition_id = $1 AND version = $2")
        .bind(api_id)
        .bind(version)
        .fetch_one(pool)
        .await
        .expect("spec version id")
}

/// Seed an extra spec_versions row directly (append-only fixture; the schema forbids
/// updates). Returns (row id, spec_hash). `marker` is planted in the spec document for
/// content-leak assertions.
async fn seed_spec_version(
    env: &Env,
    fx: &TeamFixture,
    api_id: Uuid,
    version: i64,
    source_kind: &str,
    marker: &str,
) -> (Uuid, String) {
    let id = Uuid::now_v7();
    // 64 hex chars, unique per row (unique (api_definition_id, spec_hash)).
    let spec_hash = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let spec = serde_json::json!({
        "openapi": "3.0.3",
        "info": {"title": marker, "version": format!("{version}.0.0")},
        "paths": {}
    });
    sqlx::query(
        "INSERT INTO spec_versions \
         (id, team_id, org_id, api_definition_id, version, source_kind, format, spec, spec_hash) \
         VALUES ($1, $2, $3, $4, $5, $6, 'openapi3', $7, $8)",
    )
    .bind(id)
    .bind(fx.team_id.as_uuid())
    .bind(fx.org_id.as_uuid())
    .bind(api_id)
    .bind(version)
    .bind(source_kind)
    .bind(&spec)
    .bind(&spec_hash)
    .execute(&env.pool)
    .await
    .expect("seed spec version");
    (id, spec_hash)
}

/// Seed a review event with a controlled created_at offset (seconds from now) so
/// "latest" is deterministic even under fast successive inserts.
async fn seed_review_event(
    env: &Env,
    fx: &TeamFixture,
    api_id: Uuid,
    spec_version_id: Uuid,
    decision: &str,
    offset_secs: f64,
) {
    sqlx::query(
        "INSERT INTO spec_version_review_events \
         (id, team_id, org_id, api_definition_id, spec_version_id, decision, actor_type, \
          actor_id, reason, metadata, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, 'user', NULL, '', '{}'::jsonb, \
                 now() + make_interval(secs => $7))",
    )
    .bind(Uuid::now_v7())
    .bind(fx.team_id.as_uuid())
    .bind(fx.org_id.as_uuid())
    .bind(api_id)
    .bind(spec_version_id)
    .bind(decision)
    .bind(offset_secs)
    .execute(&env.pool)
    .await
    .expect("seed review event");
}

fn specs_uri(team: &str, api: &str) -> String {
    format!("/api/v1/teams/{team}/api-definitions/{api}/specs")
}

/// Assert the uniform Page envelope shape is present.
fn assert_page_envelope(body: &serde_json::Value) {
    assert!(body["items"].is_array(), "items array present: {body}");
    assert!(
        body["total"].is_i64() || body["total"].is_u64(),
        "total is a number: {body}"
    );
    assert!(
        body["limit"].is_i64() || body["limit"].is_u64(),
        "limit is a number: {body}"
    );
    assert!(
        body["offset"].is_i64() || body["offset"].is_u64(),
        "offset is a number: {body}"
    );
}

// --- Criterion 1: full listing — order, source kinds, latest decisions, no content ---

#[tokio::test]
async fn lists_versions_newest_first_with_latest_decisions_and_no_spec_content() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let marker_v1 = unique("leakmark-v1");
    let api_name = create_api_with_v1(&env, &token, &fx.team_name, &marker_v1).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;

    let marker_v2 = unique("leakmark-v2");
    let marker_v3 = unique("leakmark-v3");
    let (v2_id, v2_hash) = seed_spec_version(&env, &fx, api_id, 2, "learned", &marker_v2).await;
    let (v3_id, v3_hash) = seed_spec_version(&env, &fx, api_id, 3, "learned", &marker_v3).await;
    // v1 has NO events; v2 ends published; v3 ends rejected.
    seed_review_event(&env, &fx, api_id, v2_id, "submitted", 0.0).await;
    seed_review_event(&env, &fx, api_id, v2_id, "published", 1.0).await;
    seed_review_event(&env, &fx, api_id, v3_id, "submitted", 2.0).await;
    seed_review_event(&env, &fx, api_id, v3_id, "rejected", 3.0).await;

    let (status, _, body) = get_json(&env, &specs_uri(&fx.team_name, &api_name), &token).await;
    assert_eq!(status, StatusCode::OK, "list spec versions: {body}");
    assert_page_envelope(&body);
    assert_eq!(body["total"], 3, "three versions exist: {body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 3, "all three versions on one page: {body}");

    // Newest first: [3, 2, 1].
    let versions: Vec<i64> = items
        .iter()
        .map(|i| i["version"].as_i64().expect("version is i64"))
        .collect();
    assert_eq!(versions, vec![3, 2, 1], "versions ordered DESC: {body}");

    let kinds: Vec<&str> = items
        .iter()
        .map(|i| i["source_kind"].as_str().expect("source_kind"))
        .collect();
    assert_eq!(
        kinds,
        vec!["learned", "learned", "imported"],
        "source kinds by version: {body}"
    );

    // latest_decision: v3 rejected, v2 published, v1 ABSENT (omitted, not null-with-key).
    assert_eq!(items[0]["latest_decision"], "rejected", "{body}");
    assert_eq!(items[1]["latest_decision"], "published", "{body}");
    // The spec pins ABSENT (field omitted, not null-with-key) for an event-less version.
    assert!(
        items[2].get("latest_decision").is_none(),
        "latest_decision must be OMITTED (not null) for an event-less version: {body}"
    );

    // Item shape: id is a uuid, spec_hash matches the seeded rows, created_at present.
    for item in items {
        let id = item["id"].as_str().expect("id string");
        Uuid::parse_str(id).expect("id is a uuid");
        assert_eq!(item["format"], "openapi3", "{body}");
        assert!(item["spec_hash"].is_string(), "spec_hash present: {body}");
        assert!(item["created_at"].is_string(), "created_at present: {body}");
        // The one property that must NEVER regress: no spec document in list items.
        assert!(
            item.get("spec").is_none(),
            "list items must not carry the spec document: {body}"
        );
    }
    assert_eq!(items[0]["id"], v3_id.to_string(), "{body}");
    assert_eq!(items[0]["spec_hash"], v3_hash, "{body}");
    assert_eq!(items[1]["id"], v2_id.to_string(), "{body}");
    assert_eq!(items[1]["spec_hash"], v2_hash, "{body}");

    // Adversarial content-leak check: no marker planted inside any spec document may
    // appear anywhere in the response, under any field name.
    let raw = body.to_string();
    for marker in [&marker_v1, &marker_v2, &marker_v3] {
        assert!(
            !raw.contains(marker.as_str()),
            "spec document content leaked into the listing: {body}"
        );
    }
}

// --- Criterion 2: pagination ---------------------------------------------------------

#[tokio::test]
async fn paginates_with_limit_and_offset_keeping_total_stable() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let api_name = create_api_with_v1(&env, &token, &fx.team_name, &unique("mark")).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;
    seed_spec_version(&env, &fx, api_id, 2, "learned", &unique("mark")).await;
    seed_spec_version(&env, &fx, api_id, 3, "learned", &unique("mark")).await;

    let base = specs_uri(&fx.team_name, &api_name);

    // Page 1: limit=2 offset=0 -> [3, 2], total 3.
    let (status, _, body) = get_json(&env, &format!("{base}?limit=2&offset=0"), &token).await;
    assert_eq!(status, StatusCode::OK, "page 1: {body}");
    assert_page_envelope(&body);
    assert_eq!(body["total"], 3, "total is page-independent: {body}");
    assert_eq!(body["limit"], 2, "limit echoed: {body}");
    assert_eq!(body["offset"], 0, "offset echoed: {body}");
    let versions: Vec<i64> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["version"].as_i64().expect("version"))
        .collect();
    assert_eq!(versions, vec![3, 2], "first page newest-first: {body}");

    // Page 2: limit=2 offset=2 -> [1], total stays 3.
    let (status, _, body) = get_json(&env, &format!("{base}?limit=2&offset=2"), &token).await;
    assert_eq!(status, StatusCode::OK, "page 2: {body}");
    assert_page_envelope(&body);
    assert_eq!(body["total"], 3, "total unchanged on page 2: {body}");
    assert_eq!(body["limit"], 2, "limit echoed: {body}");
    assert_eq!(body["offset"], 2, "offset echoed: {body}");
    let versions: Vec<i64> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["version"].as_i64().expect("version"))
        .collect();
    assert_eq!(
        versions,
        vec![1],
        "second page carries the oldest version: {body}"
    );
}

// --- Criterion 3: 'reviewed' decision (legal per DB CHECK) surfaces cleanly ----------

#[tokio::test]
async fn reviewed_decision_surfaces_as_latest_decision() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let api_name = create_api_with_v1(&env, &token, &fx.team_name, &unique("mark")).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;
    let v1_id = spec_version_id(&env.pool, api_id, 1).await;
    seed_review_event(&env, &fx, api_id, v1_id, "submitted", 0.0).await;
    seed_review_event(&env, &fx, api_id, v1_id, "reviewed", 1.0).await;

    let (status, _, body) = get_json(&env, &specs_uri(&fx.team_name, &api_name), &token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a 'reviewed' event must not break the listing: {body}"
    );
    assert_page_envelope(&body);
    assert_eq!(body["total"], 1, "{body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(items[0]["version"], 1, "{body}");
    assert_eq!(
        items[0]["latest_decision"], "reviewed",
        "latest event wins and 'reviewed' round-trips: {body}"
    );
}

// --- Criterion 4: unknown API name in an accessible team -> 404 -----------------------

#[tokio::test]
async fn unknown_api_name_returns_404_envelope() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let ghost = unique("ghost");
    let (status, rid, body) = get_json(&env, &specs_uri(&fx.team_name, &ghost), &token).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown api name must read as absent, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);
}

// --- Criterion 5: cross-org anti-enumeration ------------------------------------------

#[tokio::test]
async fn cross_org_caller_gets_404_for_name_and_uuid_team_refs() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, owner_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let marker = unique("leakmark");
    let api_name = create_api_with_v1(&env, &owner_token, &fx.team_name, &marker).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;
    let (_, v2_hash) = seed_spec_version(&env, &fx, api_id, 2, "learned", &marker).await;

    let other_org = identity::create_org(&env.pool, &unique("org-p"), "")
        .await
        .expect("other org");
    let (_, foreign_token) = user_with_org_role(&env, other_org.id, OrgRole::Admin).await;

    for team_ref in [fx.team_name.clone(), fx.team_id.as_uuid().to_string()] {
        let (status, rid, body) =
            get_json(&env, &specs_uri(&team_ref, &api_name), &foreign_token).await;
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
        let raw = body.to_string();
        assert!(
            !raw.contains(&v2_hash) && !raw.contains(&marker),
            "spec version data leaked cross-org: {body}"
        );
    }
}

// --- Criterion 6: same-org caller without a grant on the team -------------------------

#[tokio::test]
async fn same_org_caller_without_grant_is_denied_with_no_existence_oracle() {
    let Some(env) = env().await else { return };
    // One org, two teams: the API lives in team A; the caller is granted on team B only.
    let org = identity::create_org(&env.pool, &unique("org"), "")
        .await
        .expect("org");
    let team_a = identity::create_team(&env.pool, org.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&env.pool, org.id, &unique("team-b"), "")
        .await
        .expect("team b");
    let fx = TeamFixture {
        org_id: org.id,
        team_id: team_a.id,
        team_name: team_a.name.clone(),
    };
    let (_, admin_token) = user_with_org_role(&env, org.id, OrgRole::Admin).await;

    let marker = unique("leakmark");
    let api_name = create_api_with_v1(&env, &admin_token, &fx.team_name, &marker).await;
    let api_id = api_definition_id(&env.pool, fx.team_id, &api_name).await;
    let (_, v2_hash) = seed_spec_version(&env, &fx, api_id, 2, "learned", &marker).await;

    // Sanity (auth contract): a member holding api-definitions:read ON TEAM A reads the
    // listing — the denial below can never pass vacuously against a broken read path.
    let (granted, granted_token) = user_with_org_role(&env, org.id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        granted,
        org.id,
        team_a.id,
        Resource::ApiDefinitions,
        Action::Read,
        None,
    )
    .await
    .expect("api-definitions:read on team a");
    let (status, _, body) =
        get_json(&env, &specs_uri(&fx.team_name, &api_name), &granted_token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "api-definitions:read grant on the team must open the listing, got {status}: {body}"
    );
    assert_eq!(
        body["total"], 2,
        "granted reader sees both versions: {body}"
    );

    // The no-grant caller: same org, api-definitions:read granted ONLY on team B.
    let (holder, no_grant_token) = user_with_org_role(&env, org.id, OrgRole::Member).await;
    identity::add_grant(
        &env.pool,
        holder,
        org.id,
        team_b.id,
        Resource::ApiDefinitions,
        Action::Read,
        None,
    )
    .await
    .expect("api-definitions:read on team b");

    // Existing API under team A.
    let (status_existing, rid_existing, body_existing) =
        get_json(&env, &specs_uri(&fx.team_name, &api_name), &no_grant_token).await;
    // Never-existing API under team A.
    let (status_absent, _, body_absent) = get_json(
        &env,
        &specs_uri(&fx.team_name, &unique("ghost")),
        &no_grant_token,
    )
    .await;

    // Load-bearing invariant: the denial must not vary with resource existence, so the
    // status can never be used to probe which API names team A owns.
    assert_eq!(
        status_existing, status_absent,
        "same-org no-grant denial must be identical for existing vs absent APIs \
         (no oracle): existing={body_existing} absent={body_absent}"
    );
    assert!(
        status_existing == StatusCode::FORBIDDEN || status_existing == StatusCode::NOT_FOUND,
        "no-grant caller must be denied, got {status_existing}: {body_existing}"
    );
    // Pin the codebase's current convention (team-level NoMatchingGrant -> 403, as
    // exercised for the api-definitions-style tenant reads in rate_limit_api.rs).
    assert_eq!(
        status_existing,
        StatusCode::FORBIDDEN,
        "pinned convention for same-org no-grant reads, body: {body_existing}"
    );
    assert_error_envelope(&body_existing, "forbidden", rid_existing);

    // No spec-version data may ride along on the denial.
    let raw = body_existing.to_string();
    assert!(
        !raw.contains(&v2_hash) && !raw.contains(&marker),
        "spec version data leaked to a no-grant caller: {body_existing}"
    );
}
