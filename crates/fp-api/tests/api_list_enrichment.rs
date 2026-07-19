//! Black-box contract tests for the ENRICHED api-definitions list endpoint
//! `GET /api/v1/teams/{team}/api-definitions?limit=&offset=`.
//!
//! Written from acceptance criteria only — the endpoint is exercised strictly over
//! HTTP through the production router/middleware stack. Fixtures use pre-existing
//! public endpoints (POST api-definitions, POST specs/{v}/publish, PATCH
//! mcp/tools/{name}) plus direct sqlx INSERTs into `route_configs` /
//! `spec_versions` (fixtures may touch the DB; the endpoint under test never is).
//!
//! Contract under test (enrichment on top of the previous list contract):
//! - 200 with the uniform Page envelope {"items", "total", "limit", "offset"}.
//! - Each item carries, IN ADDITION to the previous fields (id, name, display_name,
//!   description, optional published_spec_version_id, revision, created_at,
//!   updated_at):
//!     - "tool_count": i64 — ALL generated tools for the API, enabled or not;
//!     - "route_binding_count": i64;
//!     - "latest_version": optional i64 — highest spec version number, key ABSENT
//!       (not null-with-key) when the API has no spec versions;
//!     - "published_version": optional i64 — version number of the published spec,
//!       key ABSENT when nothing is published.
//! - Acceptance intent: the list view renders WITHOUT one /status call per row, so
//!   each row's enrichment must EQUAL what GET .../api-definitions/{name}/status
//!   reports for that API (tool_count, route_binding_count, latest_spec.version).
//!   The parity test does the N+1 itself — the endpoint must make it unnecessary.
//! - Ordering by name; limit/offset pagination unchanged with page-independent total.
//! - Unknown team -> 404; cross-org caller -> 404 (anti-enumeration, by team name
//!   AND team UUID); same-org caller with no grant -> 403 (conventions pinned in
//!   spec_versions_list.rs / api_bindings_tools.rs).
//!
//! Parallel-safe: every org/team/user/api/route-config name is uuid-suffixed and
//! unique per test; the org/team is created fresh per test so the listing contains
//! ONLY this test's rows; no global row-count assumptions beyond those fixtures;
//! in-process router via `oneshot` (no TCP ports). Skipped (with a notice) when
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

struct TeamFixture {
    org_id: OrgId,
    team_id: TeamId,
    team_name: String,
}

/// One uuid-unique org with one uuid-unique team. The team is exclusively this
/// test's, so its api-definitions listing contains only rows created here.
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

fn list_uri(team: &str) -> String {
    format!("/api/v1/teams/{team}/api-definitions")
}

fn status_uri(team: &str, api: &str) -> String {
    format!("/api/v1/teams/{team}/api-definitions/{api}/status")
}

/// Create an API definition over HTTP with an explicit name and optional inline
/// OpenAPI document / route binding. Returns the created api's uuid.
async fn create_api(
    env: &Env,
    token: &str,
    team_name: &str,
    api_name: &str,
    openapi: Option<serde_json::Value>,
    route_config_id: Option<Uuid>,
) -> Uuid {
    let mut payload = serde_json::json!({
        "name": api_name,
        "display_name": "List Enrichment Fixture",
    });
    if let Some(openapi) = openapi {
        payload["openapi"] = openapi;
    }
    if let Some(rc_id) = route_config_id {
        payload["route_binding"] = serde_json::json!({"route_config_id": rc_id});
    }
    let response = env
        .app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/teams/{team_name}/api-definitions"),
            token,
            Some(payload),
        ))
        .await
        .expect("create api");
    let status = response.status();
    let body = json_of(response).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create api definition fixture {api_name}: {body}"
    );
    Uuid::parse_str(body["api"]["id"].as_str().expect("api id")).expect("api uuid")
}

/// A minimal OpenAPI document with `ops` operations (1 or 2 here).
fn openapi_doc(ops: usize) -> serde_json::Value {
    let mut paths = serde_json::json!({
        "/widgets": {"get": {"operationId": "listWidgets"}}
    });
    if ops >= 2 {
        paths["/widgets"]["post"] = serde_json::json!({"operationId": "createWidget"});
    }
    serde_json::json!({
        "openapi": "3.0.3",
        "info": {"title": "Widgets", "version": "1.0.0"},
        "paths": paths
    })
}

/// Publish a spec version through the real product surface.
async fn publish_spec(env: &Env, token: &str, team_name: &str, api_name: &str, version: i64) {
    let response = env
        .app
        .clone()
        .oneshot(request(
            "POST",
            &format!(
                "/api/v1/teams/{team_name}/api-definitions/{api_name}/specs/{version}/publish"
            ),
            token,
            Some(serde_json::json!({"reason": "test"})),
        ))
        .await
        .expect("publish");
    let status = response.status();
    let body = json_of(response).await;
    assert_eq!(status, StatusCode::OK, "publish fixture: {body}");
}

/// Seed an extra spec_versions row directly (append-only fixture) so
/// latest_version can exceed the published version without a second import path.
async fn seed_spec_version(env: &Env, fx: &TeamFixture, api_id: Uuid, version: i64) {
    // 64 hex chars, unique per row (unique (api_definition_id, spec_hash)).
    let spec_hash = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let spec = serde_json::json!({
        "openapi": "3.0.3",
        "info": {"title": "seeded", "version": format!("{version}.0.0")},
        "paths": {}
    });
    sqlx::query(
        "INSERT INTO spec_versions \
         (id, team_id, org_id, api_definition_id, version, source_kind, format, spec, spec_hash) \
         VALUES ($1, $2, $3, $4, $5, 'learned', 'openapi3', $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(fx.team_id.as_uuid())
    .bind(fx.org_id.as_uuid())
    .bind(api_id)
    .bind(version)
    .bind(&spec)
    .bind(&spec_hash)
    .execute(&env.pool)
    .await
    .expect("seed spec version");
}

/// Seed a route_configs row directly (fixture only — pre-existing gateway scope
/// the API binds to at create time).
async fn seed_route_config(env: &Env, fx: &TeamFixture, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, '{\"virtual_hosts\":[]}'::jsonb)",
    )
    .bind(id)
    .bind(fx.team_id.as_uuid())
    .bind(fx.org_id.as_uuid())
    .bind(name)
    .execute(&env.pool)
    .await
    .expect("seed route config");
    id
}

/// The three canonical fixture APIs, name-ordered A < B < C via a shared unique
/// prefix so list ordering (and pagination slicing) is deterministic:
/// - A: 2 operations (=> 2 tools, imported v1), v1 PUBLISHED, plus a directly
///   seeded spec_versions row v2 -> latest 2, published 1;
/// - B: NO openapi -> no versions, no tools, nothing published;
/// - C: 1 operation (=> 1 tool, v1), one create-time route binding, UNPUBLISHED.
struct ThreeApis {
    name_a: String,
    name_b: String,
    name_c: String,
}

async fn build_three_apis(env: &Env, fx: &TeamFixture, token: &str) -> ThreeApis {
    let prefix = unique("apix");
    let name_a = format!("{prefix}-a");
    let name_b = format!("{prefix}-b");
    let name_c = format!("{prefix}-c");

    let api_a = create_api(
        env,
        token,
        &fx.team_name,
        &name_a,
        Some(openapi_doc(2)),
        None,
    )
    .await;
    publish_spec(env, token, &fx.team_name, &name_a, 1).await;
    seed_spec_version(env, fx, api_a, 2).await;

    create_api(env, token, &fx.team_name, &name_b, None, None).await;

    let rc_id = seed_route_config(env, fx, &unique("rc")).await;
    create_api(
        env,
        token,
        &fx.team_name,
        &name_c,
        Some(openapi_doc(1)),
        Some(rc_id),
    )
    .await;

    ThreeApis {
        name_a,
        name_b,
        name_c,
    }
}

/// Find the row for `name` in a listing body (panics if absent).
fn row<'a>(body: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["name"] == name)
        .unwrap_or_else(|| panic!("api {name} missing from listing: {body}"))
}

/// Assert one row's full enrichment quartet. `latest`/`published` == None means the
/// key must be strictly ABSENT (omitted, not null-with-key).
fn assert_enrichment(
    item: &serde_json::Value,
    tool_count: i64,
    binding_count: i64,
    latest: Option<i64>,
    published: Option<i64>,
) {
    let name = &item["name"];
    assert_eq!(
        item["tool_count"],
        serde_json::json!(tool_count),
        "tool_count for {name}: {item}"
    );
    assert_eq!(
        item["route_binding_count"],
        serde_json::json!(binding_count),
        "route_binding_count for {name}: {item}"
    );
    match latest {
        Some(v) => assert_eq!(
            item["latest_version"],
            serde_json::json!(v),
            "latest_version for {name}: {item}"
        ),
        None => assert!(
            item.get("latest_version").is_none(),
            "latest_version must be OMITTED (not null) when {name} has no versions: {item}"
        ),
    }
    match published {
        Some(v) => assert_eq!(
            item["published_version"],
            serde_json::json!(v),
            "published_version for {name}: {item}"
        ),
        None => assert!(
            item.get("published_version").is_none(),
            "published_version must be OMITTED (not null) when {name} has nothing \
             published: {item}"
        ),
    }
}

// --- Scenario 1: enrichment correctness for the three canonical shapes ----------------

#[tokio::test]
async fn list_rows_carry_exact_enrichment_for_published_empty_and_bound_apis() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let apis = build_three_apis(&env, &fx, &token).await;

    let (status, _, body) = get_json(&env, &list_uri(&fx.team_name), &token).await;
    assert_eq!(status, StatusCode::OK, "list api definitions: {body}");
    assert_page_envelope(&body);
    assert_eq!(body["total"], 3, "exactly this test's three APIs: {body}");

    // Ordering by name (A < B < C by construction).
    let names: Vec<&str> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        vec![
            apis.name_a.as_str(),
            apis.name_b.as_str(),
            apis.name_c.as_str()
        ],
        "items ordered by name: {body}"
    );

    // The previous (pre-enrichment) item fields must still be present.
    for item in body["items"].as_array().expect("items") {
        Uuid::parse_str(item["id"].as_str().expect("id string")).expect("id is a uuid");
        assert!(item["name"].is_string(), "name present: {body}");
        assert!(
            item["revision"].is_i64() || item["revision"].is_u64(),
            "revision present: {body}"
        );
        assert!(item["created_at"].is_string(), "created_at present: {body}");
        assert!(item["updated_at"].is_string(), "updated_at present: {body}");
    }

    // A: 2 tools, 0 bindings, latest 2 (seeded row), published 1 (the real publish).
    assert_enrichment(row(&body, &apis.name_a), 2, 0, Some(2), Some(1));
    // A has a published spec, so the pre-existing published_spec_version_id rides too.
    assert!(
        row(&body, &apis.name_a)["published_spec_version_id"].is_string(),
        "published API carries published_spec_version_id: {body}"
    );
    // B: nothing at all — counts 0, both version keys strictly absent.
    assert_enrichment(row(&body, &apis.name_b), 0, 0, None, None);
    // C: 1 tool, 1 binding, latest 1, unpublished.
    assert_enrichment(row(&body, &apis.name_c), 1, 1, Some(1), None);

    // tool_count counts ALL generated tools, enabled or not: disable one of A's
    // tools through the real PATCH surface and re-list — the count must not move.
    let (status, _, tools) = get_json(
        &env,
        &format!(
            "/api/v1/teams/{}/api-definitions/{}/tools",
            fx.team_name, apis.name_a
        ),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "tools fixture lookup: {tools}");
    let victim = tools["items"][0]["name"]
        .as_str()
        .expect("tool name")
        .to_string();
    let response = env
        .app
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("/api/v1/teams/{}/mcp/tools/{victim}", fx.team_name),
            &token,
            Some(serde_json::json!({"enabled": false})),
        ))
        .await
        .expect("disable tool");
    assert_eq!(response.status(), StatusCode::OK, "disable tool fixture");

    let (status, _, after) = get_json(&env, &list_uri(&fx.team_name), &token).await;
    assert_eq!(status, StatusCode::OK, "re-list after disable: {after}");
    assert_eq!(
        row(&after, &apis.name_a)["tool_count"],
        2,
        "tool_count includes disabled tools (enabled or not): {after}"
    );
}

// --- Scenario 2: row enrichment equals the per-API /status report ---------------------

#[tokio::test]
async fn each_row_matches_its_own_status_endpoint() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let apis = build_three_apis(&env, &fx, &token).await;

    let (status, _, body) = get_json(&env, &list_uri(&fx.team_name), &token).await;
    assert_eq!(status, StatusCode::OK, "list api definitions: {body}");
    assert_eq!(body["total"], 3, "{body}");

    // The acceptance intent is "no /status call per row needed" — so the test does
    // the N+1 the endpoint must obviate, and pins row == status for every API.
    for name in [&apis.name_a, &apis.name_b, &apis.name_c] {
        let item = row(&body, name);
        let (st, _, report) = get_json(&env, &status_uri(&fx.team_name, name), &token).await;
        assert_eq!(st, StatusCode::OK, "status for {name}: {report}");

        assert_eq!(
            item["tool_count"], report["tool_count"],
            "row tool_count must equal /status tool_count for {name}: \
             row={item} status={report}"
        );
        assert_eq!(
            item["route_binding_count"], report["route_binding_count"],
            "row route_binding_count must equal /status route_binding_count for {name}: \
             row={item} status={report}"
        );
        let row_latest = item.get("latest_version").and_then(|v| v.as_i64());
        let status_latest = report
            .get("latest_spec")
            .and_then(|s| s.get("version"))
            .and_then(|v| v.as_i64());
        assert_eq!(
            row_latest, status_latest,
            "row latest_version must equal /status latest_spec.version for {name}: \
             row={item} status={report}"
        );
    }
}

// --- Scenario 3: pagination unchanged, enrichment rides on every page -----------------

#[tokio::test]
async fn pagination_slices_name_order_with_stable_total_and_enrichment() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let apis = build_three_apis(&env, &fx, &token).await;
    let base = list_uri(&fx.team_name);

    // Page 1: limit=2 offset=0 -> [A, B], total 3.
    let (status, _, page1) = get_json(&env, &format!("{base}?limit=2&offset=0"), &token).await;
    assert_eq!(status, StatusCode::OK, "page 1: {page1}");
    assert_page_envelope(&page1);
    assert_eq!(page1["total"], 3, "total is page-independent: {page1}");
    assert_eq!(page1["limit"], 2, "limit echoed: {page1}");
    assert_eq!(page1["offset"], 0, "offset echoed: {page1}");
    let names: Vec<&str> = page1["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        vec![apis.name_a.as_str(), apis.name_b.as_str()],
        "first page is the first two by name: {page1}"
    );
    // Enrichment must ride along on paginated pages, not only on the full list.
    assert_enrichment(row(&page1, &apis.name_a), 2, 0, Some(2), Some(1));
    assert_enrichment(row(&page1, &apis.name_b), 0, 0, None, None);

    // Page 2: limit=2 offset=2 -> [C], total stays 3.
    let (status, _, page2) = get_json(&env, &format!("{base}?limit=2&offset=2"), &token).await;
    assert_eq!(status, StatusCode::OK, "page 2: {page2}");
    assert_page_envelope(&page2);
    assert_eq!(page2["total"], 3, "total unchanged on page 2: {page2}");
    assert_eq!(page2["limit"], 2, "limit echoed: {page2}");
    assert_eq!(page2["offset"], 2, "offset echoed: {page2}");
    let names: Vec<&str> = page2["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        vec![apis.name_c.as_str()],
        "second page carries the last API by name: {page2}"
    );
    assert_enrichment(row(&page2, &apis.name_c), 1, 1, Some(1), None);
}

// --- Scenario 4: auth conventions still hold on the enriched endpoint -----------------

#[tokio::test]
async fn unknown_team_cross_org_and_no_grant_conventions_hold() {
    let Some(env) = env().await else { return };
    // One org, two teams: the APIs live in team A; the no-grant caller is granted
    // on team B only.
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
    let apis = build_three_apis(&env, &fx, &admin_token).await;

    // Unknown team name -> 404 envelope even for an org admin.
    let ghost_team = unique("ghost-team");
    let (status, rid, body) = get_json(&env, &list_uri(&ghost_team), &admin_token).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown team must read as absent, got {status}: {body}"
    );
    assert_error_envelope(&body, "not_found", rid);

    // Sanity (auth contract): a member holding api-definitions:read ON TEAM A reads
    // the enriched listing — the denials below can never pass vacuously.
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
    let (status, _, body) = get_json(&env, &list_uri(&fx.team_name), &granted_token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "api-definitions:read grant must open the enriched listing, got {status}: {body}"
    );
    assert_eq!(body["total"], 3, "granted reader sees all rows: {body}");
    assert_enrichment(row(&body, &apis.name_a), 2, 0, Some(2), Some(1));

    // Cross-org caller -> 404 (never 403) by team name AND team UUID; no data leak.
    let other_org = identity::create_org(&env.pool, &unique("org-p"), "")
        .await
        .expect("other org");
    let (_, foreign_token) = user_with_org_role(&env, other_org.id, OrgRole::Admin).await;
    for team_ref in [fx.team_name.clone(), fx.team_id.as_uuid().to_string()] {
        let (status, rid, body) = get_json(&env, &list_uri(&team_ref), &foreign_token).await;
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
            !raw.contains(&apis.name_a)
                && !raw.contains(&apis.name_b)
                && !raw.contains(&apis.name_c),
            "api names leaked cross-org: {body}"
        );
    }

    // Same-org caller granted ONLY on team B -> 403 forbidden envelope on team A's
    // listing (pinned convention from the sibling slices), with no data leak.
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
    let (status, rid, body) = get_json(&env, &list_uri(&fx.team_name), &no_grant_token).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "pinned convention for same-org no-grant reads, got {status}: {body}"
    );
    assert_error_envelope(&body, "forbidden", rid);
    let raw = body.to_string();
    assert!(
        !raw.contains(&apis.name_a) && !raw.contains(&apis.name_b) && !raw.contains(&apis.name_c),
        "api names leaked to a no-grant caller: {body}"
    );
}
