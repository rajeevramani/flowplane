//! Black-box acceptance tests for the REST MCP tool catalog
//! `GET /api/v1/teams/{team}/mcp/tools` (bead fpv2-zl8.3).
//!
//! Written from acceptance criteria only — the endpoint is exercised strictly over
//! HTTP through the production router/middleware stack. The expected static tool set
//! is derived from the public `fp_core::mcp_declarations::STATIC_TOOL_DECLS` registry
//! (never a hardcoded count), and dynamic tools are seeded through pre-existing public
//! product surfaces (POST api-definitions with inline OpenAPI, POST specs publish,
//! PATCH mcp/tools/{name} disable) plus the public storage spec-version helper for an
//! unpublished version.
//!
//! Contract under test:
//! - 200 JSON array; each row is exactly {name, description, input_schema, resource,
//!   action, risk, kind, enabled, executable_by_caller}; kind in {static, dynamic};
//!   risk in {read, mutate, delete}.
//! - EVERY static registry entry is present, kind="static", enabled=true, with
//!   resource/action/risk matching the declaration.
//! - Dynamic rows: only api_tools of the CURRENTLY PUBLISHED spec version; default
//!   shows enabled only; ?include_disabled=true adds disabled rows; non-published
//!   spec versions never appear. kind="dynamic", name "api_"-prefixed, resource
//!   "mcp-tools", action "execute".
//! - Authz: catalog read requires (mcp-tools, read) on the path team (org Admin
//!   implicit; grantless same-org member 403). include_disabled=true additionally
//!   requires (mcp-tools, update) and FAILS CLOSED (403, not a silent downgrade).
//! - executable_by_caller: static row iff the caller passes that row's own
//!   (resource, action); dynamic row iff (mcp-tools, execute) AND enabled — a
//!   disabled row is executable_by_caller=false even for an org Admin.
//! - PARITY: for a principal holding (mcp-tools, read), the MCP `tools/list` name set
//!   equals the set of catalog rows with executable_by_caller=true, both directions.
//! - Cross-org caller: anti-enumeration — the same 404/not_found convention as other
//!   team-scoped reads (never a 403 existence oracle).
//!
//! Parallel-safe: every org/team/user/api name is uuid-suffixed and unique per test;
//! static-set assertions are membership/subset based (never global row counts beyond
//! this team's seeded dynamic rows); in-process router via `oneshot`. Skips with a
//! notice when FLOWPLANE_TEST_DATABASE_URL is unset.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_core::mcp_declarations::STATIC_TOOL_DECLS;
use fp_domain::api_lifecycle::{SpecFormat, SpecSourceKind, SpecVersionInput};
use fp_domain::authz::{Action, Resource, TeamRef};
use fp_domain::{ApiDefinitionId, OrgId, OrgRole, TeamId, UserId};
use fp_storage::repos::{api_lifecycle as storage_api_lifecycle, identity};
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

struct TeamFixture {
    org_id: OrgId,
    team_id: TeamId,
    team_name: String,
    team: TeamRef,
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
        team: TeamRef {
            id: team.id,
            org_id: org.id,
        },
    }
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

/// Seed one (resource, action) grant on `team_id` for a user principal.
async fn grant(env: &Env, user: UserId, fx: &TeamFixture, resource: Resource, action: Action) {
    identity::add_grant(
        &env.pool, user, fx.org_id, fx.team_id, resource, action, None,
    )
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

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

struct CatalogResponse {
    status: StatusCode,
    content_type: Option<String>,
    request_id: Option<Uuid>,
    body: serde_json::Value,
}

fn catalog_uri(team: &str, include_disabled: bool) -> String {
    if include_disabled {
        format!("/api/v1/teams/{team}/mcp/tools?include_disabled=true")
    } else {
        format!("/api/v1/teams/{team}/mcp/tools")
    }
}

/// GET the catalog as `token`, capturing status, content-type and request id.
async fn get_catalog(
    env: &Env,
    team: &str,
    include_disabled: bool,
    token: &str,
) -> CatalogResponse {
    let response = env
        .app
        .clone()
        .oneshot(request(
            "GET",
            &catalog_uri(team, include_disabled),
            token,
            None,
        ))
        .await
        .expect("catalog response");
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok());
    CatalogResponse {
        status,
        content_type,
        request_id,
        body: json_of(response).await,
    }
}

/// Assert a 200 catalog response and return its rows.
fn rows_of(response: &CatalogResponse) -> Vec<serde_json::Value> {
    assert_eq!(
        response.status,
        StatusCode::OK,
        "catalog read must succeed: {}",
        response.body
    );
    response
        .body
        .as_array()
        .unwrap_or_else(|| panic!("catalog must be a JSON array: {}", response.body))
        .clone()
}

/// The exact row key set the spec pins.
const ROW_KEYS: [&str; 9] = [
    "name",
    "description",
    "input_schema",
    "resource",
    "action",
    "risk",
    "kind",
    "enabled",
    "executable_by_caller",
];

fn assert_row_shape(row: &serde_json::Value) {
    let obj = row
        .as_object()
        .unwrap_or_else(|| panic!("catalog row must be an object: {row}"));
    let keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = ROW_KEYS.into_iter().collect();
    assert_eq!(
        keys, expected,
        "row must carry exactly the pinned key set: {row}"
    );
    let kind = row["kind"].as_str().expect("kind string");
    assert!(
        kind == "static" || kind == "dynamic",
        "kind must be static|dynamic: {row}"
    );
    let risk = row["risk"].as_str().expect("risk string");
    assert!(
        ["read", "mutate", "delete"].contains(&risk),
        "risk must be read|mutate|delete: {row}"
    );
    assert!(row["enabled"].is_boolean(), "enabled is a bool: {row}");
    assert!(
        row["executable_by_caller"].is_boolean(),
        "executable_by_caller is a bool: {row}"
    );
}

fn find_row<'a>(rows: &'a [serde_json::Value], name: &str) -> Option<&'a serde_json::Value> {
    rows.iter().find(|r| r["name"] == name)
}

fn executable_names(rows: &[serde_json::Value]) -> BTreeSet<String> {
    rows.iter()
        .filter(|r| r["executable_by_caller"] == true)
        .map(|r| r["name"].as_str().expect("name").to_string())
        .collect()
}

/// Assert the standard error envelope for `code` (shape pinned by the other
/// team-scoped read suites: {"code", ..., "request_id"} matching x-request-id),
/// plus the JSON content type on errors.
fn assert_error_envelope(response: &CatalogResponse, code: &str) {
    assert!(
        response.body.is_object(),
        "error responses must be the envelope object: {}",
        response.body
    );
    assert_eq!(
        response.body["code"], code,
        "unexpected error code in {}",
        response.body
    );
    let rid = response.request_id.expect("x-request-id header present");
    assert_eq!(
        response.body["request_id"],
        rid.to_string(),
        "envelope and header request id agree: {}",
        response.body
    );
    let content_type = response
        .content_type
        .as_deref()
        .expect("content-type header present on errors");
    assert!(
        content_type.starts_with("application/json"),
        "error responses must be application/json, got {content_type}"
    );
}

// --- MCP JSON-RPC helpers (same surface as mcp_static_tools.rs) -----------------------

fn mcp_request(token: &str, session: Option<&str>, body: serde_json::Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/mcp")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    builder.body(Body::from(body.to_string())).expect("request")
}

async fn initialize(env: &Env, token: &str) -> String {
    let response = env
        .app
        .clone()
        .oneshot(mcp_request(
            token,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            }),
        ))
        .await
        .expect("initialize");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session")
        .to_string()
}

async fn tools_list(env: &Env, token: &str, session: &str, team: &str) -> BTreeSet<String> {
    let response = env
        .app
        .clone()
        .oneshot(mcp_request(
            token,
            Some(session),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": { "team": team }
            }),
        ))
        .await
        .expect("tools/list");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    body["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools array in {body}"))
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_string())
        .collect()
}

// --- Dynamic tool seeding fixtures ----------------------------------------------------

/// Create an API definition over HTTP with an inline two-operation OpenAPI document
/// (v1 imported, tools generated). Returns (api_name, api uuid).
async fn create_api(env: &Env, token: &str, team_name: &str) -> (String, Uuid) {
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
                "openapi": {
                    "openapi": "3.0.3",
                    "info": {"title": "Catalog", "version": "1.0.0"},
                    "paths": {
                        "/items": {
                            "get": {"operationId": "listItems"},
                            "post": {"operationId": "createItem"}
                        }
                    }
                }
            })),
        ))
        .await
        .expect("create api");
    let status = response.status();
    let body = json_of(response).await;
    assert_eq!(status, StatusCode::CREATED, "create api fixture: {body}");
    let api_id = Uuid::parse_str(body["api"]["id"].as_str().expect("api id")).expect("api uuid");
    (api_name, api_id)
}

/// Publish spec `version` through the product publish surface.
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
            Some(serde_json::json!({ "reason": "test" })),
        ))
        .await
        .expect("publish response");
    let status = response.status();
    let body = json_of(response).await;
    assert_eq!(status, StatusCode::OK, "publish fixture: {body}");
}

/// Disable one generated tool through the real product surface.
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

/// Fixture lookup: the generated api_tools names for an API definition, name-sorted.
async fn tool_names_of(pool: &PgPool, api_id: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM api_tools WHERE api_definition_id = $1 ORDER BY name")
        .bind(api_id)
        .fetch_all(pool)
        .await
        .expect("tool names")
}

/// The team's standard dynamic seeding for these tests:
/// - one API with v1 PUBLISHED carrying two tools, one of which is then DISABLED;
/// - a second spec version v2 on the same API, NOT published (ghost op);
/// - a second API, imported but NEVER published (its tools exist but must not serve).
///
/// Returns (enabled `api_*` name, disabled `api_*` name, never-servable `api_*` names).
async fn seed_dynamic_tools(
    env: &Env,
    fx: &TeamFixture,
    admin_token: &str,
) -> (String, String, Vec<String>) {
    let (api_name, api_id) = create_api(env, admin_token, &fx.team_name).await;
    publish_spec(env, admin_token, &fx.team_name, &api_name, 1).await;
    let names = tool_names_of(&env.pool, api_id).await;
    assert_eq!(names.len(), 2, "two operations -> two tools: {names:?}");
    let enabled_tool = names[0].clone();
    let disabled_tool = names[1].clone();
    disable_tool(env, admin_token, &fx.team_name, &disabled_tool).await;

    // v2 on the same API, never published: its (formula-derived) tool must never appear.
    let mut tx = env.pool.begin().await.expect("spec tx");
    storage_api_lifecycle::create_spec_version(
        &mut tx,
        fx.team,
        ApiDefinitionId::from(api_id),
        &SpecVersionInput {
            source_kind: SpecSourceKind::Learned,
            format: SpecFormat::OpenApi3,
            spec: serde_json::json!({
                "openapi": "3.0.3",
                "info": { "title": "Catalog", "version": "2" },
                "paths": { "/ghosts": { "get": { "operationId": "ghostOp" } } }
            }),
        },
    )
    .await
    .expect("v2 spec");
    tx.commit().await.expect("spec commit");
    let ghost_v2_tool = format!("api_{api_name}-ghostop");

    // A second API, imported (tool rows generated) but never published.
    let (_, inert_api_id) = create_api(env, admin_token, &fx.team_name).await;
    let inert_names = tool_names_of(&env.pool, inert_api_id).await;
    assert!(
        !inert_names.is_empty(),
        "import generates tool rows even before publish"
    );

    let mut never_servable: Vec<String> = inert_names
        .into_iter()
        .map(|n| format!("api_{n}"))
        .collect();
    never_servable.push(ghost_v2_tool);

    (
        format!("api_{enabled_tool}"),
        format!("api_{disabled_tool}"),
        never_servable,
    )
}

// --- Criterion 1+2: full static registry, exact row shape ------------------------------

#[tokio::test]
async fn catalog_contains_full_static_registry_with_exact_row_shape() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    let response = get_catalog(&env, &fx.team_name, false, &admin_token).await;
    let rows = rows_of(&response);
    let content_type = response
        .content_type
        .as_deref()
        .expect("content-type present");
    assert!(
        content_type.starts_with("application/json"),
        "catalog must be application/json, got {content_type}"
    );

    for row in &rows {
        assert_row_shape(row);
    }

    // Every declared static tool is present, with metadata matching the registry —
    // derived from the public constant, never a hardcoded count.
    assert!(
        !STATIC_TOOL_DECLS.is_empty(),
        "sanity: the static registry is non-empty"
    );
    for decl in STATIC_TOOL_DECLS {
        let row = find_row(&rows, decl.name)
            .unwrap_or_else(|| panic!("static tool {} missing from the catalog", decl.name));
        assert_eq!(row["kind"], "static", "{}: {row}", decl.name);
        assert_eq!(
            row["enabled"], true,
            "{}: static rows are always enabled: {row}",
            decl.name
        );
        assert_eq!(row["description"], decl.description, "{}: {row}", decl.name);
        assert_eq!(
            row["resource"],
            decl.resource.as_str(),
            "{}: {row}",
            decl.name
        );
        assert_eq!(row["action"], decl.action.as_str(), "{}: {row}", decl.name);
        assert_eq!(row["risk"], decl.risk.as_str(), "{}: {row}", decl.name);
        assert_eq!(
            row["input_schema"],
            (decl.input_schema)(),
            "{}: input_schema must match the registry declaration: {row}",
            decl.name
        );
        // Org admin passes every static row's own (resource, action).
        assert_eq!(
            row["executable_by_caller"], true,
            "{}: org admin must be able to execute every static tool: {row}",
            decl.name
        );
    }

    // No unexpected static rows: every kind="static" row is a registry entry.
    let declared: BTreeSet<&str> = STATIC_TOOL_DECLS.iter().map(|d| d.name).collect();
    for row in rows.iter().filter(|r| r["kind"] == "static") {
        let name = row["name"].as_str().expect("name");
        assert!(
            declared.contains(name),
            "catalog serves a static tool absent from the registry: {row}"
        );
    }
}

// --- Criterion 3: dynamic rows follow the published version + enabled filter -----------

#[tokio::test]
async fn dynamic_rows_follow_published_version_and_enabled_filter() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let (enabled_tool, disabled_tool, never_servable) =
        seed_dynamic_tools(&env, &fx, &admin_token).await;

    // Default view: the enabled tool only.
    let response = get_catalog(&env, &fx.team_name, false, &admin_token).await;
    let rows = rows_of(&response);
    for row in &rows {
        assert_row_shape(row);
    }
    let row = find_row(&rows, &enabled_tool)
        .unwrap_or_else(|| panic!("enabled dynamic tool {enabled_tool} missing: {rows:?}"));
    assert_eq!(row["kind"], "dynamic", "{row}");
    assert!(
        row["name"].as_str().expect("name").starts_with("api_"),
        "dynamic names are api_-prefixed: {row}"
    );
    assert_eq!(row["resource"], "mcp-tools", "{row}");
    assert_eq!(row["action"], "execute", "{row}");
    assert_eq!(row["enabled"], true, "{row}");
    assert_eq!(
        row["executable_by_caller"], true,
        "org admin passes (mcp-tools, execute) and the row is enabled: {row}"
    );
    assert!(
        find_row(&rows, &disabled_tool).is_none(),
        "default catalog must show enabled tools only; {disabled_tool} leaked: {rows:?}"
    );
    for ghost in &never_servable {
        assert!(
            find_row(&rows, ghost).is_none(),
            "tool {ghost} of a non-published spec version must never appear: {rows:?}"
        );
    }

    // include_disabled=true: enabled AND disabled rows; unpublished still excluded.
    let response = get_catalog(&env, &fx.team_name, true, &admin_token).await;
    let rows = rows_of(&response);
    for row in &rows {
        assert_row_shape(row);
    }
    assert!(
        find_row(&rows, &enabled_tool).is_some(),
        "include_disabled must still list the enabled tool: {rows:?}"
    );
    let row = find_row(&rows, &disabled_tool).unwrap_or_else(|| {
        panic!("include_disabled=true must list the disabled tool {disabled_tool}: {rows:?}")
    });
    assert_eq!(row["kind"], "dynamic", "{row}");
    assert_eq!(row["enabled"], false, "{row}");
    assert_eq!(
        row["executable_by_caller"], false,
        "a DISABLED dynamic row is not executable even for an org admin holding \
         every grant: {row}"
    );
    for ghost in &never_servable {
        assert!(
            find_row(&rows, ghost).is_none(),
            "tool {ghost} of a non-published spec version must never appear even \
             with include_disabled=true: {rows:?}"
        );
    }
}

// --- Criterion 4: authorization — read gate and include_disabled update gate -----------

#[tokio::test]
async fn catalog_requires_read_and_include_disabled_requires_update() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;

    // Grantless same-org member: 403 on the plain catalog read.
    let (_, grantless_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    let response = get_catalog(&env, &fx.team_name, false, &grantless_token).await;
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "member with no grants must be denied the catalog: {}",
        response.body
    );
    assert_error_envelope(&response, "forbidden");

    // Member holding ONLY (mcp-tools, read): plain read 200 ...
    let (reader, reader_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    grant(&env, reader, &fx, Resource::McpTools, Action::Read).await;
    let response = get_catalog(&env, &fx.team_name, false, &reader_token).await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "(mcp-tools, read) must open the plain catalog: {}",
        response.body
    );

    // ... but include_disabled=true FAILS CLOSED with 403 — never a silent
    // downgrade to the enabled-only view.
    let response = get_catalog(&env, &fx.team_name, true, &reader_token).await;
    assert_ne!(
        response.status,
        StatusCode::OK,
        "include_disabled without (mcp-tools, update) must NOT silently downgrade \
         to an enabled-only 200: {}",
        response.body
    );
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "include_disabled without (mcp-tools, update) must be 403: {}",
        response.body
    );
    assert_error_envelope(&response, "forbidden");

    // Positive control (the 403 above cannot pass vacuously): read+update opens it.
    let (auditor, auditor_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    grant(&env, auditor, &fx, Resource::McpTools, Action::Read).await;
    grant(&env, auditor, &fx, Resource::McpTools, Action::Update).await;
    let response = get_catalog(&env, &fx.team_name, true, &auditor_token).await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "(mcp-tools, read)+(mcp-tools, update) must open include_disabled: {}",
        response.body
    );

    // Org admin passes both implicitly.
    let response = get_catalog(&env, &fx.team_name, true, &admin_token).await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "org admin must pass the include_disabled gate implicitly: {}",
        response.body
    );
}

// --- Criterion 5: executable_by_caller is per-row (resource, action) -------------------

#[tokio::test]
async fn executable_by_caller_reflects_each_rows_own_grant() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let (enabled_tool, _, _) = seed_dynamic_tools(&env, &fx, &admin_token).await;

    // Member with (mcp-tools, read) [to open the catalog] + (clusters, read) only.
    let (member, member_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    grant(&env, member, &fx, Resource::McpTools, Action::Read).await;
    grant(&env, member, &fx, Resource::Clusters, Action::Read).await;

    let response = get_catalog(&env, &fx.team_name, false, &member_token).await;
    let rows = rows_of(&response);

    // Static rows: executable iff the caller passes THAT row's own (resource, action),
    // i.e. exactly the (clusters, read) declarations — derived from the registry.
    for decl in STATIC_TOOL_DECLS {
        let row = find_row(&rows, decl.name)
            .unwrap_or_else(|| panic!("static tool {} missing from the catalog", decl.name));
        let expected = decl.resource == Resource::Clusters && decl.action == Action::Read;
        assert_eq!(
            row["executable_by_caller"], expected,
            "{}: caller holds only clusters:read (+mcp-tools:read), so executable \
             must be {expected}: {row}",
            decl.name
        );
    }
    // Sanity: the expectation above is non-vacuous in both directions.
    assert!(
        STATIC_TOOL_DECLS
            .iter()
            .any(|d| d.resource == Resource::Clusters && d.action == Action::Read),
        "registry sanity: a (clusters, read) static tool exists"
    );
    assert!(
        STATIC_TOOL_DECLS
            .iter()
            .any(|d| !(d.resource == Resource::Clusters && d.action == Action::Read)),
        "registry sanity: a non-(clusters, read) static tool exists"
    );

    // Dynamic row: not executable without (mcp-tools, execute).
    let row = find_row(&rows, &enabled_tool)
        .unwrap_or_else(|| panic!("enabled dynamic tool {enabled_tool} missing: {rows:?}"));
    assert_eq!(
        row["executable_by_caller"], false,
        "dynamic tools need (mcp-tools, execute), which this caller lacks: {row}"
    );

    // Second member adds (mcp-tools, execute): the enabled dynamic row flips to
    // executable while non-granted static rows stay non-executable.
    let (executor, executor_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    grant(&env, executor, &fx, Resource::McpTools, Action::Read).await;
    grant(&env, executor, &fx, Resource::McpTools, Action::Execute).await;
    let response = get_catalog(&env, &fx.team_name, false, &executor_token).await;
    let rows = rows_of(&response);
    let row = find_row(&rows, &enabled_tool)
        .unwrap_or_else(|| panic!("enabled dynamic tool {enabled_tool} missing: {rows:?}"));
    assert_eq!(
        row["executable_by_caller"], true,
        "(mcp-tools, execute) + enabled row => executable: {row}"
    );
    for decl in STATIC_TOOL_DECLS {
        let row = find_row(&rows, decl.name)
            .unwrap_or_else(|| panic!("static tool {} missing from the catalog", decl.name));
        assert_eq!(
            row["executable_by_caller"], false,
            "{}: an mcp-tools-only principal passes no static row's own \
             (resource, action): {row}",
            decl.name
        );
    }
}

// --- Criterion 6 (key): parity between MCP tools/list and the executable catalog -------

#[tokio::test]
async fn tools_list_parity_with_executable_catalog_rows() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let (enabled_tool, disabled_tool, _) = seed_dynamic_tools(&env, &fx, &admin_token).await;

    // Org admin: tools/list name set == catalog rows with executable_by_caller=true,
    // in BOTH directions (asserted by set equality).
    let session = initialize(&env, &admin_token).await;
    let listed = tools_list(&env, &admin_token, &session, &fx.team_name).await;
    let response = get_catalog(&env, &fx.team_name, false, &admin_token).await;
    let default_rows = rows_of(&response);
    let executable = executable_names(&default_rows);
    assert_eq!(
        listed, executable,
        "MCP tools/list and the executable catalog rows must agree exactly \
         (tools/list={listed:?} catalog-executable={executable:?})"
    );
    // Non-vacuous: the parity set includes static tools and the enabled dynamic tool.
    assert!(
        executable.contains(&enabled_tool),
        "parity set must include the enabled dynamic tool: {executable:?}"
    );
    assert!(
        executable.iter().any(|n| !n.starts_with("api_")),
        "parity set must include static tools: {executable:?}"
    );

    // Disabled-row three-surface agreement:
    // (1) absent from tools/list,
    assert!(
        !listed.contains(&disabled_tool),
        "disabled tool must be absent from tools/list: {listed:?}"
    );
    // (2) absent from the default catalog entirely,
    assert!(
        find_row(&default_rows, &disabled_tool).is_none(),
        "disabled tool must be absent from the default catalog: {default_rows:?}"
    );
    // (3) present ONLY under include_disabled=true, with executable_by_caller=false.
    let response = get_catalog(&env, &fx.team_name, true, &admin_token).await;
    let audit_rows = rows_of(&response);
    let row = find_row(&audit_rows, &disabled_tool).unwrap_or_else(|| {
        panic!("disabled tool {disabled_tool} must appear under include_disabled: {audit_rows:?}")
    });
    assert_eq!(
        row["executable_by_caller"], false,
        "the disabled row is never executable, on any surface: {row}"
    );

    // Parity also holds for a partially-granted member holding (mcp-tools, read):
    // clusters:read grants make exactly the clusters read tools listable/executable.
    let (member, member_token) = user_with_org_role(&env, fx.org_id, OrgRole::Member).await;
    grant(&env, member, &fx, Resource::McpTools, Action::Read).await;
    grant(&env, member, &fx, Resource::Clusters, Action::Read).await;
    let session = initialize(&env, &member_token).await;
    let listed = tools_list(&env, &member_token, &session, &fx.team_name).await;
    let response = get_catalog(&env, &fx.team_name, false, &member_token).await;
    let executable = executable_names(&rows_of(&response));
    assert_eq!(
        listed, executable,
        "parity must hold for a partially-granted principal too \
         (tools/list={listed:?} catalog-executable={executable:?})"
    );
    assert!(
        !executable.is_empty(),
        "partial-parity sanity: clusters:read yields a non-empty set"
    );
}

// --- Criterion 7: cross-org anti-enumeration -------------------------------------------

#[tokio::test]
async fn cross_org_caller_gets_not_found_with_no_leak() {
    let Some(env) = env().await else { return };
    let fx = org_with_team(&env).await;
    let (_, admin_token) = user_with_org_role(&env, fx.org_id, OrgRole::Admin).await;
    let (enabled_tool, disabled_tool, _) = seed_dynamic_tools(&env, &fx, &admin_token).await;

    let other_org = identity::create_org(&env.pool, &unique("org-p"), "")
        .await
        .expect("other org");
    let (_, foreign_token) = user_with_org_role(&env, other_org.id, OrgRole::Admin).await;

    for team_ref in [fx.team_name.clone(), fx.team_id.as_uuid().to_string()] {
        for include_disabled in [false, true] {
            let response = get_catalog(&env, &team_ref, include_disabled, &foreign_token).await;
            assert_ne!(
                response.status,
                StatusCode::FORBIDDEN,
                "403 for team {team_ref} confirms the team exists to an outsider: {}",
                response.body
            );
            assert_eq!(
                response.status,
                StatusCode::NOT_FOUND,
                "cross-org catalog read (team {team_ref}, include_disabled=\
                 {include_disabled}) must read as absent, got {}: {}",
                response.status,
                response.body
            );
            assert_error_envelope(&response, "not_found");
            let raw = response.body.to_string();
            assert!(
                !raw.contains(&enabled_tool) && !raw.contains(&disabled_tool),
                "tool names leaked cross-org: {}",
                response.body
            );
        }
    }
}
