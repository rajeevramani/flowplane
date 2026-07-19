//! Black-box contract tests for the API-lifecycle read-model list endpoints
//! `GET /api/v1/teams/{team}/api-definitions/{name}/route-bindings` and
//! `GET /api/v1/teams/{team}/api-definitions/{name}/tools`.
//!
//! Written from acceptance criteria only — both endpoints are exercised strictly over
//! HTTP through the production router/middleware stack. Fixtures use pre-existing
//! public endpoints (POST api-definitions with an inline OpenAPI document + a
//! `route_binding`, PATCH mcp/tools/{name} to disable a tool) plus a direct sqlx
//! INSERT into `route_configs` (fixtures may touch the DB; the endpoints under test
//! never are).
//!
//! Contract under test:
//! - 200 with the uniform Page envelope {"items", "total", "limit", "offset"}.
//! - route-bindings items ordered by binding name; item shape id (uuid), name,
//!   api_definition_id (uuid), route_config_id (uuid), optional listener_id /
//!   virtual_host / route (keys OMITTED when null), created_at. Typed IDs ONLY —
//!   no resolved route-config/listener names ride along.
//! - tools items ordered by tool name; item shape id, name, api_definition_id,
//!   spec_version_id, operation_id, method, path, input_schema, output_schema,
//!   enabled (bool), created_at, updated_at.
//! - CRITICAL: the tools list INCLUDES disabled tools exactly as
//!   PATCH /mcp/tools/{name} {"enabled": false} left them (enabled:false visible,
//!   every other field unchanged, total unchanged).
//! - limit/offset pagination with page-independent total.
//! - Unknown API name -> 404; cross-org caller -> 404 (anti-enumeration, by team name
//!   AND team UUID); same-org caller with no grant -> 403 with NO existence oracle
//!   (conventions pinned in spec_versions_list.rs / spec_review_events.rs).
//!
//! Parallel-safe: every org/team/user/api/route-config name is uuid-suffixed and
//! unique per test; no global row-count assertions; in-process router via `oneshot`
//! (no TCP ports). Skipped (with a notice) when FLOWPLANE_TEST_DATABASE_URL is unset.

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

/// Seed a route_configs row directly (fixture only — pre-existing gateway scope the
/// API binds to at create time). The uuid-unique `name` doubles as a leak marker:
/// it must NEVER appear in a route-bindings listing (typed IDs only).
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

/// Create an API definition over HTTP whose OpenAPI document carries three
/// operations (distinct operationIds -> three generated tools), optionally bound to
/// a route config at create time. Returns (api_name, api definition uuid).
async fn create_api(
    env: &Env,
    token: &str,
    team_name: &str,
    route_config_id: Option<Uuid>,
) -> (String, Uuid) {
    let api_name = unique("api");
    let mut payload = serde_json::json!({
        "name": api_name,
        "openapi": {
            "openapi": "3.0.3",
            "info": {"title": "Widgets", "version": "1.0.0"},
            "paths": {
                "/widgets": {
                    "get": {"operationId": "listWidgets"},
                    "post": {"operationId": "createWidget"}
                },
                "/widgets/{id}": {
                    "get": {"operationId": "getWidget"}
                }
            }
        }
    });
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
        "create api definition fixture: {body}"
    );
    let api_id = Uuid::parse_str(body["api"]["id"].as_str().expect("api id")).expect("api uuid");
    (api_name, api_id)
}

/// Resolve the imported spec version's row id (fixture lookup, not under test).
async fn spec_version_v1_id(pool: &PgPool, api_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM spec_versions WHERE api_definition_id = $1 AND version = 1")
        .bind(api_id)
        .fetch_one(pool)
        .await
        .expect("spec version id")
}

/// Disable one generated tool through the real product surface:
/// PATCH /api/v1/teams/{team}/mcp/tools/{tool} {"enabled": false}.
async fn disable_tool(env: &Env, token: &str, team_name: &str, tool_name: &str) {
    let response = env
        .app
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("/api/v1/teams/{team_name}/mcp/tools/{tool_name}"),
            token,
            Some(serde_json::json!({"enabled": false})),
        ))
        .await
        .expect("disable tool");
    let status = response.status();
    let body = json_of(response).await;
    assert_eq!(status, StatusCode::OK, "disable tool fixture: {body}");
}

fn bindings_uri(team: &str, api: &str) -> String {
    format!("/api/v1/teams/{team}/api-definitions/{api}/route-bindings")
}

fn tools_uri(team: &str, api: &str) -> String {
    format!("/api/v1/teams/{team}/api-definitions/{api}/tools")
}

/// Clone `item` with the given keys removed (for field-by-field stability diffs).
fn without(item: &serde_json::Value, keys: &[&str]) -> serde_json::Value {
    let mut clone = item.clone();
    if let Some(obj) = clone.as_object_mut() {
        for key in keys {
            obj.remove(*key);
        }
    }
    clone
}

// --- Scenario 1: bindings list — typed IDs, absent-when-null optionals ---------------

#[tokio::test]
async fn bindings_list_returns_typed_binding_with_null_optionals_omitted() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    // The uuid-unique route-config name is the resolved-name leak marker.
    let rc_name = unique("rc-leakmark");
    let rc_id = seed_route_config(&env, &fx, &rc_name).await;
    let (api_name, api_id) = create_api(&env, &token, &fx.team_name, Some(rc_id)).await;

    let (status, _, body) = get_json(&env, &bindings_uri(&fx.team_name, &api_name), &token).await;
    assert_eq!(status, StatusCode::OK, "list route bindings: {body}");
    assert_page_envelope(&body);
    assert_eq!(
        body["total"], 1,
        "exactly the one create-time binding: {body}"
    );
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{body}");
    let item = &items[0];

    // id is a uuid; name is a non-empty string; created_at present.
    Uuid::parse_str(item["id"].as_str().expect("id string")).expect("id is a uuid");
    assert!(
        !item["name"]
            .as_str()
            .expect("binding name string")
            .is_empty(),
        "binding name present: {body}"
    );
    assert!(item["created_at"].is_string(), "created_at present: {body}");

    // Typed FK ids match the fixture rows exactly.
    assert_eq!(
        item["api_definition_id"],
        api_id.to_string(),
        "api_definition_id is the owning API's uuid: {body}"
    );
    assert_eq!(
        item["route_config_id"],
        rc_id.to_string(),
        "route_config_id is the seeded route config's uuid: {body}"
    );

    // Null optionals: the KEYS must be strictly absent, not null-with-key.
    for key in ["listener_id", "virtual_host", "route"] {
        assert!(
            item.get(key).is_none(),
            "{key} must be OMITTED (not null) when the binding has none: {body}"
        );
    }

    // Typed IDs only: the resolved route-config name must not ride along anywhere,
    // under any field name.
    assert!(
        !body.to_string().contains(&rc_name),
        "resolved route-config name leaked into the bindings listing: {body}"
    );
}

// --- Scenario 2: tools list — order, full shape, disabled rows stay listed -----------

#[tokio::test]
async fn tools_list_orders_by_name_and_keeps_disabled_tools_listed() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let (api_name, api_id) = create_api(&env, &token, &fx.team_name, None).await;
    let v1_id = spec_version_v1_id(&env.pool, api_id).await;

    let (status, _, body) = get_json(&env, &tools_uri(&fx.team_name, &api_name), &token).await;
    assert_eq!(status, StatusCode::OK, "list tools: {body}");
    assert_page_envelope(&body);
    assert_eq!(body["total"], 3, "three operations -> three tools: {body}");
    let items = body["items"].as_array().expect("items").clone();
    assert_eq!(items.len(), 3, "{body}");

    // Ordered by tool name ascending.
    let names: Vec<String> = items
        .iter()
        .map(|i| i["name"].as_str().expect("tool name").to_string())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "tools ordered by name: {body}");

    // Full item shape, and per-operation method/path mapping (keyed by operation_id
    // so no tool-name derivation scheme is assumed).
    let expected: std::collections::BTreeMap<&str, (&str, &str)> = [
        ("listWidgets", ("get", "/widgets")),
        ("createWidget", ("post", "/widgets")),
        ("getWidget", ("get", "/widgets/{id}")),
    ]
    .into_iter()
    .collect();
    let mut seen_ops = Vec::new();
    for item in &items {
        Uuid::parse_str(item["id"].as_str().expect("id string")).expect("id is a uuid");
        assert_eq!(
            item["api_definition_id"],
            api_id.to_string(),
            "api_definition_id is the owning API's uuid: {body}"
        );
        assert_eq!(
            item["spec_version_id"],
            v1_id.to_string(),
            "spec_version_id is the imported v1's uuid: {body}"
        );
        let op = item["operation_id"].as_str().expect("operation_id");
        let (method, path) = expected
            .get(op)
            .unwrap_or_else(|| panic!("unexpected operation_id {op}: {body}"));
        assert!(
            item["method"]
                .as_str()
                .expect("method string")
                .eq_ignore_ascii_case(method),
            "method for {op}: {body}"
        );
        assert_eq!(item["path"], *path, "path for {op}: {body}");
        assert!(
            item.get("input_schema").is_some(),
            "input_schema present for {op}: {body}"
        );
        assert!(
            item.get("output_schema").is_some(),
            "output_schema present for {op}: {body}"
        );
        assert_eq!(
            item["enabled"], true,
            "freshly generated tools start enabled: {body}"
        );
        assert!(item["created_at"].is_string(), "created_at present: {body}");
        assert!(item["updated_at"].is_string(), "updated_at present: {body}");
        seen_ops.push(op.to_string());
    }
    seen_ops.sort();
    assert_eq!(
        seen_ops,
        vec!["createWidget", "getWidget", "listWidgets"],
        "every generated operation appears exactly once: {body}"
    );

    // Disable the MIDDLE tool (by name order) through the real PATCH surface, then
    // re-list: the row must STILL be present, enabled:false, everything else as the
    // PATCH left it, total unchanged.
    let victim = items[1].clone();
    let victim_name = victim["name"].as_str().expect("victim name").to_string();
    disable_tool(&env, &token, &fx.team_name, &victim_name).await;

    let (status, _, after) = get_json(&env, &tools_uri(&fx.team_name, &api_name), &token).await;
    assert_eq!(status, StatusCode::OK, "re-list after disable: {after}");
    assert_eq!(
        after["total"], 3,
        "disabling must not shrink the read-model total: {after}"
    );
    let after_items = after["items"].as_array().expect("items");
    assert_eq!(
        after_items.len(),
        3,
        "the disabled row must STILL be listed: {after}"
    );

    let disabled = after_items
        .iter()
        .find(|i| i["name"] == victim_name.as_str())
        .unwrap_or_else(|| panic!("disabled tool {victim_name} vanished from the list: {after}"));
    assert_eq!(
        disabled["enabled"], false,
        "the list must show the tool exactly as PATCH left it (enabled:false): {after}"
    );
    // Every other field is unchanged (updated_at may legitimately move on PATCH).
    assert_eq!(
        without(disabled, &["enabled", "updated_at"]),
        without(&victim, &["enabled", "updated_at"]),
        "disable must change nothing but the enabled flag: before={victim} after={disabled}"
    );

    // The two untouched tools are byte-for-byte identical.
    for original in [&items[0], &items[2]] {
        let name = original["name"].as_str().expect("name");
        let now = after_items
            .iter()
            .find(|i| i["name"] == name)
            .unwrap_or_else(|| panic!("tool {name} vanished: {after}"));
        assert_eq!(
            now, original,
            "untouched tools must be unchanged by a sibling's disable: {after}"
        );
    }
}

// --- Scenario 3: pagination on tools --------------------------------------------------

#[tokio::test]
async fn tools_pagination_returns_middle_tool_with_stable_total() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let (api_name, _) = create_api(&env, &token, &fx.team_name, None).await;
    let base = tools_uri(&fx.team_name, &api_name);

    // Full list first: the middle-by-name tool is derived from the response itself.
    let (status, _, full) = get_json(&env, &base, &token).await;
    assert_eq!(status, StatusCode::OK, "full list: {full}");
    assert_eq!(full["total"], 3, "{full}");
    let middle = full["items"].as_array().expect("items")[1].clone();

    let (status, _, page) = get_json(&env, &format!("{base}?limit=1&offset=1"), &token).await;
    assert_eq!(status, StatusCode::OK, "paged list: {page}");
    assert_page_envelope(&page);
    assert_eq!(page["total"], 3, "total is page-independent: {page}");
    assert_eq!(page["limit"], 1, "limit echoed: {page}");
    assert_eq!(page["offset"], 1, "offset echoed: {page}");
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "limit=1 yields one item: {page}");
    assert_eq!(
        items[0], middle,
        "limit=1 offset=1 is exactly the middle tool by name order: {page}"
    );
}

// --- Scenario 4: unknown API name -> 404 for both endpoints ---------------------------

#[tokio::test]
async fn unknown_api_name_returns_404_for_both_endpoints() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let ghost = unique("ghost");
    for uri in [
        bindings_uri(&fx.team_name, &ghost),
        tools_uri(&fx.team_name, &ghost),
    ] {
        let (status, rid, body) = get_json(&env, &uri, &token).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "unknown api name must read as absent on {uri}, got {status}: {body}"
        );
        assert_error_envelope(&body, "not_found", rid);
    }
}

// --- Scenario 5: cross-org anti-enumeration -------------------------------------------

#[tokio::test]
async fn cross_org_caller_gets_404_for_both_endpoints() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, owner_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let rc_name = unique("rc-leakmark");
    let rc_id = seed_route_config(&env, &fx, &rc_name).await;
    let (api_name, _) = create_api(&env, &owner_token, &fx.team_name, Some(rc_id)).await;

    // Capture a real generated tool name as a leak marker (owner's view).
    let (status, _, owner_tools) =
        get_json(&env, &tools_uri(&fx.team_name, &api_name), &owner_token).await;
    assert_eq!(status, StatusCode::OK, "owner tools sanity: {owner_tools}");
    let tool_marker = owner_tools["items"][0]["name"]
        .as_str()
        .expect("tool name")
        .to_string();

    let other_org = identity::create_org(&env.pool, &unique("org-p"), "")
        .await
        .expect("other org");
    let (_, foreign_token) = user_with_org_role(&env, other_org.id, OrgRole::Admin).await;

    for team_ref in [fx.team_name.clone(), fx.team_id.as_uuid().to_string()] {
        for uri in [
            bindings_uri(&team_ref, &api_name),
            tools_uri(&team_ref, &api_name),
        ] {
            let (status, rid, body) = get_json(&env, &uri, &foreign_token).await;
            assert_ne!(
                status,
                StatusCode::FORBIDDEN,
                "403 on {uri} confirms the team exists to an outsider: {body}"
            );
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "cross-org access to {uri} must read as absent, got {status}: {body}"
            );
            assert_error_envelope(&body, "not_found", rid);
            let raw = body.to_string();
            assert!(
                !raw.contains(&rc_name) && !raw.contains(&tool_marker),
                "binding/tool data leaked cross-org on {uri}: {body}"
            );
        }
    }
}

// --- Scenario 6: same-org caller without a grant on the team --------------------------

#[tokio::test]
async fn same_org_caller_without_grant_gets_403_with_no_existence_oracle() {
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

    let rc_name = unique("rc-leakmark");
    let rc_id = seed_route_config(&env, &fx, &rc_name).await;
    let (api_name, _) = create_api(&env, &admin_token, &fx.team_name, Some(rc_id)).await;

    // Sanity (auth contract): a member holding api-definitions:read ON TEAM A reads
    // both listings — the denials below can never pass vacuously.
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
    let (status, _, body) = get_json(
        &env,
        &bindings_uri(&fx.team_name, &api_name),
        &granted_token,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "api-definitions:read grant must open the bindings list, got {status}: {body}"
    );
    assert_eq!(body["total"], 1, "granted reader sees the binding: {body}");
    let (status, _, body) =
        get_json(&env, &tools_uri(&fx.team_name, &api_name), &granted_token).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "api-definitions:read grant must open the tools list, got {status}: {body}"
    );
    assert_eq!(body["total"], 3, "granted reader sees the tools: {body}");
    let tool_marker = body["items"][0]["name"]
        .as_str()
        .expect("tool name")
        .to_string();

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

    let ghost = unique("ghost");
    for (existing_uri, ghost_uri) in [
        (
            bindings_uri(&fx.team_name, &api_name),
            bindings_uri(&fx.team_name, &ghost),
        ),
        (
            tools_uri(&fx.team_name, &api_name),
            tools_uri(&fx.team_name, &ghost),
        ),
    ] {
        let (status_existing, rid_existing, body_existing) =
            get_json(&env, &existing_uri, &no_grant_token).await;
        let (status_absent, _, body_absent) = get_json(&env, &ghost_uri, &no_grant_token).await;

        // Load-bearing invariant: the denial must not vary with resource existence, so
        // the status can never be used to probe which API names team A owns.
        assert_eq!(
            status_existing, status_absent,
            "same-org no-grant denial must be identical for existing vs absent APIs on \
             {existing_uri} (no oracle): existing={body_existing} absent={body_absent}"
        );
        assert!(
            status_existing == StatusCode::FORBIDDEN || status_existing == StatusCode::NOT_FOUND,
            "no-grant caller must be denied on {existing_uri}, got {status_existing}: \
             {body_existing}"
        );
        // Pin the codebase's convention (team-level NoMatchingGrant -> 403, as pinned
        // for the previous slices in spec_versions_list.rs / spec_review_events.rs).
        assert_eq!(
            status_existing,
            StatusCode::FORBIDDEN,
            "pinned convention for same-org no-grant reads on {existing_uri}: {body_existing}"
        );
        assert_error_envelope(&body_existing, "forbidden", rid_existing);

        // No binding/tool data may ride along on the denial.
        let raw = body_existing.to_string();
        assert!(
            !raw.contains(&rc_name) && !raw.contains(&tool_marker),
            "binding/tool data leaked to a no-grant caller on {existing_uri}: {body_existing}"
        );
    }
}
